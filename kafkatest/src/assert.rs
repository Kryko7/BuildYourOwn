//! Structured failures: decoded field paths, hex dumps with the differing region marked,
//! and the broker's own output.

use crate::proto::{find_be, hexdump, Conn, ProtoError};
use std::fmt::Debug;
use std::ops::Range;

/// Why a test failed. Reported separately so a crashed broker never looks like a wrong byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// A field did not have the expected value.
    Assertion,
    /// The broker did not answer in time.
    Timeout,
    /// The connection could not be opened, or was closed unexpectedly.
    Connection,
    /// The bytes on the wire could not be framed or decoded.
    Protocol,
    /// The broker process exited while the test was running.
    BrokerCrash,
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
            FailureKind::BrokerCrash => "broker crash",
            FailureKind::Harness => "harness error",
        }
    }
}

/// Everything the reporter needs to explain one failed test.
#[derive(Debug, Clone)]
pub struct Failure {
    /// What kind of failure this is.
    pub kind: FailureKind,
    /// One line per failed check, e.g. `response.topics[0].error_code: expected 3, got 0`.
    pub messages: Vec<String>,
    /// Labelled values that were expected.
    pub expected: Vec<(String, String)>,
    /// Labelled values that were seen.
    pub actual: Vec<(String, String)>,
    /// The request frame, without the length prefix.
    pub request: Option<Vec<u8>>,
    /// The response frame, without the length prefix.
    pub response: Option<Vec<u8>>,
    /// Byte ranges in the response worth pointing at.
    pub marks: Vec<Range<usize>>,
    /// Free-form context lines (what the test was doing, fixture names, ...).
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
            request: None,
            response: None,
            marks: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// The harness could not set the test up.
    pub fn harness(message: impl Into<String>) -> Failure {
        Failure::new(FailureKind::Harness, message)
    }

    /// Turn a protocol-level error into a failure, attaching the connection's last bytes.
    pub fn proto(err: ProtoError, conn: Option<&Conn>) -> Failure {
        let kind = match &err {
            ProtoError::Timeout(_) => FailureKind::Timeout,
            ProtoError::Closed | ProtoError::Io(_) => FailureKind::Connection,
            ProtoError::Frame(_) | ProtoError::Decode(_) => FailureKind::Protocol,
        };
        let mut f = Failure::new(kind, err.to_string());
        if let Some(c) = conn {
            f.request = c.last_request.clone();
            f.response = c.last_response.clone();
        }
        f
    }

    /// Add a context line.
    pub fn note(mut self, note: impl Into<String>) -> Failure {
        self.notes.push(note.into());
        self
    }

    /// Attach the request/response bytes of a connection.
    pub fn with_conn(mut self, conn: &Conn) -> Failure {
        self.request = conn.last_request.clone();
        self.response = conn.last_response.clone();
        self
    }

    /// The hex dump block shown under a failure, request first.
    pub fn hex_blocks(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if let Some(r) = &self.request {
            out.push((
                format!("request bytes ({} bytes, length prefix omitted)", r.len()),
                hexdump(r, &[], 512),
            ));
        }
        if let Some(r) = &self.response {
            let title = if self.marks.is_empty() {
                format!("response bytes ({} bytes, length prefix omitted)", r.len())
            } else {
                format!(
                    "response bytes ({} bytes, ^^ marks the bytes holding the actual value)",
                    r.len()
                )
            };
            out.push((title, hexdump(r, &self.marks, 512)));
        }
        out
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

impl WireBytes for uuid::Uuid {
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
/// let mut c = Check::new("DescribeTopicPartitions v0 for an unknown topic", &conn);
/// c.eq("response.topics[0].error_code", 3i16, topic.error_code);
/// c.eq("response.topics[0].topic_id", Uuid::nil(), topic.topic_id);
/// c.finish()?;
/// ```
pub struct Check {
    what: String,
    failures: Vec<String>,
    expected: Vec<(String, String)>,
    actual: Vec<(String, String)>,
    request: Option<Vec<u8>>,
    response: Option<Vec<u8>>,
    marks: Vec<Range<usize>>,
    notes: Vec<String>,
}

impl Check {
    /// Start a check block against the bytes currently on `conn`.
    pub fn new(what: impl Into<String>, conn: &Conn) -> Check {
        Check {
            what: what.into(),
            failures: Vec::new(),
            expected: Vec::new(),
            actual: Vec::new(),
            request: conn.last_request.clone(),
            response: conn.last_response.clone(),
            marks: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Start a check block with no bytes attached (used for on-disk assertions).
    pub fn detached(what: impl Into<String>) -> Check {
        Check {
            what: what.into(),
            failures: Vec::new(),
            expected: Vec::new(),
            actual: Vec::new(),
            request: None,
            response: None,
            marks: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Replace the attached request/response bytes.
    pub fn bytes(&mut self, conn: &Conn) -> &mut Self {
        self.request = conn.last_request.clone();
        self.response = conn.last_response.clone();
        self
    }

    /// Add a context line shown under the failure.
    pub fn note(&mut self, note: impl Into<String>) -> &mut Self {
        self.notes.push(note.into());
        self
    }

    /// Point the hex dump at a byte range of the response the test already knows.
    pub fn mark(&mut self, range: Range<usize>) -> &mut Self {
        self.marks.push(range);
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

    /// Assert `actual >= min` (version ranges, offsets, watermarks).
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

    /// Point the hex dump at every region holding the actual value.
    fn mark_value(&mut self, actual: &impl WireBytes) {
        let (Some(resp), Some(needle)) = (&self.response, actual.wire_bytes()) else {
            return;
        };
        if needle.len() < 2 {
            // One-byte values match everywhere; marking them is noise.
            return;
        }
        let found = find_be(resp, &needle);
        if found.len() <= 8 {
            self.marks.extend(found);
        }
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
            request: self.request.take(),
            response: self.response.take(),
            marks: std::mem::take(&mut self.marks),
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
        c.eq("a.b", 1i32, 1i32).at_least("a.c", 2i16, 4i16);
        assert!(c.ok());
        assert!(c.finish().is_ok());
    }

    #[test]
    fn failures_name_the_field_path() {
        let mut c = Check::detached("error codes");
        c.eq("response.topics[0].error_code", 3i16, 0i16);
        let f = c.finish().expect_err("must fail");
        assert_eq!(f.kind, FailureKind::Assertion);
        assert_eq!(
            f.messages,
            vec!["response.topics[0].error_code: expected 3, got 0"]
        );
        assert!(f.notes[0].contains("error codes"));
    }

    #[test]
    fn at_least_and_at_most_report_bounds() {
        let mut c = Check::detached("versions");
        c.at_least("api_keys[18].max_version", 4i16, 3i16);
        c.at_most("api_keys[18].min_version", 4i16, 9i16);
        let f = c.finish().expect_err("must fail");
        assert_eq!(f.messages.len(), 2);
        assert!(f.messages[0].contains("at least 4"), "{:?}", f.messages);
        assert!(f.messages[1].contains("at most 4"), "{:?}", f.messages);
    }

    #[test]
    fn bytes_eq_points_at_the_first_difference() {
        let mut c = Check::detached("segment bytes");
        c.bytes_eq("segment", &[1, 2, 3], &[1, 9, 3]);
        let f = c.finish().expect_err("must fail");
        assert!(
            f.messages[0].contains("first difference at byte 1"),
            "{:?}",
            f.messages
        );
    }

    #[test]
    fn proto_errors_pick_the_right_kind() {
        let f = Failure::proto(ProtoError::Closed, None);
        assert_eq!(f.kind, FailureKind::Connection);
        let f = Failure::proto(ProtoError::Timeout("x".into()), None);
        assert_eq!(f.kind, FailureKind::Timeout);
        let f = Failure::proto(ProtoError::Frame("x".into()), None);
        assert_eq!(f.kind, FailureKind::Protocol);
    }
}
