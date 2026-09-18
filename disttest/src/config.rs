//! `targets.yaml`: how to launch each program under test, and which reference each ladder
//! is validated against.
//!
//! A target is one of three kinds:
//!
//! * `reference` — real etcd, downloaded and cached by [`crate::node::reference`]. It serves
//!   the `node` and `cluster` ladders.
//! * `example` — one of this crate's own `examples/*.rs` binaries. `reference_primitives`
//!   serves the `primitives` ladder; `broken_node` is a deliberately wrong node.
//! * `external` (the default) — the learner's `command`, which the harness calls with the
//!   ladder's own argv appended.

use crate::stages::Ladder;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetsFile {
    targets: BTreeMap<String, TargetDef>,
}

/// How a target is obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    /// Spawned from `command`, with the ladder's argv appended.
    External,
    /// Real etcd, downloaded and cached by the harness.
    Reference,
    /// One of this crate's `examples/*.rs` binaries.
    Example,
}

fn default_kind() -> TargetKind {
    TargetKind::External
}
fn default_boot_timeout() -> u64 {
    20_000
}

/// One entry of `targets.yaml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetDef {
    /// The key this target has in `targets.yaml`.
    #[serde(skip)]
    pub name: String,
    /// Whether the harness spawns `command`, downloads etcd, or builds an example.
    #[serde(default = "default_kind")]
    pub kind: TargetKind,
    /// Reference targets only: the etcd version to download.
    #[serde(default)]
    pub version: Option<String>,
    /// argv of the program; the ladder's own arguments are appended to it.
    #[serde(default)]
    pub command: Vec<String>,
    /// Working directory of the child process.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Extra environment variables for the child process.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// How long to wait for a node to start answering on its client URL.
    #[serde(default = "default_boot_timeout")]
    pub boot_timeout_ms: u64,
}

impl TargetDef {
    /// An example target with the given example-binary name.
    pub fn example(name: &str) -> TargetDef {
        TargetDef {
            name: name.to_string(),
            kind: TargetKind::Example,
            version: None,
            command: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            boot_timeout_ms: default_boot_timeout(),
        }
    }

    /// True when this target is one of the suite's own references.
    pub fn is_reference(&self) -> bool {
        matches!(self.kind, TargetKind::Reference | TargetKind::Example)
    }

    /// Can this target take part in the given ladder at all?
    ///
    /// Real etcd has no primitives CLI, and `reference_primitives` is not a server; the
    /// learner's own `command` serves all three.
    pub fn serves(&self, ladder: Ladder) -> bool {
        match (self.kind, ladder) {
            (TargetKind::External, _) => true,
            (TargetKind::Reference, Ladder::Primitives) => false,
            (TargetKind::Reference, _) => true,
            (TargetKind::Example, Ladder::Primitives) => self.name.contains("primitives"),
            (TargetKind::Example, _) => !self.name.contains("primitives"),
        }
    }
}

/// The reference target name a ladder is validated against.
///
/// `--validate` routes every stage to the reference its own ladder names, whatever
/// `--target` said, so one command can prove the whole suite. See README, "--validate".
pub fn reference_for(ladder: Ladder) -> &'static str {
    match ladder {
        Ladder::Primitives => "reference_primitives",
        Ladder::Node | Ladder::Cluster => "etcd",
    }
}

/// Read and validate `targets.yaml`.
pub fn load_targets(path: &Path) -> Result<BTreeMap<String, TargetDef>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read targets file {}", path.display()))?;
    let file: TargetsFile = serde_yaml::from_str(&text)
        .with_context(|| format!("invalid targets file {}", path.display()))?;
    let mut targets = file.targets;
    for (name, def) in targets.iter_mut() {
        def.name = name.clone();
        match def.kind {
            TargetKind::External if def.command.is_empty() => {
                bail!("target '{name}': `command` must be non-empty for kind: external")
            }
            TargetKind::Reference | TargetKind::Example if !def.command.is_empty() => {
                bail!("target '{name}': kind: {:?} takes no `command`", def.kind)
            }
            _ => {}
        }
    }
    for ladder in Ladder::ALL {
        let want = reference_for(ladder);
        if !targets.contains_key(want) {
            bail!(
                "targets.yaml has no '{want}', which the {} ladder is validated against",
                ladder.as_str()
            );
        }
    }
    Ok(targets)
}

/// `--target` accepts a registered name or a path to an executable.
///
/// A path becomes `["<path>"]` with `kind: external`, which is what the track conventions
/// describe: the harness appends the ladder's own arguments to it.
pub fn resolve_target(spec: &str, targets: &BTreeMap<String, TargetDef>) -> Result<TargetDef> {
    if let Some(d) = targets.get(spec) {
        return Ok(d.clone());
    }
    let path = Path::new(spec);
    if !path.exists() {
        let known: Vec<_> = targets.keys().cloned().collect();
        bail!(
            "'{spec}' is neither a registered target ({}) nor an existing path",
            known.join(", ")
        );
    }
    let abs =
        std::fs::canonicalize(path).with_context(|| format!("cannot resolve target path {spec}"))?;
    let name = abs
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| spec.to_string());
    Ok(TargetDef {
        name,
        kind: TargetKind::External,
        version: None,
        command: vec![abs.to_string_lossy().to_string()],
        cwd: None,
        env: BTreeMap::new(),
        boot_timeout_ms: default_boot_timeout(),
    })
}

/// Values substituted into `command`, `cwd` and `env`.
#[derive(Debug, Clone)]
pub struct Placeholders {
    /// The node's data directory.
    pub data_dir: PathBuf,
    /// The node instance's scratch directory.
    pub tmp: PathBuf,
    /// The client port this node listens on.
    pub port: u16,
    /// The peer port this node listens on.
    pub peer_port: u16,
    /// The node's name (`m1`, `m2`, ...).
    pub node: String,
}

impl Placeholders {
    /// Replace `{DATADIR}`, `{TMP}`, `{PORT}`, `{PEERPORT}` and `{NAME}` in `s`.
    pub fn apply(&self, s: &str) -> String {
        s.replace("{DATADIR}", &self.data_dir.to_string_lossy())
            .replace("{TMP}", &self.tmp.to_string_lossy())
            .replace("{PORT}", &self.port.to_string())
            .replace("{PEERPORT}", &self.peer_port.to_string())
            .replace("{NAME}", &self.node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = "targets:\n  etcd:\n    kind: reference\n    version: '3.7.1'\n  \
                           reference_primitives:\n    kind: example\n  my_node:\n    \
                           command: ['./your_program.sh']\n    cwd: '.'\n";

    fn write(text: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("targets.yaml");
        std::fs::write(&p, text).expect("write");
        (dir, p)
    }

    #[test]
    fn parses_and_names_targets() {
        let (_d, p) = write(MINIMAL);
        let targets = load_targets(&p).expect("load");
        assert_eq!(targets["etcd"].kind, TargetKind::Reference);
        assert_eq!(targets["etcd"].name, "etcd");
        assert_eq!(targets["my_node"].kind, TargetKind::External);
        assert_eq!(targets["reference_primitives"].kind, TargetKind::Example);
    }

    #[test]
    fn every_ladder_has_a_reference() {
        let (_d, p) = write("targets:\n  my_node:\n    command: ['./x']\n");
        let err = load_targets(&p).expect_err("must demand the references");
        assert!(format!("{err}").contains("reference_primitives"), "{err}");
    }

    #[test]
    fn rejects_external_without_command() {
        let (_d, p) = write(&MINIMAL.replace("command: ['./your_program.sh']\n    ", ""));
        assert!(load_targets(&p).is_err());
    }

    #[test]
    fn ladders_route_to_their_own_reference() {
        assert_eq!(reference_for(Ladder::Primitives), "reference_primitives");
        assert_eq!(reference_for(Ladder::Node), "etcd");
        assert_eq!(reference_for(Ladder::Cluster), "etcd");
    }

    #[test]
    fn references_only_serve_their_own_ladder() {
        let (_d, p) = write(MINIMAL);
        let t = load_targets(&p).expect("load");
        assert!(!t["etcd"].serves(Ladder::Primitives));
        assert!(t["etcd"].serves(Ladder::Node));
        assert!(t["reference_primitives"].serves(Ladder::Primitives));
        assert!(!t["reference_primitives"].serves(Ladder::Cluster));
        for l in Ladder::ALL {
            assert!(t["my_node"].serves(l), "the learner's program serves all");
        }
    }

    #[test]
    fn placeholders_substitute() {
        let ph = Placeholders {
            data_dir: PathBuf::from("/t/data"),
            tmp: PathBuf::from("/t"),
            port: 1234,
            peer_port: 1235,
            node: "m1".into(),
        };
        assert_eq!(
            ph.apply("{DATADIR} {TMP} {PORT} {PEERPORT} {NAME}"),
            "/t/data /t 1234 1235 m1"
        );
    }

    #[test]
    fn unknown_target_is_an_error() {
        let targets = BTreeMap::new();
        assert!(resolve_target("definitely_not_here_xyz", &targets).is_err());
    }
}
