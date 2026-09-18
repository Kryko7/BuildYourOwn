//! Runs the shipped YAML suite against bash and asserts every non-ext test passes.

use std::path::Path;
use std::process::Command;

fn shelltest() -> Command {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_shelltest"));
    cmd.arg("--tests-dir").arg(root.join("tests/stages"));
    cmd.arg("--shells-file").arg(root.join("shells.yaml"));
    cmd.arg("--no-color");
    cmd
}

fn run(cmd: &mut Command) -> (bool, String) {
    let out = cmd.output().expect("failed to run shelltest binary");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

#[test]
fn full_suite_passes_on_bash_without_ext_stages() {
    let (ok, text) = run(shelltest().args(["--shell", "bash", "--all", "--skip-ext"]));
    assert!(ok, "non-ext suite failed on bash:\n{text}");
    assert!(text.contains("0 failed"), "{text}");
}

#[test]
fn ext_stages_pass_on_bash_too() {
    let (ok, text) = run(shelltest().args(["--shell", "bash", "--all", "--tag", "ext"]));
    assert!(ok, "ext stages failed on bash:\n{text}");
}

#[test]
fn list_shows_every_stage() {
    let (ok, text) = run(shelltest().arg("--list"));
    assert!(ok, "{text}");
    assert!(text.contains("Stage 01") && text.contains("Stage 57"), "{text}");
    assert!(text.contains("57 stages"), "{text}");
}

#[test]
fn broken_example_shell_fails_visibly() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let broken = root.join("examples/broken_shell.sh");
    let (ok, text) = run(shelltest().arg("--shell").arg(&broken).args(["--until", "5"]));
    assert!(!ok, "the broken shell should fail some tests:\n{text}");
    assert!(text.contains("FAIL") && text.contains("diff stdout"), "{text}");
    assert!(text.contains("✔"), "some tests should still pass:\n{text}");
}

#[test]
fn json_report_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let json = dir.path().join("report.json");
    let (ok, text) = run(shelltest().args(["--shell", "bash", "--stage", "5", "--json"]).arg(&json));
    assert!(ok, "{text}");
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&json).unwrap()).unwrap();
    assert_eq!(v["failed"], 0);
    assert_eq!(v["stages"][0]["stage"], 5);
}
