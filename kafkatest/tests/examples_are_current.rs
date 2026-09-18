//! `examples/captured.json` is committed and merged into `catalog.json`; this proves it is
//! still the truth.
//!
//! Recapture it with
//! `kafkatest --capture-examples examples/captured.json --broker apache_kafka`.

use kafkatest::examples::{from_hex, rebuild_request, CapturedFile, Example, ExampleSpec};
use kafkatest::stages;
use std::path::PathBuf;

const RECAPTURE: &str =
    "recapture with `kafkatest --capture-examples examples/captured.json --broker apache_kafka`";

fn path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/captured.json")
}

fn captured() -> CapturedFile {
    CapturedFile::load(&path())
        .unwrap_or_else(|e| panic!("cannot read {}: {e:#}\n{RECAPTURE}", path().display()))
}

/// The spec/capture pairs of every stage, in order.
fn pairs() -> Vec<(u32, ExampleSpec, Example)> {
    let file = captured();
    let mut out = Vec::new();
    for stage in stages::all() {
        let specs = (stage.examples)();
        let taken = file.stage(stage.number).cloned().unwrap_or_default();
        assert_eq!(
            taken.len(),
            specs.len(),
            "stage {} declares {} examples but {} were captured — {RECAPTURE}",
            stage.number,
            specs.len(),
            taken.len()
        );
        for (spec, ex) in specs.into_iter().zip(taken) {
            out.push((stage.number, spec, ex));
        }
    }
    out
}

#[test]
fn every_stage_has_captured_examples() {
    let file = captured();
    for stage in stages::all() {
        let list = file.stage(stage.number).unwrap_or_else(|| {
            panic!(
                "no captured examples for stage {} — {RECAPTURE}",
                stage.number
            )
        });
        assert!(
            !list.is_empty(),
            "stage {} has an empty example list — {RECAPTURE}",
            stage.number
        );
        assert!(
            list.len() <= 3,
            "stage {} carries {} examples; the site renders at most 3",
            stage.number,
            list.len()
        );
    }
    assert_eq!(
        file.broker, "apache_kafka",
        "the committed capture must come from the reference broker"
    );
}

#[test]
fn request_hex_re_encodes_identically() {
    for (number, spec, ex) in pairs() {
        let rebuilt = rebuild_request(&spec, &ex)
            .unwrap_or_else(|e| panic!("stage {number} example '{}': {e}", spec.title));
        match rebuilt {
            None => assert!(
                ex.request_hex.is_empty(),
                "stage {number} example '{}' has bytes but no builder",
                spec.title
            ),
            Some(hex) => assert_eq!(
                hex, ex.request_hex,
                "stage {number} example '{}' no longer encodes to the captured bytes — \
                 {RECAPTURE}",
                spec.title
            ),
        }
    }
}

#[test]
fn wire_examples_carry_a_response_and_text_examples_do_not() {
    for (number, spec, ex) in pairs() {
        let what = format!("stage {number} example '{}'", spec.title);
        assert_eq!(ex.kind, spec.expect.as_str(), "{what}: kind drifted");
        match ex.kind.as_str() {
            "wire" => {
                assert!(!ex.request_hex.is_empty(), "{what}: no request bytes");
                assert!(
                    !ex.response_hex.is_empty(),
                    "{what}: a wire example must carry the broker's answer — {RECAPTURE}"
                );
            }
            "silence" | "closed" => {
                assert!(!ex.request_hex.is_empty(), "{what}: no request bytes");
                assert!(
                    ex.response_hex.is_empty(),
                    "{what}: the broker was not supposed to answer"
                );
            }
            "text" => {
                assert!(
                    ex.request_hex.is_empty(),
                    "{what}: a text example has bytes"
                );
                assert!(
                    ex.response_hex.is_empty(),
                    "{what}: a text example has bytes"
                );
                assert!(
                    ex.request_fields.is_empty() && ex.response_fields.is_empty(),
                    "{what}: a text example has annotations"
                );
            }
            other => panic!("{what}: unknown example kind '{other}'"),
        }
        assert!(!ex.request.trim().is_empty(), "{what}: no request summary");
        assert!(
            !ex.response.trim().is_empty(),
            "{what}: no response summary"
        );
    }
}

#[test]
fn annotations_stay_inside_the_bytes_they_describe() {
    for (number, spec, ex) in pairs() {
        for (which, hex, fields) in [
            ("request", &ex.request_hex, &ex.request_fields),
            ("response", &ex.response_hex, &ex.response_fields),
        ] {
            let bytes = from_hex(hex)
                .unwrap_or_else(|| panic!("stage {number} '{}': bad {which} hex", spec.title));
            for a in fields {
                assert!(
                    a.offset + a.length <= bytes.len(),
                    "stage {number} example '{}': {which} annotation {} runs past the end \
                     ({}+{} of {} bytes)",
                    spec.title,
                    a.field,
                    a.offset,
                    a.length,
                    bytes.len()
                );
                assert!(
                    !a.field.is_empty(),
                    "stage {number} example '{}': a {which} annotation has no field path",
                    spec.title
                );
            }
            if !bytes.is_empty() {
                assert!(
                    !fields.is_empty(),
                    "stage {number} example '{}': {which} bytes with no annotations",
                    spec.title
                );
                assert_eq!(
                    fields.first().map(|a| (a.offset, a.length)),
                    Some((0, 4)),
                    "stage {number} example '{}': the first {which} annotation must be the \
                     4-byte size prefix",
                    spec.title
                );
            }
        }
    }
}

#[test]
fn the_catalog_carries_the_examples() {
    let cat: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("catalog.json"))
            .expect("catalog.json"),
    )
    .expect("catalog.json is valid JSON");
    for stage in cat["stages"].as_array().expect("stages") {
        let n = stage["number"].as_u64().unwrap_or(0);
        let examples = stage["examples"]
            .as_array()
            .unwrap_or_else(|| panic!("stage {n} has no examples array in catalog.json"));
        assert!(
            !examples.is_empty(),
            "stage {n} has no examples in catalog.json — regenerate it with \
             `kafkatest --list --json catalog.json`"
        );
        for e in examples {
            for key in [
                "title",
                "kind",
                "request",
                "request_hex",
                "response",
                "response_hex",
                "request_fields",
                "response_fields",
            ] {
                assert!(
                    !e[key].is_null(),
                    "stage {n} example '{}' has no {key}",
                    e["title"]
                );
            }
        }
    }
}
