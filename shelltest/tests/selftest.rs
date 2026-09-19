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
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
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
    // Counted from the suite rather than written down, so adding a stage cannot leave this
    // test asserting a number the suite outgrew.
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/stages");
    let mut numbers: Vec<u32> = std::fs::read_dir(&dir)
        .expect("tests/stages is readable")
        .filter_map(|e| {
            let name = e.ok()?.file_name().into_string().ok()?;
            let (num, rest) = name.split_once('_')?;
            rest.ends_with(".yaml").then(|| num.parse().ok())?
        })
        .collect();
    numbers.sort_unstable();
    assert!(
        numbers.len() > 40,
        "expected a full suite, found {numbers:?}"
    );

    let (ok, text) = run(shelltest().arg("--list"));
    assert!(ok, "{text}");
    for n in &numbers {
        assert!(
            text.contains(&format!("Stage {n:02}")),
            "Stage {n:02} missing from --list:\n{text}"
        );
    }
    assert!(
        text.contains(&format!("{} stages", numbers.len())),
        "{text}"
    );
}

#[test]
fn broken_example_shell_fails_visibly() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let broken = root.join("examples/broken_shell.sh");
    let (ok, text) = run(shelltest()
        .arg("--shell")
        .arg(&broken)
        .args(["--until", "5"]));
    assert!(!ok, "the broken shell should fail some tests:\n{text}");
    assert!(
        text.contains("FAIL") && text.contains("diff stdout"),
        "{text}"
    );
    assert!(text.contains("✔"), "some tests should still pass:\n{text}");
}

#[test]
fn json_report_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let json = dir.path().join("report.json");
    let (ok, text) = run(shelltest()
        .args(["--shell", "bash", "--stage", "5", "--json"])
        .arg(&json));
    assert!(ok, "{text}");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&json).unwrap()).unwrap();
    assert_eq!(v["failed"], 0);
    assert_eq!(v["stages"][0]["stage"], 5);
}
