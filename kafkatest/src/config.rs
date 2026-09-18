//! `brokers.yaml`: how to launch each broker under test, plus placeholder substitution.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BrokersFile {
    brokers: BTreeMap<String, BrokerDef>,
}

/// How a broker is obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrokerKind {
    /// Spawned from `command`.
    External,
    /// Apache Kafka, downloaded and formatted by the harness.
    Reference,
}

/// How the topics/records a test needs are put in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixtureStrategy {
    /// Write Kafka's on-disk format into the log dir before the broker starts.
    Files,
    /// Create topics and records through the wire protocol on a running broker.
    Api,
}

/// When the broker process is restarted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartPolicy {
    /// A fresh process for every test (required by the `files` strategy).
    PerTest,
    /// One process per stage, reused by the stage's tests.
    PerStage,
}

fn default_kind() -> BrokerKind {
    BrokerKind::External
}
fn default_boot_timeout() -> u64 {
    30_000
}

/// One entry of `brokers.yaml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerDef {
    /// The key this broker has in `brokers.yaml`.
    #[serde(skip)]
    pub name: String,
    /// Whether the harness spawns `command` or downloads Apache Kafka.
    #[serde(default = "default_kind")]
    pub kind: BrokerKind,
    /// Reference brokers only: the Apache Kafka version to download.
    #[serde(default)]
    pub version: Option<String>,
    /// argv of the broker; `{PROPS}` etc. are substituted per test.
    #[serde(default)]
    pub command: Vec<String>,
    /// Fixed port. Omitted (or 0) means "pick a free one"; the reference broker always does.
    #[serde(default)]
    pub port: Option<u16>,
    /// Where the broker keeps its logs. Omitted means `{TMP}/kraft-combined-logs`.
    #[serde(default)]
    pub log_dir: Option<String>,
    /// How the topics a test needs are put in place.
    pub fixtures: FixtureStrategy,
    /// When the broker process is restarted.
    pub restart: RestartPolicy,
    /// Working directory of the broker process.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Extra environment variables for the broker process.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// How long to wait for the broker to start listening.
    #[serde(default = "default_boot_timeout")]
    pub boot_timeout_ms: u64,
}

impl BrokerDef {
    /// A broker whose fixtures are read at startup must be restarted for every test.
    pub fn effective_restart(&self) -> RestartPolicy {
        match self.fixtures {
            FixtureStrategy::Files => RestartPolicy::PerTest,
            FixtureStrategy::Api => self.restart,
        }
    }
}

/// Values substituted into `command`, `cwd`, `log_dir` and `env`.
#[derive(Debug, Clone)]
pub struct Placeholders {
    /// The properties file written for this broker instance.
    pub props: PathBuf,
    /// The Kafka log directory.
    pub log_dir: PathBuf,
    /// The broker instance's temporary directory.
    pub tmp: PathBuf,
    /// The listener port.
    pub port: u16,
}

impl Placeholders {
    /// Replace `{PROPS}`, `{LOGDIR}`, `{TMP}` and `{PORT}` in `s`.
    pub fn apply(&self, s: &str) -> String {
        s.replace("{PROPS}", &self.props.to_string_lossy())
            .replace("{LOGDIR}", &self.log_dir.to_string_lossy())
            .replace("{TMP}", &self.tmp.to_string_lossy())
            .replace("{PORT}", &self.port.to_string())
    }
}

/// Read and validate `brokers.yaml`.
pub fn load_brokers(path: &Path) -> Result<BTreeMap<String, BrokerDef>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read brokers file {}", path.display()))?;
    let file: BrokersFile = serde_yaml::from_str(&text)
        .with_context(|| format!("invalid brokers file {}", path.display()))?;
    let mut brokers = file.brokers;
    for (name, def) in brokers.iter_mut() {
        def.name = name.clone();
        match def.kind {
            BrokerKind::External if def.command.is_empty() => {
                bail!("broker '{name}': `command` must be non-empty for kind: external")
            }
            BrokerKind::Reference if !def.command.is_empty() => {
                bail!("broker '{name}': kind: reference takes no `command`")
            }
            _ => {}
        }
        if def.port == Some(9092) && def.kind == BrokerKind::Reference {
            bail!("broker '{name}': the reference broker must use a random port");
        }
    }
    Ok(brokers)
}

/// `--broker` accepts a registered name or a path to an executable.
///
/// A path becomes `["<path>", "{PROPS}"]` with `files` fixtures and a per-test restart,
/// which is what the track conventions describe.
pub fn resolve_broker(spec: &str, brokers: &BTreeMap<String, BrokerDef>) -> Result<BrokerDef> {
    if let Some(d) = brokers.get(spec) {
        return Ok(d.clone());
    }
    let path = Path::new(spec);
    if !path.exists() {
        let known: Vec<_> = brokers.keys().cloned().collect();
        bail!(
            "'{spec}' is neither a registered broker ({}) nor an existing path",
            known.join(", ")
        );
    }
    let abs = std::fs::canonicalize(path)
        .with_context(|| format!("cannot resolve broker path {spec}"))?;
    let name = abs
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| spec.to_string());
    Ok(BrokerDef {
        name,
        kind: BrokerKind::External,
        version: None,
        command: vec![abs.to_string_lossy().to_string(), "{PROPS}".to_string()],
        port: None,
        log_dir: None,
        fixtures: FixtureStrategy::Files,
        restart: RestartPolicy::PerTest,
        cwd: None,
        env: BTreeMap::new(),
        boot_timeout_ms: default_boot_timeout(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(text: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("brokers.yaml");
        std::fs::write(&p, text).expect("write");
        (dir, p)
    }

    #[test]
    fn parses_and_names_brokers() {
        let (_d, p) = write(
            "brokers:\n  b:\n    command: [./x, '{PROPS}']\n    fixtures: files\n    restart: per_test\n",
        );
        let brokers = load_brokers(&p).expect("load");
        let b = &brokers["b"];
        assert_eq!(b.name, "b");
        assert_eq!(b.kind, BrokerKind::External);
        assert_eq!(b.effective_restart(), RestartPolicy::PerTest);
    }

    #[test]
    fn files_fixtures_force_per_test_restart() {
        let (_d, p) = write(
            "brokers:\n  b:\n    command: [./x]\n    fixtures: files\n    restart: per_stage\n",
        );
        let brokers = load_brokers(&p).expect("load");
        assert_eq!(brokers["b"].effective_restart(), RestartPolicy::PerTest);
    }

    #[test]
    fn rejects_external_without_command() {
        let (_d, p) = write("brokers:\n  b:\n    fixtures: api\n    restart: per_stage\n");
        assert!(load_brokers(&p).is_err());
    }

    #[test]
    fn placeholders_substitute() {
        let ph = Placeholders {
            props: PathBuf::from("/t/server.properties"),
            log_dir: PathBuf::from("/t/logs"),
            tmp: PathBuf::from("/t"),
            port: 1234,
        };
        assert_eq!(
            ph.apply("{PROPS} {LOGDIR} {TMP} {PORT}"),
            "/t/server.properties /t/logs /t 1234"
        );
    }

    #[test]
    fn unknown_broker_is_an_error() {
        let brokers = BTreeMap::new();
        assert!(resolve_broker("definitely_not_here_xyz", &brokers).is_err());
    }
}
