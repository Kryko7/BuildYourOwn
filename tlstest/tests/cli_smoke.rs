//! End-to-end smoke tests of the binary: selection, exit codes and the red path.
//!
//! The "server" is the `broken_server` example, which `cargo test` builds alongside these
//! tests. It accepts connections and says nothing until spoken to (so stage 01 is green)
//! and then answers with a ServerHello that has three deliberate mistakes in it (so stage
//! 02 is red) — both outcomes, without needing OpenSSL.

use std::path::PathBuf;
use std::process::{Command, Output};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_tlstest")
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
        .expect("tlstest must be runnable")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// The example binary sits next to the test binary, under `examples/`.
fn broken_server() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let deps = exe.parent()?;
    let profile = deps.parent()?;
    let candidate = profile.join("examples/broken_server");
    candidate.is_file().then_some(candidate)
}

#[test]
fn list_shows_stages_and_tickboxes() {
    let out = run(&["--list"]);
    assert!(out.status.success(), "--list must exit 0");
    let text = stdout(&out);
    assert!(text.contains("Stage 01"), "{text}");
    assert!(text.contains("src/stages/s01_accept.rs"), "{text}");
    assert!(
        text.contains("[ ] Stage 01") || text.contains("[x] Stage 01"),
        "--list must show the PLAN.md tickbox:\n{text}"
    );
    assert!(text.contains("45 stages implemented"), "{text}");
    assert!(text.contains("examples"), "{text}");
}

#[test]
fn list_json_prints_the_catalog_on_stdout() {
    let out = run(&["--list", "--json"]);
    assert!(out.status.success());
    let v: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("--list --json must print JSON");
    assert_eq!(v["track"], "tls");
    assert_eq!(v["stages"].as_array().map(Vec::len), Some(45));
    assert_eq!(v["sections"].as_array().map(Vec::len), Some(7));
}

#[test]
fn usage_errors_exit_with_2() {
    // No stage selection at all.
    let out = run(&["--server", "openssl"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));

    // An unknown server name that is not a path either.
    let out = run(&["--server", "definitely_not_a_server_xyz", "--stage", "1"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("neither a registered server"));

    // --validate needs the reference, not a path.
    let out = run(&["--server", bin(), "--validate"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--validate needs the reference"));
}

#[test]
fn a_server_that_only_accepts_passes_stage_one() {
    let Some(server) = broken_server() else {
        eprintln!("skipping: the broken_server example is not built");
        return;
    };
    let out = run(&["--server", &server.to_string_lossy(), "--stage", "1"]);
    let text = stdout(&out);
    assert!(
        text.contains("Stage 01  Accept a connection and let the client speak first")
            && text.contains("7/7 passed"),
        "stage 1 must be green against a server that only accepts:\n{text}"
    );
    assert_eq!(out.status.code(), Some(0), "all green must exit 0:\n{text}");
}

#[test]
fn a_wrong_server_hello_is_red_with_hex_dumps() {
    let Some(server) = broken_server() else {
        eprintln!("skipping: the broken_server example is not built");
        return;
    };
    let out = run(&["--server", &server.to_string_lossy(), "--stage", "2"]);
    let text = stdout(&out);
    assert_eq!(
        out.status.code(),
        Some(1),
        "a failing run must exit 1:\n{text}"
    );
    assert!(
        text.contains("record.legacy_record_version: expected"),
        "the failure must name the decoded field path:\n{text}"
    );
    assert!(
        text.contains("the server's first record"),
        "the failure must hex-dump the bytes it is talking about:\n{text}"
    );
    assert!(
        text.contains("^^"),
        "the hex dump must mark the bytes holding the actual value:\n{text}"
    );
    assert!(
        text.contains("server output"),
        "the failure must show the server's own output:\n{text}"
    );
}

#[test]
fn a_failure_names_the_transcript_and_the_key_schedule() {
    let Some(server) = broken_server() else {
        eprintln!("skipping: the broken_server example is not built");
        return;
    };
    // Stage 08 completes a handshake, which this server cannot: the failure block should
    // carry the connection trace and the transcript hashes it got as far as.
    let out = run(&["--server", &server.to_string_lossy(), "--stage", "8"]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text.contains("connection trace:"),
        "a handshake failure must show what crossed the connection:\n{text}"
    );
    assert!(
        text.contains("transcript hashes:"),
        "a handshake failure must show the transcript it built:\n{text}"
    );
    assert!(
        text.contains("client_hello") && text.contains("server_hello"),
        "a handshake failure must hex-dump the messages:\n{text}"
    );
}

#[test]
fn json_reports_use_the_shared_schema() {
    let Some(server) = broken_server() else {
        eprintln!("skipping: the broken_server example is not built");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let report = tmp.path().join("report.json");
    let out = run(&[
        "--server",
        &server.to_string_lossy(),
        "--stage",
        "2",
        "--json",
        &report.to_string_lossy(),
    ]);
    assert_eq!(out.status.code(), Some(1));
    let text = std::fs::read_to_string(&report).expect("the report must be written");
    let v: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    assert_eq!(v["target"], "broken_server");
    assert!(v.get("shell").is_none(), "tlstest reports `target`");
    assert_eq!(v["validate"], false);
    assert!(v["failed"].as_u64().unwrap_or(0) >= 1);
    let stage = &v["stages"][0];
    assert_eq!(stage["stage"], 2);
    assert_eq!(stage["file"], "src/stages/s02_record_framing.rs");
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
        "skip_reason",
    ] {
        assert!(
            test.get(key).is_some(),
            "the report is missing tests[].{key}"
        );
    }
}

#[test]
fn only_and_skip_ext_narrow_the_selection() {
    let Some(server) = broken_server() else {
        eprintln!("skipping: the broken_server example is not built");
        return;
    };
    let out = run(&[
        "--server",
        &server.to_string_lossy(),
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

    // Stage 39 is entirely ext, so --skip-ext leaves nothing to run.
    let out = run(&[
        "--server",
        &server.to_string_lossy(),
        "--stage",
        "39",
        "--skip-ext",
    ]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "an empty selection is a usage error"
    );
}

#[test]
fn the_seed_makes_a_run_reproducible() {
    let Some(server) = broken_server() else {
        eprintln!("skipping: the broken_server example is not built");
        return;
    };
    let once = run(&[
        "--server",
        &server.to_string_lossy(),
        "--stage",
        "2",
        "--seed",
        "1234",
    ]);
    let twice = run(&[
        "--server",
        &server.to_string_lossy(),
        "--stage",
        "2",
        "--seed",
        "1234",
    ]);
    let strip = |out: &Output| {
        stdout(out)
            .lines()
            .filter(|l| {
                !l.contains("ms)")
                    && !l.contains("seed")
                    && !l.contains("Total")
                    && !l.contains("listening on")
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(
        strip(&once),
        strip(&twice),
        "the same seed must produce the same bytes and the same failures"
    );
}
