//! End-to-end smoke tests of the binary: selection, routing, exit codes and the red path.
//!
//! Nothing here starts a reference: the tests that need a program under test point at a
//! two-line shell script, which is enough to prove the harness reports a broken program
//! rather than falling over.

use std::path::PathBuf;
use std::process::{Command, Output};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_disttest")
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
        .expect("disttest must be runnable")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

#[test]
fn list_prints_every_stage_with_its_ladder() {
    let out = run(&["--list"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Stage 01"), "{text}");
    assert!(text.contains("Stage 77"), "{text}");
    assert!(text.contains("primitives"), "{text}");
    assert!(text.contains("algorithms"), "{text}");
    assert!(text.contains("node"), "{text}");
    assert!(text.contains("cluster"), "{text}");
    assert!(text.contains("77 stages"), "{text}");
    // One line per stage, plus the per-ladder summary and the total.
    let stage_lines = text.lines().filter(|l| l.contains("Stage ")).count();
    assert_eq!(stage_lines, 77, "{text}");
}

#[test]
fn list_json_is_the_catalog() {
    let out = run(&["--list", "--json"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(v["track"], "dist");
    assert_eq!(v["stages"].as_array().map(Vec::len), Some(77));
    assert_eq!(v["sections"].as_array().map(Vec::len), Some(12));
}

#[test]
fn a_selection_is_required() {
    let out = run(&["--target", "etcd"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("--stage"), "{}", stderr(&out));
}

#[test]
fn an_unknown_target_names_the_ones_that_exist() {
    let out = run(&["--target", "definitely_not_a_target", "--stage", "1"]);
    assert_eq!(out.status.code(), Some(2));
    let err = stderr(&out);
    assert!(err.contains("etcd"), "{err}");
    assert!(err.contains("my_node"), "{err}");
}

#[test]
fn validate_refuses_a_target_that_is_not_a_reference() {
    let out = run(&["--target", "my_node", "--validate", "--stage", "1"]);
    assert_eq!(out.status.code(), Some(2));
    let err = stderr(&out);
    assert!(err.contains("reference"), "{err}");
    assert!(err.contains("reference_primitives"), "{err}");
}

#[test]
fn validate_announces_which_reference_each_ladder_uses() {
    // Stage 8 is a primitives stage with no external dependency beyond the example binary,
    // which cargo has already built for this test run.
    let out = run(&[
        "--target",
        "reference_primitives",
        "--validate",
        "--stage",
        "8",
    ]);
    let text = stdout(&out);
    assert!(
        text.contains("primitives  validated against reference_primitives"),
        "{text}"
    );
    assert!(
        text.contains("algorithms  validated against reference_algorithms"),
        "{text}"
    );
    assert!(
        text.contains("node        validated against etcd"),
        "{text}"
    );
    assert!(
        text.contains("cluster     validated against etcd"),
        "{text}"
    );
}

#[test]
fn a_ladder_tag_selects_whole_stages() {
    let out = run(&[
        "--target",
        "reference_primitives",
        "--all",
        "--tag",
        "primitives",
        "--only",
        "__no_test_has_this_name__",
    ]);
    // Every stage is selected but no test matches, which is a usage error, not a pass.
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("no tests selected"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_primitives_program_that_says_nothing_fails_rather_than_hanging() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("mute.sh");
    std::fs::write(&script, "#!/bin/sh\ncat > /dev/null\n").expect("write");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let out = run(&[
        "--target",
        &script.to_string_lossy(),
        "--stage",
        "1",
        "--timeout-ms",
        "1500",
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    let text = stdout(&out);
    assert!(text.contains('✘') || text.contains("FAIL"), "{text}");
    assert!(
        text.contains("timeout") || text.contains("no answer") || text.contains("crash"),
        "a mute program must be reported as a timeout or a crash:\n{text}"
    );
}

#[test]
fn a_node_that_never_listens_is_reported_as_a_harness_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("exits.sh");
    std::fs::write(&script, "#!/bin/sh\nexit 0\n").expect("write");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let out = run(&[
        "--target",
        &script.to_string_lossy(),
        "--stage",
        "21",
        "--timeout-ms",
        "3000",
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    let text = stdout(&out);
    assert!(
        text.contains("exited with status 0") || text.contains("cannot start the node"),
        "{text}"
    );
}

#[test]
fn the_json_report_has_the_shared_schema() {
    let dir = tempfile::tempdir().expect("tempdir");
    let report = dir.path().join("report.json");
    let script = dir.path().join("mute.sh");
    std::fs::write(&script, "#!/bin/sh\ncat > /dev/null\n").expect("write");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let out = run(&[
        "--target",
        &script.to_string_lossy(),
        "--stage",
        "1",
        "--timeout-ms",
        "1500",
        "--json",
        &report.to_string_lossy(),
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    let text = std::fs::read_to_string(&report).expect("the report must be written");
    let v: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    assert!(v["target"].is_string());
    assert_eq!(v["validate"], false);
    assert!(v["passed"].is_u64() && v["failed"].is_u64() && v["skipped"].is_u64());
    assert!(v["elapsed_ms"].is_u64());
    let stage = &v["stages"][0];
    assert_eq!(stage["stage"], 1);
    assert!(stage["file"]
        .as_str()
        .unwrap_or_default()
        .starts_with("src/"));
    let test = &stage["tests"][0];
    assert!(test["name"].is_string());
    assert_eq!(test["status"], "fail");
    assert_eq!(test["tags"][0], "primitives");
    assert!(test["failure_kind"].is_string());
    assert!(
        v.get("ladders").is_none(),
        "the ladder lives in each test's tags, not at the top level"
    );
}
