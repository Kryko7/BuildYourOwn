//! `catalog.json` is committed and consumed by the site; this proves it is not stale.
//!
//! Regenerate it with `wasmtest --list --json catalog.json` after adding or editing a stage.

use wasmtest::catalog;
use wasmtest::stages;

fn committed() -> serde_json::Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("catalog.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e}\nregenerate it with `wasmtest --list --json catalog.json`",
            path.display()
        )
    });
    serde_json::from_str(&text).expect("catalog.json must be valid JSON")
}

fn regenerated() -> serde_json::Value {
    let stages = stages::all();
    let cat = catalog::build(&stages, "pinned".to_string());
    serde_json::from_str(&catalog::to_json(&cat).expect("serialize")).expect("parse")
}

#[test]
fn catalog_json_matches_the_stage_registry() {
    let mut committed = committed();
    let mut fresh = regenerated();
    // generatedAt is a timestamp; everything else must match byte for byte.
    committed["generatedAt"] = serde_json::Value::String("pinned".to_string());
    fresh["generatedAt"] = serde_json::Value::String("pinned".to_string());
    assert_eq!(
        committed, fresh,
        "catalog.json is stale — regenerate it with `wasmtest --list --json catalog.json`"
    );
}

#[test]
fn catalog_has_the_shape_the_site_expects() {
    let cat = committed();
    assert_eq!(cat["track"], "wasm");
    assert!(cat["generatedAt"].is_string());

    let sections = cat["sections"].as_array().expect("sections array");
    assert_eq!(sections.len(), stages::sections().len());
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
        (1..=numbers.len() as u64).collect::<Vec<u64>>(),
        "the sections must cover 1..=n once each, implemented or not"
    );

    for stage in cat["stages"].as_array().expect("stages array") {
        let n = stage["number"].as_u64().expect("stage number");
        assert!(stage["slug"].is_string(), "stage {n} has no slug");
        assert!(stage["name"].is_string(), "stage {n} has no name");
        assert!(stage["ext"].is_boolean(), "stage {n} has no ext flag");
        let file = stage["file"].as_str().expect("file");
        assert_eq!(
            file,
            format!(
                "src/stages/s{n:02}_{}.rs",
                stage["slug"].as_str().unwrap_or("")
            )
        );
        let hints = stage["hints"].as_array().expect("hints");
        assert!(
            (2..=4).contains(&hints.len()),
            "stage {n} must carry 2-4 hints"
        );
        let tests = stage["tests"].as_array().expect("tests");
        assert!(!tests.is_empty(), "stage {n} has no tests");
        for t in tests {
            assert!(t["name"].is_string(), "a test of stage {n} has no name");
        }
        let examples = stage["examples"].as_array().expect("examples");
        assert!(
            (1..=3).contains(&examples.len()),
            "stage {n} must carry 1-3 examples"
        );
        for e in examples {
            assert_eq!(e["kind"], "module", "stage {n}: every example is a module");
            let hex = e["request_hex"].as_str().unwrap_or_default();
            assert!(
                hex.starts_with("0061736d"),
                "stage {n}: an example's bytes must start with the magic"
            );
            assert!(
                !e["response"].as_str().unwrap_or_default().trim().is_empty(),
                "stage {n}: an example must say what a correct runtime prints"
            );
            assert!(
                !e["request_fields"]
                    .as_array()
                    .expect("request_fields")
                    .is_empty(),
                "stage {n}: an example's module must be annotated"
            );
        }
    }
}

#[test]
fn every_stage_of_the_plan_has_a_tickbox() {
    let plan = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("PLAN.md");
    let ticks = catalog::plan_ticks(&plan);
    assert_eq!(
        ticks.len(),
        stages::all().len(),
        "PLAN.md must carry one tickbox per implemented stage"
    );
    for s in stages::all() {
        assert!(
            ticks.contains_key(&s.number),
            "PLAN.md has no entry for implemented stage {}",
            s.number
        );
    }
    let text = std::fs::read_to_string(&plan).expect("read PLAN.md");
    for s in stages::all() {
        let needle = format!("**Stage {:02}**", s.number);
        let line = text
            .lines()
            .find(|l| l.contains(&needle))
            .unwrap_or_else(|| panic!("PLAN.md has no line for stage {:02}", s.number));
        assert!(
            line.contains(&s.file_name()),
            "PLAN.md line for stage {:02} must name its source file:\n{line}",
            s.number
        );
        assert!(
            line.contains(&format!("{} tests", s.tests.len())),
            "PLAN.md line for stage {:02} must name its test count ({}):\n{line}",
            s.number,
            s.tests.len()
        );
        assert_eq!(
            line.contains("**[ext]**"),
            s.ext,
            "PLAN.md marks stage {:02} as ext differently from the registry",
            s.number
        );
    }
}

#[test]
fn plan_hints_match_the_registry() {
    let plan = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("PLAN.md");
    let text = std::fs::read_to_string(&plan).expect("read PLAN.md");
    for s in stages::all() {
        for h in s.hints {
            assert!(
                text.contains(h.trim()),
                "PLAN.md is missing a hint of stage {:02}: {h}",
                s.number
            );
        }
    }
}
