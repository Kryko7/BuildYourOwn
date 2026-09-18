//! Worked examples: what a broker receives and must answer, byte for byte.
//!
//! Every stage carries one to three [`ExampleSpec`]s. A spec says, in words, what the
//! request and the response are, and carries a builder that produces the *exact* bytes the
//! harness would send, encoded with the same codec the tests use — so an example can never
//! drift away from the suite.
//!
//! `kafkatest --capture-examples examples/captured.json --broker apache_kafka` boots the
//! reference broker, materializes each example's fixtures, sends the request and records
//! what Apache Kafka answered, together with a field-by-field annotation of both byte
//! strings ([`annotate`]). `--list --json` merges that file into `catalog.json` so the site
//! can render the bytes with every field highlighted.

pub mod annotate;
pub mod capture;
mod schema_core;
mod schema_data;
mod schema_groups;

use crate::fixtures::{FixtureHandle, FixtureSpec};
use crate::proto::encode_request;
use anyhow::{Context, Result};
use kafka_protocol::protocol::Request;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use uuid::Uuid;

/// The group id every group example uses, so the bytes are stable.
pub const EXAMPLE_GROUP: &str = "kafkatest-group";
/// The client id every example sends.
pub const EXAMPLE_CLIENT_ID: &str = "kafkatest";

// ---------------------------------------------------------------------------------------
// What a stage declares
// ---------------------------------------------------------------------------------------

/// What the harness does after writing an example's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    /// Read one response frame per request frame.
    Response,
    /// A correct broker answers nothing at all (`acks=0`).
    Silence,
    /// A correct broker closes the connection instead of answering.
    Closed,
    /// There is no wire exchange; `request`/`response` carry the whole example.
    Nothing,
}

impl Expect {
    /// The string that goes into the JSON as `kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            Expect::Response => "wire",
            Expect::Silence => "silence",
            Expect::Closed => "closed",
            Expect::Nothing => "text",
        }
    }
}

/// The bytes an example puts on the wire.
#[derive(Debug, Clone)]
pub enum Wire {
    /// One payload per frame; the harness prefixes each with its 4-byte size.
    Frames(Vec<Vec<u8>>),
    /// Exactly these bytes, size prefixes included (framing and fuzz examples).
    Raw(Vec<u8>),
}

impl Wire {
    /// The bytes as they go on the wire, size prefixes included.
    pub fn bytes(&self) -> Vec<u8> {
        match self {
            Wire::Raw(b) => b.clone(),
            Wire::Frames(frames) => {
                let mut out = Vec::new();
                for f in frames {
                    out.extend_from_slice(&(f.len() as i32).to_be_bytes());
                    out.extend_from_slice(f);
                }
                out
            }
        }
    }
}

/// A topic that exists while an example is captured.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExampleTopic {
    /// The real topic name on the broker.
    pub name: String,
    /// The real topic id; a fresh one every boot.
    pub id: String,
    /// How many partitions it has.
    pub partitions: i32,
}

impl ExampleTopic {
    /// The topic id as a `Uuid`, or the nil uuid when the capture recorded something odd.
    pub fn uuid(&self) -> Uuid {
        Uuid::parse_str(&self.id).unwrap_or(Uuid::nil())
    }
}

/// Everything an example's request builder may look at.
///
/// It is recorded into `captured.json` alongside the bytes, so the freshness test can
/// rebuild the request offline and prove the hex still matches.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExampleEnv {
    /// Fixture topics by the logical key the spec declared (`"t1"`).
    pub topics: BTreeMap<String, ExampleTopic>,
    /// The group id group examples use.
    pub group: String,
}

impl ExampleEnv {
    /// Build an env from a materialized fixture handle.
    pub fn from_fixtures(handle: &FixtureHandle) -> ExampleEnv {
        ExampleEnv {
            topics: handle
                .all()
                .map(|t| {
                    (
                        t.key.clone(),
                        ExampleTopic {
                            name: t.name.clone(),
                            id: t.id.to_string(),
                            partitions: t.partitions,
                        },
                    )
                })
                .collect(),
            group: EXAMPLE_GROUP.to_string(),
        }
    }

    /// A fixture topic, or an error naming the key the example forgot to declare.
    pub fn topic(&self, key: &str) -> Result<&ExampleTopic, String> {
        self.topics.get(key).ok_or_else(|| {
            format!("the example asked for fixture topic '{key}', which it never declared")
        })
    }

    /// The real name of a fixture topic.
    pub fn name(&self, key: &str) -> Result<String, String> {
        Ok(self.topic(key)?.name.clone())
    }

    /// The real id of a fixture topic.
    pub fn id(&self, key: &str) -> Result<Uuid, String> {
        Ok(self.topic(key)?.uuid())
    }

    /// Encode one request frame payload (header v1/v2 + body) with the suite's own codec.
    pub fn payload<R: Request>(
        &self,
        version: i16,
        correlation_id: i32,
        req: &R,
    ) -> Result<Vec<u8>, String> {
        encode_request(version, correlation_id, Some(EXAMPLE_CLIENT_ID), req)
            .map_err(|e| format!("cannot encode the example request: {e}"))
    }

    /// The common case: one request, one frame.
    pub fn request<R: Request>(
        &self,
        version: i16,
        correlation_id: i32,
        req: &R,
    ) -> Result<Wire, String> {
        Ok(Wire::Frames(vec![self.payload(
            version,
            correlation_id,
            req,
        )?]))
    }
}

/// Builds an example's bytes from the environment the capture put in place.
pub type ExampleBuilder = fn(&ExampleEnv) -> Result<Wire, String>;

/// One worked example of a stage: a request, the answer, and why it looks like that.
pub struct ExampleSpec {
    /// Short title, e.g. "ApiVersions v4 from a fresh client".
    pub title: &'static str,
    /// One-line human summary of the request.
    pub request: &'static str,
    /// One-line human summary of what a correct broker answers.
    pub response: &'static str,
    /// The trap, or the thing to look at.
    pub note: Option<&'static str>,
    /// Topics that must exist before the request is sent.
    pub fixtures: fn() -> FixtureSpec,
    /// What the harness does after writing the bytes.
    pub expect: Expect,
    /// Builds the exact bytes; `None` only for [`Expect::Nothing`] examples.
    pub build: Option<ExampleBuilder>,
    /// Decode the response as this version instead of the request's own.
    ///
    /// Needed exactly where the broker answers in a *different* version from the one it was
    /// asked for: an `UNSUPPORTED_VERSION` reply to ApiVersions is written in the v0 shape,
    /// because the client cannot know the broker's range until it has parsed that reply.
    pub response_version: Option<i16>,
}

fn no_fixtures() -> FixtureSpec {
    FixtureSpec::none()
}

impl ExampleSpec {
    /// An example with a real request on the wire.
    pub fn wire(
        title: &'static str,
        build: fn(&ExampleEnv) -> Result<Wire, String>,
    ) -> ExampleSpec {
        ExampleSpec {
            title,
            request: "",
            response: "",
            note: None,
            fixtures: no_fixtures,
            expect: Expect::Response,
            build: Some(build),
            response_version: None,
        }
    }

    /// An example with no wire exchange at all (stages 1, 7, 43, 45).
    pub fn text(title: &'static str) -> ExampleSpec {
        ExampleSpec {
            title,
            request: "",
            response: "",
            note: None,
            fixtures: no_fixtures,
            expect: Expect::Nothing,
            build: None,
            response_version: None,
        }
    }

    /// The human summary of the request.
    pub fn request(mut self, s: &'static str) -> ExampleSpec {
        self.request = s;
        self
    }

    /// The human summary of the response.
    pub fn response(mut self, s: &'static str) -> ExampleSpec {
        self.response = s;
        self
    }

    /// One or two sentences: the trap, or the thing to look at.
    pub fn note(mut self, s: &'static str) -> ExampleSpec {
        self.note = Some(s);
        self
    }

    /// Topics that must exist before the example's request is sent.
    pub fn with_fixtures(mut self, f: fn() -> FixtureSpec) -> ExampleSpec {
        self.fixtures = f;
        self
    }

    /// Decode the response as this api version rather than the request's own.
    pub fn response_version(mut self, version: i16) -> ExampleSpec {
        self.response_version = Some(version);
        self
    }

    /// A correct broker answers nothing (`acks=0`).
    pub fn expect_silence(mut self) -> ExampleSpec {
        self.expect = Expect::Silence;
        self
    }

    /// A correct broker closes the connection instead of answering.
    pub fn expect_closed(mut self) -> ExampleSpec {
        self.expect = Expect::Closed;
        self
    }
}

/// The examples of a stage that has none yet.
pub fn none() -> Vec<ExampleSpec> {
    Vec::new()
}

// ---------------------------------------------------------------------------------------
// What a capture writes
// ---------------------------------------------------------------------------------------

/// One annotated field inside a request or a response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FieldAnn {
    /// Byte offset into the hex string's bytes (the 4-byte size prefix is offset 0).
    pub offset: usize,
    /// Length in bytes.
    pub length: usize,
    /// Field path, e.g. `header.correlation_id` or `body.topics[0].error_code`.
    pub field: String,
    /// The decoded value, written the way the report writes it.
    pub value: String,
    /// True when this value legitimately differs from boot to boot (ids, timestamps).
    #[serde(default, skip_serializing_if = "is_false")]
    pub varies: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// One captured example: the bytes, the words, and the annotations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Example {
    /// Short title.
    pub title: String,
    /// `wire`, `silence`, `closed` or `text`.
    pub kind: String,
    /// Human summary of the request.
    pub request: String,
    /// The exact bytes sent, size prefixes included, lower-case hex.
    pub request_hex: String,
    /// Human summary of the response.
    pub response: String,
    /// The exact bytes Apache Kafka answered, size prefix included, lower-case hex.
    pub response_hex: String,
    /// The trap, or the thing to look at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Field annotations for `request_hex`.
    pub request_fields: Vec<FieldAnn>,
    /// Field annotations for `response_hex`.
    pub response_fields: Vec<FieldAnn>,
    /// The fixture topics the bytes refer to, so the request can be rebuilt offline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<ExampleEnv>,
}

/// `examples/captured.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CapturedFile {
    /// When the capture ran.
    pub generated_at: String,
    /// The broker that answered.
    pub broker: String,
    /// Its version, when the harness knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broker_version: Option<String>,
    /// Stage number (as a string, so the file is a plain JSON object) → its examples.
    pub stages: BTreeMap<String, Vec<Example>>,
}

impl CapturedFile {
    /// The examples captured for one stage.
    pub fn stage(&self, number: u32) -> Option<&Vec<Example>> {
        self.stages.get(&number.to_string())
    }

    /// Read `examples/captured.json`.
    pub fn load(path: &Path) -> Result<CapturedFile> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("cannot parse {}", path.display()))
    }

    /// Read it if it is there, otherwise an empty file.
    pub fn load_or_empty(path: &Path) -> CapturedFile {
        CapturedFile::load(path).unwrap_or_default()
    }

    /// Serialize exactly as `examples/captured.json` is committed.
    pub fn to_json(&self) -> Result<String> {
        let mut s = serde_json::to_string_pretty(self)?;
        s.push('\n');
        Ok(s)
    }
}

/// Build the catalog entry for one example: the words come from the stage's own
/// [`ExampleSpec`], the bytes and the annotations from the capture.
///
/// Splitting it that way means editing a note or a summary needs no broker: only a change
/// that moves a byte needs `--capture-examples` run again, which is what the freshness test
/// checks.
pub fn merge(spec: &ExampleSpec, captured: Option<&Example>) -> Example {
    Example {
        title: spec.title.to_string(),
        kind: spec.expect.as_str().to_string(),
        request: spec.request.to_string(),
        request_hex: captured.map(|c| c.request_hex.clone()).unwrap_or_default(),
        response: spec.response.to_string(),
        response_hex: captured.map(|c| c.response_hex.clone()).unwrap_or_default(),
        note: spec.note.map(str::to_string),
        request_fields: captured
            .map(|c| c.request_fields.clone())
            .unwrap_or_default(),
        response_fields: captured
            .map(|c| c.response_fields.clone())
            .unwrap_or_default(),
        env: captured.and_then(|c| c.env.clone()),
    }
}

/// Re-encode a captured example's request from the stage's builder and the environment the
/// capture recorded, so a test can prove `request_hex` is still what the suite would send.
///
/// `None` when the example has no bytes at all ([`Expect::Nothing`]).
pub fn rebuild_request(spec: &ExampleSpec, captured: &Example) -> Result<Option<String>, String> {
    let Some(build) = spec.build else {
        return Ok(None);
    };
    let env = captured.env.clone().unwrap_or_else(|| ExampleEnv {
        topics: BTreeMap::new(),
        group: EXAMPLE_GROUP.to_string(),
    });
    Ok(Some(to_hex(&build(&env)?.bytes())))
}

/// Lower-case hex, no separators — how every `*_hex` field is written.
pub fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Undo [`to_hex`].
pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stages;

    #[test]
    fn hex_round_trips() {
        let b = vec![0u8, 0x12, 0xff, 0x7f];
        assert_eq!(to_hex(&b), "0012ff7f");
        assert_eq!(from_hex("0012ff7f"), Some(b));
        assert_eq!(from_hex("abc"), None);
        assert_eq!(from_hex("zz"), None);
    }

    #[test]
    fn frames_carry_their_size_prefix() {
        let w = Wire::Frames(vec![vec![1, 2, 3]]);
        assert_eq!(w.bytes(), vec![0, 0, 0, 3, 1, 2, 3]);
        let r = Wire::Raw(vec![9, 9]);
        assert_eq!(r.bytes(), vec![9, 9]);
    }

    #[test]
    fn every_stage_declares_at_least_one_example() {
        for s in stages::all() {
            let ex = (s.examples)();
            assert!(
                !ex.is_empty(),
                "stage {} declares no examples; every stage needs 1-3",
                s.number
            );
            assert!(
                ex.len() <= 3,
                "stage {} declares {} examples; the site renders at most 3",
                s.number,
                ex.len()
            );
        }
    }

    #[test]
    fn example_specs_are_complete() {
        for s in stages::all() {
            for e in (s.examples)() {
                let where_ = format!("stage {} example '{}'", s.number, e.title);
                assert!(!e.title.trim().is_empty(), "{where_}: empty title");
                assert!(
                    !e.request.trim().is_empty(),
                    "{where_}: needs a one-line request summary"
                );
                assert!(
                    !e.response.trim().is_empty(),
                    "{where_}: needs a one-line response summary"
                );
                match e.expect {
                    Expect::Nothing => assert!(
                        e.build.is_none(),
                        "{where_}: a text example must not build bytes"
                    ),
                    _ => assert!(
                        e.build.is_some(),
                        "{where_}: a wire example must build its bytes"
                    ),
                }
            }
        }
    }

    #[test]
    fn examples_that_need_no_fixtures_build_offline() {
        // Anything that does not name a fixture topic must encode from an empty env, which
        // is what proves the builders never reach for state they did not declare.
        let env = ExampleEnv {
            topics: BTreeMap::new(),
            group: EXAMPLE_GROUP.to_string(),
        };
        for s in stages::all() {
            for e in (s.examples)() {
                if !(e.fixtures)().is_empty() {
                    continue;
                }
                if let Some(build) = e.build {
                    let wire = build(&env).unwrap_or_else(|err| {
                        panic!("stage {} example '{}': {err}", s.number, e.title)
                    });
                    assert!(
                        !wire.bytes().is_empty(),
                        "stage {} example '{}' encodes to nothing",
                        s.number,
                        e.title
                    );
                }
            }
        }
    }
}
