//! `catalog.json` is committed and consumed by the site; this proves it is not stale.
//!
//! Regenerate it with `tlstest --list --json catalog.json` after adding a stage.

use tlstest::catalog;
use tlstest::examples::CapturedFile;
use tlstest::stages;

fn manifest(relative: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn committed() -> serde_json::Value {
    let path = manifest("catalog.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e}\nregenerate it with `tlstest --list --json catalog.json`",
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
    // generatedAt is a timestamp; everything else must match byte for byte.
    committed["generatedAt"] = serde_json::Value::String("pinned".to_string());
    fresh["generatedAt"] = serde_json::Value::String("pinned".to_string());
    assert_eq!(
        committed, fresh,
        "catalog.json is stale — regenerate it with `tlstest --list --json catalog.json`"
    );
}

#[test]
fn catalog_has_the_shape_the_site_expects() {
    let cat = committed();
    assert_eq!(cat["track"], "tls");
    assert!(cat["generatedAt"].is_string());

    let sections = cat["sections"].as_array().expect("sections array");
    assert_eq!(sections.len(), 7);
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
        (1..=45).collect::<Vec<u64>>(),
        "the sections must cover all 45 stages"
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
            "stage {n} must carry 2-4 hints, has {}",
            hints.len()
        );
        let tests = stage["tests"].as_array().expect("tests");
        assert!(!tests.is_empty(), "stage {n} has no tests");
        for t in tests {
            assert!(t["name"].is_string(), "a test of stage {n} has no name");
        }
        let examples = stage["examples"].as_array().expect("examples");
        assert!(
            (1..=3).contains(&examples.len()),
            "stage {n} must carry 1-3 examples, has {}",
            examples.len()
        );
        for e in examples {
            assert!(
                e["title"].is_string(),
                "an example of stage {n} has no title"
            );
            assert!(
                !e["request"].as_str().unwrap_or("").is_empty(),
                "an example of stage {n} has no request summary"
            );
            assert!(
                !e["response"].as_str().unwrap_or("").is_empty(),
                "an example of stage {n} has no response summary"
            );
            assert!(
                ["wire", "text", "closed", "silence"].contains(&e["kind"].as_str().unwrap_or("")),
                "an example of stage {n} has an unknown kind"
            );
        }
    }
}

#[test]
fn the_suite_is_the_size_the_plan_promises() {
    let cat = committed();
    let stages = cat["stages"].as_array().expect("stages");
    assert_eq!(stages.len(), 45);
    let tests: usize = stages
        .iter()
        .map(|s| s["tests"].as_array().map(Vec::len).unwrap_or(0))
        .sum();
    assert!(
        tests >= 320,
        "the plan promises at least 320 tests, the catalog has {tests}"
    );
    let examples: usize = stages
        .iter()
        .map(|s| s["examples"].as_array().map(Vec::len).unwrap_or(0))
        .sum();
    assert!(examples >= 45, "every stage needs at least one example");
}

#[test]
fn plan_md_lists_every_stage_with_its_hints_and_test_count() {
    let plan = manifest("PLAN.md");
    let ticks = catalog::plan_ticks(&plan);
    assert_eq!(
        ticks.len(),
        45,
        "PLAN.md must carry a tickbox for all 45 stages"
    );
    let text = std::fs::read_to_string(&plan).expect("read PLAN.md");
    let cat = committed();
    for stage in cat["stages"].as_array().expect("stages") {
        let n = stage["number"].as_u64().expect("number");
        assert!(
            ticks.contains_key(&(n as u32)),
            "PLAN.md has no entry for stage {n}"
        );
        let line = text
            .lines()
            .find(|l| l.contains(&format!("**Stage {n:02}**")))
            .unwrap_or_else(|| panic!("PLAN.md has no line for stage {n:02}"));
        assert!(
            !line.contains("(planned)"),
            "stage {n:02} is implemented but PLAN.md says (planned)"
        );
        let file = stage["file"].as_str().unwrap_or("");
        assert!(
            line.contains(file),
            "PLAN.md line for stage {n:02} does not name {file}"
        );
        let count = stage["tests"].as_array().map(Vec::len).unwrap_or(0);
        assert!(
            line.contains(&format!("{count} tests")),
            "PLAN.md line for stage {n:02} does not say '{count} tests': {line}"
        );
        let ext = stage["ext"].as_bool().unwrap_or(false);
        assert_eq!(
            line.contains("**[ext]**"),
            ext,
            "stage {n:02} is marked wrongly in PLAN.md: {line}"
        );
        for hint in stage["hints"].as_array().expect("hints") {
            let hint = hint.as_str().unwrap_or("");
            assert!(
                text.contains(hint),
                "PLAN.md is missing a hint of stage {n:02}: {hint}"
            );
        }
    }
}
