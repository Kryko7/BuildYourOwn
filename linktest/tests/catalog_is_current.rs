//! `catalog.json` is committed and consumed by the site; this proves it is not stale.
//!
//! Regenerate it with `linktest --list --json catalog.json` after adding a stage.

use linktest::catalog;
use linktest::stages;

fn committed() -> serde_json::Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("catalog.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e}\nregenerate it with `linktest --list --json catalog.json`",
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
        "catalog.json is stale — regenerate it with `linktest --list --json catalog.json`"
    );
}

#[test]
fn catalog_has_the_shape_the_site_expects() {
    let cat = committed();
    assert_eq!(cat["track"], "link");
    assert!(cat["generatedAt"].is_string());

    let sections = cat["sections"].as_array().expect("sections array");
    assert_eq!(sections.len(), 6);
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
        (1..=u64::from(catalog::PLANNED_STAGES)).collect::<Vec<u64>>(),
        "the sections must cover every planned stage, implemented or not"
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
            "stage {n} must carry 1-3 examples, has {}",
            examples.len()
        );
        for e in examples {
            assert!(e["title"].is_string(), "stage {n}: an example has no title");
            assert!(e["command"].is_string(), "stage {n}: no command line");
            assert!(e["request"].is_string() && !e["request"].as_str().unwrap_or("").is_empty());
            assert!(e["response"].is_string() && !e["response"].as_str().unwrap_or("").is_empty());
            if e["kind"] != "text" {
                let hex = e["request_hex"].as_str().unwrap_or_default();
                assert!(!hex.is_empty(), "stage {n}: an example has no input bytes");
                assert!(hex.len().is_multiple_of(2), "stage {n}: odd-length hex");
                assert!(
                    linktest::examples::from_hex(hex).is_some(),
                    "stage {n}: request_hex is not hex"
                );
                let fields = e["request_fields"].as_array().expect("request_fields");
                assert!(
                    !fields.is_empty(),
                    "stage {n}: an example has no annotations"
                );
                let len = hex.len() / 2;
                for f in fields {
                    let at = f["offset"].as_u64().unwrap_or(0) as usize;
                    let n_bytes = f["length"].as_u64().unwrap_or(0) as usize;
                    assert!(
                        at + n_bytes <= len,
                        "stage {n}: annotation {} runs past the end of the bytes",
                        f["field"]
                    );
                }
            }
        }
    }
}

#[test]
fn plan_md_lists_every_stage() {
    let plan = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("PLAN.md");
    let ticks = catalog::plan_ticks(&plan);
    assert_eq!(
        ticks.len(),
        catalog::PLANNED_STAGES as usize,
        "PLAN.md must carry a tickbox for every planned stage"
    );
    for s in stages::all() {
        assert!(
            ticks.contains_key(&s.number),
            "PLAN.md has no entry for implemented stage {}",
            s.number
        );
    }
    let text = std::fs::read_to_string(&plan).expect("read PLAN.md");
    let implemented: Vec<u32> = stages::all().iter().map(|s| s.number).collect();
    for n in 1..=catalog::PLANNED_STAGES {
        let line = text
            .lines()
            .find(|l| l.contains(&format!("**Stage {n:02}**")))
            .unwrap_or_else(|| panic!("PLAN.md has no line for stage {n:02}"));
        let planned = line.contains("(planned)");
        assert_eq!(
            planned,
            !implemented.contains(&n),
            "stage {n:02} is marked wrongly in PLAN.md (line: {line})"
        );
    }
}

#[test]
fn plan_md_names_the_right_file_and_test_count_for_every_stage() {
    let plan = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("PLAN.md");
    let text = std::fs::read_to_string(&plan).expect("read PLAN.md");
    for s in stages::all() {
        let needle = format!("(`{}`, {} tests)", s.file_name(), s.tests.len());
        assert!(
            text.contains(&needle),
            "PLAN.md must say `{needle}` for stage {:02}",
            s.number
        );
    }
}
