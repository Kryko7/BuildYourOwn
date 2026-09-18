//! `linkers.yaml`: how to invoke each linker `--linker <name>` knows about.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkersFile {
    linkers: BTreeMap<String, LinkerDef>,
}

/// How a linker is obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkerKind {
    /// Run `command`, which is the learner's `your_program.sh`.
    External,
    /// GNU ld, resolved from `$LD` or `/usr/bin/ld`; the `--validate` target.
    Reference,
}

fn default_kind() -> LinkerKind {
    LinkerKind::External
}

/// One entry of `linkers.yaml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkerDef {
    /// The key this linker has in `linkers.yaml`.
    #[serde(skip)]
    pub name: String,
    /// Whether the harness runs `command` or resolves GNU ld.
    #[serde(default = "default_kind")]
    pub kind: LinkerKind,
    /// argv prefix of the linker; the harness appends `-o <out>`, the flags and the inputs.
    #[serde(default)]
    pub command: Vec<String>,
    /// Working directory of the linker process. `{TMP}` is the test's own directory, which
    /// is where the input objects are written.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Extra environment variables for the linker process.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// Values substituted into `command`, `cwd` and `env`.
#[derive(Debug, Clone)]
pub struct Placeholders {
    /// The test's temporary directory, where the inputs live and the output is written.
    pub tmp: PathBuf,
}

impl Placeholders {
    /// Replace `{TMP}` in `s`.
    pub fn apply(&self, s: &str) -> String {
        s.replace("{TMP}", &self.tmp.to_string_lossy())
    }
}

/// Read and validate `linkers.yaml`.
pub fn load_linkers(path: &Path) -> Result<BTreeMap<String, LinkerDef>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read linkers file {}", path.display()))?;
    let file: LinkersFile = serde_yaml::from_str(&text)
        .with_context(|| format!("invalid linkers file {}", path.display()))?;
    let mut linkers = file.linkers;
    for (name, def) in linkers.iter_mut() {
        def.name = name.clone();
        match def.kind {
            LinkerKind::External if def.command.is_empty() => {
                bail!("linker '{name}': `command` must be non-empty for kind: external")
            }
            LinkerKind::Reference if !def.command.is_empty() => {
                bail!("linker '{name}': kind: reference takes no `command` (it resolves $LD)")
            }
            _ => {}
        }
    }
    Ok(linkers)
}

/// `--linker` accepts a registered name or a path to an executable.
///
/// A path becomes `["<path>"]` with no working directory of its own, which is what the track
/// conventions describe for `./your_program.sh`.
pub fn resolve_linker(spec: &str, linkers: &BTreeMap<String, LinkerDef>) -> Result<LinkerDef> {
    if let Some(d) = linkers.get(spec) {
        return Ok(d.clone());
    }
    let path = Path::new(spec);
    if !path.exists() {
        let known: Vec<_> = linkers.keys().cloned().collect();
        bail!(
            "'{spec}' is neither a registered linker ({}) nor an existing path",
            known.join(", ")
        );
    }
    let abs =
        std::fs::canonicalize(path).with_context(|| format!("cannot resolve linker path {spec}"))?;
    let name = abs
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| spec.to_string());
    Ok(LinkerDef {
        name,
        kind: LinkerKind::External,
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
        let p = dir.path().join("linkers.yaml");
        std::fs::write(&p, text).expect("write");
        (dir, p)
    }

    #[test]
    fn parses_and_names_linkers() {
        let (_d, p) = write("linkers:\n  l:\n    command: [./your_program.sh]\n    cwd: \".\"\n");
        let linkers = load_linkers(&p).expect("load");
        let l = &linkers["l"];
        assert_eq!(l.name, "l");
        assert_eq!(l.kind, LinkerKind::External);
        assert_eq!(l.command, vec!["./your_program.sh"]);
    }

    #[test]
    fn a_reference_linker_takes_no_command() {
        let (_d, p) = write("linkers:\n  gnu_ld:\n    kind: reference\n    command: [ld]\n");
        assert!(load_linkers(&p).is_err());
    }

    #[test]
    fn rejects_external_without_command() {
        let (_d, p) = write("linkers:\n  l:\n    cwd: \".\"\n");
        assert!(load_linkers(&p).is_err());
    }

    #[test]
    fn placeholders_substitute() {
        let ph = Placeholders {
            tmp: PathBuf::from("/t/i0001"),
        };
        assert_eq!(ph.apply("{TMP}/out"), "/t/i0001/out");
    }

    #[test]
    fn unknown_linker_is_an_error() {
        let linkers = BTreeMap::new();
        assert!(resolve_linker("definitely_not_here_xyz", &linkers).is_err());
    }
}
