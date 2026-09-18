//! Stage 08 — Quorum math: N, R, W and overlap.
//!
//! The only primitives stage whose answers the tester knows exactly, because the three
//! numbers are closed-form: a read of R and a write of W out of N overlap when `R + W > N`,
//! two writes can run without seeing each other when `2W <= N`, and the smallest read that
//! always catches the latest write is `N - W + 1`. The oracles are
//! [`oracles::quorum_overlaps`], [`oracles::write_conflict_possible`] and
//! [`oracles::smallest_read_quorum`], and the stage sweeps every small triple rather than
//! spot-checking the famous ones.
//!
//! The trap is answering from a remembered configuration instead of from the numbers in the
//! command. Every sweep here asks for triples in an order no default agrees with, and the
//! seeded sweep goes well past the sizes anyone would hard-code.

use crate::assert::{Check, Failure};
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::prim::{oracles, PrimProc};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;

/// Stage 08.
pub fn stage() -> Stage {
    Stage {
        number: 8,
        slug: "quorum_math",
        name: "Quorum math: N, R, W and overlap",
        ext: false,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `quorum`: a read and a write quorum overlap when R + W > N",
            "Two writes can conflict when 2W <= N — that is a different question from overlap",
            "The smallest read quorum that always sees the latest write is N - W + 1",
            "Answer for the numbers you were given, not for the defaults",
        ],
        examples,
        tests: vec![
            Test::new(
                "a read and a write overlap exactly when R + W is more than N",
                overlap_over_every_small_triple,
            ),
            Test::new(
                "two writes can conflict exactly when 2W is at most N",
                conflict_over_every_small_triple,
            ),
            Test::new(
                "the smallest read quorum is N - W + 1",
                min_r_over_every_small_triple,
            ),
            Test::new(
                "the three-replica settings everyone quotes",
                the_classic_three_replica_settings,
            ),
            Test::new(
                "the edges: reading from nobody, writing to everybody, a single replica",
                the_edges,
            ),
            Test::new(
                "a seeded sweep of larger triples agrees with the arithmetic",
                a_seeded_sweep_of_larger_triples,
            ),
            Test::new(
                "a larger read quorum never loses an overlap it already had",
                overlap_is_monotone_in_r,
            )
            .ext(),
            Test::new(
                "the same triple always gets the same answer",
                the_answer_does_not_drift,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("The three-replica settings", "quorum", || {
            lines(&["check 3 2 2", "check 3 1 2", "check 3 3 1"])
        })
        .request("N = 3 with the three settings a store actually ships with")
        .response("2/2 overlaps, 1/2 does not, and 3/1 overlaps but lets two writes conflict")
        .note(
            "`overlap` and `write_conflict` answer different questions and a program that \
             derives one from the other is wrong on the third line: R = 3, W = 1 reads from \
             everybody, so every read sees every write, while two writes of one replica each \
             can land on different nodes and neither will ever see the other.",
        ),
        prim_example("Writing to everybody, reading from one", "quorum", || {
            lines(&["check 5 1 5", "check 5 5 1", "check 1 1 1"])
        })
        .request("the two extremes of N = 5, and the degenerate single replica")
        .response("`min_r` is 1 when W = 5, and 5 when W = 1")
        .note(
            "`min_r` is N - W + 1 and nothing else: it is the smallest read that cannot miss \
             a write of W, not the R that was passed in and not a majority. With W = N one \
             replica is enough, which is exactly the trade a write-all store is making.",
        ),
    ]
}

/// What one `check` answered.
struct Checked {
    /// The triple that was asked about.
    triple: (i64, i64, i64),
    /// `overlap` from the answer.
    overlap: bool,
    /// `write_conflict` from the answer.
    write_conflict: bool,
    /// `min_r` from the answer.
    min_r: i64,
}

/// Ask `check n r w` and decode all three fields.
async fn check_triple(p: &mut PrimProc, n: i64, r: i64, w: i64) -> Result<Checked, Failure> {
    let command = format!("check {n} {r} {w}");
    let answer = p.send(&command).await?;
    Ok(Checked {
        triple: (n, r, w),
        overlap: p.expect_bool(&answer, &command, "overlap")?,
        write_conflict: p.expect_bool(&answer, &command, "write_conflict")?,
        min_r: p.expect_i64(&answer, &command, "min_r")?,
    })
}

/// Every triple with `1 <= n <= max_n` and `0 <= r, w <= n`, largest N first.
///
/// Descending N is not decoration: a program that latched onto the first configuration it
/// saw answers the whole sweep for that one, and starting at the largest N makes the
/// mismatch show up on the second line rather than the last.
fn small_triples(max_n: i64) -> Vec<(i64, i64, i64)> {
    let mut out = Vec::new();
    for n in (1..=max_n).rev() {
        for r in 0..=n {
            for w in 0..=n {
                out.push((n, r, w));
            }
        }
    }
    out
}

/// One row of the seeded sweep's disagreement list: the triple asked about, the three
/// numbers the arithmetic gives, and the three the program answered.
type Disagreement = ((i64, i64, i64), (bool, bool, i64), (bool, bool, i64));

/// Ask about every triple in the list.
async fn check_all(p: &mut PrimProc, triples: &[(i64, i64, i64)]) -> Result<Vec<Checked>, Failure> {
    let mut out = Vec::with_capacity(triples.len());
    for (n, r, w) in triples {
        out.push(check_triple(p, *n, *r, *w).await?);
    }
    Ok(out)
}

dist_test!(overlap_over_every_small_triple, |ctx| {
    let triples = small_triples(6);
    let p = ctx.prim("quorum").await?;
    let answers = check_all(p, &triples).await?;
    let wrong: Vec<((i64, i64, i64), bool, bool)> = answers
        .iter()
        .map(|a| {
            (
                a.triple,
                oracles::quorum_overlaps(a.triple.0, a.triple.1, a.triple.2),
                a.overlap,
            )
        })
        .filter(|(_, want, got)| want != got)
        .take(5)
        .collect();
    let mut c = Check::new("overlap over every (n, r, w) with n up to six");
    c.observe("triples asked", answers.len());
    c.that(
        "((n, r, w), expected, check.overlap)",
        "r + w > n, for every one of them",
        wrong.is_empty(),
        wrong,
    );
    c.finish()
});

dist_test!(conflict_over_every_small_triple, |ctx| {
    let triples = small_triples(6);
    let p = ctx.prim("quorum").await?;
    let answers = check_all(p, &triples).await?;
    let wrong: Vec<((i64, i64, i64), bool, bool)> = answers
        .iter()
        .map(|a| {
            (
                a.triple,
                oracles::write_conflict_possible(a.triple.0, a.triple.2),
                a.write_conflict,
            )
        })
        .filter(|(_, want, got)| want != got)
        .take(5)
        .collect();
    let mut c = Check::new("write_conflict over every (n, r, w) with n up to six");
    c.observe("triples asked", answers.len());
    c.that(
        "((n, r, w), expected, check.write_conflict)",
        "2w <= n, whatever r happens to be",
        wrong.is_empty(),
        wrong,
    );
    c.finish()
});

dist_test!(min_r_over_every_small_triple, |ctx| {
    let triples = small_triples(6);
    let p = ctx.prim("quorum").await?;
    let answers = check_all(p, &triples).await?;
    let wrong: Vec<((i64, i64, i64), i64, i64)> = answers
        .iter()
        .map(|a| {
            (
                a.triple,
                oracles::smallest_read_quorum(a.triple.0, a.triple.2),
                a.min_r,
            )
        })
        .filter(|(_, want, got)| want != got)
        .take(5)
        .collect();
    let mut c = Check::new("min_r over every (n, r, w) with n up to six");
    c.observe("triples asked", answers.len());
    c.that(
        "((n, r, w), expected, check.min_r)",
        "n - w + 1, never below one and never a function of r",
        wrong.is_empty(),
        wrong,
    );
    c.finish()
});

dist_test!(the_classic_three_replica_settings, |ctx| {
    let p = ctx.prim("quorum").await?;
    let majority = check_triple(p, 3, 2, 2).await?;
    let fast_read = check_triple(p, 3, 1, 2).await?;
    let write_one = check_triple(p, 3, 3, 1).await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("N = 3 at R/W of 2/2, 1/2 and 3/1");
    c.eq("check(3,2,2).overlap", true, majority.overlap);
    c.eq(
        "check(3,2,2).write_conflict",
        false,
        majority.write_conflict,
    );
    c.eq("check(3,2,2).min_r", 2, majority.min_r);
    c.eq("check(3,1,2).overlap", false, fast_read.overlap);
    c.eq("check(3,1,2).min_r", 2, fast_read.min_r);
    c.eq("check(3,3,1).overlap", true, write_one.overlap);
    c.eq(
        "check(3,3,1).write_conflict",
        true,
        write_one.write_conflict,
    );
    c.eq("check(3,3,1).min_r", 3, write_one.min_r);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(the_edges, |ctx| {
    // R = 0 reads nothing and can therefore never overlap; W = N is written everywhere and
    // one replica is enough to read it; N = 1 is the degenerate store where every quorum is
    // the same node.
    let p = ctx.prim("quorum").await?;
    let read_nobody = check_triple(p, 5, 0, 5).await?;
    let write_all = check_triple(p, 5, 1, 5).await?;
    let write_one = check_triple(p, 5, 5, 1).await?;
    let single = check_triple(p, 1, 1, 1).await?;
    let single_zero = check_triple(p, 1, 0, 1).await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("the edges of the arithmetic");
    c.eq("check(5,0,5).overlap", false, read_nobody.overlap);
    c.eq("check(5,0,5).min_r", 1, read_nobody.min_r);
    c.eq("check(5,1,5).overlap", true, write_all.overlap);
    c.eq(
        "check(5,1,5).write_conflict",
        false,
        write_all.write_conflict,
    );
    c.eq("check(5,1,5).min_r", 1, write_all.min_r);
    c.eq("check(5,5,1).overlap", true, write_one.overlap);
    c.eq(
        "check(5,5,1).write_conflict",
        true,
        write_one.write_conflict,
    );
    c.eq("check(5,5,1).min_r", 5, write_one.min_r);
    c.eq("check(1,1,1).overlap", true, single.overlap);
    c.eq("check(1,1,1).write_conflict", false, single.write_conflict);
    c.eq("check(1,1,1).min_r", 1, single.min_r);
    c.eq("check(1,0,1).overlap", false, single_zero.overlap);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_seeded_sweep_of_larger_triples, |ctx| {
    // Far past anything worth hard-coding, and reproducible from --seed.
    let mut triples = Vec::new();
    for _ in 0..60 {
        let n = ctx.rng.random_range(1..=64i64);
        let r = ctx.rng.random_range(0..=n);
        let w = ctx.rng.random_range(0..=n);
        triples.push((n, r, w));
    }
    let seed = ctx.seed;
    let p = ctx.prim("quorum").await?;
    let answers = check_all(p, &triples).await?;
    let wrong: Vec<Disagreement> = answers
        .iter()
        .map(|a| {
            let (n, r, w) = a.triple;
            (
                a.triple,
                (
                    oracles::quorum_overlaps(n, r, w),
                    oracles::write_conflict_possible(n, w),
                    oracles::smallest_read_quorum(n, w),
                ),
                (a.overlap, a.write_conflict, a.min_r),
            )
        })
        .filter(|(_, want, got)| want != got)
        .take(5)
        .collect();
    let mut c = Check::new("60 seeded triples with n up to 64");
    c.note(format!("seed {seed:#x}"));
    c.observe("triples asked", answers.len());
    c.that(
        "((n, r, w), expected (overlap, write_conflict, min_r), answered)",
        "the same three numbers the arithmetic gives",
        wrong.is_empty(),
        wrong,
    );
    c.finish()
});

dist_test!(overlap_is_monotone_in_r, |ctx| {
    // Nothing about a larger read quorum can take an overlap away, and `min_r` does not
    // move at all when only r changes. Both fall out of the formulas, which is the point:
    // a program that answers from a stored configuration breaks one of them.
    let p = ctx.prim("quorum").await?;
    let mut answers = Vec::new();
    for r in 0..=7 {
        answers.push(check_triple(p, 7, r, 3).await?);
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("n = 7, w = 3, with r walked from 0 to 7");
    let mut once_overlapping = false;
    for a in &answers {
        let r = a.triple.1;
        if a.overlap {
            once_overlapping = true;
        } else if once_overlapping {
            c.that(
                &format!("check(7,{r},3).overlap"),
                "an overlap, because a smaller r already had one",
                false,
                a.overlap,
            );
        }
        c.eq(&format!("check(7,{r},3).min_r"), 5, a.min_r);
        // 2w = 6 is at most n = 7, so two writes can miss each other whatever r is.
        c.eq(
            &format!("check(7,{r},3).write_conflict"),
            true,
            a.write_conflict,
        );
    }
    c.eq(
        "the smallest r that overlaps",
        Some(5),
        answers.iter().find(|a| a.overlap).map(|a| a.triple.1),
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(the_answer_does_not_drift, |ctx| {
    let p = ctx.prim("quorum").await?;
    let first = check_triple(p, 5, 3, 3).await?;
    let mut repeats = Vec::new();
    for _ in 0..3 {
        // Other triples in between, so a program that caches the last answer is caught.
        check_triple(p, 9, 1, 1).await?;
        check_triple(p, 2, 2, 2).await?;
        repeats.push(check_triple(p, 5, 3, 3).await?);
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("one triple asked four times with other triples in between");
    for (i, again) in repeats.iter().enumerate() {
        c.eq(
            &format!("check(5,3,3)[{}].overlap", i + 1),
            first.overlap,
            again.overlap,
        );
        c.eq(
            &format!("check(5,3,3)[{}].write_conflict", i + 1),
            first.write_conflict,
            again.write_conflict,
        );
        c.eq(
            &format!("check(5,3,3)[{}].min_r", i + 1),
            first.min_r,
            again.min_r,
        );
    }
    c.eq("check(5,3,3).overlap", true, first.overlap);
    c.eq("check(5,3,3).min_r", 3, first.min_r);
    c.block("transcript", transcript);
    c.finish()
});
