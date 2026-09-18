//! End-to-end smoke tests of the binary: selection, exit codes and the red path.
//!
//! The "linker" is the `broken_linker` example, which `cargo test` builds alongside these
//! tests. It links a one-object program correctly enough to produce a runnable binary (so
//! the structural tests of stage 01 are green) and then forgets the relocation addend (so
//! every test that *runs* the program is red), which exercises both outcomes without needing
//! a learner's linker.

use std::path::PathBuf;
use std::process::{Command, Output};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_linktest")
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn run(args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .current_dir(manifest_dir())
        .env("NO_COLOR", "1")
        .output()
        .expect("linktest must be runnable")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// The example binary sits next to the test binary, under `examples/`.
fn broken_linker() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let deps = exe.parent()?;
    let profile = deps.parent()?;
    let candidate = profile.join("examples/broken_linker");
    candidate.is_file().then_some(candidate)
}

#[test]
fn list_shows_stages_and_tickboxes() {
    let out = run(&["--list"]);
    assert!(out.status.success(), "--list must exit 0");
    let text = stdout(&out);
    assert!(text.contains("Stage 01"), "{text}");
    assert!(text.contains("src/stages/s01_link_one_object.rs"), "{text}");
    assert!(
        text.contains("[ ] Stage 01") || text.contains("[x] Stage 01"),
        "--list must show the PLAN.md tickbox:\n{text}"
    );
    assert!(text.contains("stages implemented"), "{text}");
    assert!(text.contains("still planned"), "{text}");
}

#[test]
fn list_json_prints_the_catalog_on_stdout() {
    let out = run(&["--list", "--json"]);
    assert!(out.status.success());
    let v: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("--list --json must print JSON");
    assert_eq!(v["track"], "link");
    assert!(v["stages"].as_array().map(Vec::len).unwrap_or(0) >= 40);
    assert!(v["stages"][0]["examples"][0]["request_hex"].is_string());
}

#[test]
fn usage_errors_exit_with_2() {
    // No stage selection at all.
    let out = run(&["--linker", "gnu_ld"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));

    // An unknown linker name that is not a path either.
    let out = run(&["--linker", "definitely_not_a_linker_xyz", "--stage", "1"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("neither a registered linker"));

    // --validate needs the registered reference linker, not a path.
    let out = run(&["--linker", bin(), "--validate"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn the_reference_linker_passes_the_first_stage() {
    let out = run(&["--linker", "gnu_ld", "--validate", "--stage", "1"]);
    let text = stdout(&out);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stage 1 must be green against GNU ld:\n{text}"
    );
    assert!(
        text.contains("Stage 01  Link one object and run it"),
        "{text}"
    );
    assert!(text.contains("0 failed"), "{text}");
}

#[test]
fn a_forgotten_addend_is_red_with_an_explanation() {
    let Some(linker) = broken_linker() else {
        eprintln!("skipping: the broken_linker example is not built");
        return;
    };
    let out = run(&["--linker", &linker.to_string_lossy(), "--stage", "1"]);
    let text = stdout(&out);
    assert_eq!(
        out.status.code(),
        Some(1),
        "a failing run must exit 1:\n{text}"
    );
    assert!(
        text.contains("program.stdout: expected"),
        "the failure must name the field that was wrong:\n{text}"
    );
    assert!(
        text.contains("linker command"),
        "the failure must show the command line that produced it:\n{text}"
    );
    assert!(
        text.contains("output program headers"),
        "the failure must show what the linker actually laid out:\n{text}"
    );
    assert!(
        text.contains("✔") && text.contains("✘"),
        "the broken linker gets some of stage 1 right and some wrong:\n{text}"
    );
}

#[test]
fn json_reports_use_the_shared_schema() {
    let Some(linker) = broken_linker() else {
        eprintln!("skipping: the broken_linker example is not built");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let report = tmp.path().join("report.json");
    let out = run(&[
        "--linker",
        &linker.to_string_lossy(),
        "--stage",
        "1",
        "--json",
        &report.to_string_lossy(),
    ]);
    assert_eq!(out.status.code(), Some(1));
    let text = std::fs::read_to_string(&report).expect("the report must be written");
    let v: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    assert_eq!(v["target"], "broken_linker");
    assert!(v.get("shell").is_none(), "linktest reports `target`");
    assert_eq!(v["validate"], false);
    assert!(v["failed"].as_u64().unwrap_or(0) >= 1);
    assert!(v["passed"].as_u64().unwrap_or(0) >= 1);
    let stage = &v["stages"][0];
    assert_eq!(stage["stage"], 1);
    assert_eq!(stage["file"], "src/stages/s01_link_one_object.rs");
    let test = &stage["tests"][0];
    for key in [
        "name",
        "status",
        "ext",
        "duration_ms",
        "failures",
        "actual",
        "notes",
        "skip_reason",
        "failure_kind",
    ] {
        assert!(
            test.get(key).is_some(),
            "the report is missing tests[].{key}"
        );
    }
}

#[test]
fn only_and_skip_ext_narrow_the_selection() {
    let out = run(&[
        "--linker",
        "gnu_ld",
        "--stage",
        "1",
        "--only",
        "marked executable",
    ]);
    let text = stdout(&out);
    assert!(
        text.contains("1/1 passed"),
        "--only must narrow to one test:\n{text}"
    );

    // Every test of an ext stage carries the tag, so --skip-ext leaves nothing to run.
    let ext_stage = linktest::stages::all()
        .into_iter()
        .find(|s| s.ext)
        .map(|s| s.number)
        .expect("the plan has ext stages");
    let out = run(&[
        "--linker",
        "gnu_ld",
        "--stage",
        &ext_stage.to_string(),
        "--skip-ext",
    ]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "an empty selection is a usage error"
    );
}

#[test]
fn the_seed_is_reported_so_a_run_can_be_reproduced() {
    let out = run(&["--linker", "gnu_ld", "--stage", "1", "--seed", "1234"]);
    let text = stdout(&out);
    assert!(text.contains("seed 0x4d2"), "{text}");
    assert!(text.contains("gnu_ld"), "{text}");
}
