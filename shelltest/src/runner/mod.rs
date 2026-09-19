//! Runs one test case: sandbox → launch → capture → normalize → assert.

pub mod pipe;
pub mod pty;

use crate::config::ShellDef;
use crate::fixtures::Sandbox;
use crate::loader::{Mode, Step, TestCase};
use crate::matchers::Matcher;
use crate::normalize::{normalize_expected, Normalizer};
use anyhow::{Context, Result};
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use std::os::fd::AsFd;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum ExitInfo {
    Code(i32),
    Signal(i32),
    /// The shell was still running when the harness stopped the test.
    Killed,
}

impl std::fmt::Display for ExitInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExitInfo::Code(c) => write!(f, "{c}"),
            ExitInfo::Signal(s) => write!(f, "killed by signal {s}"),
            ExitInfo::Killed => write!(f, "(still running; stopped by harness)"),
        }
    }
}

impl From<ExitStatus> for ExitInfo {
    fn from(st: ExitStatus) -> Self {
        match (st.code(), st.signal()) {
            (Some(c), _) => ExitInfo::Code(c),
            (None, Some(s)) => ExitInfo::Signal(s),
            _ => ExitInfo::Killed,
        }
    }
}

/// Raw capture from either runner.
#[derive(Debug, Default)]
pub struct Captured {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub terminal: Vec<u8>,
    pub exit: Option<ExitInfo>,
    pub timed_out: bool,
    pub step_error: Option<String>,
}

pub struct RunOptions {
    pub keep_tmp: bool,
    pub default_timeout_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pass,
    Fail,
    Skip,
}

#[derive(Debug, Clone)]
pub struct TestResult {
    pub name: String,
    pub status: Status,
    pub ext: bool,
    pub duration_ms: u128,
    pub skip_reason: Option<String>,
    pub failures: Vec<String>,
    pub detail: Option<Detail>,
}

/// Everything the failure block needs; also shown for passing tests in verbose mode.
#[derive(Debug, Clone, Default)]
pub struct Detail {
    pub mode: Option<Mode>,
    pub input: String,
    pub expected: Vec<(String, String)>,
    pub actual: Vec<(String, String)>,
    /// (label, expected text, actual text) for line-based matchers that failed.
    pub diffs: Vec<(String, String, String)>,
    pub sandbox: Option<PathBuf>,
}

pub fn run_test(tc: &TestCase, shell: &ShellDef, opts: &RunOptions) -> TestResult {
    let start = Instant::now();
    let mut result = TestResult {
        name: tc.name.clone(),
        status: Status::Skip,
        ext: tc.is_ext(),
        duration_ms: 0,
        skip_reason: None,
        failures: vec![],
        detail: None,
    };
    if tc.skip_on.iter().any(|s| s == &shell.name) {
        result.skip_reason = tc.reason.clone();
        return result;
    }
    match run_inner(tc, shell, opts) {
        Ok((failures, detail)) => {
            result.status = if failures.is_empty() {
                Status::Pass
            } else {
                Status::Fail
            };
            result.failures = failures;
            result.detail = Some(detail);
        }
        Err(e) => {
            result.status = Status::Fail;
            result.failures.push(format!("harness error: {e:#}"));
        }
    }
    result.duration_ms = start.elapsed().as_millis();
    result
}

fn describe_steps(steps: &[Step]) -> String {
    steps
        .iter()
        .map(|s| match s {
            Step::Send(t) => format!("send  {t:?}"),
            Step::Wait(t) => format!("wait  {t:?}"),
            Step::SleepMs(ms) => format!("sleep {ms} ms"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `keys` sugar: wait for the prompt, then after every item ending in a newline wait
/// for the next prompt; after anything else (tab, arrows, partial text) pause briefly.
pub fn keys_to_steps(keys: &[String], prompt: &str, delay_ms: u64) -> Vec<Step> {
    let mut steps = vec![Step::Wait(prompt.to_string())];
    for k in keys {
        steps.push(Step::Send(k.clone()));
        if k.ends_with('\r') || k.ends_with('\n') {
            steps.push(Step::Wait(prompt.to_string()));
        } else {
            steps.push(Step::SleepMs(delay_ms));
        }
    }
    steps
}

fn run_inner(tc: &TestCase, shell: &ShellDef, opts: &RunOptions) -> Result<(Vec<String>, Detail)> {
    let sb = Sandbox::create(&tc.fixtures, shell, opts.keep_tmp)?;
    let sub = |s: &str| sb.vars.apply(s);
    let timeout = Duration::from_millis(tc.timeout_ms.unwrap_or(opts.default_timeout_ms));
    let argv = match tc.mode {
        Mode::Pipe => &shell.pipe_command,
        Mode::Pty => &shell.pty_command,
    };
    let mut cmd = Command::new(&argv[0]);
    if let Some(base) = std::path::Path::new(&argv[0]).file_name() {
        cmd.arg0(base);
    }
    cmd.args(argv[1..].iter().chain(tc.shell_args.iter()).map(|a| sub(a)));
    cmd.env_clear().envs(&sb.env).current_dir(&sb.cwd);

    let (cap, input_desc) = match tc.mode {
        Mode::Pipe => {
            let mut input = tc
                .input
                .iter()
                .map(|l| sub(l))
                .collect::<Vec<_>>()
                .join("\n");
            input.push('\n');
            let cap =
                pipe::run(cmd, input.as_bytes(), timeout).context("launching shell (pipe mode)")?;
            (
                cap,
                tc.input
                    .iter()
                    .map(|l| sub(l))
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        }
        Mode::Pty => {
            let steps: Vec<Step> = if tc.keys.is_empty() {
                tc.steps
                    .iter()
                    .map(|s| match s {
                        Step::Send(t) => Step::Send(sub(t)),
                        Step::Wait(t) => Step::Wait(sub(t)),
                        Step::SleepMs(ms) => Step::SleepMs(*ms),
                    })
                    .collect()
            } else {
                let keys: Vec<String> = tc.keys.iter().map(|k| sub(k)).collect();
                keys_to_steps(&keys, &shell.prompt, tc.key_delay_ms.unwrap_or(100))
            };
            let cap = pty::run(cmd, &steps, timeout).context("launching shell (pty mode)")?;
            (cap, describe_steps(&steps))
        }
    };

    let mut failures = Vec::new();
    let detail = Detail {
        mode: Some(tc.mode),
        input: input_desc,
        sandbox: sb.kept_path().map(Into::into),
        ..Default::default()
    };
    if cap.timed_out {
        failures.push(format!(
            "timed out after {} ms; shell was killed",
            timeout.as_millis()
        ));
    }
    if let Some(e) = &cap.step_error {
        failures.push(e.clone());
    }

    let lossy = |b: &[u8]| String::from_utf8_lossy(b).to_string();
    let mut acc = Checks { failures, detail };
    let mut actual_rows = Vec::new();
    match tc.mode {
        Mode::Pipe => {
            let n = Normalizer::pipe(&shell.prompt, &tc.normalize);
            let stdout = n.apply(&lossy(&cap.stdout));
            let stderr = n.apply(&lossy(&cap.stderr));
            acc.check(
                "stdout",
                tc.expect.stdout.as_ref(),
                &stdout,
                n.trim_lines,
                &sub,
            );
            acc.check(
                "stderr",
                tc.expect.stderr.as_ref(),
                &stderr,
                n.trim_lines,
                &sub,
            );
            actual_rows.push(("stdout".to_string(), stdout));
            actual_rows.push(("stderr".to_string(), stderr));
        }
        Mode::Pty => {
            let n = Normalizer::pty(&shell.prompt, &tc.normalize);
            let term = n.apply(&lossy(&cap.terminal));
            acc.check(
                "terminal",
                tc.expect.terminal.as_ref(),
                &term,
                n.trim_lines,
                &sub,
            );
            actual_rows.push(("terminal".to_string(), term));
        }
    }
    for (path, m) in &tc.expect.files {
        let p = sb.resolve(path);
        let label = format!("file {}", p.display());
        match std::fs::read(&p) {
            Ok(bytes) => acc.check(&label, Some(m), &lossy(&bytes), false, &sub),
            Err(e) => {
                acc.detail.expected.push((label.clone(), m.describe()));
                acc.failures.push(format!("{label}: cannot read ({e})"));
            }
        }
    }
    for path in &tc.expect.file_absent {
        let p = sb.resolve(path);
        acc.detail
            .expected
            .push((format!("file {}", p.display()), "absent".into()));
        if p.exists() {
            acc.failures
                .push(format!("file {} should not exist", p.display()));
        }
    }
    let Checks {
        mut failures,
        mut detail,
    } = acc;
    let exit = cap.exit.clone().unwrap_or(ExitInfo::Killed);
    if let Some(code) = tc.expect.exit_code {
        detail.expected.push(("exit code".into(), code.to_string()));
        if exit != ExitInfo::Code(code) {
            failures.push(format!("exit code: expected {code}, got {exit}"));
        }
    }
    actual_rows.push(("exit code".to_string(), exit.to_string()));
    detail.actual = actual_rows;
    Ok((failures, detail))
}

struct Checks {
    failures: Vec<String>,
    detail: Detail,
}

impl Checks {
    /// Records the expectation and, on mismatch, a failure plus diff material.
    fn check(
        &mut self,
        label: &str,
        m: Option<&Matcher>,
        actual: &str,
        trim: bool,
        sub: &dyn Fn(&str) -> String,
    ) {
        let Some(m) = m else { return };
        let m = m.map_text(sub);
        let m = if trim {
            m.map_exact(normalize_expected)
        } else {
            m
        };
        self.detail.expected.push((label.to_string(), m.describe()));
        if let Err(why) = m.check(actual) {
            self.failures.push(format!("{label}: {why}"));
            if let Some(exp) = m.expected_text() {
                self.detail
                    .diffs
                    .push((label.to_string(), exp, actual.to_string()));
            }
        }
    }
}

// ---- helpers shared by the pipe and pty runners ----

pub(crate) fn set_nonblocking(fd: &impl AsFd) -> Result<()> {
    use nix::fcntl::{fcntl, FcntlArg, OFlag};
    let flags = fcntl(fd.as_fd(), FcntlArg::F_GETFL)?;
    let flags = OFlag::from_bits_truncate(flags) | OFlag::O_NONBLOCK;
    fcntl(fd.as_fd(), FcntlArg::F_SETFL(flags))?;
    Ok(())
}

pub(crate) enum ReadState {
    Open,
    Eof,
}

/// Drain everything currently readable from a non-blocking fd.
pub(crate) fn read_available(fd: &impl AsFd, into: &mut Vec<u8>) -> Result<ReadState> {
    let mut buf = [0u8; 8192];
    loop {
        match nix::unistd::read(fd.as_fd(), &mut buf) {
            Ok(0) => return Ok(ReadState::Eof),
            Ok(n) => into.extend_from_slice(&buf[..n]),
            Err(nix::errno::Errno::EAGAIN) => return Ok(ReadState::Open),
            Err(nix::errno::Errno::EINTR) => {}
            Err(nix::errno::Errno::EIO) => return Ok(ReadState::Eof),
            Err(e) => return Err(e.into()),
        }
    }
}

/// SIGTERM the child's process group, wait briefly, then SIGKILL.
pub(crate) fn kill_child(child: &mut Child) -> ExitStatus {
    let pid = child.id() as i32;
    for sig in [Signal::SIGHUP, Signal::SIGTERM] {
        let _ = kill(Pid::from_raw(-pid), sig);
        let _ = kill(Pid::from_raw(pid), sig);
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(200) {
            if let Ok(Some(st)) = child.try_wait() {
                return st;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let _ = kill(Pid::from_raw(-pid), Signal::SIGKILL);
    let _ = child.kill();
    child.wait().unwrap_or_else(|_| ExitStatus::from_raw(9))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_sugar_waits_for_prompt_after_enter() {
        let steps = keys_to_steps(&["ech\t".to_string(), "\r".to_string()], "$ ", 50);
        assert_eq!(steps.len(), 5);
        assert!(matches!(steps[0], Step::Wait(ref p) if p == "$ "));
        assert!(matches!(steps[2], Step::SleepMs(50)));
        assert!(matches!(steps[4], Step::Wait(_)));
    }

    #[test]
    fn exit_info_display() {
        assert_eq!(ExitInfo::Code(3).to_string(), "3");
        assert_eq!(ExitInfo::from(ExitStatus::from_raw(9)), ExitInfo::Signal(9));
        assert_eq!(
            ExitInfo::from(ExitStatus::from_raw(3 << 8)),
            ExitInfo::Code(3)
        );
    }
}
