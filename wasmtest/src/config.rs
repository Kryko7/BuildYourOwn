//! `runtimes.yaml`: how to launch each runtime under test, plus placeholder substitution.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimesFile {
    runtimes: BTreeMap<String, RuntimeDef>,
}

/// How a runtime is obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    /// Spawned from `command`.
    External,
    /// `wasmtime`, downloaded and unpacked by the harness.
    Reference,
}

fn default_kind() -> RuntimeKind {
    RuntimeKind::External
}

/// One entry of `runtimes.yaml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeDef {
    /// The key this runtime has in `runtimes.yaml`.
    #[serde(skip)]
    pub name: String,
    /// Whether the harness spawns `command` or downloads `wasmtime`.
    #[serde(default = "default_kind")]
    pub kind: RuntimeKind,
    /// Reference runtimes only: the `wasmtime` version to download.
    #[serde(default)]
    pub version: Option<String>,
    /// argv prefix of the runtime; `run`, `--invoke`, the module path and the module's own
    /// arguments are appended to it. `{TMP}` is substituted per test.
    #[serde(default)]
    pub command: Vec<String>,
    /// Working directory of the runtime process.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Extra environment variables for the runtime process.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// Values substituted into `command`, `cwd` and `env`.
#[derive(Debug, Clone)]
pub struct Placeholders {
    /// The test's temporary directory, where the module under test is written.
    pub tmp: PathBuf,
}

impl Placeholders {
    /// Replace `{TMP}` in `s`.
    pub fn apply(&self, s: &str) -> String {
        s.replace("{TMP}", &self.tmp.to_string_lossy())
    }
}

/// Read and validate `runtimes.yaml`.
pub fn load_runtimes(path: &Path) -> Result<BTreeMap<String, RuntimeDef>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read runtimes file {}", path.display()))?;
    let file: RuntimesFile = serde_yaml::from_str(&text)
        .with_context(|| format!("invalid runtimes file {}", path.display()))?;
    let mut runtimes = file.runtimes;
    for (name, def) in runtimes.iter_mut() {
        def.name = name.clone();
        match def.kind {
            RuntimeKind::External if def.command.is_empty() => {
                bail!("runtime '{name}': `command` must be non-empty for kind: external")
            }
            RuntimeKind::Reference if !def.command.is_empty() => {
                bail!("runtime '{name}': kind: reference takes no `command`")
            }
            _ => {}
        }
    }
    Ok(runtimes)
}

/// `--runtime` accepts a registered name or a path to an executable.
///
/// A path becomes `["<path>"]` with no working directory of its own, which is what the track
/// conventions describe: the harness appends `run [--invoke f] <module.wasm> [args...]`.
pub fn resolve_runtime(spec: &str, runtimes: &BTreeMap<String, RuntimeDef>) -> Result<RuntimeDef> {
    if let Some(d) = runtimes.get(spec) {
        return Ok(d.clone());
    }
    let path = Path::new(spec);
    if !path.exists() {
        let known: Vec<_> = runtimes.keys().cloned().collect();
        bail!(
            "'{spec}' is neither a registered runtime ({}) nor an existing path",
            known.join(", ")
        );
    }
    let abs = std::fs::canonicalize(path)
        .with_context(|| format!("cannot resolve runtime path {spec}"))?;
    let name = abs
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| spec.to_string());
    Ok(RuntimeDef {
        name,
        kind: RuntimeKind::External,
        version: None,
        command: vec![abs.to_string_lossy().to_string()],
        cwd: None,
        env: BTreeMap::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(text: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("runtimes.yaml");
        std::fs::write(&p, text).expect("write");
        (dir, p)
    }

    #[test]
    fn parses_and_names_runtimes() {
        let (_d, p) = write("runtimes:\n  r:\n    command: [./your_program.sh]\n    cwd: '.'\n");
        let runtimes = load_runtimes(&p).expect("load");
        let r = &runtimes["r"];
        assert_eq!(r.name, "r");
        assert_eq!(r.kind, RuntimeKind::External);
        assert_eq!(r.command, vec!["./your_program.sh"]);
    }

    #[test]
    fn a_reference_runtime_takes_no_command() {
        let (_d, p) = write("runtimes:\n  w:\n    kind: reference\n    command: [x]\n");
        assert!(load_runtimes(&p).is_err());
    }

    #[test]
    fn rejects_external_without_command() {
        let (_d, p) = write("runtimes:\n  r:\n    cwd: '.'\n");
        assert!(load_runtimes(&p).is_err());
    }

    #[test]
    fn placeholders_substitute() {
        let ph = Placeholders {
            tmp: PathBuf::from("/t/i0001"),
        };
        assert_eq!(ph.apply("{TMP}/mod.wasm"), "/t/i0001/mod.wasm");
    }

    #[test]
    fn unknown_runtime_is_an_error() {
        let runtimes = BTreeMap::new();
        assert!(resolve_runtime("definitely_not_here_xyz", &runtimes).is_err());
    }
}
