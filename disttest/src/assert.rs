//! Structured failures: decoded field paths, the request and response that produced them,
//! unified diffs for text, and the process output of whichever node misbehaved.
//!
//! Nothing in a stage ever builds a message by hand. A test opens a [`Check`], names the
//! field paths it cares about, and calls [`Check::finish`]; what the learner sees is then
//! the same shape for every stage of every ladder.

use similar::{ChangeTag, TextDiff};
use std::fmt::Debug;

/// Why a test failed. Reported separately so a crashed node never looks like a wrong value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// A field did not have the expected value.
    Assertion,
    /// The program did not answer in time.
    Timeout,
    /// A connection could not be opened, or was closed unexpectedly.
    Connection,
    /// The bytes on the wire could not be parsed as the protocol says.
    Protocol,
    /// A node process exited while the test was running.
    NodeCrash,
    /// The recorded history admits no linearization.
    Linearizability,
    /// The harness itself could not set the test up.
    Harness,
}

impl FailureKind {
    /// Short label used in the terminal report.
    pub fn label(self) -> &'static str {
        match self {
            FailureKind::Assertion => "assertion",
            FailureKind::Timeout => "timeout",
            FailureKind::Connection => "connection",
            FailureKind::Protocol => "protocol",
            FailureKind::NodeCrash => "node crash",
            FailureKind::Linearizability => "not linearizable",
            FailureKind::Harness => "harness error",
        }
    }
}

/// Everything the reporter needs to explain one failed test.
#[derive(Debug, Clone)]
pub struct Failure {
    /// What kind of failure this is.
    pub kind: FailureKind,
    /// One line per failed check, e.g. `range.kvs[0].mod_revision: expected 3, got 2`.
    pub messages: Vec<String>,
    /// Labelled values that were expected.
    pub expected: Vec<(String, String)>,
    /// Labelled values that were seen.
    pub actual: Vec<(String, String)>,
    /// Free-form context lines (what the test was doing, which member, which seed, ...).
    pub notes: Vec<String>,
    /// Titled blocks printed verbatim: request/response bodies, transcripts, histories.
    pub blocks: Vec<(String, String)>,
}

impl Failure {
    /// A failure of the given kind with a single message.
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Failure {
        Failure {
            kind,
            messages: vec![message.into()],
            expected: Vec::new(),
            actual: Vec::new(),
            notes: Vec::new(),
            blocks: Vec::new(),
        }
    }

    /// The harness could not set the test up.
    pub fn harness(message: impl Into<String>) -> Failure {
        Failure::new(FailureKind::Harness, message)
    }

    /// The program did not speak the protocol.
    pub fn protocol(message: impl Into<String>) -> Failure {
        Failure::new(FailureKind::Protocol, message)
    }

    /// Add a context line.
    pub fn note(mut self, note: impl Into<String>) -> Failure {
        self.notes.push(note.into());
        self
    }

    /// Attach a titled block printed verbatim under the failure.
    pub fn block(mut self, title: impl Into<String>, body: impl Into<String>) -> Failure {
        self.blocks.push((title.into(), body.into()));
        self
    }
}

/// Collects field-level checks and turns them into one [`Failure`].
///
/// ```ignore
/// let mut c = Check::new("range of a key that was never written");
/// c.eq("range.count", 0i64, resp.count);
/// c.eq("range.kvs.len()", 0usize, resp.kvs.len());
/// c.finish()?;
/// ```
pub struct Check {
    what: String,
    failures: Vec<String>,
    expected: Vec<(String, String)>,
    actual: Vec<(String, String)>,
    notes: Vec<String>,
    blocks: Vec<(String, String)>,
    kind: FailureKind,
}

impl Check {
    /// Start a check block describing what is being verified.
    pub fn new(what: impl Into<String>) -> Check {
        Check {
            what: what.into(),
            failures: Vec::new(),
            expected: Vec::new(),
            actual: Vec::new(),
            notes: Vec::new(),
            blocks: Vec::new(),
            kind: FailureKind::Assertion,
        }
    }

    /// Report this block's failures as something other than a plain assertion.
    pub fn kind(&mut self, kind: FailureKind) -> &mut Self {
        self.kind = kind;
        self
    }

    /// Add a context line shown under the failure.
    pub fn note(&mut self, note: impl Into<String>) -> &mut Self {
        self.notes.push(note.into());
        self
    }

    /// Attach a titled block printed verbatim (a request body, a transcript, a history).
    pub fn block(&mut self, title: impl Into<String>, body: impl Into<String>) -> &mut Self {
        self.blocks.push((title.into(), body.into()));
        self
    }

    /// Record a value in the report without asserting on it.
    pub fn observe(&mut self, path: &str, value: impl Debug) -> &mut Self {
        self.actual.push((path.to_string(), format!("{value:?}")));
        self
    }

    fn fail(&mut self, path: &str, expected: String, actual: String, message: String) {
        self.failures.push(message);
        self.expected.push((path.to_string(), expected));
        self.actual.push((path.to_string(), actual));
    }

    /// Assert `actual == expected`, naming the decoded field path.
    pub fn eq<T>(&mut self, path: &str, expected: T, actual: T) -> &mut Self
    where
        T: PartialEq + Debug,
    {
        if expected != actual {
            self.fail(
                path,
                format!("{expected:?}"),
                format!("{actual:?}"),
                format!("{path}: expected {expected:?}, got {actual:?}"),
            );
        }
        self
    }

    /// Assert `actual != forbidden`.
    pub fn ne<T>(&mut self, path: &str, forbidden: T, actual: T) -> &mut Self
    where
        T: PartialEq + Debug,
    {
        if forbidden == actual {
            self.fail(
                path,
                format!("not {forbidden:?}"),
                format!("{actual:?}"),
                format!("{path}: expected anything but {forbidden:?}"),
            );
        }
        self
    }

    /// Assert `actual >= min` (revisions, terms, counts).
    pub fn at_least<T>(&mut self, path: &str, min: T, actual: T) -> &mut Self
    where
        T: PartialOrd + Debug,
    {
        if actual < min {
            self.fail(
                path,
                format!(">= {min:?}"),
                format!("{actual:?}"),
                format!("{path}: expected at least {min:?}, got {actual:?}"),
            );
        }
        self
    }

    /// Assert `actual <= max`.
    pub fn at_most<T>(&mut self, path: &str, max: T, actual: T) -> &mut Self
    where
        T: PartialOrd + Debug,
    {
        if actual > max {
            self.fail(
                path,
                format!("<= {max:?}"),
                format!("{actual:?}"),
                format!("{path}: expected at most {max:?}, got {actual:?}"),
            );
        }
        self
    }

    /// Assert `lo <= actual <= hi`, the shape every statistical bound takes.
    pub fn within<T>(&mut self, path: &str, lo: T, hi: T, actual: T) -> &mut Self
    where
        T: PartialOrd + Debug,
    {
        if actual < lo || actual > hi {
            self.fail(
                path,
                format!("between {lo:?} and {hi:?}"),
                format!("{actual:?}"),
                format!("{path}: expected between {lo:?} and {hi:?}, got {actual:?}"),
            );
        }
        self
    }

    /// Assert an arbitrary condition, describing what was expected.
    pub fn that(&mut self, path: &str, expected: &str, ok: bool, actual: impl Debug) -> &mut Self {
        if !ok {
            self.fail(
                path,
                expected.to_string(),
                format!("{actual:?}"),
                format!("{path}: expected {expected}, got {actual:?}"),
            );
        }
        self
    }

    /// Assert two multi-line strings are identical, attaching a unified diff when they differ.
    pub fn text_eq(&mut self, path: &str, expected: &str, actual: &str) -> &mut Self {
        if expected != actual {
            self.failures
                .push(format!("{path}: the text differs from what was expected"));
            self.expected.push((path.to_string(), first_line(expected)));
            self.actual.push((path.to_string(), first_line(actual)));
            self.blocks.push((
                format!("{path} (- expected, + actual)"),
                diff(expected, actual),
            ));
        }
        self
    }

    /// Assert two JSON values are equal, attaching a diff of their pretty forms.
    pub fn json_eq(
        &mut self,
        path: &str,
        expected: &serde_json::Value,
        actual: &serde_json::Value,
    ) -> &mut Self {
        if expected != actual {
            self.failures
                .push(format!("{path}: the JSON differs from what was expected"));
            let (e, a) = (pretty(expected), pretty(actual));
            self.expected.push((path.to_string(), first_line(&e)));
            self.actual.push((path.to_string(), first_line(&a)));
            self.blocks
                .push((format!("{path} (- expected, + actual)"), diff(&e, &a)));
        }
        self
    }

    /// True when nothing has failed so far.
    pub fn ok(&self) -> bool {
        self.failures.is_empty()
    }

    /// How many checks have failed so far.
    pub fn failed(&self) -> usize {
        self.failures.len()
    }

    /// Turn the collected checks into a result.
    pub fn finish(&mut self) -> Result<(), Failure> {
        if self.failures.is_empty() {
            return Ok(());
        }
        let mut notes = vec![format!("while checking {}", self.what)];
        notes.append(&mut self.notes);
        Err(Failure {
            kind: self.kind,
            messages: std::mem::take(&mut self.failures),
            expected: std::mem::take(&mut self.expected),
            actual: std::mem::take(&mut self.actual),
            notes,
            blocks: std::mem::take(&mut self.blocks),
        })
    }
}

fn first_line(s: &str) -> String {
    let line = s.lines().next().unwrap_or_default();
    if s.lines().count() > 1 {
        format!("{line} ... ({} lines)", s.lines().count())
    } else {
        line.to_string()
    }
}

fn pretty(v: &serde_json::Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

/// A unified diff, `-` for expected and `+` for actual, capped so a report stays readable.
pub fn diff(expected: &str, actual: &str) -> String {
    let d = TextDiff::from_lines(expected, actual);
    let mut out = String::new();
    let mut shown = 0usize;
    for change in d.iter_all_changes() {
        let sign = match change.tag() {
            ChangeTag::Delete => '-',
            ChangeTag::Insert => '+',
            ChangeTag::Equal => ' ',
        };
        if change.tag() == ChangeTag::Equal && shown > 40 {
            continue;
        }
        shown += 1;
        if shown > 120 {
            out.push_str("... (diff truncated)\n");
            break;
        }
        out.push(sign);
        out.push(' ');
        out.push_str(change.value().trim_end_matches('\n'));
        out.push('\n');
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passing_checks_produce_no_failure() {
        let mut c = Check::new("nothing");
        c.eq("a.b", 1i32, 1i32).at_least("a.c", 2i16, 4i16);
        assert!(c.ok());
        assert!(c.finish().is_ok());
    }

    #[test]
    fn failures_name_the_field_path() {
        let mut c = Check::new("the response header");
        c.eq("put.header.revision", 3i64, 2i64);
        let f = c.finish().expect_err("must fail");
        assert_eq!(f.kind, FailureKind::Assertion);
        assert_eq!(f.messages, vec!["put.header.revision: expected 3, got 2"]);
        assert!(f.notes[0].contains("the response header"));
    }

    #[test]
    fn bounds_report_which_side_was_crossed() {
        let mut c = Check::new("bounds");
        c.at_least("hll.estimate", 900u64, 500u64);
        c.at_most("hll.estimate", 1100u64, 5000u64);
        c.within("bloom.fpr", 0.0f64, 0.05, 0.4);
        let f = c.finish().expect_err("must fail");
        assert_eq!(f.messages.len(), 3);
        assert!(f.messages[0].contains("at least 900"), "{:?}", f.messages);
        assert!(f.messages[1].contains("at most 1100"), "{:?}", f.messages);
        assert!(
            f.messages[2].contains("between 0.0 and 0.05"),
            "{:?}",
            f.messages
        );
    }

    #[test]
    fn text_diff_is_attached_and_signed() {
        let mut c = Check::new("transcript");
        c.text_eq("stdout", "a\nb\nc\n", "a\nx\nc\n");
        let f = c.finish().expect_err("must fail");
        let (title, body) = &f.blocks[0];
        assert!(title.contains("expected"), "{title}");
        assert!(body.contains("- b"), "{body}");
        assert!(body.contains("+ x"), "{body}");
    }

    #[test]
    fn json_diff_pretty_prints_both_sides() {
        let mut c = Check::new("body");
        c.json_eq(
            "response",
            &serde_json::json!({"count": 1}),
            &serde_json::json!({"count": 2}),
        );
        let f = c.finish().expect_err("must fail");
        assert!(f.blocks[0].1.contains("\"count\""), "{:?}", f.blocks);
    }

    #[test]
    fn a_check_can_choose_its_failure_kind() {
        let mut c = Check::new("history");
        c.kind(FailureKind::Linearizability)
            .that("history", "a linearization", false, "none");
        assert_eq!(
            c.finish().expect_err("must fail").kind,
            FailureKind::Linearizability
        );
    }
}
