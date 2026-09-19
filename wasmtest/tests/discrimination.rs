//! Does this suite discriminate?
//!
//! A conformance tester that passes everything proves nothing, and one that fails
//! everything proves just as little. What makes a red mark mean something is that the suite
//! goes red *where the program is wrong* and stays green *where it is right* — and the only
//! way to know it does that is to point it at a program whose bugs are known in advance.
//!
//! `examples/broken_runtime.rs` is that program. It parses the command line and checks the
//! eight-byte module header, and then gives up: it never decodes a section, never validates
//! anything, never executes an instruction. So the header stage should be almost entirely
//! green — the header really is checked — and everything downstream of it should be red.
//!
//! The half of this that matters is the green half. Any tester can fail a broken program;
//! this one has to agree with it about the part it gets right, or the failures it reports
//! are noise.

use std::path::PathBuf;
use std::process::Command;

fn wasmtest() -> Command {
    Command::new(env!("CARGO_BIN_EXE_wasmtest"))
}

/// The broken runtime, if `cargo test` built it alongside these tests.
fn broken_runtime() -> Option<PathBuf> {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_wasmtest"));
    let profile = exe.parent()?.to_path_buf();
    let candidate = profile.join("examples/broken_runtime");
    candidate.is_file().then_some(candidate)
}

/// `(passed, total)` from a run's `Total:` line.
fn tally(text: &str) -> (usize, usize) {
    for line in text.lines().rev() {
        if let Some(rest) = line.split("Total: ").nth(1) {
            if let Some(frac) = rest.split_whitespace().next() {
                if let Some((p, t)) = frac.split_once('/') {
                    if let (Ok(p), Ok(t)) = (p.trim().parse(), t.trim().parse()) {
                        return (p, t);
                    }
                }
            }
        }
    }
    panic!("no `Total:` line in:\n{text}");
}

fn run(args: &[&str]) -> (String, Option<i32>) {
    let out = wasmtest()
        .args(args)
        .arg("--no-color")
        .output()
        .expect("wasmtest must run");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        out.status.code(),
    )
}

/// What the broken runtime should do at a stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Every test green: the suite must not invent failures here.
    AllPass,
    /// Every test red: nothing about this stage is implemented.
    AllFail,
    /// Some of each — the interesting one, and the reason this file exists.
    Mixed,
}

fn verdict(passed: usize, total: usize) -> Verdict {
    assert!(total > 0, "a stage with no tests cannot discriminate");
    if passed == total {
        Verdict::AllPass
    } else if passed == 0 {
        Verdict::AllFail
    } else {
        Verdict::Mixed
    }
}

#[test]
fn the_broken_runtime_goes_red_where_it_is_wrong_and_green_where_it_is_not() {
    let Some(rt) = broken_runtime() else {
        eprintln!("skipping: build with `cargo test --release` so the example is built");
        return;
    };
    let rt = rt.to_string_lossy().to_string();

    // Stage 01 is the header, which the broken runtime genuinely checks; every other stage
    // here needs something it never does. The expectations come from the bugs that file
    // documents about itself, not from what a run happened to produce.
    let expected: &[(&str, Verdict, &str)] = &[
        (
            "1",
            Verdict::Mixed,
            "the header is checked, but nothing runs",
        ),
        ("2", Verdict::AllFail, "LEB128 decoding never happens"),
        ("3", Verdict::AllFail, "sections are never read"),
        ("7", Verdict::AllFail, "validation never happens"),
        ("13", Verdict::AllFail, "no instruction is executed"),
        ("21", Verdict::AllFail, "no control flow is executed"),
        ("40", Verdict::AllFail, "WASI is not implemented"),
    ];

    let mut wrong = Vec::new();
    for (stage, want, why) in expected {
        let (text, code) = run(&["--runtime", &rt, "--stage", stage]);
        let (passed, total) = tally(&text);
        let got = verdict(passed, total);
        if got != *want {
            wrong.push(format!(
                "stage {stage}: expected {want:?} ({why}), got {got:?} — {passed}/{total} passed"
            ));
        }
        // A stage with any failure must exit non-zero, or a CI run would go green on a red
        // suite, which is the one failure mode that makes all of this worthless.
        let expect_failure = got != Verdict::AllPass;
        if expect_failure && code == Some(0) {
            wrong.push(format!(
                "stage {stage}: {passed}/{total} passed but exit code was 0"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "the suite did not discriminate:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn the_stage_it_gets_right_is_green_test_by_test() {
    let Some(rt) = broken_runtime() else {
        eprintln!("skipping: build with `cargo test --release` so the example is built");
        return;
    };
    let rt = rt.to_string_lossy().to_string();

    // The broken runtime refuses malformed headers correctly, so every test that only asks
    // "is this module refused?" must be green. If any of these went red, the suite would be
    // reporting a bug the program does not have.
    let (text, code) = run(&["--runtime", &rt, "--stage", "1", "--only", "refused"]);
    let (passed, total) = tally(&text);
    assert_eq!(
        passed, total,
        "every 'is it refused?' test must pass against a runtime that really does refuse:\n{text}"
    );
    assert_eq!(code, Some(0), "an all-green run must exit 0:\n{text}");
    assert!(total >= 5, "expected several refusal tests, found {total}");
}

#[test]
fn a_green_run_and_a_red_run_are_told_apart_by_the_exit_code() {
    let Some(rt) = broken_runtime() else {
        eprintln!("skipping: build with `cargo test --release` so the example is built");
        return;
    };
    let rt = rt.to_string_lossy().to_string();
    let (_, red) = run(&["--runtime", &rt, "--stage", "2"]);
    assert_eq!(red, Some(1), "a failing stage must exit 1");
    let (_, green) = run(&["--runtime", &rt, "--stage", "1", "--only", "refused"]);
    assert_eq!(green, Some(0), "a passing selection must exit 0");
}
