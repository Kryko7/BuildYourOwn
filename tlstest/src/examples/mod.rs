//! Worked examples: what a TLS 1.3 server receives and must answer, byte for byte.
//!
//! Every stage carries one to three [`ExampleSpec`]s. A spec says, in words, what the
//! client sends and what a correct server answers, and names a [`Scenario`] the capture can
//! replay against a real server. `tlstest --capture-examples examples/captured.json
//! --server openssl` runs each one, records the bytes and annotates both of them field by
//! field with [`crate::tls::annotate`]; `--list --json` merges the result into
//! `catalog.json`, which is what the site renders.
//!
//! Splitting the words (in the stage file) from the bytes (in `captured.json`) means that
//! editing a note needs no server, and only a change that moves a byte needs a recapture —
//! which is what `tests/examples_are_current.rs` checks.

pub mod capture;

use crate::certs::{CertKind, KeyKind};
use crate::config::ServerOptions;
use crate::tls::annotate::FieldAnn;
use crate::tls::client::ClientConfig;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// What the capture does after writing an example's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    /// Read this many records and keep them all.
    Records(usize),
    /// Read until an alert arrives.
    UntilAlert,
    /// Read until the server closes the connection.
    UntilClose,
    /// Prove nothing comes back.
    Silence,
}

/// Which byte string of a completed handshake an example shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// The ClientHello record, header included.
    ClientHelloRecord,
    /// The ClientHello handshake message.
    ClientHello,
    /// The ServerHello handshake message.
    ServerHello,
    /// The HelloRetryRequest, which is a ServerHello with a fixed random.
    HelloRetryRequest,
    /// EncryptedExtensions, after decryption.
    EncryptedExtensions,
    /// The Certificate message, after decryption.
    Certificate,
    /// CertificateVerify, after decryption.
    CertificateVerify,
    /// The server's Finished, after decryption.
    ServerFinished,
    /// The client's Finished, after decryption.
    ClientFinished,
    /// The first NewSessionTicket, after decryption.
    NewSessionTicket,
    /// EncryptedExtensions, Certificate, CertificateVerify and Finished, end to end.
    ServerFlight,
    /// The server's CertificateRequest, after decryption.
    CertificateRequest,
    /// The client's own Certificate, after encryption is removed.
    ClientCertificate,
    /// The client's own CertificateVerify.
    ClientCertificateVerify,
    /// Nothing at all.
    None,
}

impl Part {
    /// How this part is annotated.
    fn annotate(self, bytes: &[u8]) -> (Vec<FieldAnn>, bool) {
        match self {
            Part::ClientHelloRecord => crate::tls::annotate::records(bytes),
            Part::ServerFlight => crate::tls::annotate::handshake_flight(bytes),
            Part::None => (Vec::new(), true),
            _ => crate::tls::annotate::handshake(bytes),
        }
    }
}

/// Everything an example's configuration builder may look at.
#[derive(Debug, Clone)]
pub struct ExampleEnv {
    /// The `server_name` the example sends.
    pub server_name: String,
    /// The seed the example's randomness comes from; fixed per example so the bytes are
    /// as stable as a TLS handshake can be.
    pub seed: u64,
}

impl ExampleEnv {
    /// The default client configuration for an example.
    pub fn config(&self) -> ClientConfig {
        ClientConfig::seeded(self.seed).with_server_name(Some(&self.server_name))
    }
}

/// How an example is replayed against a real server.
#[derive(Clone)]
pub enum Scenario {
    /// Put these exact bytes on the wire at connect time and record what came back.
    Raw {
        /// Builds the bytes.
        build: fn(&ExampleEnv) -> Result<Vec<u8>, String>,
        /// What to read afterwards.
        expect: Expect,
    },
    /// Run a full handshake and show two named parts of it.
    Handshake {
        /// The configuration to hand the client.
        config: fn(&ExampleEnv) -> ClientConfig,
        /// The part shown as the "request".
        request: Part,
        /// The part shown as the "response".
        response: Part,
    },
    /// Run a handshake and echo one line; show both application-data records.
    Echo {
        /// The line, without its newline.
        line: &'static str,
    },
    /// No wire exchange at all; the words carry the whole example.
    Text,
}

impl Scenario {
    /// The string that goes into the JSON as `kind`.
    pub fn kind(&self) -> &'static str {
        match self {
            Scenario::Text => "text",
            Scenario::Raw {
                expect: Expect::UntilClose,
                ..
            } => "closed",
            Scenario::Raw {
                expect: Expect::Silence,
                ..
            } => "silence",
            _ => "wire",
        }
    }
}

/// One worked example of a stage.
pub struct ExampleSpec {
    /// Short title, e.g. "A minimal ClientHello and the ServerHello it earns".
    pub title: &'static str,
    /// One-line human summary of what the client sends.
    pub request: &'static str,
    /// One-line human summary of what a correct server answers.
    pub response: &'static str,
    /// The trap, or the thing to look at.
    pub note: Option<&'static str>,
    /// How the server must be started for this example.
    pub server: fn() -> ServerOptions,
    /// How the example is replayed.
    pub scenario: Scenario,
    /// True when the bytes are deliberately not valid TLS, so the annotator is expected to
    /// stop part way through them.
    pub malformed: bool,
}

fn default_server() -> ServerOptions {
    ServerOptions::default()
}

/// A server started with an RSA-2048 certificate, for the examples that show RSA-PSS.
pub fn rsa_server() -> ServerOptions {
    ServerOptions::with_cert(CertKind::Leaf(KeyKind::Rsa2048))
}

/// A server started with the three-certificate chain.
pub fn chain_server() -> ServerOptions {
    ServerOptions::with_cert(CertKind::Chain)
}

impl ExampleSpec {
    /// An example that puts exact bytes on the wire.
    pub fn raw(
        title: &'static str,
        build: fn(&ExampleEnv) -> Result<Vec<u8>, String>,
        expect: Expect,
    ) -> ExampleSpec {
        ExampleSpec {
            title,
            request: "",
            response: "",
            note: None,
            server: default_server,
            scenario: Scenario::Raw { build, expect },
            malformed: false,
        }
    }

    /// An example that runs a handshake and shows two of its messages.
    pub fn handshake(
        title: &'static str,
        config: fn(&ExampleEnv) -> ClientConfig,
        request: Part,
        response: Part,
    ) -> ExampleSpec {
        ExampleSpec {
            title,
            request: "",
            response: "",
            note: None,
            server: default_server,
            scenario: Scenario::Handshake {
                config,
                request,
                response,
            },
            malformed: false,
        }
    }

    /// An example that echoes one line over a completed handshake.
    pub fn echo(title: &'static str, line: &'static str) -> ExampleSpec {
        ExampleSpec {
            title,
            request: "",
            response: "",
            note: None,
            server: default_server,
            scenario: Scenario::Echo { line },
            malformed: false,
        }
    }

    /// An example with no wire exchange at all.
    pub fn text(title: &'static str) -> ExampleSpec {
        ExampleSpec {
            title,
            request: "",
            response: "",
            note: None,
            server: default_server,
            scenario: Scenario::Text,
            malformed: false,
        }
    }

    /// The human summary of what the client sends.
    pub fn request(mut self, s: &'static str) -> ExampleSpec {
        self.request = s;
        self
    }

    /// The human summary of what a correct server answers.
    pub fn response(mut self, s: &'static str) -> ExampleSpec {
        self.response = s;
        self
    }

    /// One or two sentences: the trap, or the thing to look at.
    pub fn note(mut self, s: &'static str) -> ExampleSpec {
        self.note = Some(s);
        self
    }

    /// Start the server differently for this example.
    pub fn with_server(mut self, f: fn() -> ServerOptions) -> ExampleSpec {
        self.server = f;
        self
    }

    /// Say that these bytes are deliberately not valid TLS.
    ///
    /// The capture normally refuses an example whose annotation walk does not land exactly
    /// on the last byte — that check is what keeps the annotations honest. An example whose
    /// whole point is that the bytes are wrong has to opt out of it.
    pub fn malformed(mut self) -> ExampleSpec {
        self.malformed = true;
        self
    }
}

/// The examples of a stage that has none.
pub fn none() -> Vec<ExampleSpec> {
    Vec::new()
}

/// One captured example: the bytes, the words, and the annotations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Example {
    /// Short title.
    pub title: String,
    /// `wire`, `closed`, `silence` or `text`.
    pub kind: String,
    /// Human summary of what the client sends.
    pub request: String,
    /// The exact bytes, lower-case hex.
    pub request_hex: String,
    /// Human summary of what a correct server answers.
    pub response: String,
    /// The exact bytes the reference answered, lower-case hex.
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
    /// The server that answered.
    pub server: String,
    /// Its version, when the harness knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_version: Option<String>,
    /// Stage number (as a string) → its examples.
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

/// Build the catalog entry for one example: the words from the stage's own [`ExampleSpec`],
/// the bytes and the annotations from the capture.
pub fn merge(spec: &ExampleSpec, captured: Option<&Example>) -> Example {
    Example {
        title: spec.title.to_string(),
        kind: spec.scenario.kind().to_string(),
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stages;

    #[test]
    fn every_stage_declares_between_one_and_three_examples() {
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
            }
        }
    }

    #[test]
    fn raw_example_builders_work_offline() {
        let env = ExampleEnv {
            server_name: "localhost".into(),
            seed: 0x7157,
        };
        for s in stages::all() {
            for e in (s.examples)() {
                if let Scenario::Raw { build, .. } = e.scenario {
                    let bytes = build(&env).unwrap_or_else(|err| {
                        panic!("stage {} example '{}': {err}", s.number, e.title)
                    });
                    assert!(
                        !bytes.is_empty(),
                        "stage {} example '{}' builds no bytes",
                        s.number,
                        e.title
                    );
                }
            }
        }
    }

    #[test]
    fn scenario_kinds_map_to_the_json_vocabulary() {
        assert_eq!(Scenario::Text.kind(), "text");
        assert_eq!(Scenario::Echo { line: "x" }.kind(), "wire");
        fn build(_: &ExampleEnv) -> Result<Vec<u8>, String> {
            Ok(vec![0])
        }
        assert_eq!(
            Scenario::Raw {
                build,
                expect: Expect::UntilClose
            }
            .kind(),
            "closed"
        );
    }
}
