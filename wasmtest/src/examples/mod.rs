//! Worked examples: a module, its annotated bytes, and what a correct runtime prints.
//!
//! Every stage carries one to three [`ExampleSpec`]s. A spec says, in words, what the module
//! is and what running it must produce, and carries a builder that produces the **exact**
//! bytes — with the suite's own encoder, the one the tests use — so an example can never
//! drift away from the suite.
//!
//! Unlike `kafkatest`, nothing has to be captured from the reference: a WebAssembly module is
//! input, not a conversation, so the bytes and their annotations are known offline and
//! `--list --json` renders them straight into `catalog.json`. The only thing a human writes
//! is the prose and the expected output, and `--validate` proves that output is what
//! `wasmtime` really prints, because every example is also a test somewhere in its stage.
//!
//! The JSON keys are the ones `kafkatest` already emits (`request`/`response` and their
//! `_hex` and `_fields` companions), so the site renders both tracks with one component.
//! For this track `request` is the module, `response` is the terminal output, and
//! `response_hex` is always empty.

use crate::wasm::{Ann, Module};
use serde::{Deserialize, Serialize};

/// Builds an example's module.
pub type ExampleBuilder = fn() -> Module;

/// One worked example of a stage.
pub struct ExampleSpec {
    /// Short title, e.g. "A module that adds two numbers".
    pub title: &'static str,
    /// One-line human summary of what the module is.
    pub summary: &'static str,
    /// The command line a learner would type, with `mod.wasm` standing for the file.
    pub command: &'static str,
    /// What a correct runtime prints, written exactly as it appears in a terminal.
    pub output: &'static str,
    /// The trap, or the thing to look at.
    pub note: Option<&'static str>,
    /// Builds the exact module bytes.
    pub build: ExampleBuilder,
}

impl ExampleSpec {
    /// An example built from a module.
    pub fn module(title: &'static str, build: ExampleBuilder) -> ExampleSpec {
        ExampleSpec {
            title,
            summary: "",
            command: "",
            output: "",
            note: None,
            build,
        }
    }

    /// The human summary of the module.
    pub fn summary(mut self, s: &'static str) -> ExampleSpec {
        self.summary = s;
        self
    }

    /// The command line a learner would type.
    pub fn command(mut self, s: &'static str) -> ExampleSpec {
        self.command = s;
        self
    }

    /// What a correct runtime prints.
    pub fn output(mut self, s: &'static str) -> ExampleSpec {
        self.output = s;
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

/// One example as it is written into `catalog.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Example {
    /// Short title.
    pub title: String,
    /// Always `module` for this track.
    pub kind: String,
    /// What the module is, and the command line that runs it.
    pub request: String,
    /// The exact module bytes, lower-case hex.
    pub request_hex: String,
    /// What a correct runtime prints.
    pub response: String,
    /// Always empty here: a WebAssembly runtime answers in text, not in bytes.
    pub response_hex: String,
    /// The trap, or the thing to look at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Per-section and per-field annotations for `request_hex`.
    pub request_fields: Vec<Ann>,
    /// Always empty here.
    pub response_fields: Vec<Ann>,
}

/// Render a stage's declaration into the catalog entry, encoding the module as it goes.
pub fn render(spec: &ExampleSpec) -> Example {
    let m = (spec.build)();
    let mut anns = m.anns.clone();
    anns.sort_by_key(|a| (a.offset, a.length));
    Example {
        title: spec.title.to_string(),
        kind: "module".to_string(),
        request: if spec.command.is_empty() {
            spec.summary.to_string()
        } else {
            format!("{}  —  `{}`", spec.summary, spec.command)
        },
        request_hex: m.hex(),
        response: spec.output.to_string(),
        response_hex: String::new(),
        note: spec.note.map(str::to_string),
        request_fields: anns,
        response_fields: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stages;

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
    fn example_specs_are_complete_and_encode_to_real_modules() {
        for s in stages::all() {
            for e in (s.examples)() {
                let where_ = format!("stage {} example '{}'", s.number, e.title);
                assert!(!e.title.trim().is_empty(), "{where_}: empty title");
                assert!(
                    !e.summary.trim().is_empty(),
                    "{where_}: needs a one-line summary"
                );
                assert!(
                    !e.command.trim().is_empty(),
                    "{where_}: needs the command line a learner would type"
                );
                assert!(
                    !e.output.trim().is_empty(),
                    "{where_}: needs the output a correct runtime prints"
                );
                let rendered = render(&e);
                assert!(
                    !rendered.request_hex.is_empty(),
                    "{where_}: encodes to no bytes"
                );
                assert!(
                    rendered.request_hex.starts_with("0061736d"),
                    "{where_}: a module starts with the magic \\0asm"
                );
                assert!(
                    !rendered.request_fields.is_empty(),
                    "{where_}: the module carries no annotations"
                );
                for a in &rendered.request_fields {
                    assert!(
                        (a.offset + a.length) * 2 <= rendered.request_hex.len(),
                        "{where_}: annotation {a:?} runs past the module"
                    );
                }
            }
        }
    }

    #[test]
    fn rendering_is_deterministic() {
        for s in stages::all() {
            for e in (s.examples)() {
                assert_eq!(
                    render(&e),
                    render(&e),
                    "stage {} example '{}' does not render the same twice",
                    s.number,
                    e.title
                );
            }
        }
    }
}
