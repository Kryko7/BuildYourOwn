//! Running one module through the runtime under test.
//!
//! The contract is a subset of `wasmtime`'s command line, so the reference runtime is
//! literally `wasmtime`:
//!
//! ```text
//! <runtime> run --invoke <export> <module.wasm> [args...]
//! <runtime> run <module.wasm> [args...]
//! ```
//!
//! Every invocation is its own process, in its own process group, with its own temporary
//! directory. Nothing is shared between tests, so a runtime that crashes, hangs or leaks
//! cannot poison the next one. `stdout` and `stderr` are captured **separately** and never
//! merged: `wasmtime` prints experimental-feature warnings on stderr while the results go to
//! stdout, and a suite that folded them together would compare against noise.

pub mod reference;

use crate::config::{Placeholders, RuntimeDef, RuntimeKind};
use anyhow::{bail, Context, Result};
use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How a runtime process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exit {
    /// The exit status, when the process exited normally.
    pub code: Option<i32>,
    /// The signal that killed it, when one did.
    pub signal: Option<i32>,
}

impl Exit {
    /// True only for a normal exit with status 0.
    pub fn success(self) -> bool {
        self.code == Some(0)
    }

    /// True when the process exited normally with a non-zero status, or was killed.
    pub fn failed(self) -> bool {
        !self.success()
    }

    /// `exit status 1` / `signal 11 (SIGSEGV)` — how every report writes an outcome.
    pub fn label(self) -> String {
        match (self.code, self.signal) {
            (Some(c), _) => format!("exit status {c}"),
            (_, Some(s)) => format!("signal {s} ({})", signal_name(s)),
            _ => "an outcome the harness could not read".to_string(),
        }
    }
}

fn signal_name(sig: i32) -> &'static str {
    match sig {
        2 => "SIGINT",
        4 => "SIGILL",
        6 => "SIGABRT",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        15 => "SIGTERM",
        _ => "a signal this suite does not name",
    }
}

/// Everything one run of the runtime produced.
#[derive(Debug, Clone)]
pub struct Run {
    /// The exact argv the harness used, for the failure block.
    pub argv: Vec<String>,
    /// Standard output, as text (invalid UTF-8 is replaced, and `raw_stdout` keeps the bytes).
    pub stdout: String,
    /// Standard error, as text.
    pub stderr: String,
    /// Standard output as it came off the pipe.
    pub raw_stdout: Vec<u8>,
    /// How the process ended.
    pub exit: Exit,
    /// True when the harness had to kill it because it ran past the deadline.
    pub timed_out: bool,
    /// Wall-clock duration.
    pub duration: Duration,
}

impl Run {
    /// `stdout` split into lines with trailing whitespace removed and a trailing blank line
    /// dropped — the shape `--invoke` results come back in.
    pub fn lines(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .stdout
            .split('\n')
            .map(|l| l.trim_end_matches('\r').to_string())
            .collect();
        while v.last().is_some_and(|l| l.is_empty()) {
            v.pop();
        }
        v
    }

    /// The single line `stdout` holds, or an empty string when it holds none.
    pub fn first_line(&self) -> String {
        self.lines().first().cloned().unwrap_or_default()
    }

    /// True when `stderr` contains `needle`, case-insensitively — how every trap reason and
    /// every decode complaint is matched, because the wording around it is the runtime's own.
    pub fn stderr_has(&self, needle: &str) -> bool {
        self.stderr.to_lowercase().contains(&needle.to_lowercase())
    }

    /// The command line, quoted enough to paste into a shell.
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

    /// The last `n` lines of stderr, for the report's `runtime output` block.
    pub fn stderr_tail(&self, n: usize) -> String {
        let lines: Vec<&str> = self.stderr.lines().collect();
        let from = lines.len().saturating_sub(n);
        lines[from..].join("\n")
    }
}

/// The runtime under test: what to spawn, and where.
#[derive(Debug, Clone)]
pub struct RuntimeHandle {
    /// The definition this handle came from.
    pub def: RuntimeDef,
    /// The argv prefix, placeholders already substituted.
    pub argv0: Vec<String>,
    /// The working directory of every invocation.
    pub cwd: Option<PathBuf>,
    /// Environment overrides.
    pub env: Vec<(String, String)>,
}

impl RuntimeHandle {
    /// Resolve a runtime definition into something that can be spawned.
    ///
    /// A reference runtime is downloaded (once) here; an external one is taken as written,
    /// with its first argument resolved against `cwd` so a relative `./your_program.sh`
    /// works from anywhere.
    pub fn prepare(def: &RuntimeDef) -> Result<RuntimeHandle> {
        let cwd = def.cwd.as_ref().map(PathBuf::from);
        let argv0 = match def.kind {
            RuntimeKind::Reference => {
                let version = def
                    .version
                    .clone()
                    .unwrap_or_else(|| reference::DEFAULT_VERSION.to_string());
                let exe = reference::ensure_installed(&version)?;
                vec![exe.to_string_lossy().to_string()]
            }
            RuntimeKind::External => {
                if def.command.is_empty() {
                    bail!("runtime '{}' has an empty command", def.name);
                }
                let mut argv = def.command.clone();
                let first = Path::new(&argv[0]);
                if first.is_relative() && argv[0].contains('/') {
                    let base = cwd.clone().unwrap_or_else(|| PathBuf::from("."));
                    let joined = base.join(first);
                    if joined.exists() {
                        argv[0] = std::fs::canonicalize(&joined)
                            .unwrap_or(joined)
                            .to_string_lossy()
                            .to_string();
                    }
                }
                argv
            }
        };
        Ok(RuntimeHandle {
            def: def.clone(),
            argv0,
            cwd,
            env: def.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        })
    }

    /// The version string the runtime reports, when it answers `--version` at all.
    pub fn version(&self) -> Option<String> {
        let mut argv = self.argv0.clone();
        argv.push("--version".to_string());
        let out = Command::new(&argv[0]).args(&argv[1..]).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!text.is_empty()).then_some(text)
    }

    /// Run `run [--invoke export] <module> [args...]` and capture everything.
    pub fn invoke(
        &self,
        module_path: &Path,
        export: Option<&str>,
        args: &[String],
        stdin: Option<&[u8]>,
        timeout: Duration,
        tmp: &Path,
    ) -> Result<Run> {
        let ph = Placeholders {
            tmp: tmp.to_path_buf(),
        };
        let mut argv: Vec<String> = self.argv0.iter().map(|a| ph.apply(a)).collect();
        argv.push("run".to_string());
        if let Some(name) = export {
            argv.push("--invoke".to_string());
            argv.push(name.to_string());
        }
        argv.push(module_path.to_string_lossy().to_string());
        // Never `--` here: `wasmtime run --invoke f mod.wasm -- 3` passes "--" to the
        // function and fails to parse it as a number. The module's own arguments follow the
        // path directly, which is what the contract says.
        argv.extend(args.iter().cloned());
        run_capture(
            &argv,
            self.cwd.as_deref(),
            &self.env,
            stdin,
            timeout,
            tmp,
        )
    }
}

/// Spawn a command in its own process group, drain both pipes, and enforce a deadline.
fn run_capture(
    argv: &[String],
    cwd: Option<&Path>,
    env: &[(String, String)],
    stdin: Option<&[u8]>,
    timeout: Duration,
    tmp: &Path,
) -> Result<Run> {
    let started = Instant::now();
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    // A fresh temporary directory per run, so a runtime that writes files cannot see the
    // previous test's.
    cmd.current_dir(cwd.unwrap_or(tmp));
    for (k, v) in env {
        cmd.env(k, v);
    }
    // Put the child in its own process group so a hung runtime that spawned helpers can be
    // killed as a group rather than left behind.
    // SAFETY: `setpgid` is async-signal-safe and touches nothing this process owns.
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = cmd
        .spawn()
        .with_context(|| format!("cannot run {}", argv.join(" ")))?;
    let pid = child.id() as i32;

    if let (Some(bytes), Some(mut sink)) = (stdin, child.stdin.take()) {
        let owned = bytes.to_vec();
        std::thread::spawn(move || {
            use std::io::Write;
            let _ = sink.write_all(&owned);
            let _ = sink.flush();
        });
    }

    let drain = |mut pipe: Option<std::process::ChildStdout>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(p) = pipe.as_mut() {
                let _ = p.read_to_end(&mut buf);
            }
            buf
        })
    };
    let out_thread = drain(child.stdout.take());
    let err_pipe = child.stderr.take();
    let err_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut p) = err_pipe {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });

    let deadline = started + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {}
            Err(e) => return Err(e).context("cannot wait for the runtime process"),
        }
        if Instant::now() >= deadline {
            timed_out = true;
            // SAFETY: killing the group we created above; the child is still ours to reap.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
                libc::kill(pid, libc::SIGKILL);
            }
            match child.wait() {
                Ok(s) => break s,
                Err(e) => return Err(e).context("cannot reap the runtime process after a kill"),
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    };

    let raw_stdout = out_thread.join().unwrap_or_default();
    let raw_stderr = err_thread.join().unwrap_or_default();
    Ok(Run {
        argv: argv.to_vec(),
        stdout: String::from_utf8_lossy(&raw_stdout).to_string(),
        stderr: String::from_utf8_lossy(&raw_stderr).to_string(),
        raw_stdout,
        exit: Exit {
            code: status.code(),
            signal: status.signal(),
        },
        timed_out,
        duration: started.elapsed(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str, timeout_ms: u64) -> Run {
        let dir = tempfile::tempdir().expect("tempdir");
        run_capture(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                script.to_string(),
            ],
            None,
            &[],
            None,
            Duration::from_millis(timeout_ms),
            dir.path(),
        )
        .expect("run")
    }

    #[test]
    fn stdout_and_stderr_stay_apart() {
        let r = sh("echo out; echo err 1>&2", 5_000);
        assert_eq!(r.stdout.trim(), "out");
        assert_eq!(r.stderr.trim(), "err");
        assert!(r.exit.success());
        assert!(!r.timed_out);
    }

    #[test]
    fn a_non_zero_exit_is_reported_with_its_status() {
        let r = sh("exit 7", 5_000);
        assert_eq!(r.exit.code, Some(7));
        assert!(r.exit.failed());
        assert_eq!(r.exit.label(), "exit status 7");
    }

    #[test]
    fn a_hung_process_is_killed_at_the_deadline() {
        let r = sh("sleep 30", 250);
        assert!(r.timed_out, "the harness must kill a runtime that hangs");
        assert!(r.duration < Duration::from_secs(5));
    }

    #[test]
    fn stdin_reaches_the_process() {
        let dir = tempfile::tempdir().expect("tempdir");
        let r = run_capture(
            &["/bin/cat".to_string()],
            None,
            &[],
            Some(b"hello stdin"),
            Duration::from_millis(5_000),
            dir.path(),
        )
        .expect("run");
        assert_eq!(r.stdout, "hello stdin");
    }

    #[test]
    fn lines_drops_the_trailing_newline_only() {
        let r = sh("printf '7\\n-1\\n'", 5_000);
        assert_eq!(r.lines(), vec!["7", "-1"]);
        assert_eq!(r.first_line(), "7");
    }

    #[test]
    fn stderr_matching_is_case_insensitive() {
        let r = sh("echo 'WASM TRAP: Integer Divide By Zero' 1>&2", 5_000);
        assert!(r.stderr_has("integer divide by zero"));
        assert!(!r.stderr_has("out of bounds"));
    }
}
