//! Terminal output (the shape `shelltest` and `kafkatest` use) and the `--json` report.
//!
//! The JSON schema is the one the other testers in this repo write, with `target` naming the
//! linker, so the site reads every track with one parser. `failure_kind` tells a linker that
//! crashed apart from a linker that emitted one wrong byte.

use crate::assert::{Failure, FailureKind};
use crate::runner::{Status, TestResult};
use crate::stages::Stage;
use owo_colors::OwoColorize;
use serde::Serialize;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static COLOR: AtomicBool = AtomicBool::new(true);

/// Turn ANSI colouring on or off for the whole process.
pub fn set_color(enabled: bool) {
    COLOR.store(enabled, Ordering::Relaxed);
}

#[derive(Clone, Copy)]
enum Tint {
    Green,
    Red,
    Yellow,
    Cyan,
    Bold,
    Dim,
}

fn c(s: &str, t: Tint) -> String {
    if !COLOR.load(Ordering::Relaxed) {
        return s.to_string();
    }
    match t {
        Tint::Green => s.green().to_string(),
        Tint::Red => s.red().to_string(),
        Tint::Yellow => s.yellow().to_string(),
        Tint::Cyan => s.cyan().to_string(),
        Tint::Bold => s.bold().to_string(),
        Tint::Dim => s.dimmed().to_string(),
    }
}

/// Writes the human-readable run log.
pub struct Reporter {
    /// True when the run is a suite self-check against the reference linker.
    pub validate: bool,
    /// Show detail for passing tests too.
    pub verbose: bool,
}

impl Reporter {
    fn out(&self, s: &str) {
        let mut o = std::io::stdout().lock();
        let _ = writeln!(o, "{s}");
        let _ = o.flush();
    }

    /// Print the header of a stage.
    pub fn stage_header(&self, st: &Stage) {
        self.out(&format!(
            "\n{} {}",
            c(&format!("Stage {:02}", st.number), Tint::Bold),
            c(st.name, Tint::Cyan)
        ));
    }

    /// Print one test's result, with the failure block when it failed.
    pub fn test_result(&self, r: &TestResult) {
        let time = c(&format!("({} ms)", r.duration_ms), Tint::Dim);
        let notes = |this: &Self| {
            if r.notes.is_empty() {
                return;
            }
            this.section("info");
            for n in &r.notes {
                this.row(n);
            }
        };
        match r.status {
            Status::Pass => {
                self.out(&format!("  {} {} {time}", c("✔", Tint::Green), r.name));
                notes(self);
            }
            Status::Skip => {
                let why = r.skip_reason.as_deref().unwrap_or("no reason given");
                self.out(&format!(
                    "  {} {} {}",
                    c("-", Tint::Yellow),
                    r.name,
                    c(&format!("skipped: {why}"), Tint::Yellow)
                ));
            }
            Status::Fail => {
                let tag = if self.validate { "SUITE BUG" } else { "FAIL" };
                let kind = r
                    .failure
                    .as_ref()
                    .map(|f| f.kind)
                    .unwrap_or(FailureKind::Assertion);
                let tag = if kind == FailureKind::Assertion {
                    tag.to_string()
                } else {
                    format!("{tag} [{}]", kind.label())
                };
                self.out(&format!(
                    "  {} {} {} {time}",
                    c("✘", Tint::Red),
                    r.name,
                    c(&tag, Tint::Red)
                ));
                if let Some(f) = &r.failure {
                    for m in &f.messages {
                        self.out(&format!("      {}", c(m, Tint::Red)));
                    }
                    self.failure_block(f);
                }
                notes(self);
            }
        }
    }

    fn section(&self, title: &str) {
        self.out(&format!(
            "    {} {}",
            c("┌", Tint::Dim),
            c(title, Tint::Bold)
        ));
    }

    fn row(&self, text: &str) {
        for line in text.split('\n') {
            self.out(&format!("    {} {line}", c("│", Tint::Dim)));
        }
    }

    fn failure_block(&self, f: &Failure) {
        if !f.notes.is_empty() {
            self.section("context");
            for n in &f.notes {
                self.row(n);
            }
        }
        if !f.expected.is_empty() {
            self.section("expected");
            for (k, v) in &f.expected {
                self.row(&format!("{k}: {v}"));
            }
        }
        if !f.actual.is_empty() {
            self.section("actual");
            for (k, v) in &f.actual {
                self.row(&format!("{k}: {v}"));
            }
        }
        for (title, body) in &f.blocks {
            self.section(title);
            self.row(body);
        }
    }

    /// Print the per-stage summary line.
    pub fn stage_summary(&self, st: &Stage, results: &[TestResult]) {
        let (passed, failed, skipped) = counts(results);
        let ran = passed + failed;
        let summary = format!("{passed}/{ran} passed");
        let tint = if failed == 0 { Tint::Green } else { Tint::Red };
        let skip = if skipped > 0 {
            c(&format!("  ({skipped} skipped)"), Tint::Yellow)
        } else {
            String::new()
        };
        self.out(&format!(
            "{}  {:<44} {}{skip}",
            c(&format!("Stage {:02}", st.number), Tint::Bold),
            st.name,
            c(&summary, tint)
        ));
    }

    /// Print the totals; returns true when every selected test passed.
    pub fn total(
        &self,
        all: &[(&Stage, Vec<TestResult>)],
        elapsed: Duration,
        target: &str,
    ) -> bool {
        let flat: Vec<&TestResult> = all.iter().flat_map(|(_, r)| r.iter()).collect();
        let passed = flat.iter().filter(|r| r.status == Status::Pass).count();
        let failed = flat.iter().filter(|r| r.status == Status::Fail).count();
        let skipped = flat.iter().filter(|r| r.status == Status::Skip).count();
        let ran = passed + failed;
        let line = format!(
            "Total: {passed}/{ran} passed, {failed} failed, {skipped} skipped  ({:.1}s)",
            elapsed.as_secs_f64()
        );
        self.out("");
        self.out(&c(&line, if failed == 0 { Tint::Green } else { Tint::Red }));
        if skipped > 0 {
            for (st, results) in all {
                for r in results.iter().filter(|r| r.status == Status::Skip) {
                    self.out(&c(
                        &format!(
                            "  skipped  Stage {:02}  {}  — {}",
                            st.number,
                            r.name,
                            r.skip_reason.as_deref().unwrap_or("no reason given")
                        ),
                        Tint::Yellow,
                    ));
                }
            }
        }
        if failed > 0 && self.validate {
            self.out(&c(
                &format!(
                    "SUITE BUGS: the reference linker '{target}' failed {failed} test(s). \
                     Fix the expectations, not the linker:"
                ),
                Tint::Red,
            ));
            for (st, results) in all {
                for r in results.iter().filter(|r| r.status == Status::Fail) {
                    self.out(&format!("  {}  {}", c(&st.file_name(), Tint::Dim), r.name));
                }
            }
        }
        failed == 0
    }
}

fn counts(results: &[TestResult]) -> (usize, usize, usize) {
    let n = |s: Status| results.iter().filter(|r| r.status == s).count();
    (n(Status::Pass), n(Status::Fail), n(Status::Skip))
}

/// Top-level `--json` document; the same shape the other testers in this repo write.
#[derive(Serialize)]
pub struct JsonReport<'a> {
    /// The linker that was tested.
    pub target: &'a str,
    /// True when the run was a `--validate` self-check.
    pub validate: bool,
    /// Per-stage results.
    pub stages: Vec<JsonStage>,
    /// Total passed.
    pub passed: usize,
    /// Total failed.
    pub failed: usize,
    /// Total skipped.
    pub skipped: usize,
    /// Wall-clock duration of the run.
    pub elapsed_ms: u128,
}

/// One stage in the JSON report.
#[derive(Serialize)]
pub struct JsonStage {
    /// Stage number.
    pub stage: u32,
    /// Stage name.
    pub name: String,
    /// Source file of the stage.
    pub file: String,
    /// Tests that passed.
    pub passed: usize,
    /// Tests that failed.
    pub failed: usize,
    /// Tests that were skipped.
    pub skipped: usize,
    /// The tests themselves.
    pub tests: Vec<JsonTest>,
}

/// One test in the JSON report.
#[derive(Serialize)]
pub struct JsonTest {
    /// Test name.
    pub name: String,
    /// Pass, fail or skip.
    pub status: Status,
    /// True when the test carries the `ext` tag.
    pub ext: bool,
    /// Wall-clock duration.
    pub duration_ms: u128,
    /// One line per failed check.
    pub failures: Vec<String>,
    /// Why the test was skipped.
    pub skip_reason: Option<String>,
    /// Labelled values that were seen (field path → value).
    pub actual: Vec<(String, String)>,
    /// What kind of failure this was, when it failed.
    pub failure_kind: Option<FailureKind>,
    /// Informational lines the test attached with `ctx.note(..)`, whatever the outcome.
    pub notes: Vec<String>,
}

/// Write the `--json` report.
pub fn write_json(
    path: &Path,
    target: &str,
    validate: bool,
    all: &[(&Stage, Vec<TestResult>)],
    elapsed: Duration,
) -> anyhow::Result<()> {
    let stages: Vec<JsonStage> = all
        .iter()
        .map(|(st, results)| {
            let (passed, failed, skipped) = counts(results);
            JsonStage {
                stage: st.number,
                name: st.name.to_string(),
                file: st.file_name(),
                passed,
                failed,
                skipped,
                tests: results
                    .iter()
                    .map(|r| JsonTest {
                        name: r.name.clone(),
                        status: r.status,
                        ext: r.ext,
                        duration_ms: r.duration_ms,
                        failures: r.messages(),
                        skip_reason: r.skip_reason.clone(),
                        actual: r
                            .failure
                            .as_ref()
                            .map(|f| f.actual.clone())
                            .unwrap_or_default(),
                        failure_kind: r.failure.as_ref().map(|f| f.kind),
                        notes: r.notes.clone(),
                    })
                    .collect(),
            }
        })
        .collect();
    let report = JsonReport {
        target,
        validate,
        passed: stages.iter().map(|s| s.passed).sum(),
        failed: stages.iter().map(|s| s.failed).sum(),
        skipped: stages.iter().map(|s| s.skipped).sum(),
        stages,
        elapsed_ms: elapsed.as_millis(),
    };
    std::fs::write(path, serde_json::to_string_pretty(&report)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert::Failure;

    fn stage() -> Stage {
        Stage {
            number: 3,
            slug: "identify_the_input",
            name: "Identify the input",
            ext: false,
            hints: &["a", "b"],
            examples: crate::examples::none,
            tests: Vec::new(),
        }
    }

    #[test]
    fn json_uses_target_and_counts_correctly() {
        let st = stage();
        let results = vec![
            TestResult {
                name: "ok".into(),
                status: Status::Pass,
                ext: false,
                duration_ms: 1,
                failure: None,
                skip_reason: None,
                notes: vec!["linked in 4 ms".into()],
            },
            TestResult {
                name: "bad".into(),
                status: Status::Fail,
                ext: true,
                duration_ms: 2,
                failure: Some(Failure::new(FailureKind::LinkerCrash, "boom")),
                skip_reason: None,
                notes: Vec::new(),
            },
        ];
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("r.json");
        write_json(
            &p,
            "gnu_ld",
            true,
            &[(&st, results)],
            Duration::from_secs(2),
        )
        .expect("write");
        let text = std::fs::read_to_string(&p).expect("read");
        let v: serde_json::Value = serde_json::from_str(&text).expect("parse");
        assert_eq!(v["target"], "gnu_ld");
        assert_eq!(v["validate"], true);
        assert_eq!(v["passed"], 1);
        assert_eq!(v["failed"], 1);
        assert_eq!(
            v["stages"][0]["file"],
            "src/stages/s03_identify_the_input.rs"
        );
        assert_eq!(v["stages"][0]["tests"][1]["status"], "fail");
        assert_eq!(v["stages"][0]["tests"][1]["failure_kind"], "linker_crash");
        assert_eq!(v["stages"][0]["tests"][1]["failures"][0], "boom");
        assert!(v.get("shell").is_none(), "linktest reports `target`");
        assert_eq!(v["stages"][0]["tests"][0]["notes"][0], "linked in 4 ms");
    }

    #[test]
    fn counts_split_pass_fail_skip() {
        let r = |s: Status| TestResult {
            name: "t".into(),
            status: s,
            ext: false,
            duration_ms: 0,
            failure: None,
            skip_reason: None,
            notes: Vec::new(),
        };
        assert_eq!(
            counts(&[
                r(Status::Pass),
                r(Status::Fail),
                r(Status::Skip),
                r(Status::Pass)
            ]),
            (2, 1, 1)
        );
    }
}
