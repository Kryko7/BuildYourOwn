//! Running a child process with a deadline, and killing it as a process group.
//!
//! Two very different children go through here: the **linker under test**, which is a shell
//! script that may spawn anything, and the **binary the linker produced**, which is a
//! freestanding program that may do absolutely anything including spinning forever. Both are
//! started in their own process group so that a timeout kills the whole tree, both have
//! their output drained by dedicated threads (so a chatty child can never deadlock on a full
//! pipe), and neither ever runs with elevated privileges.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// What a child process did.
#[derive(Debug, Clone)]
pub struct Output {
    /// The argv that was run, for the report.
    pub argv: Vec<String>,
    /// Exit status, when the process exited normally.
    pub code: Option<i32>,
    /// Signal number, when the process was killed by one.
    pub signal: Option<i32>,
    /// True when the deadline expired and the harness killed the process group.
    pub timed_out: bool,
    /// Everything the process wrote to stdout, lossily decoded.
    pub stdout: String,
    /// Raw stdout, for tests that compare bytes.
    pub stdout_bytes: Vec<u8>,
    /// Everything the process wrote to stderr, lossily decoded.
    pub stderr: String,
    /// How long it took.
    pub duration: Duration,
}

impl Output {
    /// True when the process exited with status 0.
    pub fn success(&self) -> bool {
        self.code == Some(0) && !self.timed_out
    }

    /// How the report names the outcome: `exit 0`, `exit 1`, `signal 11 (SIGSEGV)`, `timeout`.
    pub fn status_line(&self) -> String {
        if self.timed_out {
            return "killed after the timeout expired".to_string();
        }
        match (self.code, self.signal) {
            (Some(c), _) => format!("exit {c}"),
            (_, Some(s)) => format!("signal {s} ({})", signal_name(s)),
            _ => "no status at all".to_string(),
        }
    }

    /// The command line as a single copy-pasteable string.
    pub fn command_line(&self) -> String {
        self.argv
            .iter()
            .map(|a| {
                if a.contains(' ') {
                    format!("'{a}'")
                } else {
                    a.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The conventional name of a fatal signal.
pub fn signal_name(sig: i32) -> &'static str {
    match sig {
        2 => "SIGINT",
        4 => "SIGILL",
        6 => "SIGABRT",
        7 => "SIGBUS",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        15 => "SIGTERM",
        _ => "unknown signal",
    }
}

/// What to run.
#[derive(Debug, Clone)]
pub struct Spec {
    /// The program.
    pub program: PathBuf,
    /// Its arguments.
    pub args: Vec<String>,
    /// Working directory.
    pub cwd: PathBuf,
    /// Extra environment variables.
    pub env: Vec<(String, String)>,
    /// Bytes to write on the child's stdin; an empty vector still closes stdin immediately.
    pub stdin: Vec<u8>,
    /// How long to let it run.
    pub timeout: Duration,
}

impl Spec {
    /// A program with arguments, run in `cwd`, with the given deadline.
    pub fn new(program: &Path, args: &[String], cwd: &Path, timeout: Duration) -> Spec {
        Spec {
            program: program.to_path_buf(),
            args: args.to_vec(),
            cwd: cwd.to_path_buf(),
            env: Vec::new(),
            stdin: Vec::new(),
            timeout,
        }
    }

    /// Feed these bytes to the child's stdin.
    pub fn stdin(mut self, bytes: Vec<u8>) -> Spec {
        self.stdin = bytes;
        self
    }

    /// Add an environment variable.
    pub fn env(mut self, key: &str, value: &str) -> Spec {
        self.env.push((key.to_string(), value.to_string()));
        self
    }
}

/// Run a child to completion, or kill its process group when the deadline expires.
pub fn run(spec: &Spec) -> std::io::Result<Output> {
    let mut argv = vec![spec.program.to_string_lossy().to_string()];
    argv.extend(spec.args.iter().cloned());

    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args)
        .current_dir(&spec.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    // Its own process group, so a timeout can take the whole tree down: a wrapper script
    // that spawns a real linker must not outlive the test.
    // SAFETY: `setpgid` is async-signal-safe and touches nothing the parent owns.
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let started = Instant::now();
    let mut child = cmd.spawn()?;
    let pid = child.id() as i32;

    if let Some(mut si) = child.stdin.take() {
        let bytes = spec.stdin.clone();
        // A separate thread: a child that never reads its stdin must not block the harness.
        std::thread::spawn(move || {
            let _ = si.write_all(&bytes);
            let _ = si.flush();
        });
    }
    let out_pipe = child.stdout.take();
    let err_pipe = child.stderr.take();
    let out_thread = std::thread::spawn(move || drain(out_pipe));
    let err_thread = std::thread::spawn(move || drain(err_pipe));

    let mut timed_out = false;
    let status = loop {
        match child.try_wait()? {
            Some(s) => break s,
            None => {
                if started.elapsed() >= spec.timeout {
                    timed_out = true;
                    // SAFETY: killing a process group we created ourselves.
                    unsafe {
                        libc::killpg(pid, libc::SIGKILL);
                        libc::kill(pid, libc::SIGKILL);
                    }
                    break child.wait()?;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    };

    let stdout_bytes = out_thread.join().unwrap_or_default();
    let stderr_bytes = err_thread.join().unwrap_or_default();
    // Reap anything the child left behind in its group.
    // SAFETY: the group is ours; SIGKILL to an already-empty group is a no-op.
    unsafe {
        libc::killpg(pid, libc::SIGKILL);
    }

    use std::os::unix::process::ExitStatusExt;
    Ok(Output {
        argv,
        code: status.code(),
        signal: status.signal(),
        timed_out,
        stdout: String::from_utf8_lossy(&stdout_bytes).to_string(),
        stdout_bytes,
        stderr: String::from_utf8_lossy(&stderr_bytes).to_string(),
        duration: started.elapsed(),
    })
}

fn drain(pipe: Option<impl Read>) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Some(mut p) = pipe {
        let _ = p.read_to_end(&mut buf);
    }
    buf
}

/// Find an executable on `PATH`, for the optional interop leg (`readelf`, `nm`, `objdump`).
pub fn which(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        let p = PathBuf::from(program);
        return p.is_file().then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str, timeout_ms: u64) -> Output {
        let dir = tempfile::tempdir().expect("tempdir");
        run(&Spec::new(
            Path::new("/bin/sh"),
            &["-c".to_string(), script.to_string()],
            dir.path(),
            Duration::from_millis(timeout_ms),
        ))
        .expect("the shell must be runnable")
    }

    #[test]
    fn output_and_status_come_back() {
        let out = sh("echo hi; echo oops >&2; exit 3", 5_000);
        assert_eq!(out.code, Some(3));
        assert_eq!(out.stdout.trim(), "hi");
        assert_eq!(out.stderr.trim(), "oops");
        assert!(!out.timed_out);
        assert_eq!(out.status_line(), "exit 3");
    }

    #[test]
    fn a_signal_is_reported_as_a_signal() {
        let out = sh("kill -SEGV $$", 5_000);
        assert_eq!(out.signal, Some(11));
        assert!(
            out.status_line().contains("SIGSEGV"),
            "{}",
            out.status_line()
        );
    }

    #[test]
    fn a_hang_is_killed_at_the_deadline() {
        let out = sh("sleep 30", 200);
        assert!(out.timed_out, "the sleep should have been killed");
        assert!(out.duration < Duration::from_secs(5));
    }

    #[test]
    fn a_child_that_writes_a_lot_never_deadlocks() {
        let out = sh("head -c 400000 /dev/zero | tr '\\0' 'x'", 10_000);
        assert_eq!(out.stdout_bytes.len(), 400_000);
    }

    #[test]
    fn stdin_is_delivered_and_closed() {
        let out = sh("cat", 5_000);
        assert_eq!(out.stdout, "");
        assert!(
            out.success(),
            "a child reading an empty stdin must still exit"
        );
    }

    #[test]
    fn which_finds_the_shell() {
        assert!(which("sh").is_some());
        assert!(which("definitely-not-a-program-xyz").is_none());
    }
}
