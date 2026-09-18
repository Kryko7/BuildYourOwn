//! `catalog.json` is committed and consumed by the site; this proves it is not stale.
//!
//! Regenerate it with `disttest --list --json catalog.json` after adding a stage, and
//! recapture the examples with `disttest --capture-examples examples/captured.json --target
//! etcd` first if a request or an answer moved.

use disttest::catalog;
use disttest::examples::CapturedFile;
use disttest::stages::{self, Ladder};

fn manifest(relative: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn committed() -> serde_json::Value {
    let path = manifest("catalog.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e}\nregenerate it with `disttest --list --json catalog.json`",
            path.display()
        )
    });
    serde_json::from_str(&text).expect("catalog.json must be valid JSON")
}

fn regenerated() -> serde_json::Value {
    let stages = stages::all();
    let captured = CapturedFile::load_or_empty(&manifest(catalog::CAPTURED_EXAMPLES));
    let cat = catalog::build(&stages, "pinned".to_string(), &captured);
    serde_json::from_str(&catalog::to_json(&cat).expect("serialize")).expect("parse")
}

#[test]
fn catalog_json_matches_the_stage_registry() {
    let mut committed = committed();
    let mut fresh = regenerated();
    // generatedAt is a timestamp; everything else must match field for field.
    committed["generatedAt"] = serde_json::Value::String("pinned".to_string());
    fresh["generatedAt"] = serde_json::Value::String("pinned".to_string());
    assert_eq!(
        committed, fresh,
        "catalog.json is stale — regenerate it with `disttest --list --json catalog.json`"
    );
}

#[test]
fn catalog_has_the_shape_the_site_expects() {
    let cat = committed();
    assert_eq!(cat["track"], "dist");
    assert!(cat["generatedAt"].is_string());

    let sections = cat["sections"].as_array().expect("sections array");
    assert_eq!(sections.len(), 8);
    let mut numbers: Vec<u64> = sections
        .iter()
        .flat_map(|s| {
            s["stages"]
                .as_array()
                .expect("section stages")
                .iter()
                .filter_map(serde_json::Value::as_u64)
        })
        .collect();
    numbers.sort_unstable();
    assert_eq!(
        numbers,
        (1..=55).collect::<Vec<u64>>(),
        "every stage belongs to exactly one section"
    );

    let stages = cat["stages"].as_array().expect("stages array");
    assert_eq!(stages.len(), 55);
    for s in stages {
        let number = s["number"].as_u64().unwrap_or_default();
        let where_ = format!("stage {number}");
        assert!(s["name"].is_string(), "{where_}: no name");
        assert!(s["slug"].is_string(), "{where_}: no slug");
        assert!(s["ext"].is_boolean(), "{where_}: no ext flag");
        let ladder = s["ladder"].as_str().unwrap_or_default();
        assert!(
            Ladder::parse(ladder).is_some(),
            "{where_}: ladder {ladder:?} is not one of the three"
        );
        assert!(
            s["file"]
                .as_str()
                .unwrap_or_default()
                .starts_with("src/stages/s"),
            "{where_}: the file path is wrong"
        );
        let hints = s["hints"].as_array().expect("hints");
        assert!(
            (2..=4).contains(&hints.len()),
            "{where_}: needs 2-4 hints, has {}",
            hints.len()
        );
        let tests = s["tests"].as_array().expect("tests");
        assert!(!tests.is_empty(), "{where_}: no tests");
        for t in tests {
            assert!(t["name"].is_string(), "{where_}: a test with no name");
            let tags: Vec<&str> = t["tags"]
                .as_array()
                .map(|a| a.iter().filter_map(serde_json::Value::as_str).collect())
                .unwrap_or_default();
            assert_eq!(
                tags.first(),
                Some(&ladder),
                "{where_}: every test must carry its ladder as its first tag"
            );
        }
        let examples = s["examples"].as_array().expect("examples");
        assert!(
            (1..=3).contains(&examples.len()),
            "{where_}: needs 1-3 examples, has {}",
            examples.len()
        );
        for e in examples {
            assert!(e["title"].is_string(), "{where_}: an example with no title");
            assert!(
                !e["request"].as_str().unwrap_or_default().trim().is_empty(),
                "{where_}: an example with no request summary"
            );
            assert!(
                !e["response"].as_str().unwrap_or_default().trim().is_empty(),
                "{where_}: an example with no response summary"
            );
            let kind = e["kind"].as_str().unwrap_or_default();
            assert!(
                ["node", "cluster", "primitives", "workload"].contains(&kind),
                "{where_}: example kind {kind:?} is not one the site renders"
            );
        }
    }
}

#[test]
fn every_stage_is_ticked_or_not_in_the_plan() {
    let ticks = catalog::plan_ticks(&manifest("PLAN.md"));
    for s in stages::all() {
        assert!(
            ticks.contains_key(&s.number),
            "PLAN.md has no tickbox for stage {} — `--list` reads those boxes",
            s.number
        );
    }
    assert_eq!(
        ticks.len(),
        55,
        "PLAN.md must carry exactly one tickbox per stage"
    );
}

#[test]
fn the_plan_names_every_stage_file_and_its_ladder() {
    let text = std::fs::read_to_string(manifest("PLAN.md")).expect("PLAN.md must be readable");
    for s in stages::all() {
        assert!(
            text.contains(&s.file_name()),
            "PLAN.md does not name {} for stage {}",
            s.file_name(),
            s.number
        );
        assert!(
            text.contains(s.name),
            "PLAN.md does not name stage {} ({:?})",
            s.number,
            s.name
        );
    }
    for ladder in Ladder::ALL {
        assert!(
            text.contains(ladder.as_str()),
            "PLAN.md never mentions the {} ladder",
            ladder.as_str()
        );
    }
}

#[test]
fn the_captured_examples_cover_every_stage_that_declares_one() {
    let captured = CapturedFile::load_or_empty(&manifest(catalog::CAPTURED_EXAMPLES));
    if captured.stages.is_empty() {
        // A fresh checkout may not have run a capture yet; the catalog test above still
        // holds, because it merges the same empty file.
        return;
    }
    for s in stages::all() {
        let declared = (s.examples)().len();
        let taken = captured.stage(s.number).map(Vec::len).unwrap_or(0);
        assert_eq!(
            declared, taken,
            "stage {} declares {declared} example(s) but the capture holds {taken}; \
             re-run `disttest --capture-examples examples/captured.json --target etcd`",
            s.number
        );
    }
}
