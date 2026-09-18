//! Structured failures: what was expected, what the linker actually produced, the command
//! line that produced it, and whatever block of evidence explains the difference — the
//! linker's stderr, the output's program header table, a hex dump with the bytes a
//! relocation should have patched marked.

use crate::elf::hexdump;
use std::fmt::Debug;
use std::ops::Range;

/// Why a test failed. Reported separately so a linker that crashed never looks like a linker
/// that emitted one wrong byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// Something in the output did not have the expected value.
    Assertion,
    /// The linker, or the program it produced, did not finish in time.
    Timeout,
    /// The linker refused a link that should have succeeded.
    LinkFailed,
    /// The linker accepted a link that should have been an error.
    LinkAccepted,
    /// The linker was killed by a signal.
    LinkerCrash,
    /// The output is not an ELF file this parser can read.
    MalformedOutput,
    /// The produced binary died: a signal, or a status nobody asked for.
    ProgramCrash,
    /// The harness itself could not set the test up.
    Harness,
}

impl FailureKind {
    /// Short label used in the terminal report.
    pub fn label(self) -> &'static str {
        match self {
            FailureKind::Assertion => "assertion",
            FailureKind::Timeout => "timeout",
            FailureKind::LinkFailed => "link failed",
            FailureKind::LinkAccepted => "link accepted",
            FailureKind::LinkerCrash => "linker crash",
            FailureKind::MalformedOutput => "malformed output",
            FailureKind::ProgramCrash => "program crash",
            FailureKind::Harness => "harness error",
        }
    }
}

/// Everything the reporter needs to explain one failed test.
#[derive(Debug, Clone)]
pub struct Failure {
    /// What kind of failure this is.
    pub kind: FailureKind,
    /// One line per failed check, e.g. `output.e_entry: expected 0x401000, got 0x0`.
    pub messages: Vec<String>,
    /// Labelled values that were expected.
    pub expected: Vec<(String, String)>,
    /// Labelled values that were seen.
    pub actual: Vec<(String, String)>,
    /// Free-form context lines (what the test was doing, which object was which).
    pub notes: Vec<String>,
    /// Titled blocks of evidence, printed under the failure in order.
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

    /// The harness could not set the test up (could not write a file, could not build an
    /// object, could not start the linker).
    pub fn harness(message: impl Into<String>) -> Failure {
        Failure::new(FailureKind::Harness, message)
    }

    /// Add a context line.
    pub fn note(mut self, note: impl Into<String>) -> Failure {
        self.notes.push(note.into());
        self
    }

    /// Attach a titled block of evidence.
    pub fn block(mut self, title: impl Into<String>, body: impl Into<String>) -> Failure {
        let body = body.into();
        if !body.trim().is_empty() {
            self.blocks.push((title.into(), body));
        }
        self
    }

    /// Attach a hex dump with some ranges marked.
    pub fn hex(self, title: impl Into<String>, bytes: &[u8], marks: &[Range<usize>]) -> Failure {
        self.block(title, hexdump(bytes, marks, 512))
    }
}

/// Values whose little-endian wire form can be pointed at in a hex dump.
pub trait Bytes {
    /// The bytes this value would occupy in a file, when that is well defined.
    fn le_bytes(&self) -> Option<Vec<u8>>;
}

macro_rules! le_num {
    ($($t:ty),*) => {$(
        impl Bytes for $t {
            fn le_bytes(&self) -> Option<Vec<u8>> {
                Some(self.to_le_bytes().to_vec())
            }
        }
    )*};
}
le_num!(i8, i16, i32, i64, u8, u16, u32, u64);

impl Bytes for bool {
    fn le_bytes(&self) -> Option<Vec<u8>> {
        Some(vec![u8::from(*self)])
    }
}

impl Bytes for String {
    fn le_bytes(&self) -> Option<Vec<u8>> {
        Some(self.as_bytes().to_vec())
    }
}

impl Bytes for &str {
    fn le_bytes(&self) -> Option<Vec<u8>> {
        Some(self.as_bytes().to_vec())
    }
}

impl Bytes for usize {
    fn le_bytes(&self) -> Option<Vec<u8>> {
        None
    }
}

impl<T: Bytes> Bytes for Option<T> {
    fn le_bytes(&self) -> Option<Vec<u8>> {
        self.as_ref().and_then(Bytes::le_bytes)
    }
}

impl<T: Bytes> Bytes for Vec<T> {
    fn le_bytes(&self) -> Option<Vec<u8>> {
        None
    }
}

impl<A, B> Bytes for (A, B) {
    fn le_bytes(&self) -> Option<Vec<u8>> {
        None
    }
}

/// Collects field-level checks and turns them into one [`Failure`].
///
/// ```ignore
/// let mut c = Check::new("the ELF header of the output");
/// c.eq("output.e_type", ET_EXEC, exe.e_type);
/// c.eq("output.e_entry", start_addr, exe.entry);
/// c.finish()?;
/// ```
pub struct Check {
    what: String,
    failures: Vec<String>,
    expected: Vec<(String, String)>,
    actual: Vec<(String, String)>,
    notes: Vec<String>,
    blocks: Vec<(String, String)>,
}

impl Check {
    /// Start a check block.
    pub fn new(what: impl Into<String>) -> Check {
        Check {
            what: what.into(),
            failures: Vec::new(),
            expected: Vec::new(),
            actual: Vec::new(),
            notes: Vec::new(),
            blocks: Vec::new(),
        }
    }

    /// Add a context line shown under the failure.
    pub fn note(&mut self, note: impl Into<String>) -> &mut Self {
        self.notes.push(note.into());
        self
    }

    /// Attach a titled block of evidence shown under the failure.
    pub fn block(&mut self, title: impl Into<String>, body: impl Into<String>) -> &mut Self {
        let body = body.into();
        if !body.trim().is_empty() {
            self.blocks.push((title.into(), body));
        }
        self
    }

    /// Attach a hex dump with some ranges marked.
    pub fn hex(&mut self, title: impl Into<String>, bytes: &[u8], marks: &[Range<usize>]) -> &mut Self {
        self.block(title, hexdump(bytes, marks, 512))
    }

    /// Record a value in the report without asserting on it.
    pub fn observe(&mut self, path: &str, value: impl Debug) -> &mut Self {
        self.actual.push((path.to_string(), format!("{value:?}")));
        self
    }

    /// Assert `actual == expected`, naming the field path.
    pub fn eq<T>(&mut self, path: &str, expected: T, actual: T) -> &mut Self
    where
        T: PartialEq + Debug + Bytes,
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

    /// Assert `actual == expected` for an address, printed in hex on both sides.
    pub fn addr_eq(&mut self, path: &str, expected: u64, actual: u64) -> &mut Self {
        if expected != actual {
            self.failures.push(format!(
                "{path}: expected 0x{expected:x}, got 0x{actual:x}"
            ));
            self.expected
                .push((path.to_string(), format!("0x{expected:x}")));
            self.actual.push((path.to_string(), format!("0x{actual:x}")));
        }
        self
    }

    /// Assert `actual != forbidden`.
    pub fn ne<T>(&mut self, path: &str, forbidden: T, actual: T) -> &mut Self
    where
        T: PartialEq + Debug + Bytes,
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
        T: Ord + Debug + Bytes,
    {
        if actual < min {
            self.failures
                .push(format!("{path}: expected at least {min:?}, got {actual:?}"));
            self.expected.push((path.to_string(), format!(">= {min:?}")));
            self.actual.push((path.to_string(), format!("{actual:?}")));
        }
        self
    }

    /// Assert `actual <= max`.
    pub fn at_most<T>(&mut self, path: &str, max: T, actual: T) -> &mut Self
    where
        T: Ord + Debug + Bytes,
    {
        if actual > max {
            self.failures
                .push(format!("{path}: expected at most {max:?}, got {actual:?}"));
            self.expected.push((path.to_string(), format!("<= {max:?}")));
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

    /// Assert two byte strings are identical, pointing at the first difference.
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
                hexdump(expected, std::slice::from_ref(&(at..at + 1)), 256),
            ));
            self.actual.push((
                path.to_string(),
                hexdump(actual, std::slice::from_ref(&(at..at + 1)), 256),
            ));
        }
        self
    }

    /// Assert that a diagnostic mentions a symbol (or file) name.
    ///
    /// Deliberately loose: the suite never pins a linker's wording, only that the message
    /// names the thing the learner has to go and look at.
    pub fn mentions(&mut self, path: &str, needle: &str, haystack: &str) -> &mut Self {
        if !haystack.contains(needle) {
            self.failures.push(format!(
                "{path}: the diagnostic never mentions '{needle}'"
            ));
            self.expected
                .push((path.to_string(), format!("a message naming '{needle}'")));
            self.actual
                .push((path.to_string(), one_line(haystack, 200)));
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
            notes,
            blocks: std::mem::take(&mut self.blocks),
        })
    }
}

/// Squash a multi-line string onto one line for a report row.
pub fn one_line(text: &str, limit: usize) -> String {
    let joined: String = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" / ");
    if joined.chars().count() > limit {
        let cut: String = joined.chars().take(limit).collect();
        format!("{cut}…")
    } else {
        joined
    }
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
        let mut c = Check::new("the entry point");
        c.addr_eq("output.e_entry", 0x401000, 0);
        let f = c.finish().expect_err("must fail");
        assert_eq!(f.kind, FailureKind::Assertion);
        assert_eq!(f.messages, vec!["output.e_entry: expected 0x401000, got 0x0"]);
        assert!(f.notes[0].contains("the entry point"));
    }

    #[test]
    fn mentions_is_about_the_symbol_not_the_wording() {
        let mut c = Check::new("the diagnostic");
        c.mentions(
            "linker.stderr",
            "other",
            "ld: a.o: undefined reference to `other'",
        );
        assert!(c.ok(), "any wording naming the symbol is fine");
        c.mentions("linker.stderr", "missing_one", "ld: something went wrong");
        assert!(!c.ok());
    }

    #[test]
    fn bytes_eq_points_at_the_first_difference() {
        let mut c = Check::new("relocated bytes");
        c.bytes_eq("output..text", &[1, 2, 3], &[1, 9, 3]);
        let f = c.finish().expect_err("must fail");
        assert!(
            f.messages[0].contains("first difference at byte 1"),
            "{:?}",
            f.messages
        );
    }

    #[test]
    fn one_line_squashes_and_truncates() {
        assert_eq!(one_line("a\n\nb\n", 100), "a / b");
        assert_eq!(one_line("abcdef", 3), "abc…");
    }
}
