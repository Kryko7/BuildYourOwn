//! Console output, unified diffs, and the `--json` report.

use crate::loader::Stage;
use crate::runner::{Detail, Status, TestResult};
use owo_colors::OwoColorize;
use serde::Serialize;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static COLOR: AtomicBool = AtomicBool::new(true);

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

pub struct Reporter {
    pub validate: bool,
    pub verbose: bool,
}

impl Reporter {
    fn out(&self, s: &str) {
        let mut o = std::io::stdout().lock();
        let _ = writeln!(o, "{s}");
        let _ = o.flush();
    }

    pub fn stage_header(&self, st: &Stage) {
        self.out(&format!(
            "\n{} {}",
            c(&format!("Stage {:02}", st.stage), Tint::Bold),
            c(&st.name, Tint::Cyan)
        ));
    }

    pub fn test_result(&self, r: &TestResult) {
        let time = c(&format!("({} ms)", r.duration_ms), Tint::Dim);
        match r.status {
            Status::Pass => {
                self.out(&format!("  {} {} {time}", c("✔", Tint::Green), r.name));
                if self.verbose {
                    if let Some(d) = &r.detail {
                        self.detail_block(d, &[]);
                    }
                }
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
                self.out(&format!(
                    "  {} {} {} {time}",
                    c("✘", Tint::Red),
                    r.name,
                    c(tag, Tint::Red)
                ));
                for f in &r.failures {
                    self.out(&format!("      {}", c(f, Tint::Red)));
                }
                if let Some(d) = &r.detail {
                    self.detail_block(d, &r.failures);
                }
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

    fn value(&self, label: &str, v: &str) {
        let v = &visible(v);
        if v.contains('\n') {
            self.row(&format!("{label}:"));
            for line in v.split('\n') {
                self.row(&format!("    {line}"));
            }
        } else {
            self.row(&format!("{label}: {v:?}"));
        }
    }

    fn detail_block(&self, d: &Detail, _failures: &[String]) {
        let input_title = match d.mode {
            Some(crate::loader::Mode::Pty) => "key steps sent to the terminal",
            _ => "input sent on stdin",
        };
        self.section(input_title);
        self.row(if d.input.is_empty() {
            "(nothing)"
        } else {
            &d.input
        });
        self.section("expected");
        for (label, desc) in &d.expected {
            self.row(&format!("{label}: {desc}"));
        }
        self.section("actual (normalized)");
        for (label, v) in &d.actual {
            self.value(label, v);
        }
        for (label, exp, act) in &d.diffs {
            self.section(&format!("diff {label}  (-expected  +actual)"));
            self.row(&unified_diff(exp, act));
        }
        if let Some(p) = &d.sandbox {
            self.out(&format!(
                "    {}",
                c(&format!("sandbox kept at {}", p.display()), Tint::Yellow)
            ));
        }
    }

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
            "{}  {:<34} {}{skip}",
            c(&format!("Stage {:02}", st.stage), Tint::Bold),
            st.name,
            c(&summary, tint)
        ));
    }

    pub fn total(&self, all: &[(&Stage, Vec<TestResult>)], elapsed: Duration, shell: &str) -> bool {
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
        if failed > 0 && self.validate {
            self.out(&c(&format!("SUITE BUGS: the reference shell '{shell}' failed {failed} test(s). Fix the expectations, not the shell:"), Tint::Red));
            for (st, results) in all {
                for r in results.iter().filter(|r| r.status == Status::Fail) {
                    self.out(&format!("  {}  {}", c(&st.file_name(), Tint::Dim), r.name));
                }
            }
        }
        failed == 0
    }
}

/// Show control characters (bell, backspace, escape) as `^G`-style caret notation.
pub fn visible(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\n' | '\t' => c.to_string(),
            c if (c as u32) < 0x20 => {
                format!("^{}", char::from_u32(c as u32 + 0x40).unwrap_or('?'))
            }
            '\x7f' => "^?".to_string(),
            c => c.to_string(),
        })
        .collect()
}

fn counts(results: &[TestResult]) -> (usize, usize, usize) {
    let n = |s: Status| results.iter().filter(|r| r.status == s).count();
    (n(Status::Pass), n(Status::Fail), n(Status::Skip))
}

pub fn unified_diff(expected: &str, actual: &str) -> String {
    let e = format!("{expected}\n");
    let a = format!("{actual}\n");
    let diff = similar::TextDiff::from_lines(&e, &a);
    let text = diff
        .unified_diff()
        .context_radius(3)
        .header("expected", "actual")
        .to_string();
    text.lines()
        .map(|l| match l.chars().next() {
            Some('-') if !l.starts_with("---") => c(l, Tint::Red),
            Some('+') if !l.starts_with("+++") => c(l, Tint::Green),
            Some('@') => c(l, Tint::Cyan),
            _ => l.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Serialize)]
pub struct JsonReport<'a> {
    pub shell: &'a str,
    pub validate: bool,
    pub stages: Vec<JsonStage>,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub elapsed_ms: u128,
}

#[derive(Serialize)]
pub struct JsonStage {
    pub stage: u32,
    pub name: String,
    pub file: String,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub tests: Vec<JsonTest>,
}

#[derive(Serialize)]
pub struct JsonTest {
    pub name: String,
    pub status: Status,
    pub ext: bool,
    pub duration_ms: u128,
    pub failures: Vec<String>,
    pub skip_reason: Option<String>,
    pub actual: Vec<(String, String)>,
}

pub fn write_json(
    path: &Path,
    shell: &str,
    validate: bool,
    all: &[(&Stage, Vec<TestResult>)],
    elapsed: Duration,
) -> anyhow::Result<()> {
    let stages: Vec<JsonStage> = all
        .iter()
        .map(|(st, results)| {
            let (passed, failed, skipped) = counts(results);
            JsonStage {
                stage: st.stage,
                name: st.name.clone(),
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
                        failures: r.failures.clone(),
                        skip_reason: r.skip_reason.clone(),
                        actual: r
                            .detail
                            .as_ref()
                            .map(|d| d.actual.clone())
                            .unwrap_or_default(),
                    })
                    .collect(),
            }
        })
        .collect();
    let report = JsonReport {
        shell,
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

    #[test]
    fn diff_marks_changed_lines() {
        set_color(false);
        let d = unified_diff("a\nb", "a\nc");
        assert!(d.contains("-b") && d.contains("+c"), "{d}");
        assert!(d.contains("--- expected"));
    }

    #[test]
    fn caret_notation() {
        assert_eq!(visible("a\x07b\x1b[A\n"), "a^Gb^[[A\n");
    }

    #[test]
    fn json_counts() {
        let st = Stage {
            stage: 1,
            name: "x".into(),
            file: "01_x.yaml".into(),
            tests: vec![],
        };
        let r = TestResult {
            name: "t".into(),
            status: Status::Fail,
            ext: false,
            duration_ms: 1,
            skip_reason: None,
            failures: vec!["boom".into()],
            detail: None,
        };
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("r.json");
        write_json(&p, "bash", false, &[(&st, vec![r])], Duration::from_secs(1)).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        assert_eq!(v["failed"], 1);
        assert_eq!(v["stages"][0]["tests"][0]["status"], "fail");
    }
}
