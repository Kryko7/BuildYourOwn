//! End-to-end smoke tests of the binary: selection, exit codes and the red path.
//!
//! The "runtime" is the `broken_runtime` example, which `cargo test` builds alongside these
//! tests. It checks the module header and nothing else, so every "is this module refused?"
//! test is green and everything that asks a module to *run* is red — which exercises both
//! outcomes without needing `wasmtime`.

use std::path::PathBuf;
use std::process::{Command, Output};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_wasmtest")
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
        .expect("wasmtest must be runnable")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// The example binary sits next to the test binary, under `examples/`.
fn broken_runtime() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let deps = exe.parent()?;
    let profile = deps.parent()?;
    let candidate = profile.join("examples/broken_runtime");
    candidate.is_file().then_some(candidate)
}

#[test]
fn list_shows_stages_and_tickboxes() {
    let out = run(&["--list"]);
    assert!(out.status.success(), "--list must exit 0");
    let text = stdout(&out);
    assert!(text.contains("Stage 01"), "{text}");
    assert!(
        text.contains("src/stages/s01_magic_and_version.rs"),
        "{text}"
    );
    assert!(
        text.contains("[ ] Stage 01") || text.contains("[x] Stage 01"),
        "--list must show the PLAN.md tickbox:\n{text}"
    );
    assert!(text.contains("stages implemented"), "{text}");
    assert!(text.contains("examples"), "{text}");
}

#[test]
fn list_json_prints_the_catalog_on_stdout() {
    let out = run(&["--list", "--json"]);
    assert!(out.status.success());
    let v: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("--list --json must print JSON");
    assert_eq!(v["track"], "wasm");
    assert_eq!(v["stages"].as_array().map(Vec::len), Some(45));
}

#[test]
fn usage_errors_exit_with_2() {
    // No stage selection at all.
    let out = run(&["--runtime", "wasmtime"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));

    // An unknown runtime name that is not a path either.
    let out = run(&["--runtime", "definitely_not_a_runtime_xyz", "--stage", "1"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("neither a registered runtime"));

    // --validate needs a registered runtime, not a path.
    let out = run(&["--runtime", bin(), "--validate"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn a_runtime_that_checks_the_header_passes_the_header_tests() {
    let Some(rt) = broken_runtime() else {
        eprintln!("skipping: the broken_runtime example is not built");
        return;
    };
    // Every test that only asks "is this module refused?" is green, because the header
    // really is checked. The one that asks the module to *run* is red, because nothing
    // after the header is ever decoded.
    let out = run(&["--runtime", &rt.to_string_lossy(), "--stage", "1"]);
    let text = stdout(&out);
    assert!(
        text.contains("Stage 01  The magic number and the version") && text.contains("7/8 passed"),
        "the header tests should pass and the execution test should not:\n{text}"
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "one failure must exit 1:\n{text}"
    );

    let out = run(&[
        "--runtime",
        &rt.to_string_lossy(),
        "--stage",
        "1",
        "--only",
        "refused",
    ]);
    let text = stdout(&out);
    assert!(
        text.contains("7/7 passed"),
        "every 'is it refused?' test must be green:\n{text}"
    );
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn a_runtime_that_executes_nothing_is_red_with_the_module_listing() {
    let Some(rt) = broken_runtime() else {
        eprintln!("skipping: the broken_runtime example is not built");
        return;
    };
    let out = run(&["--runtime", &rt.to_string_lossy(), "--stage", "13"]);
    let text = stdout(&out);
    assert_eq!(
        out.status.code(),
        Some(1),
        "a failing run must exit 1:\n{text}"
    );
    assert!(
        text.contains("stdout.lines: expected"),
        "the failure must name what was expected:\n{text}"
    );
    assert!(
        text.contains("command") && text.contains("stderr"),
        "the failure must show the command line and both streams:\n{text}"
    );
    assert!(
        text.contains("header.magic") && text.contains("code["),
        "the failure must show the module's annotated listing:\n{text}"
    );
}

#[test]
fn json_reports_use_the_shared_schema() {
    let Some(rt) = broken_runtime() else {
        eprintln!("skipping: the broken_runtime example is not built");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let report = tmp.path().join("report.json");
    let out = run(&[
        "--runtime",
        &rt.to_string_lossy(),
        "--stage",
        "13",
        "--json",
        &report.to_string_lossy(),
    ]);
    assert_eq!(out.status.code(), Some(1));
    let text = std::fs::read_to_string(&report).expect("the report must be written");
    let v: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    assert_eq!(v["target"], "broken_runtime");
    assert!(v.get("shell").is_none(), "wasmtest reports `target`");
    assert_eq!(v["validate"], false);
    assert!(v["failed"].as_u64().unwrap_or(0) >= 1);
    let stage = &v["stages"][0];
    assert_eq!(stage["stage"], 13);
    assert_eq!(stage["file"], "src/stages/s13_i32_arithmetic.rs");
    let test = &stage["tests"][0];
    for key in [
        "name",
        "status",
        "ext",
        "duration_ms",
        "failures",
        "actual",
        "notes",
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
    let Some(rt) = broken_runtime() else {
        eprintln!("skipping: the broken_runtime example is not built");
        return;
    };
    let out = run(&[
        "--runtime",
        &rt.to_string_lossy(),
        "--stage",
        "1",
        "--only",
        "an empty file is refused",
    ]);
    let text = stdout(&out);
    assert!(
        text.contains("1/1 passed"),
        "--only must narrow to one test:\n{text}"
    );

    // Stage 04 is entirely ext, so --skip-ext leaves nothing to run.
    let out = run(&[
        "--runtime",
        &rt.to_string_lossy(),
        "--stage",
        "4",
        "--skip-ext",
    ]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "an empty selection is a usage error"
    );
}

#[test]
fn the_wasi_tag_selects_the_wasi_section() {
    let Some(rt) = broken_runtime() else {
        eprintln!("skipping: the broken_runtime example is not built");
        return;
    };
    let out = run(&[
        "--runtime",
        &rt.to_string_lossy(),
        "--all",
        "--tag",
        "wasi",
        "--only",
        "reaches stdout",
    ]);
    let text = stdout(&out);
    assert!(
        text.contains("Stage 40"),
        "--tag wasi must reach the WASI stages:\n{text}"
    );
    assert!(
        !text.contains("Stage 13"),
        "--tag wasi must not reach the numeric stages:\n{text}"
    );
}
