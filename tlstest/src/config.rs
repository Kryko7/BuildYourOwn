//! `servers.yaml`: how to launch each TLS server under test.
//!
//! The command line is fixed by the track's contract, so an entry only says how to *start*
//! the program; the harness appends the flags itself:
//!
//! ```text
//! <command...> -accept <port> -cert <cert.pem> -key <key.pem> -rev [-naccept <n>]
//! ```
//!
//! That is a subset of `openssl s_server`'s own flags, which is what lets the reference be
//! the real thing rather than a model of it.

use crate::certs::{CertKind, KeyKind};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServersFile {
    servers: BTreeMap<String, ServerDef>,
}

/// How a server is obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerKind {
    /// Spawned from `command`.
    External,
    /// The system `openssl`, run as `s_server -tls1_3`.
    Reference,
}

/// When the server process is restarted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartPolicy {
    /// A fresh process for every test. The only safe default: a test that abandons a
    /// handshake, or one that sends `-naccept`'s last connection, changes what the next
    /// test would see.
    PerTest,
    /// One process per stage, reused by the stage's tests.
    PerStage,
}

fn default_kind() -> ServerKind {
    ServerKind::External
}
fn default_restart() -> RestartPolicy {
    RestartPolicy::PerTest
}
fn default_boot_timeout() -> u64 {
    10_000
}

/// One entry of `servers.yaml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerDef {
    /// The key this server has in `servers.yaml`.
    #[serde(skip)]
    pub name: String,
    /// Whether the harness spawns `command` or resolves the system `openssl`.
    #[serde(default = "default_kind")]
    pub kind: ServerKind,
    /// How to start the program; the contract's flags are appended by the harness.
    #[serde(default)]
    pub command: Vec<String>,
    /// Working directory of the server process.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Extra environment variables.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// When the process is restarted.
    #[serde(default = "default_restart")]
    pub restart: RestartPolicy,
    /// How long to wait for the server to start listening.
    #[serde(default = "default_boot_timeout")]
    pub boot_timeout_ms: u64,
    /// Extra arguments appended after the contract's flags (reference servers only).
    #[serde(default)]
    pub extra_args: Vec<String>,
}

/// Values substituted into `command`, `cwd` and `env`.
#[derive(Debug, Clone)]
pub struct Placeholders {
    /// The port the server must listen on.
    pub port: u16,
    /// The certificate PEM.
    pub cert: PathBuf,
    /// The private key PEM.
    pub key: PathBuf,
    /// The server instance's temporary directory.
    pub tmp: PathBuf,
}

impl Placeholders {
    /// Replace `{PORT}`, `{CERT}`, `{KEY}` and `{TMP}` in `s`.
    pub fn apply(&self, s: &str) -> String {
        s.replace("{PORT}", &self.port.to_string())
            .replace("{CERT}", &self.cert.to_string_lossy())
            .replace("{KEY}", &self.key.to_string_lossy())
            .replace("{TMP}", &self.tmp.to_string_lossy())
    }
}

/// How one test wants the server started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerOptions {
    /// Which certificate and key to hand it.
    pub cert: CertKind,
    /// `-naccept n`, when the test wants the server to exit after n connections.
    pub naccept: Option<u32>,
    /// Extra arguments for the reference server only, e.g. `-num_tickets 0`.
    pub reference_args: Vec<String>,
    /// Ask the server to request a client certificate, and whether it must insist on one.
    ///
    /// `None` is the default and adds no flags. `Some(false)` adds `-verify 2 -CAfile CA`,
    /// which *requests* a certificate and carries on without one; `Some(true)` adds
    /// `-Verify 2 -CAfile CA`, which requires one and fails the handshake otherwise. That
    /// capital letter is the entire difference, in openssl and in the contract.
    pub client_auth: Option<bool>,
}

impl Default for ServerOptions {
    fn default() -> ServerOptions {
        ServerOptions {
            cert: CertKind::Leaf(KeyKind::EcdsaP256),
            naccept: None,
            reference_args: Vec::new(),
            client_auth: None,
        }
    }
}

impl ServerOptions {
    /// Start the server with this certificate kind.
    pub fn with_cert(cert: CertKind) -> ServerOptions {
        ServerOptions {
            cert,
            ..ServerOptions::default()
        }
    }

    /// Start the server with `-naccept n`.
    pub fn with_naccept(mut self, n: u32) -> ServerOptions {
        self.naccept = Some(n);
        self
    }

    /// Ask the server to request a client certificate (`-verify`), without requiring one.
    pub fn requesting_client_cert(mut self) -> ServerOptions {
        self.client_auth = Some(false);
        self
    }

    /// Ask the server to require a client certificate (`-Verify`).
    pub fn requiring_client_cert(mut self) -> ServerOptions {
        self.client_auth = Some(true);
        self
    }

    /// Pass extra arguments to the reference server (ignored for anyone else's).
    pub fn with_reference_args(mut self, args: &[&str]) -> ServerOptions {
        self.reference_args = args.iter().map(|a| (*a).to_string()).collect();
        self
    }

    /// True when this is the plain default, which the runner can reuse a process for.
    pub fn is_default(&self) -> bool {
        *self == ServerOptions::default()
    }
}

/// Read and validate `servers.yaml`.
pub fn load_servers(path: &Path) -> Result<BTreeMap<String, ServerDef>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read servers file {}", path.display()))?;
    let file: ServersFile = serde_yaml::from_str(&text)
        .with_context(|| format!("invalid servers file {}", path.display()))?;
    let mut servers = file.servers;
    for (name, def) in servers.iter_mut() {
        def.name = name.clone();
        match def.kind {
            ServerKind::External if def.command.is_empty() => {
                bail!("server '{name}': `command` must be non-empty for kind: external")
            }
            ServerKind::Reference if !def.command.is_empty() => {
                bail!("server '{name}': kind: reference takes no `command`; it resolves openssl")
            }
            _ => {}
        }
    }
    Ok(servers)
}

/// `--server` accepts a registered name or a path to an executable.
///
/// A path becomes `["<path>"]` with a per-test restart, which is what the track's
/// conventions describe for `./your_program.sh`.
pub fn resolve_server(spec: &str, servers: &BTreeMap<String, ServerDef>) -> Result<ServerDef> {
    if let Some(d) = servers.get(spec) {
        return Ok(d.clone());
    }
    let path = Path::new(spec);
    if !path.exists() {
        let known: Vec<_> = servers.keys().cloned().collect();
        bail!(
            "'{spec}' is neither a registered server ({}) nor an existing path",
            known.join(", ")
        );
    }
    let abs = std::fs::canonicalize(path)
        .with_context(|| format!("cannot resolve server path {spec}"))?;
    let name = abs
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| spec.to_string());
    Ok(ServerDef {
        name,
        kind: ServerKind::External,
        command: vec![abs.to_string_lossy().to_string()],
        cwd: None,
        env: BTreeMap::new(),
        restart: RestartPolicy::PerTest,
        boot_timeout_ms: default_boot_timeout(),
        extra_args: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(text: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("servers.yaml");
        std::fs::write(&p, text).expect("write");
        (dir, p)
    }

    #[test]
    fn parses_and_names_servers() {
        let (_d, p) = write(
            "servers:\n  openssl:\n    kind: reference\n  mine:\n    command: [./your_program.sh]\n    cwd: \".\"\n",
        );
        let servers = load_servers(&p).expect("load");
        assert_eq!(servers["openssl"].kind, ServerKind::Reference);
        assert_eq!(servers["openssl"].name, "openssl");
        assert_eq!(servers["mine"].kind, ServerKind::External);
        assert_eq!(servers["mine"].restart, RestartPolicy::PerTest);
    }

    #[test]
    fn rejects_external_without_command_and_reference_with_one() {
        let (_d, p) = write("servers:\n  mine:\n    kind: external\n");
        assert!(load_servers(&p).is_err());
        let (_d, p) = write("servers:\n  openssl:\n    kind: reference\n    command: [x]\n");
        assert!(load_servers(&p).is_err());
    }

    #[test]
    fn unknown_servers_are_an_error() {
        assert!(resolve_server("definitely_not_here_xyz", &BTreeMap::new()).is_err());
    }

    #[test]
    fn placeholders_substitute() {
        let ph = Placeholders {
            port: 4433,
            cert: PathBuf::from("/t/c.pem"),
            key: PathBuf::from("/t/k.pem"),
            tmp: PathBuf::from("/t"),
        };
        assert_eq!(
            ph.apply("{PORT} {CERT} {KEY} {TMP}"),
            "4433 /t/c.pem /t/k.pem /t"
        );
    }

    #[test]
    fn default_options_are_recognised_so_a_process_can_be_reused() {
        assert!(ServerOptions::default().is_default());
        assert!(!ServerOptions::default().with_naccept(1).is_default());
        assert!(!ServerOptions::with_cert(CertKind::Chain).is_default());
    }
}
