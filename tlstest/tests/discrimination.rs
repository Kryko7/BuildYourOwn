//! Does this suite discriminate?
//!
//! `examples/broken_server.rs` is a TLS server whose bugs are known in advance. It accepts
//! connections and waits for the client to speak first — both correct, and both things a
//! server has to get right before anything else can be judged. Then it gets the very first
//! field of its answer wrong: the record it writes carries `legacy_record_version` 0x0301,
//! and the ServerHello inside claims `legacy_version` 0x0304 and echoes the wrong session
//! id.
//!
//! The profile that produces is the point of this file. Stage 01 is entirely green. Stage 02
//! is 6/7 — one wrong field in an otherwise well-formed record, caught on its own rather
//! than taking the whole stage down with it. Everything from the key schedule onwards is
//! red, because a handshake that never gets past ServerHello cannot reach it.
//!
//! The green rows are the ones that matter. A suite that reported stage 01 red here would be
//! accusing this server of a bug it does not have, and every other failure it reported would
//! deserve the same suspicion.

use std::path::PathBuf;
use std::process::Command;

fn tlstest() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tlstest"))
}

/// The broken server, if `cargo test` built it alongside these tests.
fn broken_server() -> Option<PathBuf> {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_tlstest"));
    let candidate = exe.parent()?.join("examples/broken_server");
    candidate.is_file().then_some(candidate)
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    AllPass,
    AllFail,
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

fn run(server: &str, stage: &str) -> (usize, usize, Option<i32>) {
    let out = tlstest()
        .args(["--server", server, "--stage", stage, "--no-color"])
        .output()
        .expect("tlstest must run");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let (p, t) = tally(&text);
    (p, t, out.status.code())
}

#[test]
fn the_broken_server_is_judged_field_by_field() {
    let Some(server) = broken_server() else {
        eprintln!("skipping: build with `cargo test --release` so the example is built");
        return;
    };
    let server = server.to_string_lossy().to_string();

    let expected: &[(&str, Verdict, &str)] = &[
        (
            "1",
            Verdict::AllPass,
            "accepting and letting the client speak first are both correct",
        ),
        (
            "2",
            Verdict::Mixed,
            "the record is well formed apart from legacy_record_version",
        ),
        (
            "8",
            Verdict::AllFail,
            "supported_versions is never negotiated",
        ),
        (
            "17",
            Verdict::AllFail,
            "the ServerHello's fields are wrong and nothing past it is written",
        ),
        ("24", Verdict::AllFail, "no certificate is ever sent"),
    ];

    let mut wrong = Vec::new();
    for (stage, want, why) in expected {
        let (passed, total, code) = run(&server, stage);
        let got = verdict(passed, total);
        if got != *want {
            wrong.push(format!(
                "stage {stage}: expected {want:?} — {why} — but got {got:?} ({passed}/{total})"
            ));
        }
        let all_green = got == Verdict::AllPass;
        if all_green && code != Some(0) {
            wrong.push(format!("stage {stage}: all green but exit code {code:?}"));
        }
        if !all_green && code == Some(0) {
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
fn one_wrong_field_costs_one_test() {
    let Some(server) = broken_server() else {
        eprintln!("skipping: build with `cargo test --release` so the example is built");
        return;
    };
    // The record this server writes is correct in every respect but one. If stage 02 went
    // entirely red, its tests would be checking "is this a valid record" rather than the
    // individual fields, and a learner would have no idea which byte to look at.
    let (passed, total, _) = run(&server.to_string_lossy(), "2");
    assert!(
        passed > 0 && passed < total,
        "one wrong field in a well-formed record should cost roughly one test, got {passed}/{total}"
    );
    assert!(
        total - passed <= 2,
        "one wrong field should not take most of the stage down with it, got {passed}/{total}"
    );
}

#[test]
fn the_stage_it_gets_right_is_entirely_green() {
    let Some(server) = broken_server() else {
        eprintln!("skipping: build with `cargo test --release` so the example is built");
        return;
    };
    let (passed, total, code) = run(&server.to_string_lossy(), "1");
    assert_eq!(
        passed, total,
        "accepting and waiting for the client are correct here, so stage 01 must be all green"
    );
    assert_eq!(code, Some(0), "an all-green stage must exit 0");
}
