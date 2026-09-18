//! Loads `tests/stages/NN_name.yaml` files and validates their schema.

use crate::matchers::Matcher;
use crate::normalize::NormalizeOpts;
use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Pipe,
    Pty,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "RawStep")]
pub enum Step {
    Send(String),
    Wait(String),
    SleepMs(u64),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStep {
    send: Option<String>,
    wait: Option<String>,
    sleep_ms: Option<u64>,
}

impl TryFrom<RawStep> for Step {
    type Error = String;
    fn try_from(r: RawStep) -> std::result::Result<Self, String> {
        match (r.send, r.wait, r.sleep_ms) {
            (Some(s), None, None) => Ok(Step::Send(s)),
            (None, Some(w), None) => Ok(Step::Wait(w)),
            (None, None, Some(ms)) => Ok(Step::SleepMs(ms)),
            _ => Err("a step must have exactly one of: send, wait, sleep_ms".into()),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Fixtures {
    pub files: BTreeMap<String, String>,
    pub dirs: Vec<String>,
    pub executables: BTreeMap<String, String>,
    pub path_isolated: bool,
    pub env: BTreeMap<String, String>,
    pub histfile: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Expect {
    pub stdout: Option<Matcher>,
    pub stderr: Option<Matcher>,
    pub terminal: Option<Matcher>,
    pub exit_code: Option<i32>,
    pub files: BTreeMap<String, Matcher>,
    pub file_absent: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestCase {
    pub name: String,
    #[serde(default = "default_mode")]
    pub mode: Mode,
    #[serde(default)]
    pub fixtures: Fixtures,
    #[serde(default)]
    pub input: Vec<String>,
    /// Extra argv appended to the shell command (script mode, `-c`).
    #[serde(default)]
    pub shell_args: Vec<String>,
    #[serde(default)]
    pub steps: Vec<Step>,
    #[serde(default)]
    pub keys: Vec<String>,
    #[serde(default)]
    pub key_delay_ms: Option<u64>,
    #[serde(default)]
    pub expect: Expect,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub skip_on: Vec<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub normalize: NormalizeOpts,
}

fn default_mode() -> Mode {
    Mode::Pipe
}

impl TestCase {
    pub fn is_ext(&self) -> bool {
        self.tags.iter().any(|t| t == "ext")
    }

    fn validate(&self) -> Result<()> {
        match self.mode {
            Mode::Pipe => {
                if !self.steps.is_empty() || !self.keys.is_empty() {
                    bail!("'steps'/'keys' are only valid in pty mode (use 'input')");
                }
                if self.expect.terminal.is_some() {
                    bail!("'expect.terminal' is only valid in pty mode");
                }
            }
            Mode::Pty => {
                if !self.input.is_empty() {
                    bail!("'input' is only valid in pipe mode (use 'keys' or 'steps')");
                }
                if !self.steps.is_empty() && !self.keys.is_empty() {
                    bail!("use either 'keys' or 'steps', not both");
                }
                if self.expect.stdout.is_some() || self.expect.stderr.is_some() {
                    bail!("pty mode captures one merged stream: use 'expect.terminal'");
                }
            }
        }
        if !self.skip_on.is_empty() && self.reason.as_deref().unwrap_or("").trim().is_empty() {
            bail!("'skip_on' requires a 'reason'");
        }
        let e = &self.expect;
        let nothing = e.stdout.is_none() && e.stderr.is_none() && e.terminal.is_none()
            && e.exit_code.is_none() && e.files.is_empty() && e.file_absent.is_empty();
        if nothing {
            bail!("'expect' has no assertions");
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Stage {
    pub stage: u32,
    pub name: String,
    pub file: PathBuf,
    pub tests: Vec<TestCase>,
}

impl Stage {
    pub fn file_name(&self) -> String {
        self.file.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()
    }
}

pub fn load_stage(path: &Path) -> Result<Stage> {
    let text = std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let doc: serde_yaml::Value = serde_yaml::from_str(&text)
        .with_context(|| format!("{}: not valid YAML", path.display()))?;
    let map = doc.as_mapping().ok_or_else(|| anyhow!("{}: top level must be a mapping", path.display()))?;
    for k in map.keys() {
        let k = k.as_str().unwrap_or("?");
        if !["stage", "name", "tests"].contains(&k) {
            bail!("{}: unknown top-level key '{k}'", path.display());
        }
    }
    let stage = map
        .get("stage")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| anyhow!("{}: missing or non-integer 'stage'", path.display()))? as u32;
    let name = map
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("{}: missing 'name'", path.display()))?
        .to_string();
    let raw_tests = map
        .get("tests")
        .and_then(|v| v.as_sequence())
        .ok_or_else(|| anyhow!("{}: 'tests' must be a list", path.display()))?;
    let mut tests = Vec::new();
    for (i, raw) in raw_tests.iter().enumerate() {
        let tname = raw
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| anyhow!("{}: test #{} has no 'name'", path.display(), i + 1))?;
        let tc: TestCase = serde_yaml::from_value(raw.clone())
            .map_err(|e| anyhow!("{}: test '{tname}': {}", path.display(), clean_err(&e.to_string())))?;
        tc.validate().map_err(|e| anyhow!("{}: test '{tname}': {e}", path.display()))?;
        if tests.iter().any(|t: &TestCase| t.name == tc.name) {
            bail!("{}: duplicate test name '{}'", path.display(), tc.name);
        }
        tests.push(tc);
    }
    if tests.is_empty() {
        bail!("{}: stage has no tests", path.display());
    }
    Ok(Stage { stage, name, file: path.to_path_buf(), tests })
}

fn clean_err(e: &str) -> String {
    e.split(" at line ").next().unwrap_or(e).to_string()
}

pub fn load_dir(dir: &Path) -> Result<Vec<Stage>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("cannot read tests dir {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "yaml" || x == "yml"))
        .collect();
    paths.sort();
    let mut stages: Vec<Stage> = paths.iter().map(|p| load_stage(p)).collect::<Result<_>>()?;
    stages.sort_by_key(|s| s.stage);
    for w in stages.windows(2) {
        if w[0].stage == w[1].stage {
            bail!("stage {} defined twice: {} and {}", w[0].stage, w[0].file.display(), w[1].file.display());
        }
    }
    Ok(stages)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(yaml: &str) -> Result<Stage> {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("01_x.yaml");
        std::fs::write(&f, yaml).unwrap();
        load_stage(&f)
    }

    #[test]
    fn loads_spec_example() {
        let s = load(
            r#"
stage: 5
name: "echo builtin"
tests:
  - name: "echo prints its arguments"
    mode: pipe
    input: ["echo hello world", "exit 0"]
    expect: { stdout: "hello world", exit_code: 0 }
  - name: "tab completes a builtin"
    mode: pty
    keys: ["ech\t", "\r", "exit\r"]
    expect:
      terminal: { contains: "echo " }
"#,
        )
        .unwrap();
        assert_eq!(s.stage, 5);
        assert_eq!(s.tests.len(), 2);
        assert_eq!(s.tests[1].mode, Mode::Pty);
        assert_eq!(s.tests[1].keys[0], "ech\t");
    }

    #[test]
    fn parses_steps() {
        let s = load("stage: 1\nname: x\ntests:\n  - name: t\n    mode: pty\n    steps: [{wait: '$ '}, {send: \"a\\r\"}, {sleep_ms: 5}]\n    expect: {terminal: a}\n").unwrap();
        assert!(matches!(s.tests[0].steps[2], Step::SleepMs(5)));
        let err = load("stage: 1\nname: x\ntests:\n  - name: t\n    mode: pty\n    steps: [{wait: a, send: b}]\n    expect: {terminal: a}\n").unwrap_err().to_string();
        assert!(err.contains("exactly one"), "{err}");
    }

    #[test]
    fn rejects_bad_schema() {
        let base = "stage: 1\nname: x\ntests:\n  - name: t\n";
        let err = load(&format!("{base}    input: [a]\n    expct: {{stdout: a}}\n")).unwrap_err().to_string();
        assert!(err.contains("test 't'") && err.contains("expct"), "{err}");
        let err = load(&format!("{base}    input: [a]\n    skip_on: [zsh]\n    expect: {{stdout: a}}\n"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("reason"), "{err}");
        let err = load(&format!("{base}    keys: [a]\n    expect: {{terminal: a}}\n")).unwrap_err().to_string();
        assert!(err.contains("pty mode"), "{err}");
        let err = load(&format!("{base}    input: [a]\n    expect: {{}}\n")).unwrap_err().to_string();
        assert!(err.contains("no assertions"), "{err}");
        assert!(load("stage: 1\nname: x\nbogus: 1\ntests: []\n").unwrap_err().to_string().contains("bogus"));
    }
}
