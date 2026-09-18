//! Pty mode: the shell runs as a session leader on a pseudo-terminal, and the test
//! drives it with send/wait/sleep steps. This is the only module with `unsafe`.

use super::{kill_child, read_available, set_nonblocking, Captured, ExitInfo, ReadState};
use crate::loader::Step;
use anyhow::{Context, Result};
use nix::errno::Errno;
use nix::poll::{poll, PollFd, PollFlags, PollTimeout};
use nix::pty::{openpty, Winsize};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long to wait for the shell to exit on its own once all steps are done.
const EXIT_GRACE: Duration = Duration::from_millis(400);
/// After the shell exits, how long to keep draining output held open by grandchildren.
const DRAIN_GRACE: Duration = Duration::from_millis(200);

enum StepError {
    Timeout,
    Exited,
}

struct Session {
    master: OwnedFd,
    child: Child,
    buf: Vec<u8>,
    /// Position after the last successful `wait` match; later waits search from here.
    cursor: usize,
    eof: bool,
    exited: Option<(std::process::ExitStatus, Instant)>,
    deadline: Instant,
}

impl Session {
    fn pump(&mut self, wait: Duration) -> Result<()> {
        if self.eof {
            std::thread::sleep(wait.min(Duration::from_millis(20)));
        } else {
            let mut fds = [PollFd::new(self.master.as_fd(), PollFlags::POLLIN)];
            let ms = wait.as_millis().min(20) as u16;
            match poll(&mut fds, PollTimeout::from(ms)) {
                Ok(_) | Err(Errno::EINTR) => {}
                Err(e) => return Err(e.into()),
            }
            if matches!(read_available(&self.master, &mut self.buf)?, ReadState::Eof) {
                self.eof = true;
            }
        }
        if self.exited.is_none() {
            if let Some(st) = self.child.try_wait()? {
                self.exited = Some((st, Instant::now()));
            }
        }
        Ok(())
    }

    /// True once the shell has exited and its output has been drained (or the drain grace passed).
    fn finished(&self) -> bool {
        match self.exited {
            Some((_, at)) => self.eof || at.elapsed() > DRAIN_GRACE,
            None => false,
        }
    }

    fn send(&mut self, bytes: &[u8]) -> Result<Result<(), StepError>> {
        let mut written = 0;
        while written < bytes.len() {
            if self.exited.is_some() {
                return Ok(Err(StepError::Exited));
            }
            match nix::unistd::write(self.master.as_fd(), &bytes[written..]) {
                Ok(n) => written += n,
                Err(Errno::EAGAIN) | Err(Errno::EINTR) => self.pump(Duration::from_millis(5))?,
                Err(Errno::EIO) => return Ok(Err(StepError::Exited)),
                Err(e) => return Err(e.into()),
            }
        }
        Ok(Ok(()))
    }

    fn wait_for(&mut self, text: &str) -> Result<Result<(), StepError>> {
        let needle = text.as_bytes();
        loop {
            if let Some(pos) = find(&self.buf[self.cursor..], needle) {
                self.cursor += pos + needle.len();
                return Ok(Ok(()));
            }
            if self.finished() {
                return Ok(Ok(()));
            }
            if Instant::now() > self.deadline {
                return Ok(Err(StepError::Timeout));
            }
            self.pump(Duration::from_millis(20))?;
        }
    }

    fn sleep(&mut self, ms: u64) -> Result<Result<(), StepError>> {
        let until = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < until {
            if Instant::now() > self.deadline {
                return Ok(Err(StepError::Timeout));
            }
            self.pump(until - Instant::now())?;
        }
        Ok(Ok(()))
    }

    fn run_step(&mut self, step: &Step) -> Result<Result<(), StepError>> {
        match step {
            Step::Send(t) => self.send(t.as_bytes()),
            Step::Wait(t) => self.wait_for(t),
            Step::SleepMs(ms) => self.sleep(*ms),
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

fn spawn(mut cmd: Command) -> Result<(OwnedFd, Child)> {
    let ws = Winsize { ws_row: 24, ws_col: 120, ws_xpixel: 0, ws_ypixel: 0 };
    let pty = openpty(Some(&ws), None).context("openpty")?;
    cmd.stdin(Stdio::from(pty.slave.try_clone()?))
        .stdout(Stdio::from(pty.slave.try_clone()?))
        .stderr(Stdio::from(pty.slave));
    // SAFETY: only async-signal-safe libc calls run between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd.spawn().context("spawn failed")?;
    drop(cmd);
    set_nonblocking(&pty.master)?;
    Ok((pty.master, child))
}

pub fn run(cmd: Command, steps: &[Step], timeout: Duration) -> Result<Captured> {
    let (master, child) = spawn(cmd)?;
    let mut s = Session {
        master,
        child,
        buf: Vec::new(),
        cursor: 0,
        eof: false,
        exited: None,
        deadline: Instant::now() + timeout,
    };
    let mut cap = Captured::default();
    for (i, step) in steps.iter().enumerate() {
        match s.run_step(step)? {
            Ok(()) => {}
            Err(StepError::Timeout) => {
                cap.timed_out = true;
                break;
            }
            Err(StepError::Exited) => {
                cap.step_error = Some(format!("shell exited before step {} could run: {}", i + 1, describe(step)));
                break;
            }
        }
    }
    if !cap.timed_out {
        let start = Instant::now();
        while !s.finished() && start.elapsed() < EXIT_GRACE {
            s.pump(Duration::from_millis(20))?;
        }
    }
    cap.exit = match s.exited {
        Some((st, _)) => Some(ExitInfo::from(st)),
        None => {
            kill_child(&mut s.child);
            Some(ExitInfo::Killed)
        }
    };
    let _ = read_available(&s.master, &mut s.buf);
    cap.terminal = s.buf;
    Ok(cap)
}

fn describe(step: &Step) -> String {
    match step {
        Step::Send(t) => format!("send {t:?}"),
        Step::Wait(t) => format!("wait {t:?}"),
        Step::SleepMs(ms) => format!("sleep {ms} ms"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh() -> Command {
        let mut c = Command::new("/bin/sh");
        c.arg("-i").env_clear().env("PS1", "$ ").env("PATH", "/usr/bin:/bin").env("TERM", "dumb");
        c
    }

    fn term(cap: &Captured) -> String {
        String::from_utf8_lossy(&cap.terminal).replace('\r', "")
    }

    #[test]
    fn prompt_send_and_exit() {
        let steps = vec![
            Step::Wait("$ ".into()),
            Step::Send("echo hi\r".into()),
            Step::Wait("$ ".into()),
            Step::Send("exit 4\r".into()),
        ];
        let cap = run(sh(), &steps, Duration::from_secs(5)).unwrap();
        assert!(term(&cap).contains("hi\n"), "{:?}", term(&cap));
        assert_eq!(cap.exit, Some(ExitInfo::Code(4)));
        assert!(!cap.timed_out);
    }

    #[test]
    fn still_running_shell_is_stopped_after_steps() {
        let steps = vec![Step::Wait("$ ".into())];
        let start = Instant::now();
        let cap = run(sh(), &steps, Duration::from_secs(5)).unwrap();
        assert_eq!(cap.exit, Some(ExitInfo::Killed));
        assert!(term(&cap).contains("$ "));
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn missing_text_times_out_with_partial_output() {
        let steps = vec![Step::Wait("$ ".into()), Step::Send("echo abc\r".into()), Step::Wait("never-appears".into())];
        let cap = run(sh(), &steps, Duration::from_millis(500)).unwrap();
        assert!(cap.timed_out);
        assert!(term(&cap).contains("abc"));
    }

    #[test]
    fn sending_after_exit_is_a_step_error() {
        let steps = vec![
            Step::Wait("$ ".into()),
            Step::Send("exit\r".into()),
            Step::Wait("$ ".into()),
            Step::Send("echo late\r".into()),
        ];
        let cap = run(sh(), &steps, Duration::from_secs(5)).unwrap();
        assert!(cap.step_error.as_deref().unwrap_or("").contains("step 4"));
    }

    #[test]
    fn is_controlling_terminal() {
        let steps = vec![Step::Wait("$ ".into()), Step::Send("tty >/dev/null && echo istty; exit\r".into())];
        let cap = run(sh(), &steps, Duration::from_secs(5)).unwrap();
        assert!(term(&cap).contains("istty\n"), "{:?}", term(&cap));
    }
}
