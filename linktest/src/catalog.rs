//! `--list` output and `catalog.json`, the file the site consumes.

use crate::examples::{render, Example};
use crate::stages::{sections, Stage};
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

/// How many stages the plan has, implemented or not.
///
/// Counted from the section table rather than written down, so adding a stage to the plan
/// cannot leave this behind — which it did, silently, until the day it was noticed.
pub fn planned_stages() -> u32 {
    crate::stages::sections()
        .iter()
        .map(|s| s.stages.len() as u32)
        .sum()
}

/// A section entry of the catalog.
#[derive(Serialize)]
pub struct CatalogSection {
    /// Short id (`a`..`f`).
    pub id: String,
    /// Section title.
    pub title: String,
    /// Every stage number in the section, implemented or not.
    pub stages: Vec<u32>,
}

/// A test entry of the catalog.
#[derive(Serialize)]
pub struct CatalogTest {
    /// Test name.
    pub name: String,
    /// Present and true when the test is tagged `ext`.
    #[serde(rename = "ext", skip_serializing_if = "is_false")]
    pub ext: bool,
    /// Tags other than `ext`.
    #[serde(rename = "tags", skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Linkers this test is skipped for.
    #[serde(rename = "skipOn", skip_serializing_if = "Vec::is_empty")]
    pub skip_on: Vec<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A stage entry of the catalog.
#[derive(Serialize)]
pub struct CatalogStage {
    /// Stage number.
    pub number: u32,
    /// File-name slug.
    pub slug: String,
    /// Stage title.
    pub name: String,
    /// True when the whole stage is beyond the core track.
    pub ext: bool,
    /// Source file.
    pub file: String,
    /// Implementation hints.
    pub hints: Vec<String>,
    /// The stage's tests.
    pub tests: Vec<CatalogTest>,
    /// Worked examples: annotated input bytes, the command line, and what the output must do.
    pub examples: Vec<Example>,
}

/// The whole catalog.
#[derive(Serialize)]
pub struct Catalog {
    /// Always `link`.
    pub track: String,
    /// ISO-8601 timestamp of generation.
    #[serde(rename = "generatedAt")]
    pub generated_at: String,
    /// The plan's sections.
    pub sections: Vec<CatalogSection>,
    /// Every implemented stage.
    pub stages: Vec<CatalogStage>,
}

/// Build the catalog from the stage registry.
///
/// An example whose builder fails is a bug in the suite; rather than hiding it, the catalog
/// carries the error where the bytes would have been, and `cargo test` fails on it.
pub fn build(stages: &[Stage], generated_at: String) -> Catalog {
    Catalog {
        track: "link".to_string(),
        generated_at,
        sections: sections()
            .iter()
            .map(|s| CatalogSection {
                id: s.id.to_string(),
                title: s.title.to_string(),
                stages: s.stages.to_vec(),
            })
            .collect(),
        stages: stages
            .iter()
            .map(|s| CatalogStage {
                number: s.number,
                slug: s.slug.to_string(),
                name: s.name.to_string(),
                ext: s.ext,
                file: s.file_name(),
                hints: s.hints.iter().map(|h| h.to_string()).collect(),
                tests: s
                    .tests
                    .iter()
                    .map(|t| CatalogTest {
                        name: t.name.to_string(),
                        ext: t.is_ext(),
                        tags: t
                            .tags
                            .iter()
                            .filter(|x| **x != "ext")
                            .map(|x| x.to_string())
                            .collect(),
                        skip_on: t.skip_on.iter().map(|(l, _)| l.to_string()).collect(),
                    })
                    .collect(),
                examples: (s.examples)()
                    .iter()
                    .map(|spec| {
                        render(spec).unwrap_or_else(|e| Example {
                            title: spec.title.to_string(),
                            kind: "error".to_string(),
                            command: spec.command.to_string(),
                            request: format!("THE SUITE COULD NOT BUILD THIS EXAMPLE: {e}"),
                            request_hex: String::new(),
                            response: spec.response.to_string(),
                            response_hex: String::new(),
                            note: spec.note.map(str::to_string),
                            request_fields: Vec::new(),
                            response_fields: Vec::new(),
                            stdout: None,
                            exit_status: None,
                        })
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// Serialize the catalog exactly as `catalog.json` is committed.
pub fn to_json(catalog: &Catalog) -> Result<String> {
    let mut s = serde_json::to_string_pretty(catalog)?;
    s.push('\n');
    Ok(s)
}

/// UTC timestamp, seconds precision, without pulling in a date library.
pub fn now_iso8601() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    iso8601(secs)
}

fn iso8601(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's `civil_from_days`, days since 1970-01-01 → (year, month, day).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Tickbox state per stage, read from `PLAN.md`.
pub fn plan_ticks(plan: &Path) -> BTreeMap<u32, bool> {
    let mut ticks = BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(plan) else {
        return ticks;
    };
    let Ok(re) = regex::Regex::new(r"^- \[([ xX])\] \*\*Stage (\d+)") else {
        return ticks;
    };
    for line in text.lines() {
        if let Some(caps) = re.captures(line) {
            if let Ok(n) = caps[2].parse::<u32>() {
                ticks.insert(n, &caps[1] != " ");
            }
        }
    }
    ticks
}

/// Print the `--list` table.
pub fn list(stages: &[Stage], plan: &Path) {
    let ticks = plan_ticks(plan);
    for st in stages {
        let ext = st.tests.iter().filter(|t| t.is_ext()).count();
        let tick = match ticks.get(&st.number) {
            Some(true) => "[x]",
            Some(false) => "[ ]",
            None => "[?]",
        };
        let ext_note = if ext > 0 {
            format!(", {ext} ext")
        } else {
            String::new()
        };
        println!(
            "{tick} Stage {:02}  {:<46} {:<46} {} tests{ext_note}, {} examples",
            st.number,
            st.name,
            st.file_name(),
            st.tests.len(),
            (st.examples)().len()
        );
    }
    let total: usize = stages.iter().map(|s| s.tests.len()).sum();
    let planned = planned_stages() as usize - stages.len();
    println!(
        "\n{} stages implemented, {total} tests ({planned} stages still planned)",
        stages.len()
    );
}

/// Write `catalog.json`.
pub fn write(path: &Path, stages: &[Stage]) -> Result<()> {
    let catalog = build(stages, now_iso8601());
    std::fs::write(path, to_json(&catalog)?)
        .with_context(|| format!("cannot write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso8601_formats_known_instants() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn catalog_matches_the_documented_schema() {
        let stages = crate::stages::all();
        let cat = build(&stages, "2026-01-01T00:00:00Z".to_string());
        let v: serde_json::Value =
            serde_json::from_str(&to_json(&cat).expect("json")).expect("parse");
        assert_eq!(v["track"], "link");
        assert_eq!(v["generatedAt"], "2026-01-01T00:00:00Z");
        assert_eq!(
            v["sections"].as_array().map(Vec::len),
            Some(crate::stages::sections().len())
        );
        let first = &v["stages"][0];
        assert_eq!(first["number"], 1);
        assert!(first["file"]
            .as_str()
            .unwrap_or_default()
            .starts_with("src/stages/s01_"));
        assert!(first["hints"].as_array().map(Vec::len).unwrap_or(0) >= 2);
        assert!(first["tests"][0]["name"].is_string());
        assert!(first["examples"][0]["request_hex"].is_string());
    }

    #[test]
    fn no_example_failed_to_build() {
        let stages = crate::stages::all();
        let cat = build(&stages, "pinned".to_string());
        for s in &cat.stages {
            for e in &s.examples {
                assert!(
                    !e.request.contains("THE SUITE COULD NOT BUILD"),
                    "stage {}: {}",
                    s.number,
                    e.request
                );
            }
        }
    }

    #[test]
    fn plan_ticks_reads_the_tickboxes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("PLAN.md");
        std::fs::write(
            &p,
            "- [ ] **Stage 01** — a\n- [x] **Stage 02** — b\nnot a stage line\n",
        )
        .expect("write");
        let ticks = plan_ticks(&p);
        assert_eq!(ticks.get(&1), Some(&false));
        assert_eq!(ticks.get(&2), Some(&true));
        assert_eq!(ticks.len(), 2);
    }
}
