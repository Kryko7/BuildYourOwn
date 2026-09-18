//! End-to-end smoke tests of the binary: selection, exit codes and the red path.
//!
//! The "broker" is the `broken_broker` example, which `cargo test` builds alongside these
//! tests. It accepts connections (so stage 01 is green) and echoes the wrong correlation id
//! (so stage 02 is red), which exercises both outcomes without needing Apache Kafka.

use std::path::PathBuf;
use std::process::{Command, Output};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_kafkatest")
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
        .expect("kafkatest must be runnable")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// The example binary sits next to the test binary, under `examples/`.
fn broken_broker() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let deps = exe.parent()?;
    let profile = deps.parent()?;
    let candidate = profile.join("examples/broken_broker");
    candidate.is_file().then_some(candidate)
}

#[test]
fn list_shows_stages_and_tickboxes() {
    let out = run(&["--list"]);
    assert!(out.status.success(), "--list must exit 0");
    let text = stdout(&out);
    assert!(text.contains("Stage 01"), "{text}");
    assert!(text.contains("src/stages/s01_bind.rs"), "{text}");
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
    assert_eq!(v["track"], "kafka");
    assert!(v["stages"].as_array().map(Vec::len).unwrap_or(0) >= 20);
}

#[test]
fn usage_errors_exit_with_2() {
    // No stage selection at all.
    let out = run(&["--broker", "apache_kafka"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));

    // An unknown broker name that is not a path either.
    let out = run(&["--broker", "definitely_not_a_broker_xyz", "--stage", "1"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("neither a registered broker"));

    // --validate needs a registered broker, not a path.
    let out = run(&["--broker", bin(), "--validate"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn a_broker_that_answers_correctly_enough_passes_stage_one() {
    let Some(broker) = broken_broker() else {
        eprintln!("skipping: the broken_broker example is not built");
        return;
    };
    let out = run(&["--broker", &broker.to_string_lossy(), "--stage", "1"]);
    let text = stdout(&out);
    assert!(
        text.contains("Stage 01  Bind to the broker port") && text.contains("6/6 passed"),
        "stage 1 should be green against a broker that only binds:\n{text}"
    );
    assert_eq!(out.status.code(), Some(0), "all green must exit 0:\n{text}");
}

#[test]
fn a_wrong_correlation_id_is_red_with_a_hex_dump() {
    let Some(broker) = broken_broker() else {
        eprintln!("skipping: the broken_broker example is not built");
        return;
    };
    let out = run(&["--broker", &broker.to_string_lossy(), "--stage", "2"]);
    let text = stdout(&out);
    assert_eq!(
        out.status.code(),
        Some(1),
        "a failing run must exit 1:\n{text}"
    );
    assert!(
        text.contains("response.correlation_id: expected"),
        "the failure must name the decoded field path:\n{text}"
    );
    assert!(
        text.contains("request bytes") && text.contains("response bytes"),
        "the failure must hex-dump both directions:\n{text}"
    );
    assert!(
        text.contains("^^"),
        "the hex dump must mark the differing region:\n{text}"
    );
    assert!(
        text.contains("broker output"),
        "the failure must show the broker's own output:\n{text}"
    );
}

#[test]
fn json_reports_use_the_shared_schema() {
    let Some(broker) = broken_broker() else {
        eprintln!("skipping: the broken_broker example is not built");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let report = tmp.path().join("report.json");
    let out = run(&[
        "--broker",
        &broker.to_string_lossy(),
        "--stage",
        "2",
        "--json",
        &report.to_string_lossy(),
    ]);
    assert_eq!(out.status.code(), Some(1));
    let text = std::fs::read_to_string(&report).expect("the report must be written");
    let v: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    assert_eq!(v["target"], "broken_broker");
    assert!(v.get("shell").is_none(), "kafkatest reports `target`");
    assert_eq!(v["validate"], false);
    assert!(v["failed"].as_u64().unwrap_or(0) >= 1);
    let stage = &v["stages"][0];
    assert_eq!(stage["stage"], 2);
    assert_eq!(stage["file"], "src/stages/s02_correlation_id.rs");
    let test = &stage["tests"][0];
    for key in ["name", "status", "ext", "duration_ms", "failures", "actual"] {
        assert!(
            test.get(key).is_some(),
            "the report is missing tests[].{key}"
        );
    }
}

#[test]
fn only_and_skip_ext_narrow_the_selection() {
    let Some(broker) = broken_broker() else {
        eprintln!("skipping: the broken_broker example is not built");
        return;
    };
    let out = run(&[
        "--broker",
        &broker.to_string_lossy(),
        "--stage",
        "1",
        "--only",
        "ten connections",
    ]);
    let text = stdout(&out);
    assert!(
        text.contains("1/1 passed"),
        "--only must narrow to one test:\n{text}"
    );

    // Stage 09 is entirely ext, so --skip-ext leaves nothing to run.
    let out = run(&[
        "--broker",
        &broker.to_string_lossy(),
        "--stage",
        "9",
        "--skip-ext",
    ]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "an empty selection is a usage error"
    );
}
