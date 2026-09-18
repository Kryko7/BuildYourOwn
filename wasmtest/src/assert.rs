//! Structured failures: what was expected, what the runtime printed, the exact command line,
//! and the module that was run as an annotated hex listing.

use crate::runtime::Run;
use crate::wasm::Module;
use std::fmt::Debug;

/// Why a test failed. Reported separately so a crashed runtime never looks like a wrong
/// number on stdout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// A value was not what the spec says it must be.
    Assertion,
    /// The runtime did not finish inside the per-test deadline.
    Timeout,
    /// The runtime process was killed by a signal.
    RuntimeCrash,
    /// The runtime printed something the harness cannot read as a result at all.
    Output,
    /// The harness itself could not set the test up.
    Harness,
}

impl FailureKind {
    /// Short label used in the terminal report.
    pub fn label(self) -> &'static str {
        match self {
            FailureKind::Assertion => "assertion",
            FailureKind::Timeout => "timeout",
            FailureKind::RuntimeCrash => "runtime crash",
            FailureKind::Output => "unreadable output",
            FailureKind::Harness => "harness error",
        }
    }
}

/// Everything the reporter needs to explain one failed test.
#[derive(Debug, Clone)]
pub struct Failure {
    /// What kind of failure this is.
    pub kind: FailureKind,
    /// One line per failed check, e.g. `stdout.line[0]: expected "7", got "8"`.
    pub messages: Vec<String>,
    /// Labelled values that were expected.
    pub expected: Vec<(String, String)>,
    /// Labelled values that were seen.
    pub actual: Vec<(String, String)>,
    /// The module that was run, so the report can print its annotated bytes.
    pub module: Option<Module>,
    /// The invocation, so the report can print the command line and both streams.
    pub run: Option<Run>,
    /// Free-form context lines (what the test was doing, which export, ...).
    pub notes: Vec<String>,
}

impl Failure {
    /// A failure of the given kind with a single message.
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Failure {
        Failure {
            kind,
            messages: vec![message.into()],
            expected: Vec::new(),
            actual: Vec::new(),
            module: None,
            run: None,
            notes: Vec::new(),
        }
    }

    /// The harness could not set the test up.
    pub fn harness(message: impl Into<String>) -> Failure {
        Failure::new(FailureKind::Harness, message)
    }

    /// Add a context line.
    pub fn note(mut self, note: impl Into<String>) -> Failure {
        self.notes.push(note.into());
        self
    }

    /// Attach the module that was run.
    pub fn with_module(mut self, m: &Module) -> Failure {
        self.module = Some(m.clone());
        self
    }

    /// Attach the invocation that produced the failure.
    pub fn with_run(mut self, r: &Run) -> Failure {
        self.run = Some(r.clone());
        self
    }

    /// The blocks shown under a failure: the command line, both streams, and the module.
    pub fn detail_blocks(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if let Some(r) = &self.run {
            out.push(("command".to_string(), r.command_line()));
            out.push((
                "outcome".to_string(),
                format!(
                    "{}{}, {} ms",
                    r.exit.label(),
                    if r.timed_out {
                        " (killed: timeout)"
                    } else {
                        ""
                    },
                    r.duration.as_millis()
                ),
            ));
            out.push((
                format!("stdout ({} bytes)", r.raw_stdout.len()),
                if r.stdout.is_empty() {
                    "(empty)".to_string()
                } else {
                    r.stdout.trim_end_matches('\n').to_string()
                },
            ));
            out.push((
                "stderr".to_string(),
                if r.stderr.is_empty() {
                    "(empty)".to_string()
                } else {
                    r.stderr_tail(24)
                },
            ));
        }
        if let Some(m) = &self.module {
            out.push((
                format!("module '{}' ({} bytes)", m.label, m.len()),
                m.listing(4096),
            ));
        }
        out
    }
}

/// Collects field-level checks and turns them into one [`Failure`].
///
/// ```ignore
/// let mut c = Check::new("add(3, 4)", &run);
/// c.module(&module);
/// c.eq("stdout", "7", run.first_line().as_str());
/// c.finish()?;
/// ```
pub struct Check {
    what: String,
    failures: Vec<String>,
    expected: Vec<(String, String)>,
    actual: Vec<(String, String)>,
    module: Option<Module>,
    run: Option<Run>,
    notes: Vec<String>,
}

impl Check {
    /// Start a check block against one invocation.
    pub fn new(what: impl Into<String>, run: &Run) -> Check {
        Check {
            what: what.into(),
            failures: Vec::new(),
            expected: Vec::new(),
            actual: Vec::new(),
            module: None,
            run: Some(run.clone()),
            notes: Vec::new(),
        }
    }

    /// Start a check block with no invocation attached (assertions about bytes on disk, or
    /// about several runs at once).
    pub fn detached(what: impl Into<String>) -> Check {
        Check {
            what: what.into(),
            failures: Vec::new(),
            expected: Vec::new(),
            actual: Vec::new(),
            module: None,
            run: None,
            notes: Vec::new(),
        }
    }

    /// Attach the module whose annotated bytes belong under this failure.
    pub fn module(&mut self, m: &Module) -> &mut Self {
        self.module = Some(m.clone());
        self
    }

    /// Replace the attached invocation.
    pub fn run(&mut self, r: &Run) -> &mut Self {
        self.run = Some(r.clone());
        self
    }

    /// Add a context line shown under the failure.
    pub fn note(&mut self, note: impl Into<String>) -> &mut Self {
        self.notes.push(note.into());
        self
    }

    /// Record a value in the report without asserting on it.
    pub fn observe(&mut self, path: &str, value: impl Debug) -> &mut Self {
        self.actual.push((path.to_string(), format!("{value:?}")));
        self
    }

    /// Assert `actual == expected`, naming the value's path.
    pub fn eq<T>(&mut self, path: &str, expected: T, actual: T) -> &mut Self
    where
        T: PartialEq + Debug,
    {
        if expected != actual {
            self.failures
                .push(format!("{path}: expected {expected:?}, got {actual:?}"));
            self.expected
                .push((path.to_string(), format!("{expected:?}")));
            self.actual.push((path.to_string(), format!("{actual:?}")));
        }
        self
    }

    /// Assert `actual != forbidden`.
    pub fn ne<T>(&mut self, path: &str, forbidden: T, actual: T) -> &mut Self
    where
        T: PartialEq + Debug,
    {
        if forbidden == actual {
            self.failures
                .push(format!("{path}: expected anything but {forbidden:?}"));
            self.expected
                .push((path.to_string(), format!("not {forbidden:?}")));
            self.actual.push((path.to_string(), format!("{actual:?}")));
        }
        self
    }

    /// Assert `actual >= min`.
    pub fn at_least<T>(&mut self, path: &str, min: T, actual: T) -> &mut Self
    where
        T: PartialOrd + Debug,
    {
        if actual < min {
            self.failures
                .push(format!("{path}: expected at least {min:?}, got {actual:?}"));
            self.expected
                .push((path.to_string(), format!(">= {min:?}")));
            self.actual.push((path.to_string(), format!("{actual:?}")));
        }
        self
    }

    /// Assert `actual <= max`.
    pub fn at_most<T>(&mut self, path: &str, max: T, actual: T) -> &mut Self
    where
        T: PartialOrd + Debug,
    {
        if actual > max {
            self.failures
                .push(format!("{path}: expected at most {max:?}, got {actual:?}"));
            self.expected
                .push((path.to_string(), format!("<= {max:?}")));
            self.actual.push((path.to_string(), format!("{actual:?}")));
        }
        self
    }

    /// Assert an arbitrary condition, describing what was expected.
    pub fn that(&mut self, path: &str, expected: &str, ok: bool, actual: impl Debug) -> &mut Self {
        if !ok {
            self.failures
                .push(format!("{path}: expected {expected}, got {actual:?}"));
            self.expected.push((path.to_string(), expected.to_string()));
            self.actual.push((path.to_string(), format!("{actual:?}")));
        }
        self
    }

    /// Assert two byte strings are identical, pointing at the first differing byte.
    pub fn bytes_eq(&mut self, path: &str, expected: &[u8], actual: &[u8]) -> &mut Self {
        if expected != actual {
            let at = expected
                .iter()
                .zip(actual.iter())
                .position(|(a, b)| a != b)
                .unwrap_or_else(|| expected.len().min(actual.len()));
            self.failures.push(format!(
                "{path}: {} bytes expected, {} seen, first difference at byte {at}",
                expected.len(),
                actual.len()
            ));
            self.expected.push((
                path.to_string(),
                crate::wasm::hex::hexdump(expected, std::slice::from_ref(&(at..at + 1)), 256),
            ));
            self.actual.push((
                path.to_string(),
                crate::wasm::hex::hexdump(actual, std::slice::from_ref(&(at..at + 1)), 256),
            ));
        }
        self
    }

    /// True when nothing has failed so far.
    pub fn ok(&self) -> bool {
        self.failures.is_empty()
    }

    /// Turn the collected checks into a result.
    pub fn finish(&mut self) -> Result<(), Failure> {
        if self.failures.is_empty() {
            return Ok(());
        }
        let mut notes = vec![format!("while checking {}", self.what)];
        notes.append(&mut self.notes);
        Err(Failure {
            kind: FailureKind::Assertion,
            messages: std::mem::take(&mut self.failures),
            expected: std::mem::take(&mut self.expected),
            actual: std::mem::take(&mut self.actual),
            module: self.module.take(),
            run: self.run.take(),
            notes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passing_checks_produce_no_failure() {
        let mut c = Check::detached("nothing");
        c.eq("a", 1i32, 1i32).at_least("b", 2i64, 4i64);
        assert!(c.ok());
        assert!(c.finish().is_ok());
    }

    #[test]
    fn failures_name_the_value_path() {
        let mut c = Check::detached("add(3, 4)");
        c.eq("stdout.line[0]", "7", "8");
        let f = c.finish().expect_err("must fail");
        assert_eq!(f.kind, FailureKind::Assertion);
        assert_eq!(f.messages, vec![r#"stdout.line[0]: expected "7", got "8""#]);
        assert!(f.notes[0].contains("add(3, 4)"));
    }

    #[test]
    fn bounds_read_like_the_spec() {
        let mut c = Check::detached("memory.grow");
        c.at_least("stdout", 1i32, 0i32);
        c.at_most("pages", 2i32, 9i32);
        let f = c.finish().expect_err("must fail");
        assert!(f.messages[0].contains("at least 1"), "{:?}", f.messages);
        assert!(f.messages[1].contains("at most 2"), "{:?}", f.messages);
    }

    #[test]
    fn bytes_eq_points_at_the_first_difference() {
        let mut c = Check::detached("module bytes");
        c.bytes_eq("module", &[1, 2, 3], &[1, 9, 3]);
        let f = c.finish().expect_err("must fail");
        assert!(
            f.messages[0].contains("first difference at byte 1"),
            "{:?}",
            f.messages
        );
    }

    #[test]
    fn a_failure_carries_the_module_listing() {
        let m = crate::wasm::ModuleBuilder::new("empty").build();
        let f = Failure::new(FailureKind::Assertion, "boom").with_module(&m);
        let blocks = f.detail_blocks();
        let (title, body) = blocks.last().expect("a module block");
        assert!(title.contains("module 'empty'"), "{title}");
        assert!(body.contains("header.magic"), "{body}");
    }
}
