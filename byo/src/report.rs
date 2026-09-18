//! The testers' `--json` document.
//!
//! `shelltest` and `kafkatest` emit the same shape; the only difference is the name of the
//! top-level field naming the program under test (`shell` vs `target`), so it is accepted
//! under either spelling here.

use serde::{Deserialize, Serialize};

/// A whole tester run.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Report {
    /// The shell/broker that was tested.
    #[serde(alias = "shell")]
    pub target: String,
    /// True when the run was a `--validate` self-check of the suite.
    #[serde(default)]
    pub validate: bool,
    /// Per-stage results.
    #[serde(default)]
    pub stages: Vec<Stage>,
    /// Total tests passed.
    #[serde(default)]
    pub passed: i64,
    /// Total tests failed.
    #[serde(default)]
    pub failed: i64,
    /// Total tests skipped.
    #[serde(default)]
    pub skipped: i64,
    /// Wall-clock duration of the run in milliseconds.
    #[serde(default)]
    pub elapsed_ms: i64,
}

/// One stage of a run.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Stage {
    /// Stage number, 1-based.
    pub stage: u32,
    /// Human-readable stage name.
    #[serde(default)]
    pub name: String,
    /// The stage's source file.
    #[serde(default)]
    pub file: String,
    /// Tests passed in this stage.
    #[serde(default)]
    pub passed: i64,
    /// Tests failed in this stage.
    #[serde(default)]
    pub failed: i64,
    /// Tests skipped in this stage.
    #[serde(default)]
    pub skipped: i64,
    /// The tests themselves.
    #[serde(default)]
    pub tests: Vec<Test>,
}

/// One test of a stage.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Test {
    /// Test name.
    pub name: String,
    /// `pass`, `fail` or `skip`.
    pub status: String,
    /// True when the test goes beyond the core track.
    #[serde(default)]
    pub ext: bool,
    /// Wall-clock duration in milliseconds.
    #[serde(default)]
    pub duration_ms: i64,
    /// One line per failed check.
    #[serde(default)]
    pub failures: Vec<String>,
    /// Why the test was skipped, when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    /// Labelled values that were observed.
    #[serde(default)]
    pub actual: Vec<(String, String)>,
    /// `kafkatest` only: what kind of failure this was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<String>,
}

/// Read and parse a report written by either tester.
pub fn read(path: &std::path::Path) -> anyhow::Result<Report> {
    use anyhow::Context;
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read the tester's JSON report at {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| {
        format!(
            "the tester's JSON report at {} is not valid",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_shelltest_shape() {
        let r: Report = serde_json::from_str(
            r#"{"shell":"bash","validate":false,"stages":[{"stage":1,"name":"Prompt","file":"01.yaml",
                "passed":1,"failed":0,"skipped":0,"tests":[{"name":"t","status":"pass","ext":false,
                "duration_ms":3,"failures":[],"skip_reason":null,"actual":[["stdout","$ "]]}]}],
                "passed":1,"failed":0,"skipped":0,"elapsed_ms":12}"#,
        )
        .unwrap();
        assert_eq!(r.target, "bash");
        assert_eq!(r.stages[0].tests[0].actual[0].0, "stdout");
    }

    #[test]
    fn accepts_kafkatest_shape() {
        let r: Report = serde_json::from_str(
            r#"{"target":"broken","validate":false,"stages":[],"passed":0,"failed":2,"skipped":0,
                "elapsed_ms":5}"#,
        )
        .unwrap();
        assert_eq!(r.target, "broken");
        assert_eq!(r.failed, 2);
    }
}
