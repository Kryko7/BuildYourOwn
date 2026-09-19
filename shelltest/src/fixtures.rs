//! Per-test sandbox: temp dir, fake executables, controlled environment.

use crate::config::ShellDef;
use crate::loader::Fixtures;
use crate::normalize::Placeholders;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub struct Sandbox {
    pub tmp: PathBuf,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub vars: Placeholders,
    /// `None` when the directory is kept (`--keep-tmp`).
    dir: Option<tempfile::TempDir>,
}

impl Sandbox {
    pub fn create(fx: &Fixtures, shell: &ShellDef, keep: bool) -> Result<Sandbox> {
        let dir = tempfile::Builder::new()
            .prefix("shelltest-")
            .tempdir()
            .context("creating sandbox dir")?;
        let tmp = std::fs::canonicalize(dir.path())?;
        let bin = tmp.join("bin");
        let home = tmp.join("home");
        let histfile = tmp.join(".history");
        let init = tmp.join(".shelltest_init");
        std::fs::create_dir_all(&bin)?;
        std::fs::create_dir_all(&home)?;
        let vars = Placeholders {
            pairs: vec![
                ("{TMP}", tmp.to_string_lossy().to_string()),
                ("{BIN}", bin.to_string_lossy().to_string()),
                ("{HOME}", home.to_string_lossy().to_string()),
                ("{HISTFILE}", histfile.to_string_lossy().to_string()),
                ("{INIT}", init.to_string_lossy().to_string()),
                ("{SHELL}", shell.pipe_command[0].clone()),
            ],
        };
        let resolve = |p: &str| -> PathBuf { tmp.join(vars.apply(p)) };

        for d in &fx.dirs {
            std::fs::create_dir_all(resolve(d))?;
        }
        for (path, content) in &fx.files {
            let p = resolve(path);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&p, vars.apply(content))?;
        }
        for (name, body) in &fx.executables {
            let mut script = vars.apply(body);
            if !script.starts_with("#!") {
                script = format!("#!/bin/sh\n{script}");
            }
            if !script.ends_with('\n') {
                script.push('\n');
            }
            let p = if name.contains('/') {
                resolve(name)
            } else {
                bin.join(name)
            };
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&p, script)?;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755))?;
        }
        if let Some(h) = &fx.histfile {
            std::fs::write(&histfile, vars.apply(h))?;
        }
        if let Some(script) = &shell.init_script {
            std::fs::write(&init, vars.apply(script))?;
        }

        let mut path = bin.to_string_lossy().to_string();
        if !fx.path_isolated {
            if let Some(host) = std::env::var_os("PATH") {
                path = format!("{path}:{}", host.to_string_lossy());
            }
        }
        let mut env = BTreeMap::new();
        env.insert("PATH".into(), path);
        env.insert("HOME".into(), home.to_string_lossy().to_string());
        env.insert("HISTFILE".into(), histfile.to_string_lossy().to_string());
        env.insert("TERM".into(), "dumb".into());
        env.insert("LANG".into(), "C".into());
        env.insert("LC_ALL".into(), "C".into());
        for (k, v) in &shell.env {
            env.insert(k.clone(), vars.apply(v));
        }
        for (k, v) in &fx.env {
            env.insert(k.clone(), vars.apply(v));
        }
        let cwd = fx
            .cwd
            .as_deref()
            .map(resolve)
            .unwrap_or_else(|| tmp.clone());
        std::fs::create_dir_all(&cwd)?;

        let dir = if keep {
            let _ = dir.keep();
            None
        } else {
            Some(dir)
        };
        Ok(Sandbox {
            tmp,
            cwd,
            env,
            vars,
            dir,
        })
    }

    pub fn kept_path(&self) -> Option<&Path> {
        self.dir.is_none().then_some(self.tmp.as_path())
    }

    pub fn resolve(&self, p: &str) -> PathBuf {
        self.tmp.join(self.vars.apply(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell() -> ShellDef {
        ShellDef {
            name: "sh".into(),
            pipe_command: vec!["/bin/sh".into()],
            pty_command: vec!["/bin/sh".into()],
            env: [("PS1".to_string(), "$ ".to_string())].into(),
            init_script: Some("echo init {TMP}".into()),
            prompt: "$ ".into(),
        }
    }

    #[test]
    fn builds_sandbox() {
        let fx: Fixtures = serde_yaml::from_str(
            "files: {'sub/a.txt': 'A', '{HOME}/b': 'B'}\ndirs: [d1]\nexecutables: {greet: 'echo hi $1'}\npath_isolated: true\nenv: {FOO: '{BIN}'}\nhistfile: \"echo a\\n\"\ncwd: sub\n",
        )
        .unwrap();
        let sb = Sandbox::create(&fx, &shell(), false).unwrap();
        assert_eq!(
            std::fs::read_to_string(sb.tmp.join("sub/a.txt")).unwrap(),
            "A"
        );
        assert_eq!(std::fs::read_to_string(sb.tmp.join("home/b")).unwrap(), "B");
        assert!(sb.tmp.join("d1").is_dir());
        let greet = std::fs::read_to_string(sb.tmp.join("bin/greet")).unwrap();
        assert!(greet.starts_with("#!/bin/sh\necho hi $1\n"));
        assert_eq!(sb.env["PATH"], sb.tmp.join("bin").to_string_lossy());
        assert_eq!(sb.env["FOO"], sb.env["PATH"]);
        assert_eq!(sb.env["PS1"], "$ ");
        assert_eq!(
            std::fs::read_to_string(sb.tmp.join(".history")).unwrap(),
            "echo a\n"
        );
        assert!(std::fs::read_to_string(sb.tmp.join(".shelltest_init"))
            .unwrap()
            .contains(&sb.tmp.to_string_lossy().to_string()));
        assert_eq!(sb.cwd, sb.tmp.join("sub"));
        assert!(sb.kept_path().is_none());
        let path = sb.tmp.clone();
        drop(sb);
        assert!(!path.exists());
    }
}
