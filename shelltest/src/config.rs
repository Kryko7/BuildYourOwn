//! `shells.yaml`: how to launch each reference shell (or the shell under test).

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShellsFile {
    shells: BTreeMap<String, ShellDef>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShellDef {
    #[serde(skip)]
    pub name: String,
    pub pipe_command: Vec<String>,
    pub pty_command: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub init_script: Option<String>,
    #[serde(default = "default_prompt")]
    pub prompt: String,
}

fn default_prompt() -> String {
    "$ ".to_string()
}

pub fn load_shells(path: &Path) -> Result<BTreeMap<String, ShellDef>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read shells file {}", path.display()))?;
    let file: ShellsFile = serde_yaml::from_str(&text)
        .with_context(|| format!("invalid shells file {}", path.display()))?;
    let mut shells = file.shells;
    for (name, def) in shells.iter_mut() {
        def.name = name.clone();
        if def.pipe_command.is_empty() || def.pty_command.is_empty() {
            bail!("shell '{name}': pipe_command and pty_command must be non-empty");
        }
    }
    Ok(shells)
}

/// `--shell` accepts a registered name or a path to a shell binary.
pub fn resolve_shell(spec: &str, shells: &BTreeMap<String, ShellDef>) -> Result<ShellDef> {
    let mut def = match shells.get(spec) {
        Some(d) => d.clone(),
        None => {
            let path = Path::new(spec);
            if !path.exists() {
                let known: Vec<_> = shells.keys().cloned().collect();
                bail!("'{spec}' is neither a registered shell ({}) nor an existing path", known.join(", "));
            }
            let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            ShellDef {
                name,
                pipe_command: vec![spec.to_string()],
                pty_command: vec![spec.to_string()],
                env: BTreeMap::new(),
                init_script: None,
                prompt: default_prompt(),
            }
        }
    };
    def.pipe_command[0] = resolve_program(&def.pipe_command[0])?;
    def.pty_command[0] = resolve_program(&def.pty_command[0])?;
    Ok(def)
}

/// Children run with a controlled PATH and cwd, so the program must be absolute up front.
pub fn resolve_program(prog: &str) -> Result<String> {
    let p = Path::new(prog);
    if prog.contains('/') {
        let abs = std::fs::canonicalize(p).with_context(|| format!("shell binary not found: {prog}"))?;
        return Ok(abs.to_string_lossy().to_string());
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|dir| dir.join(prog))
        .find(|cand| is_executable(cand.as_path()))
        .map(|p| p.to_string_lossy().to_string())
        .ok_or_else(|| anyhow!("'{prog}' not found in PATH"))
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata().map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shells_and_resolves_names() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("shells.yaml");
        std::fs::write(&f, "shells:\n  sh:\n    pipe_command: [sh]\n    pty_command: [sh, -i]\n").unwrap();
        let shells = load_shells(&f).unwrap();
        let def = resolve_shell("sh", &shells).unwrap();
        assert_eq!(def.name, "sh");
        assert!(def.pipe_command[0].starts_with('/'));
        assert_eq!(def.prompt, "$ ");
        assert!(resolve_shell("no_such_shell_xyz", &shells).is_err());
    }

    #[test]
    fn resolves_paths_directly() {
        let shells = BTreeMap::new();
        let def = resolve_shell("/bin/sh", &shells).unwrap();
        assert_eq!(def.name, "sh");
        assert_eq!(def.pty_command, def.pipe_command);
    }
}
