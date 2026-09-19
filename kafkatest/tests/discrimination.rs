//! Does this suite discriminate?
//!
//! `examples/broken_broker.rs` does the one thing every Kafka broker must do — accept TCP
//! connections and answer length-framed requests — and then gets the very first field of
//! every answer wrong: the correlation id comes back incremented by one.
//!
//! That is a good bug to test a test suite with, because it is invisible to anything that
//! only checks framing. The response is the right length, the right shape and arrives at the
//! right time; one four-byte field in the header does not match the request it answers. A
//! suite that only asked "did the broker reply" would call this broker correct, and a client
//! built against it would mismatch every response to every request under concurrency.
//!
//! So stage 01 — bind and accept — is entirely green, because that part is genuinely right,
//! and everything that reads a correlation id back is red.

use std::path::PathBuf;
use std::process::Command;

fn kafkatest() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kafkatest"))
}

fn broken_broker() -> Option<PathBuf> {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_kafkatest"));
    let candidate = exe.parent()?.join("examples/broken_broker");
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

fn run(broker: &str, stage: &str) -> (usize, usize, Option<i32>) {
    let out = kafkatest()
        .args(["--broker", broker, "--stage", stage, "--no-color"])
        .output()
        .expect("kafkatest must run");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let (p, t) = tally(&text);
    (p, t, out.status.code())
}

#[test]
fn one_wrong_header_field_is_caught_everywhere_it_is_read() {
    let Some(broker) = broken_broker() else {
        eprintln!("skipping: build with `cargo test --release` so the example is built");
        return;
    };
    let broker = broker.to_string_lossy().to_string();

    let expected: &[(&str, Verdict, &str)] = &[
        (
            "1",
            Verdict::AllPass,
            "binding and accepting are genuinely correct",
        ),
        (
            "2",
            Verdict::Mixed,
            "the reply is well framed; its correlation id is not the one that was sent",
        ),
        (
            "4",
            Verdict::AllFail,
            "nothing that reads the header back can match",
        ),
        (
            "10",
            Verdict::AllFail,
            "no real API is implemented behind the header",
        ),
    ];

    let mut wrong = Vec::new();
    for (stage, want, why) in expected {
        let (passed, total, code) = run(&broker, stage);
        let got = verdict(passed, total);
        if got != *want {
            wrong.push(format!(
                "stage {stage}: expected {want:?} — {why} — but got {got:?} ({passed}/{total})"
            ));
        }
        if got == Verdict::AllPass && code != Some(0) {
            wrong.push(format!("stage {stage}: all green but exit code {code:?}"));
        }
        if got != Verdict::AllPass && code == Some(0) {
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
fn the_stage_it_gets_right_is_entirely_green() {
    let Some(broker) = broken_broker() else {
        eprintln!("skipping: build with `cargo test --release` so the example is built");
        return;
    };
    let (passed, total, code) = run(&broker.to_string_lossy(), "1");
    assert_eq!(
        passed, total,
        "accepting connections is correct here, so stage 01 must be all green"
    );
    assert_eq!(code, Some(0), "an all-green stage must exit 0");
}

#[test]
fn a_well_framed_wrong_answer_is_still_wrong() {
    let Some(broker) = broken_broker() else {
        eprintln!("skipping: build with `cargo test --release` so the example is built");
        return;
    };
    // The reply has the right length prefix and the right number of bytes. Only the value of
    // one field is wrong, and the suite has to notice — otherwise the whole track is testing
    // that a socket answers rather than that a protocol is spoken.
    let (passed, total, _) = run(&broker.to_string_lossy(), "2");
    assert!(
        passed < total,
        "a well-framed reply with the wrong correlation id must not pass ({passed}/{total})"
    );
}
