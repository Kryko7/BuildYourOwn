//! Structured failures: the handshake message that went wrong, its hex, the field path
//! inside it, the transcript hash that was in force and the key-schedule label being
//! derived at that moment.
//!
//! A TLS failure is not one byte in one response: it is a position in a state machine. So a
//! [`Failure`] carries a list of named [`HexBlock`]s (the ClientHello, the ServerHello, the
//! record that would not decrypt) instead of one request and one response, and a `context`
//! block that a test fills with the transcript and the schedule.

use crate::tls::{hex, TlsError};
use std::fmt::Debug;
use std::ops::Range;

/// Why a test failed. Reported separately so a crashed server never looks like a wrong byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// A field did not have the expected value.
    Assertion,
    /// The server did not answer in time.
    Timeout,
    /// The connection could not be opened, or closed unexpectedly.
    Connection,
    /// The bytes on the wire could not be framed as records or handshake messages.
    Protocol,
    /// A signature, a MAC or an AEAD tag did not check out.
    Crypto,
    /// The server sent an alert the test was not expecting.
    Alert,
    /// The server process exited while the test was running.
    ServerCrash,
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
            FailureKind::Crypto => "crypto",
            FailureKind::Alert => "alert",
            FailureKind::ServerCrash => "server crash",
            FailureKind::Harness => "harness error",
        }
    }
}

/// One named byte string shown under a failure.
#[derive(Debug, Clone)]
pub struct HexBlock {
    /// What these bytes are: `client_hello (517 bytes)`, `the record that would not open`.
    pub title: String,
    /// The bytes.
    pub bytes: Vec<u8>,
    /// Byte ranges worth pointing at.
    pub marks: Vec<Range<usize>>,
}

/// Everything the reporter needs to explain one failed test.
#[derive(Debug, Clone)]
pub struct Failure {
    /// What kind of failure this is.
    pub kind: FailureKind,
    /// One line per failed check, e.g. `server_hello.legacy_version: expected 0x0303, got 0x0304`.
    pub messages: Vec<String>,
    /// Labelled values that were expected.
    pub expected: Vec<(String, String)>,
    /// Labelled values that were seen.
    pub actual: Vec<(String, String)>,
    /// Named byte strings to hex-dump, in the order they should be shown.
    pub blocks: Vec<HexBlock>,
    /// Free-form context: what the test was doing, the transcript, the key schedule.
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
            blocks: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// The harness could not set the test up.
    pub fn harness(message: impl Into<String>) -> Failure {
        Failure::new(FailureKind::Harness, message)
    }

    /// Turn a TLS-level error into a failure of the right kind.
    pub fn tls(err: TlsError) -> Failure {
        let kind = match &err {
            TlsError::Timeout(_) => FailureKind::Timeout,
            TlsError::Closed | TlsError::Io(_) => FailureKind::Connection,
            TlsError::Record(_) | TlsError::Decode(_) | TlsError::Protocol(_) => {
                FailureKind::Protocol
            }
            TlsError::Crypto(_) => FailureKind::Crypto,
            TlsError::Alert(_, _) => FailureKind::Alert,
        };
        Failure::new(kind, err.to_string())
    }

    /// Add a context line.
    pub fn note(mut self, note: impl Into<String>) -> Failure {
        self.notes.push(note.into());
        self
    }

    /// Add several context lines.
    pub fn notes(mut self, notes: impl IntoIterator<Item = String>) -> Failure {
        self.notes.extend(notes);
        self
    }

    /// Attach a named byte string.
    pub fn block(mut self, title: impl Into<String>, bytes: &[u8]) -> Failure {
        if !bytes.is_empty() {
            self.blocks.push(HexBlock {
                title: title.into(),
                bytes: bytes.to_vec(),
                marks: Vec::new(),
            });
        }
        self
    }

    /// The hex blocks, with their byte counts in the titles.
    pub fn hex_blocks(&self) -> Vec<(String, Vec<u8>, Vec<Range<usize>>)> {
        self.blocks
            .iter()
            .map(|b| {
                let title = if b.marks.is_empty() {
                    format!("{} ({} bytes)", b.title, b.bytes.len())
                } else {
                    format!(
                        "{} ({} bytes, ^^ marks the bytes holding the actual value)",
                        b.title,
                        b.bytes.len()
                    )
                };
                (title, b.bytes.clone(), b.marks.clone())
            })
            .collect()
    }
}

/// Values whose big-endian wire form can be pointed at in a hex dump.
pub trait WireBytes {
    /// The bytes this value would occupy on the wire, when that is well defined.
    fn wire_bytes(&self) -> Option<Vec<u8>>;
}

macro_rules! wire_num {
    ($($t:ty),*) => {$(
        impl WireBytes for $t {
            fn wire_bytes(&self) -> Option<Vec<u8>> {
                Some(self.to_be_bytes().to_vec())
            }
        }
    )*};
}
wire_num!(i8, i16, i32, i64, u8, u16, u32, u64);

impl WireBytes for bool {
    fn wire_bytes(&self) -> Option<Vec<u8>> {
        Some(vec![u8::from(*self)])
    }
}

impl WireBytes for String {
    fn wire_bytes(&self) -> Option<Vec<u8>> {
        Some(self.as_bytes().to_vec())
    }
}

impl WireBytes for &str {
    fn wire_bytes(&self) -> Option<Vec<u8>> {
        Some(self.as_bytes().to_vec())
    }
}

impl WireBytes for usize {
    fn wire_bytes(&self) -> Option<Vec<u8>> {
        None
    }
}

impl<T: WireBytes> WireBytes for Option<T> {
    fn wire_bytes(&self) -> Option<Vec<u8>> {
        self.as_ref().and_then(WireBytes::wire_bytes)
    }
}

impl<T: WireBytes> WireBytes for Vec<T> {
    fn wire_bytes(&self) -> Option<Vec<u8>> {
        None
    }
}

impl<A, B> WireBytes for (A, B) {
    fn wire_bytes(&self) -> Option<Vec<u8>> {
        None
    }
}

/// Collects field-level checks and turns them into one [`Failure`].
///
/// ```ignore
/// let mut c = Check::new("the ServerHello of a well-formed ClientHello");
/// c.block("server_hello", &client.server_hello_bytes);
/// c.context(&client);
/// c.eq("server_hello.legacy_version", 0x0303u16, hello.legacy_version);
/// c.eq("server_hello.legacy_session_id_echo", sent, hello.legacy_session_id_echo);
/// c.finish()?;
/// ```
pub struct Check {
    what: String,
    failures: Vec<String>,
    expected: Vec<(String, String)>,
    actual: Vec<(String, String)>,
    blocks: Vec<HexBlock>,
    notes: Vec<String>,
}

impl Check {
    /// Start a check block.
    pub fn new(what: impl Into<String>) -> Check {
        Check {
            what: what.into(),
            failures: Vec::new(),
            expected: Vec::new(),
            actual: Vec::new(),
            blocks: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Attach a named byte string to be hex-dumped under a failure.
    pub fn block(&mut self, title: impl Into<String>, bytes: &[u8]) -> &mut Self {
        if !bytes.is_empty() {
            self.blocks.push(HexBlock {
                title: title.into(),
                bytes: bytes.to_vec(),
                marks: Vec::new(),
            });
        }
        self
    }

    /// Add a context line shown under the failure.
    pub fn note(&mut self, note: impl Into<String>) -> &mut Self {
        self.notes.push(note.into());
        self
    }

    /// Add several context lines.
    pub fn note_all(&mut self, notes: impl IntoIterator<Item = String>) -> &mut Self {
        self.notes.extend(notes);
        self
    }

    /// Say which transcript hash and key-schedule label were in force.
    ///
    /// Every check on an encrypted message should carry this: in TLS almost every wrong
    /// byte is really a wrong transcript or a wrong label, and the two lines here are
    /// usually the answer.
    pub fn keying(&mut self, transcript_hash: &[u8], label: &str) -> &mut Self {
        self.notes.push(format!(
            "transcript hash in force: {}",
            hex(transcript_hash)
        ));
        self.notes
            .push(format!("key schedule step in force: {label}"));
        self
    }

    /// Point the last attached block at a byte range.
    pub fn mark(&mut self, range: Range<usize>) -> &mut Self {
        if let Some(b) = self.blocks.last_mut() {
            b.marks.push(range);
        }
        self
    }

    /// Record a value in the report without asserting on it.
    pub fn observe(&mut self, path: &str, value: impl Debug) -> &mut Self {
        self.actual.push((path.to_string(), format!("{value:?}")));
        self
    }

    /// Assert `actual == expected`, naming the decoded field path.
    pub fn eq<T>(&mut self, path: &str, expected: T, actual: T) -> &mut Self
    where
        T: PartialEq + Debug + WireBytes,
    {
        if expected != actual {
            self.failures
                .push(format!("{path}: expected {expected:?}, got {actual:?}"));
            self.expected
                .push((path.to_string(), format!("{expected:?}")));
            self.actual.push((path.to_string(), format!("{actual:?}")));
            self.mark_value(&actual);
        }
        self
    }

    /// Assert `actual != forbidden`.
    pub fn ne<T>(&mut self, path: &str, forbidden: T, actual: T) -> &mut Self
    where
        T: PartialEq + Debug + WireBytes,
    {
        if forbidden == actual {
            self.failures
                .push(format!("{path}: expected anything but {forbidden:?}"));
            self.expected
                .push((path.to_string(), format!("not {forbidden:?}")));
            self.actual.push((path.to_string(), format!("{actual:?}")));
            self.mark_value(&actual);
        }
        self
    }

    /// Assert `actual >= min`.
    pub fn at_least<T>(&mut self, path: &str, min: T, actual: T) -> &mut Self
    where
        T: Ord + Debug + WireBytes,
    {
        if actual < min {
            self.failures
                .push(format!("{path}: expected at least {min:?}, got {actual:?}"));
            self.expected
                .push((path.to_string(), format!(">= {min:?}")));
            self.actual.push((path.to_string(), format!("{actual:?}")));
            self.mark_value(&actual);
        }
        self
    }

    /// Assert `actual <= max`.
    pub fn at_most<T>(&mut self, path: &str, max: T, actual: T) -> &mut Self
    where
        T: Ord + Debug + WireBytes,
    {
        if actual > max {
            self.failures
                .push(format!("{path}: expected at most {max:?}, got {actual:?}"));
            self.expected
                .push((path.to_string(), format!("<= {max:?}")));
            self.actual.push((path.to_string(), format!("{actual:?}")));
            self.mark_value(&actual);
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

    /// Assert two byte strings are identical, marking the first differing byte.
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
            self.expected.push((path.to_string(), hex(expected)));
            self.actual.push((path.to_string(), hex(actual)));
            let mark = std::ops::Range {
                start: at,
                end: at + 1,
            };
            self.blocks.push(HexBlock {
                title: format!("{path}: expected"),
                bytes: expected.to_vec(),
                marks: vec![mark.clone()],
            });
            self.blocks.push(HexBlock {
                title: format!("{path}: actual"),
                bytes: actual.to_vec(),
                marks: vec![mark],
            });
        }
        self
    }

    /// Point every attached block at the region holding the actual value.
    fn mark_value(&mut self, actual: &impl WireBytes) {
        let Some(needle) = actual.wire_bytes() else {
            return;
        };
        if needle.len() < 2 {
            // One-byte values match everywhere; marking them is noise.
            return;
        }
        for block in &mut self.blocks {
            let found = find(&block.bytes, &needle);
            if found.len() <= 8 {
                block.marks.extend(found);
            }
        }
    }

    /// True when nothing has failed so far.
    pub fn ok(&self) -> bool {
        self.failures.is_empty()
    }

    /// How many checks have failed.
    pub fn failures(&self) -> usize {
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
            kind: FailureKind::Assertion,
            messages: std::mem::take(&mut self.failures),
            expected: std::mem::take(&mut self.expected),
            actual: std::mem::take(&mut self.actual),
            blocks: std::mem::take(&mut self.blocks),
            notes,
        })
    }
}

/// Every occurrence of `needle` in `haystack`.
pub fn find(haystack: &[u8], needle: &[u8]) -> Vec<Range<usize>> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in 0..=haystack.len() - needle.len() {
        if &haystack[i..i + needle.len()] == needle {
            out.push(i..i + needle.len());
        }
    }
    out
}

/// Render bytes as an annotated hex dump, marking the given byte ranges with `^`.
pub fn hexdump(bytes: &[u8], marks: &[Range<usize>], max_bytes: usize) -> String {
    let shown = bytes.len().min(max_bytes);
    let mut out = String::new();
    for row in 0..shown.div_ceil(16) {
        let start = row * 16;
        let end = (start + 16).min(shown);
        let mut hex_part = String::new();
        let mut ascii = String::new();
        for (i, cell) in (start..start + 16).enumerate() {
            if i % 8 == 0 && i != 0 {
                hex_part.push(' ');
            }
            match bytes.get(cell).filter(|_| cell < end) {
                Some(&c) => {
                    hex_part.push_str(&format!("{c:02x} "));
                    ascii.push(if (0x20..0x7f).contains(&c) {
                        char::from(c)
                    } else {
                        '.'
                    });
                }
                None => {
                    hex_part.push_str("   ");
                    ascii.push(' ');
                }
            }
        }
        out.push_str(&format!("{start:04x}  {hex_part} |{ascii}|\n"));
        let mut caret = String::new();
        let mut any = false;
        for i in start..end {
            if i % 8 == 0 && i != start {
                caret.push(' ');
            }
            if marks.iter().any(|m| m.contains(&i)) {
                caret.push_str("^^ ");
                any = true;
            } else {
                caret.push_str("   ");
            }
        }
        if any {
            out.push_str(&format!("      {caret}\n"));
        }
    }
    if bytes.len() > shown {
        out.push_str(&format!("      … {} more bytes\n", bytes.len() - shown));
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
        let mut c = Check::new("the ServerHello");
        c.eq("server_hello.legacy_version", 0x0303u16, 0x0304u16);
        let f = c.finish().expect_err("must fail");
        assert_eq!(f.kind, FailureKind::Assertion);
        assert_eq!(
            f.messages,
            vec!["server_hello.legacy_version: expected 771, got 772"]
        );
        assert!(f.notes[0].contains("the ServerHello"));
    }

    #[test]
    fn a_failing_check_marks_the_bytes_in_every_attached_block() {
        let mut c = Check::new("the ServerHello");
        c.block("server_hello", &[2, 0, 0, 5, 0x03, 0x04, 9, 9, 9]);
        c.eq("server_hello.legacy_version", 0x0303u16, 0x0304u16);
        let f = c.finish().expect_err("must fail");
        assert_eq!(f.blocks[0].marks, vec![4..6]);
        let dump = hexdump(&f.blocks[0].bytes, &f.blocks[0].marks, 64);
        assert!(dump.contains("^^"), "{dump}");
    }

    #[test]
    fn keying_records_the_transcript_and_the_label() {
        let mut c = Check::new("the server's Finished");
        c.keying(&[0xab; 4], "s hs traffic → server_handshake_traffic_secret");
        c.eq("finished.verify_data", 1u16, 2u16);
        let f = c.finish().expect_err("must fail");
        assert!(f
            .notes
            .iter()
            .any(|n| n.contains("transcript hash in force: abababab")));
        assert!(f.notes.iter().any(|n| n.contains("s hs traffic")));
    }

    #[test]
    fn bytes_eq_points_at_the_first_difference_in_both_blocks() {
        let mut c = Check::new("verify_data");
        c.bytes_eq("finished.verify_data", &[1, 2, 3], &[1, 9, 3]);
        let f = c.finish().expect_err("must fail");
        assert!(
            f.messages[0].contains("first difference at byte 1"),
            "{:?}",
            f.messages
        );
        assert_eq!(f.blocks.len(), 2);
        assert_eq!(f.blocks[0].marks, vec![1..2]);
    }

    #[test]
    fn tls_errors_pick_the_right_kind() {
        assert_eq!(Failure::tls(TlsError::Closed).kind, FailureKind::Connection);
        assert_eq!(
            Failure::tls(TlsError::Timeout("x".into())).kind,
            FailureKind::Timeout
        );
        assert_eq!(
            Failure::tls(TlsError::Record("x".into())).kind,
            FailureKind::Protocol
        );
        assert_eq!(
            Failure::tls(TlsError::Crypto("x".into())).kind,
            FailureKind::Crypto
        );
        assert_eq!(
            Failure::tls(TlsError::Alert(
                crate::tls::AlertLevel::Fatal,
                crate::tls::AlertDescription::HANDSHAKE_FAILURE
            ))
            .kind,
            FailureKind::Alert
        );
    }

    #[test]
    fn the_hex_dump_shows_offsets_ascii_and_a_truncation_note() {
        let dump = hexdump(&(0..40u8).collect::<Vec<u8>>(), &[], 32);
        assert!(dump.starts_with("0000  00 01 02"), "{dump}");
        assert!(dump.contains("8 more bytes"), "{dump}");
    }
}
