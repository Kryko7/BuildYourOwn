//! Worked examples: what a stage's program is asked, and what the reference answered.
//!
//! Every stage carries one to three [`ExampleSpec`]s. A spec says in words what the
//! exchange is, and carries the exact thing to run — a request against a reference node, a
//! scripted sequence against a reference cluster, or a conversation with the reference
//! primitives CLI. `disttest --capture-examples examples/captured.json --target etcd` runs
//! them all and records what came back, and `--list --json` merges that file into
//! `catalog.json`, so the site can show "what to expect" without anyone running anything.
//!
//! Splitting the words from the bytes is deliberate: editing a note needs no reference,
//! and only a change that moves a byte needs a recapture — which is exactly what the
//! freshness test checks.

pub mod capture;
mod specs;

pub use specs::*;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// What kind of exchange an example is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExampleKind {
    /// One HTTP/JSON request against a single node.
    Node,
    /// A scripted sequence against a three-member cluster.
    Cluster,
    /// A conversation with the primitives CLI.
    Primitives,
    /// A recorded history and the checker's verdict on it.
    Workload,
}

impl ExampleKind {
    /// The string that goes into the JSON as `kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            ExampleKind::Node => "node",
            ExampleKind::Cluster => "cluster",
            ExampleKind::Primitives => "primitives",
            ExampleKind::Workload => "workload",
        }
    }
}

/// One step of a cluster example: a request, and which member it goes to.
#[derive(Debug, Clone)]
pub struct ClusterStep {
    /// Member index, from zero.
    pub member: usize,
    /// The path, e.g. `/v3/kv/put`.
    pub path: String,
    /// The JSON body.
    pub body: Value,
    /// A word about why this step is here, shown in the transcript.
    pub note: String,
}

impl ClusterStep {
    /// A step against one member.
    pub fn new(member: usize, path: &str, body: Value, note: &str) -> ClusterStep {
        ClusterStep {
            member,
            path: path.to_string(),
            body,
            note: note.to_string(),
        }
    }
}

/// What the harness runs to capture an example.
#[derive(Clone)]
pub enum ExampleBody {
    /// Optional setup requests, then the request that is the example.
    Node {
        /// Requests sent first, whose answers are not shown.
        setup: fn() -> Vec<(String, Value)>,
        /// The path of the request that is the example.
        path: &'static str,
        /// The body of that request.
        body: fn() -> Value,
    },
    /// A scripted sequence against a three-member cluster; the transcript is the example.
    Cluster {
        /// The steps, in order.
        steps: fn() -> Vec<ClusterStep>,
    },
    /// A conversation with the primitives CLI for one topic.
    Primitives {
        /// The topic the program is started with.
        topic: &'static str,
        /// The commands, one per line.
        commands: fn() -> Vec<String>,
    },
    /// A short workload against a three-member cluster, recorded and checked.
    Workload {
        /// How long the workload runs, in milliseconds.
        duration_ms: u64,
        /// How many client tasks it uses.
        clients: usize,
        /// How many keys they share.
        keys: usize,
    },
}

impl ExampleBody {
    /// Which kind of example this body produces.
    pub fn kind(&self) -> ExampleKind {
        match self {
            ExampleBody::Node { .. } => ExampleKind::Node,
            ExampleBody::Cluster { .. } => ExampleKind::Cluster,
            ExampleBody::Primitives { .. } => ExampleKind::Primitives,
            ExampleBody::Workload { .. } => ExampleKind::Workload,
        }
    }
}

/// One worked example of a stage.
#[derive(Clone)]
pub struct ExampleSpec {
    /// Short title, e.g. "A put and the header it answers with".
    pub title: &'static str,
    /// One-line human summary of what is asked.
    pub request: &'static str,
    /// One-line human summary of what a correct program answers.
    pub response: &'static str,
    /// The trap, or the thing to look at.
    pub note: Option<&'static str>,
    /// What the harness runs to capture it.
    pub body: ExampleBody,
}

impl ExampleSpec {
    /// An example built from a body.
    pub fn new(title: &'static str, body: ExampleBody) -> ExampleSpec {
        ExampleSpec {
            title,
            request: "",
            response: "",
            note: None,
            body,
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
}

/// The examples of a stage that has none yet.
pub fn none() -> Vec<ExampleSpec> {
    Vec::new()
}

// ---------------------------------------------------------------------------------------
// What a capture writes
// ---------------------------------------------------------------------------------------

/// One annotated field inside a captured request or response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FieldAnn {
    /// Byte offset into the captured text.
    pub offset: usize,
    /// Length in bytes.
    pub length: usize,
    /// Field path, e.g. `header.revision`.
    pub field: String,
    /// The value, written the way the report writes it.
    pub value: String,
    /// True when this value legitimately differs from run to run (ids, revisions, times).
    #[serde(default, skip_serializing_if = "is_false")]
    pub varies: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// One captured example: the exchange, the words, and the annotations.
///
/// The field names are the ones every tester in this repo writes, so the site reads all of
/// them with one parser. `request_hex` and `response_hex` hold the exact bytes that went
/// over the wire (or over the pipe); `request` and `response` hold the readable form.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Example {
    /// Short title.
    pub title: String,
    /// `node`, `cluster`, `primitives` or `workload`.
    pub kind: String,
    /// Human summary of the request, followed by the request as it was sent.
    pub request: String,
    /// The exact request bytes, lower-case hex.
    pub request_hex: String,
    /// Human summary of the response, followed by what came back.
    pub response: String,
    /// The exact response bytes, lower-case hex.
    pub response_hex: String,
    /// The trap, or the thing to look at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Field annotations for `request_hex`.
    pub request_fields: Vec<FieldAnn>,
    /// Field annotations for `response_hex`.
    pub response_fields: Vec<FieldAnn>,
}

/// `examples/captured.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CapturedFile {
    /// When the capture ran.
    pub generated_at: String,
    /// The reference that answered.
    pub target: String,
    /// Its version, when the harness knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_version: Option<String>,
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

/// Build the catalog entry for one example: the words come from the stage's own spec, the
/// bytes and the annotations from the capture.
pub fn merge(spec: &ExampleSpec, captured: Option<&Example>) -> Example {
    let (request_tail, response_tail) = match captured {
        Some(c) => (
            c.request
                .split_once("\n\n")
                .map(|(_, t)| t.to_string())
                .unwrap_or_else(|| c.request.clone()),
            c.response
                .split_once("\n\n")
                .map(|(_, t)| t.to_string())
                .unwrap_or_else(|| c.response.clone()),
        ),
        None => (String::new(), String::new()),
    };
    let join = |head: &str, tail: String| {
        if tail.trim().is_empty() {
            head.to_string()
        } else {
            format!("{head}\n\n{tail}")
        }
    };
    Example {
        title: spec.title.to_string(),
        kind: spec.body.kind().as_str().to_string(),
        request: join(spec.request, request_tail),
        request_hex: captured.map(|c| c.request_hex.clone()).unwrap_or_default(),
        response: join(spec.response, response_tail),
        response_hex: captured.map(|c| c.response_hex.clone()).unwrap_or_default(),
        note: spec.note.map(str::to_string),
        request_fields: captured
            .map(|c| c.request_fields.clone())
            .unwrap_or_default(),
        response_fields: captured
            .map(|c| c.response_fields.clone())
            .unwrap_or_default(),
    }
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

/// Annotate a JSON document: one entry per top-level field, with where it sits in the text.
///
/// Values that change from run to run — revisions, member ids, lease ids, terms — are
/// marked `varies`, so the site can render them differently and the freshness test knows
/// not to compare them.
pub fn annotate_json(text: &str, prefix: &str) -> Vec<FieldAnn> {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    let Some(map) = value.as_object() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (k, v) in map {
        let needle = format!("\"{k}\"");
        let Some(offset) = text.find(&needle) else {
            continue;
        };
        let rendered = serde_json::to_string(v).unwrap_or_default();
        out.push(FieldAnn {
            offset,
            length: needle.len(),
            field: format!("{prefix}{k}"),
            value: if rendered.len() > 120 {
                format!("{}…", &rendered[..120])
            } else {
                rendered
            },
            varies: VARYING_FIELDS.contains(&k.as_str()),
        });
    }
    out.sort_by_key(|a| a.offset);
    out
}

/// Fields whose value is different every time the reference is started.
pub const VARYING_FIELDS: &[&str] = &[
    "cluster_id",
    "member_id",
    "raft_term",
    "revision",
    "ID",
    "leader",
    "raftIndex",
    "raftTerm",
    "raftAppliedIndex",
    "dbSize",
    "dbSizeInUse",
    "header",
];

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
    fn json_annotations_point_at_real_offsets() {
        let text = r#"{"key":"Zm9v","value":"YmFy"}"#;
        let anns = annotate_json(text, "request.");
        assert_eq!(anns.len(), 2);
        assert_eq!(anns[0].field, "request.key");
        assert_eq!(
            &text[anns[0].offset..anns[0].offset + anns[0].length],
            "\"key\""
        );
        assert!(!anns[0].varies);
        let anns = annotate_json(r#"{"header":{"revision":"2"}}"#, "response.");
        assert!(anns[0].varies, "a header changes from run to run");
        assert!(annotate_json("not json", "x.").is_empty());
    }

    #[test]
    fn merge_keeps_the_words_and_takes_the_bytes() {
        let spec = ExampleSpec::new(
            "t",
            ExampleBody::Node {
                setup: specs::no_setup,
                path: "/v3/kv/put",
                body: specs::empty_body,
            },
        )
        .request("what is asked")
        .response("what comes back")
        .note("the trap");
        let captured = Example {
            request: "stale words\n\nPOST /v3/kv/put".into(),
            request_hex: "7b7d".into(),
            response: "stale words\n\n200 OK".into(),
            response_hex: "7b7d".into(),
            ..Default::default()
        };
        let m = merge(&spec, Some(&captured));
        assert_eq!(m.kind, "node");
        assert_eq!(m.request, "what is asked\n\nPOST /v3/kv/put");
        assert_eq!(m.response, "what comes back\n\n200 OK");
        assert_eq!(m.note.as_deref(), Some("the trap"));
        assert_eq!(m.request_hex, "7b7d");
        let empty = merge(&spec, None);
        assert_eq!(empty.request, "what is asked");
        assert!(empty.request_hex.is_empty());
    }

    #[test]
    fn every_stage_declares_one_to_three_examples() {
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
    fn example_specs_are_complete_and_match_their_ladder() {
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
                let ok = matches!(
                    (s.ladder, e.body.kind()),
                    (
                        stages::Ladder::Primitives | stages::Ladder::Algorithms,
                        ExampleKind::Primitives
                    ) | (stages::Ladder::Node, ExampleKind::Node)
                        | (
                            stages::Ladder::Cluster,
                            ExampleKind::Cluster | ExampleKind::Workload | ExampleKind::Node
                        )
                );
                assert!(
                    ok,
                    "{where_}: a {:?} stage cannot carry a {:?} example",
                    s.ladder,
                    e.body.kind()
                );
            }
        }
    }
}
