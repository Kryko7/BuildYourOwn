//! Does this suite discriminate?
//!
//! A conformance tester that fails everything is as useless as one that passes everything.
//! What makes a red mark mean something is that the suite goes red *where the program is
//! wrong*, green *where it is right*, and partially red where the program is partially
//! right — and the only way to know it does that is to point it at a shell whose bugs are
//! known in advance.
//!
//! `examples/broken_shell.sh` is that shell. It has four bugs on purpose: `echo` drops its
//! last argument, `exit N` ignores N, the command-not-found message has the wrong wording
//! and goes to the wrong stream, and `type` is missing entirely. Everything else — the
//! prompt, the read loop, EOF handling — is correct.
//!
//! So the profile below is not "mostly red". Stage 01 is entirely green, because the prompt
//! really does work. Stages 04 and 05 are *mixed*, because `exit` and `echo` half work, and
//! those two are the most valuable rows in the table: they are the ones that would break if
//! the suite were testing whole behaviours instead of individual ones.

use std::process::Command;

fn shelltest() -> Command {
    Command::new(env!("CARGO_BIN_EXE_shelltest"))
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

fn run(stage: &str) -> (usize, usize, Option<i32>) {
    let out = shelltest()
        .args(["--shell", "examples/broken_shell.sh", "--stage", stage])
        .arg("--no-color")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("shelltest must run");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let (p, t) = tally(&text);
    (p, t, out.status.code())
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

#[test]
fn the_broken_shell_is_judged_bug_by_bug() {
    // Each row is a claim about the broken shell, taken from the bugs it documents about
    // itself — not from whatever a run happened to produce.
    let expected: &[(&str, Verdict, &str)] = &[
        (
            "1",
            Verdict::AllPass,
            "the prompt and the read loop are correct, so nothing here may go red",
        ),
        (
            "2",
            Verdict::AllFail,
            "the command-not-found message has the wrong wording and stream",
        ),
        (
            "4",
            Verdict::Mixed,
            "`exit` exits but ignores its status argument",
        ),
        (
            "5",
            Verdict::Mixed,
            "`echo` prints, but drops its last argument",
        ),
        ("6", Verdict::AllFail, "`type` is not implemented at all"),
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
        match got {
            Verdict::AllPass if code != Some(0) => {
                wrong.push(format!("stage {stage}: all green but exit code {code:?}"))
            }
            Verdict::AllFail | Verdict::Mixed if code == Some(0) => wrong.push(format!(
                "stage {stage}: {passed}/{total} passed but exit code was 0"
            )),
            _ => {}
        }
    }
    assert!(
        wrong.is_empty(),
        "the suite did not discriminate:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn a_partially_right_builtin_is_judged_partially() {
    // This is the row that proves the tests are about behaviours rather than features. The
    // broken `echo` writes to stdout, ends with a newline and handles no arguments — all
    // correct — and drops its last argument, which is not. A suite whose stage-05 tests were
    // one big "does echo work" would report 0/9 here and tell a learner nothing about where
    // to look.
    let (passed, total, _) = run("5");
    assert!(
        passed > 0,
        "the parts of echo that work must be reported as working ({passed}/{total})"
    );
    assert!(
        passed < total,
        "dropping the last argument must be caught ({passed}/{total})"
    );
}

#[test]
fn the_stage_it_gets_right_is_entirely_green() {
    // The half that matters. Any tester can fail a broken program; this one has to agree
    // with it about the part it gets right, or every failure it reports is suspect.
    let (passed, total, code) = run("1");
    assert_eq!(
        passed, total,
        "the broken shell's prompt and read loop are correct, so stage 01 must be all green"
    );
    assert_eq!(code, Some(0), "an all-green stage must exit 0");
}
