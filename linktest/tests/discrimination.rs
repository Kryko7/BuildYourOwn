//! Does this suite discriminate?
//!
//! `examples/broken_linker.rs` is the most interesting of the repo's wrong programs, because
//! it is almost right. It reads relocatable objects, concatenates sections, assigns
//! addresses, resolves symbols, writes an `ET_EXEC` with three `PT_LOAD` segments and a
//! symbol table, and produces a file the kernel runs without complaint. It gets exactly one
//! thing wrong: **it ignores the relocation addend**, so `S + A - P` comes out as `S - P`
//! and every RIP-relative reference lands four bytes past what it meant to name.
//!
//! A linker like that is the hardest kind of program to test, because nothing crashes. The
//! output is a valid ELF, it loads, it runs, and it prints the wrong string. So the profile
//! below is mostly *mixed*: the structural tests pass because the structure really is right,
//! and the tests that check where a relocation landed fail because it landed in the wrong
//! place. A suite that only checked "did the linker produce a runnable executable" would
//! call this linker correct.

use std::path::PathBuf;
use std::process::Command;

fn linktest() -> Command {
    Command::new(env!("CARGO_BIN_EXE_linktest"))
}

fn broken_linker() -> Option<PathBuf> {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_linktest"));
    let candidate = exe.parent()?.join("examples/broken_linker");
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

fn run(linker: &str, stage: &str) -> (usize, usize, Option<i32>) {
    let out = linktest()
        .args(["--linker", linker, "--stage", stage, "--no-color"])
        .output()
        .expect("linktest must run");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let (p, t) = tally(&text);
    (p, t, out.status.code())
}

#[test]
fn a_linker_that_is_almost_right_is_judged_almost_right() {
    let Some(linker) = broken_linker() else {
        eprintln!("skipping: build with `cargo test --release` so the example is built");
        return;
    };
    let linker = linker.to_string_lossy().to_string();

    let expected: &[(&str, Verdict, &str)] = &[
        (
            "1",
            Verdict::Mixed,
            "a runnable executable comes out, but what it prints is wrong",
        ),
        (
            "2",
            Verdict::Mixed,
            "inputs are validated correctly; the output's contents are not",
        ),
        (
            "16",
            Verdict::AllFail,
            "every cross-object reference misses by the addend",
        ),
        (
            "24",
            Verdict::Mixed,
            "PC32 and PLT32 are computed without the addend",
        ),
        (
            "30",
            Verdict::AllFail,
            "references across sections all land four bytes out",
        ),
    ];

    let mut wrong = Vec::new();
    for (stage, want, why) in expected {
        let (passed, total, code) = run(&linker, stage);
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
fn the_structure_it_gets_right_is_credited() {
    let Some(linker) = broken_linker() else {
        eprintln!("skipping: build with `cargo test --release` so the example is built");
        return;
    };
    // This is the claim that matters for a linker that produces valid output. The ELF header,
    // the program headers, the segment permissions and the entry point are all correct, and
    // the suite has to say so — otherwise a learner fixing the addend bug would have no way
    // to tell which of the other failures were real.
    let (passed, total, _) = run(&broken_linker().unwrap().to_string_lossy(), "1");
    assert!(
        passed > 0,
        "a linker that emits a valid, runnable ELF must be credited for it ({passed}/{total})"
    );
    assert!(
        passed < total,
        "and must still be caught printing the wrong string ({passed}/{total})"
    );
    let _ = linker;
}

#[test]
fn running_the_output_is_what_catches_it() {
    let Some(linker) = broken_linker() else {
        eprintln!("skipping: build with `cargo test --release` so the example is built");
        return;
    };
    // Nothing about this linker's output is structurally invalid — readelf is happy with it
    // and the kernel loads it. The bug is only visible by executing the result and reading
    // what it printed, which is why this track runs every program it links rather than just
    // inspecting it.
    let out = linktest()
        .args([
            "--linker",
            &linker.to_string_lossy(),
            "--stage",
            "1",
            "--no-color",
            "--verbose",
        ])
        .output()
        .expect("linktest must run");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("program.stdout") || text.contains("stdout"),
        "the failure should be about what the program printed:\n{text}"
    );
}
