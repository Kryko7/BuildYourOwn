//! Pipe mode: the shell reads a script from stdin; stdout/stderr are captured separately.

use super::{kill_child, read_available, set_nonblocking, Captured, ExitInfo, ReadState};
use anyhow::{Context, Result};
use nix::errno::Errno;
use nix::poll::{poll, PollFd, PollFlags, PollTimeout};
use std::os::fd::AsFd;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const EXIT_GRACE: Duration = Duration::from_millis(300);

pub fn run(mut cmd: Command, input: &[u8], timeout: Duration) -> Result<Captured> {
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
    let mut child = cmd.spawn().context("spawn failed")?;
    let mut stdin = child.stdin.take();
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    for fd in [&stdout as &dyn AsFd, &stderr] {
        set_nonblocking(&fd.as_fd())?;
    }
    if let Some(si) = &stdin {
        set_nonblocking(si)?;
    }

    let deadline = Instant::now() + timeout;
    let mut cap = Captured::default();
    let (mut out_open, mut err_open, mut written) = (true, true, 0usize);
    let mut exited: Option<(std::process::ExitStatus, Instant)> = None;

    loop {
        {
            let mut fds = Vec::with_capacity(3);
            if let Some(si) = &stdin {
                fds.push(PollFd::new(si.as_fd(), PollFlags::POLLOUT));
            }
            if out_open {
                fds.push(PollFd::new(stdout.as_fd(), PollFlags::POLLIN));
            }
            if err_open {
                fds.push(PollFd::new(stderr.as_fd(), PollFlags::POLLIN));
            }
            match poll(&mut fds, PollTimeout::from(20u16)) {
                Ok(_) | Err(Errno::EINTR) => {}
                Err(e) => return Err(e.into()),
            }
        }
        if let Some(si) = &stdin {
            match nix::unistd::write(si.as_fd(), &input[written..]) {
                Ok(n) => written += n,
                Err(Errno::EAGAIN) | Err(Errno::EINTR) => {}
                Err(Errno::EPIPE) => written = input.len(),
                Err(e) => return Err(e.into()),
            }
            if written >= input.len() {
                stdin = None;
            }
        }
        if out_open && matches!(read_available(&stdout, &mut cap.stdout)?, ReadState::Eof) {
            out_open = false;
        }
        if err_open && matches!(read_available(&stderr, &mut cap.stderr)?, ReadState::Eof) {
            err_open = false;
        }
        if exited.is_none() {
            if let Some(st) = child.try_wait()? {
                exited = Some((st, Instant::now()));
            }
        }
        if let Some((st, at)) = exited {
            if (!out_open && !err_open) || at.elapsed() > EXIT_GRACE {
                cap.exit = Some(ExitInfo::from(st));
                break;
            }
        } else if Instant::now() > deadline {
            kill_child(&mut child);
            cap.timed_out = true;
            cap.exit = Some(ExitInfo::Killed);
            break;
        }
    }
    Ok(cap)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh() -> Command {
        Command::new("/bin/sh")
    }

    #[test]
    fn captures_streams_and_exit_code() {
        let cap = run(sh(), b"echo out\necho err >&2\nexit 3\n", Duration::from_secs(5)).unwrap();
        assert_eq!(cap.stdout, b"out\n");
        assert_eq!(cap.stderr, b"err\n");
        assert_eq!(cap.exit, Some(ExitInfo::Code(3)));
        assert!(!cap.timed_out);
    }

    #[test]
    fn eof_on_stdin_ends_the_shell() {
        let cap = run(sh(), b"echo a\n", Duration::from_secs(5)).unwrap();
        assert_eq!(cap.exit, Some(ExitInfo::Code(0)));
    }

    #[test]
    fn hung_shell_is_killed_and_reported() {
        let start = Instant::now();
        let cap = run(sh(), b"echo partial\nsleep 30\n", Duration::from_millis(300)).unwrap();
        assert!(cap.timed_out);
        assert_eq!(cap.stdout, b"partial\n");
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn large_input_does_not_deadlock() {
        let mut input = b"cat <<'EOF' | wc -c\n".to_vec();
        let payload = vec![b'x'; 200_000];
        input.extend_from_slice(&payload);
        input.extend_from_slice(b"\nEOF\n");
        let cap = run(sh(), &input, Duration::from_secs(10)).unwrap();
        assert_eq!(String::from_utf8_lossy(&cap.stdout).trim(), "200001");
    }

    #[test]
    fn background_child_holding_pipes_does_not_hang() {
        let start = Instant::now();
        let cap = run(sh(), b"sleep 5 &\necho done\n", Duration::from_secs(10)).unwrap();
        assert_eq!(cap.stdout, b"done\n");
        assert!(start.elapsed() < Duration::from_secs(3));
    }
}
