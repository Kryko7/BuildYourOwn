//! `examples/captured.json` is committed, and the catalog merges it in; this proves the
//! two agree with the stage registry.
//!
//! Recapture with
//! `tlstest --capture-examples examples/captured.json --server openssl`, then regenerate
//! `catalog.json`.

use tlstest::examples::{CapturedFile, Scenario};
use tlstest::stages;
use tlstest::tls::unhex;

fn manifest(relative: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn captured() -> CapturedFile {
    let path = manifest(tlstest::catalog::CAPTURED_EXAMPLES);
    CapturedFile::load(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e:#}\nrecapture with `tlstest --capture-examples {} --server openssl`",
            path.display(),
            tlstest::catalog::CAPTURED_EXAMPLES
        )
    })
}

#[test]
fn every_stage_has_its_examples_captured() {
    let file = captured();
    for stage in stages::all() {
        let declared = (stage.examples)();
        let taken = file
            .stage(stage.number)
            .unwrap_or_else(|| panic!("no captured examples for stage {}", stage.number));
        assert_eq!(
            declared.len(),
            taken.len(),
            "stage {} declares {} examples but {} were captured",
            stage.number,
            declared.len(),
            taken.len()
        );
        for (spec, example) in declared.iter().zip(taken) {
            assert_eq!(
                spec.title, example.title,
                "stage {} has a captured example out of order",
                stage.number
            );
            assert_eq!(spec.scenario.kind(), example.kind);
        }
    }
}

#[test]
fn captured_bytes_are_hex_and_their_annotations_lie_inside_them() {
    let file = captured();
    for stage in stages::all() {
        let Some(taken) = file.stage(stage.number) else {
            continue;
        };
        for example in taken {
            for (label, hex_text, fields) in [
                ("request", &example.request_hex, &example.request_fields),
                ("response", &example.response_hex, &example.response_fields),
            ] {
                let where_ = format!("stage {} example '{}' {label}", stage.number, example.title);
                let bytes = unhex(hex_text).unwrap_or_else(|| panic!("{where_}: not valid hex"));
                for f in fields {
                    assert!(
                        f.offset + f.length <= bytes.len(),
                        "{where_}: annotation {} runs past the end ({} + {} > {})",
                        f.field,
                        f.offset,
                        f.length,
                        bytes.len()
                    );
                    assert!(
                        !f.field.is_empty(),
                        "{where_}: an annotation has no field path"
                    );
                    assert!(!f.value.is_empty(), "{where_}: {} has no value", f.field);
                }
                if !bytes.is_empty() && !fields.is_empty() {
                    assert_eq!(
                        fields[0].offset, 0,
                        "{where_}: the first annotation must start at byte 0"
                    );
                }
            }
        }
    }
}

#[test]
fn wire_examples_really_carry_bytes() {
    let file = captured();
    for stage in stages::all() {
        let Some(taken) = file.stage(stage.number) else {
            continue;
        };
        for example in taken {
            let where_ = format!("stage {} example '{}'", stage.number, example.title);
            match example.kind.as_str() {
                "wire" => {
                    assert!(
                        !example.request_hex.is_empty(),
                        "{where_}: no request bytes"
                    );
                    assert!(
                        !example.response_hex.is_empty(),
                        "{where_}: no response bytes"
                    );
                    assert!(
                        !example.request_fields.is_empty(),
                        "{where_}: the request bytes are not annotated"
                    );
                }
                "text" => {
                    assert!(
                        example.request_hex.is_empty(),
                        "{where_}: a text example has bytes"
                    );
                    assert!(
                        example.response_hex.is_empty(),
                        "{where_}: a text example has bytes"
                    );
                }
                "closed" | "silence" => {
                    assert!(
                        !example.request_hex.is_empty(),
                        "{where_}: no request bytes"
                    );
                }
                other => panic!("{where_}: unknown kind {other}"),
            }
        }
    }
}

#[test]
fn raw_example_builders_still_produce_the_captured_request() {
    // The words come from the stage file and the bytes from the capture, so a change that
    // moves a byte has to be recaptured. This catches the ones that can be rebuilt offline:
    // a `Raw` example's request is a pure function of its seed.
    let file = captured();
    for stage in stages::all() {
        let Some(taken) = file.stage(stage.number) else {
            continue;
        };
        for (index, spec) in (stage.examples)().iter().enumerate() {
            let Scenario::Raw { build, .. } = spec.scenario else {
                continue;
            };
            let Some(example) = taken.get(index) else {
                continue;
            };
            let env = tlstest::examples::ExampleEnv {
                server_name: "localhost".to_string(),
                seed: u64::from(stage.number) * 100 + index as u64 + 1,
            };
            let bytes = build(&env)
                .unwrap_or_else(|e| panic!("stage {} example '{}': {e}", stage.number, spec.title));
            assert_eq!(
                tlstest::tls::hex(&bytes),
                example.request_hex,
                "stage {} example '{}' has drifted — recapture with --capture-examples",
                stage.number,
                spec.title
            );
        }
    }
}

#[test]
fn the_capture_says_which_server_answered() {
    let file = captured();
    assert_eq!(file.server, "openssl");
    assert!(!file.generated_at.is_empty());
    assert!(
        file.server_version
            .as_deref()
            .unwrap_or_default()
            .starts_with("OpenSSL 3"),
        "the examples must come from an OpenSSL 3.x, got {:?}",
        file.server_version
    );
}
