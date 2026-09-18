//! Worked examples: what the linker is handed, and what it has to produce.
//!
//! Every stage carries one to three [`ExampleSpec`]s. A spec says, in words, what the inputs
//! are and what a correct output must contain, and carries a builder that produces the
//! *exact* input bytes — with the same writer the tests use, so an example can never drift
//! away from the suite. [`annotate`] then walks those bytes and labels every field, so the
//! site can render an annotated hex dump of a real relocatable object next to the linker
//! command line, the program's expected stdout and its expected exit status.
//!
//! Unlike the network tracks in this repo there is nothing to capture from a reference
//! implementation: an input object is deterministic, so `--list --json` builds the whole
//! catalog offline and `catalog.json` is reproducible on any machine.
//!
//! **Why there are no output bytes.** A linker chooses its own layout; the addresses GNU ld
//! picks are not a requirement and the suite never asserts them. So an example's `response`
//! describes what must be *true* of the output ("`e_entry` is the address of `_start`", "the
//! displacement at `.text+0x19` is `S + A − P`") and `response_hex` stays empty. What the
//! program prints and exits with is pinned exactly, in `stdout` and `exit_status`.

pub mod annotate;

pub use annotate::FieldAnn;
use serde::{Deserialize, Serialize};

/// What kind of example this is; the site uses it to pick a renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExampleKind {
    /// One relocatable object, annotated, that links and runs.
    Object,
    /// An archive (`.a`), annotated as the object its first member holds.
    Archive,
    /// An input the linker has to refuse.
    Error,
    /// No bytes at all — a command line and a description.
    Text,
}

impl ExampleKind {
    /// The string that goes into the JSON as `kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            ExampleKind::Object => "object",
            ExampleKind::Archive => "archive",
            ExampleKind::Error => "error",
            ExampleKind::Text => "text",
        }
    }
}

/// Builds an example's input bytes.
pub type ExampleBuilder = fn() -> Result<Vec<u8>, String>;

/// One worked example of a stage.
pub struct ExampleSpec {
    /// Short title, e.g. "One object, one section, one syscall".
    pub title: &'static str,
    /// The linker command line the example is about.
    pub command: &'static str,
    /// One-line human summary of the inputs.
    pub request: &'static str,
    /// One-line human summary of what a correct output must contain.
    pub response: &'static str,
    /// The trap, or the thing to look at.
    pub note: Option<&'static str>,
    /// What kind of example this is.
    pub kind: ExampleKind,
    /// Builds the input object to annotate; `None` for a [`ExampleKind::Text`] example.
    pub build: Option<ExampleBuilder>,
    /// What the linked program prints, when the example links and runs.
    pub stdout: Option<&'static str>,
    /// What the linked program exits with.
    pub exit_status: Option<i32>,
}

impl ExampleSpec {
    /// An example whose input is one relocatable object.
    pub fn object(
        title: &'static str,
        command: &'static str,
        build: ExampleBuilder,
    ) -> ExampleSpec {
        ExampleSpec {
            title,
            command,
            request: "",
            response: "",
            note: None,
            kind: ExampleKind::Object,
            build: Some(build),
            stdout: None,
            exit_status: None,
        }
    }

    /// An example whose input is an archive.
    pub fn archive(
        title: &'static str,
        command: &'static str,
        build: ExampleBuilder,
    ) -> ExampleSpec {
        ExampleSpec {
            kind: ExampleKind::Archive,
            ..ExampleSpec::object(title, command, build)
        }
    }

    /// An example the linker has to refuse.
    pub fn error(title: &'static str, command: &'static str, build: ExampleBuilder) -> ExampleSpec {
        ExampleSpec {
            kind: ExampleKind::Error,
            ..ExampleSpec::object(title, command, build)
        }
    }

    /// An example with no bytes: a command line and a description.
    pub fn text(title: &'static str, command: &'static str) -> ExampleSpec {
        ExampleSpec {
            title,
            command,
            request: "",
            response: "",
            note: None,
            kind: ExampleKind::Text,
            build: None,
            stdout: None,
            exit_status: None,
        }
    }

    /// The human summary of the inputs.
    pub fn request(mut self, s: &'static str) -> ExampleSpec {
        self.request = s;
        self
    }

    /// The human summary of what the output must contain.
    pub fn response(mut self, s: &'static str) -> ExampleSpec {
        self.response = s;
        self
    }

    /// One or two sentences: the trap, or the thing to look at.
    pub fn note(mut self, s: &'static str) -> ExampleSpec {
        self.note = Some(s);
        self
    }

    /// What the linked program prints and exits with.
    pub fn runs(mut self, stdout: &'static str, exit_status: i32) -> ExampleSpec {
        self.stdout = Some(stdout);
        self.exit_status = Some(exit_status);
        self
    }
}

/// The examples of a stage that has none yet.
pub fn none() -> Vec<ExampleSpec> {
    Vec::new()
}

/// One example as it is written into `catalog.json`.
///
/// The field names are the ones the other testers in this repo emit, so the site's renderer
/// is shared; `command`, `stdout` and `exit_status` are the three linker-specific additions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Example {
    /// Short title.
    pub title: String,
    /// `object`, `archive`, `error` or `text`.
    pub kind: String,
    /// The linker command line.
    pub command: String,
    /// Human summary of the inputs.
    pub request: String,
    /// The exact input bytes, lower-case hex; empty for a `text` example.
    pub request_hex: String,
    /// Human summary of what a correct output must contain.
    pub response: String,
    /// Always empty for this track: the output's layout is the linker's own choice.
    pub response_hex: String,
    /// The trap, or the thing to look at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Field annotations for `request_hex`.
    pub request_fields: Vec<FieldAnn>,
    /// Field annotations for `response_hex`; always empty, kept so the schema matches.
    pub response_fields: Vec<FieldAnn>,
    /// What the linked program prints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    /// What the linked program exits with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_status: Option<i32>,
}

/// Build the catalog entry for one example: the words come from the stage's own
/// [`ExampleSpec`], the bytes and the annotations from its builder.
pub fn render(spec: &ExampleSpec) -> Result<Example, String> {
    let (hex, fields) = match spec.build {
        Some(build) => {
            let bytes = build().map_err(|e| format!("example '{}': {e}", spec.title))?;
            let fields =
                annotate::object(&bytes).map_err(|e| format!("example '{}': {e}", spec.title))?;
            (to_hex(&bytes), fields)
        }
        None => (String::new(), Vec::new()),
    };
    Ok(Example {
        title: spec.title.to_string(),
        kind: spec.kind.as_str().to_string(),
        command: spec.command.to_string(),
        request: spec.request.to_string(),
        request_hex: hex,
        response: spec.response.to_string(),
        response_hex: String::new(),
        note: spec.note.map(str::to_string),
        request_fields: fields,
        response_fields: Vec::new(),
        stdout: spec.stdout.map(str::to_string),
        exit_status: spec.exit_status,
    })
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
    fn example_specs_are_complete_and_build() {
        for s in stages::all() {
            for e in (s.examples)() {
                let where_ = format!("stage {} example '{}'", s.number, e.title);
                assert!(!e.title.trim().is_empty(), "{where_}: empty title");
                assert!(!e.command.trim().is_empty(), "{where_}: no command line");
                assert!(
                    !e.request.trim().is_empty(),
                    "{where_}: needs a one-line summary of the inputs"
                );
                assert!(
                    !e.response.trim().is_empty(),
                    "{where_}: needs a one-line summary of the output"
                );
                match e.kind {
                    ExampleKind::Text => assert!(
                        e.build.is_none(),
                        "{where_}: a text example must not build bytes"
                    ),
                    _ => {
                        assert!(e.build.is_some(), "{where_}: needs an input to annotate");
                        let rendered = render(&e).unwrap_or_else(|err| panic!("{where_}: {err}"));
                        assert!(
                            !rendered.request_hex.is_empty(),
                            "{where_}: encoded to nothing"
                        );
                        assert!(
                            !rendered.request_fields.is_empty(),
                            "{where_}: produced no annotations"
                        );
                    }
                }
            }
        }
    }
}
