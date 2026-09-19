//! Does this suite discriminate?
//!
//! `examples/broken_node.rs` is the subtlest wrong program in this repo, and the best
//! argument for testing behaviours rather than features. It speaks the etcd v3 JSON subset
//! correctly: it puts, it ranges, it handles prefixes and limits and transactions, it hands
//! back sensible revisions. Point a casual test suite at it and it looks like a working
//! key/value store.
//!
//! It is wrong in one specific, famous way: **it acknowledges a write before the write is
//! durable.** Nothing about that is visible until the process is killed and restarted, which
//! is exactly what stage 35 does and nothing before it does.
//!
//! So the profile below is mostly green — stages 22 and 23 are *entirely* green, because the
//! read and range semantics really are right — and then falls off a cliff at the stage whose
//! whole subject is the bug. That shape is the claim this file exists to defend: the suite
//! knows the difference between a store that is wrong about ranges and a store that is wrong
//! about durability, and it says which.

use std::process::Command;

fn disttest() -> Command {
    Command::new(env!("CARGO_BIN_EXE_disttest"))
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

fn run(stage: &str) -> (usize, usize, Option<i32>) {
    let out = disttest()
        .args(["--target", "broken_node", "--stage", stage, "--no-color"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("disttest must run");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let (p, t) = tally(&text);
    (p, t, out.status.code())
}

#[test]
fn a_store_that_is_wrong_only_about_durability_fails_only_where_that_shows() {
    let expected: &[(&str, Verdict, &str)] = &[
        (
            "22",
            Verdict::AllPass,
            "put and the response header are genuinely correct",
        ),
        (
            "23",
            Verdict::AllPass,
            "reading one key is genuinely correct",
        ),
        (
            "35",
            Verdict::AllFail,
            "the acknowledged write does not survive SIGKILL, which is the whole bug",
        ),
    ];

    let mut wrong = Vec::new();
    for (stage, want, why) in expected {
        let (passed, total, code) = run(stage);
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
fn the_stages_it_gets_right_are_entirely_green() {
    // Two whole stages, green end to end, against a program that is genuinely broken. This
    // is the property that makes the red mark at stage 35 worth anything: the suite is not
    // failing this node out of general suspicion, it is failing it for one reason.
    for stage in ["22", "23"] {
        let (passed, total, code) = run(stage);
        assert_eq!(
            passed, total,
            "stage {stage} must be all green against a store whose reads are correct"
        );
        assert_eq!(code, Some(0), "an all-green stage must exit 0");
    }
}

#[test]
fn the_durability_stage_is_where_it_dies() {
    // Not "somewhere after stage 30" — here. A suite that reported this bug at stage 22
    // would send a learner to debug the wrong code.
    let (before, before_total, _) = run("23");
    let (at, at_total, code) = run("35");
    assert_eq!(
        before, before_total,
        "the stage before is unaffected ({before}/{before_total})"
    );
    assert_eq!(at, 0, "every durability test must fail ({at}/{at_total})");
    assert_eq!(code, Some(1), "a red stage must exit 1");
}
