//! Stage 11 — HyperLogLog.
//!
//! A sketch that answers "how many distinct things have I seen" out of a few kilobytes, and
//! the second stage whose answer is a distribution. The absolute properties are cheap to
//! state and easy to get wrong — an empty sketch counts zero, the same item a thousand
//! times counts once, and the order the items arrived in cannot matter — and the interesting
//! one is the error, which has to sit inside `1.04 / sqrt(2^p)`.
//!
//! One trial is one draw from that distribution, so a single run says almost nothing: the
//! envelope test averages the relative error over several independent element sets and
//! records every one of them. The slack factor is three standard errors, which a correct
//! implementation clears by a wide margin and a broken one misses by orders of magnitude,
//! because the usual failures here — no linear counting below the threshold, a hash whose
//! low bits do not move, a rank counted from the wrong end — are not small biases.

use crate::assert::{Check, Failure};
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::prim::{oracles, PrimProc};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;

/// Stage 11.
pub fn stage() -> Stage {
    Stage {
        number: 11,
        slug: "hyperloglog",
        name: "HyperLogLog",
        ext: true,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `hll`: 2^p registers, each holding the longest run of leading zeros it has seen",
            "The estimate is the harmonic mean of the registers, times alpha times m squared",
            "Small cardinalities need linear counting, or the estimate is badly wrong",
            "The relative error must sit inside 1.04 / sqrt(2^p) over many trials",
        ],
        examples,
        tests: vec![
            Test::new("count on an empty sketch is zero", empty_counts_zero),
            Test::new(
                "init reports two to the power p registers",
                init_reports_registers,
            ),
            Test::new(
                "the estimate of a small set is close to exact",
                small_sets_are_exact,
            ),
            Test::new(
                "adding the same item a thousand times does not change the estimate",
                duplicates_are_free,
            ),
            Test::new(
                "counting twice gives the same answer",
                count_is_not_destructive,
            )
            .ext(),
            Test::new(
                "the insertion order does not change the estimate",
                order_does_not_matter,
            )
            .ext(),
            Test::new(
                "a mean relative error inside the standard-error envelope",
                error_inside_the_envelope,
            )
            .min_timeout_ms(120_000),
            Test::new(
                "a larger precision is not worse than a smaller one",
                more_registers_is_not_worse,
            )
            .min_timeout_ms(60_000),
            Test::new("init a second time resets the sketch", init_resets).ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example(
            "A sketch counting four things, one of them twice",
            "hll",
            || {
                lines(&[
                    "init 12", "add a", "add b", "add c", "add b", "add d", "count",
                ])
            },
        )
        .request("a sketch of 2^12 registers, five adds over four distinct items")
        .response("4096 registers, and an estimate of exactly 4")
        .note(
            "Five adds, four distinct items, and the answer is the distinct count: a register \
             only ever moves up to a longer run of zeros, so the second `add b` writes the \
             same value it wrote the first time and changes nothing.",
        ),
        prim_example("Where the raw estimator cannot be trusted", "hll", || {
            lines(&["init 4", "add a", "count", "init 14", "add a", "count"])
        })
        .request("one item in a sketch of 16 registers, then the same item in a sketch of 16384")
        .response("1 both times")
        .note(
            "With one item in 16 registers the harmonic-mean estimator is wildly biased — it \
             is asymptotic, and one item is not the asymptote. Linear counting over the \
             registers still at zero is what makes a small set come back as itself, and the \
             usual threshold to switch at is 2.5 m.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Shared moves
// ---------------------------------------------------------------------------------------

/// Add `n` distinct items under a tag, then ask for the estimate.
async fn count_of(p: &mut PrimProc, p_bits: u32, tag: &str, n: usize) -> Result<i64, Failure> {
    p.send(&format!("init {p_bits}")).await?;
    for i in 0..n {
        p.send(&format!("add {tag}-{i}")).await?;
    }
    p.num("count", "estimate").await
}

/// How far an estimate is from the truth, as a fraction of the truth.
fn relative_error(estimate: i64, exact: usize) -> f64 {
    if exact == 0 {
        return if estimate == 0 { 0.0 } else { 1.0 };
    }
    (estimate as f64 - exact as f64).abs() / exact as f64
}

/// The mean of a list of errors, or zero for an empty one.
fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(empty_counts_zero, |ctx| {
    let p = ctx.prim("hll").await?;
    p.send("init 12").await?;
    let before = p.num("count", "estimate").await?;
    p.send("add solo").await?;
    let after = p.num("count", "estimate").await?;

    let mut c = Check::new("a sketch nothing has been added to");
    c.eq("count.estimate on an empty sketch", 0, before);
    c.eq("count.estimate after one item", 1, after);
    c.note("every register is zero, so linear counting gives m * ln(m/m) = 0; an estimator that skips that case answers alpha * m, not nothing");
    c.finish()
});

dist_test!(init_reports_registers, |ctx| {
    let p = ctx.prim("hll").await?;
    let mut seen = Vec::new();
    for bits in [4u32, 8, 12, 14] {
        let cmd = format!("init {bits}");
        let v = p.send(&cmd).await?;
        seen.push((bits, p.expect_i64(&v, &cmd, "registers")?));
    }

    let mut c = Check::new("what init says about the sketch it made");
    for (bits, registers) in seen {
        c.eq(&format!("init {bits}.registers"), 1i64 << bits, registers);
    }
    c.finish()
});

dist_test!(small_sets_are_exact, |ctx| {
    let p = ctx.prim("hll").await?;
    let mut measured = Vec::new();
    for n in [1usize, 10, 100, 1_000] {
        let estimate = count_of(p, 14, &format!("small{n}"), n).await?;
        measured.push((n, estimate));
    }

    let mut c = Check::new("small cardinalities, where linear counting is the estimator");
    for (n, estimate) in &measured {
        let slack = (*n as f64 * 0.05).max(2.0);
        c.observe(&format!("count.estimate for {n} items"), estimate);
        c.within(
            &format!("count.estimate for {n} items"),
            (*n as f64 - slack).round() as i64,
            (*n as f64 + slack).round() as i64,
            *estimate,
        );
    }
    c.note("far below 2.5 m the register-based estimator is not merely noisy, it is biased by tens of percent; linear counting is exact to a couple of items here");
    c.finish()
});

dist_test!(duplicates_are_free, |ctx| {
    let p = ctx.prim("hll").await?;
    let before = count_of(p, 12, "base", 500).await?;
    for _ in 0..1_000 {
        p.send("add base-0").await?;
    }
    let after = p.num("count", "estimate").await?;
    p.send("add something-new").await?;
    let grown = p.num("count", "estimate").await?;

    let mut c = Check::new("one item added a thousand times");
    c.eq("count.estimate after 1000 duplicate adds", before, after);
    c.that(
        "count.estimate after a genuinely new item",
        "an estimate that moved, because the item really was new",
        grown >= after,
        (after, grown),
    );
    c.note("a sketch that counts adds rather than distinct hashes passes every other test in this stage and fails this one");
    c.finish()
});

dist_test!(count_is_not_destructive, |ctx| {
    let p = ctx.prim("hll").await?;
    let first = count_of(p, 12, "stable", 800).await?;
    let second = p.num("count", "estimate").await?;
    let third = p.num("count", "estimate").await?;

    let mut c = Check::new("asking the same sketch three times");
    c.eq("count.estimate (second call)", first, second);
    c.eq("count.estimate (third call)", first, third);
    c.note("count reads the registers; it must not normalise, clear or rebuild them");
    c.finish()
});

dist_test!(order_does_not_matter, |ctx| {
    // A seeded shuffle, so a failure is reproducible with the seed that found it.
    let n = 2_000usize;
    let mut order: Vec<usize> = (0..n).collect();
    for i in (1..n).rev() {
        let j = ctx.rng.random_range(0..=i);
        order.swap(i, j);
    }
    let seed = ctx.seed;
    let p = ctx.prim("hll").await?;
    let forwards = count_of(p, 12, "order", n).await?;
    p.send("init 12").await?;
    for i in &order {
        p.send(&format!("add order-{i}")).await?;
    }
    let shuffled = p.num("count", "estimate").await?;

    let mut c = Check::new("the same two thousand items in two different orders");
    c.eq(
        "count.estimate after a shuffled insertion",
        forwards,
        shuffled,
    );
    c.note(format!("the shuffle comes from --seed {seed}"));
    c.note("every register keeps a maximum, and a maximum does not care what order it saw its inputs in");
    c.finish()
});

dist_test!(error_inside_the_envelope, |ctx| {
    let precision = 14u32;
    let exact = 10_000usize;
    let trials = 3usize;
    let seed = ctx.seed;
    let p = ctx.prim("hll").await?;
    let mut errors = Vec::new();
    let mut estimates = Vec::new();
    for trial in 0..trials {
        let estimate = count_of(p, precision, &format!("s{seed:x}t{trial}"), exact).await?;
        estimates.push(estimate);
        errors.push(relative_error(estimate, exact));
    }
    let se = oracles::hll_standard_error(precision);
    let bound = se * 3.0;
    let observed = mean(&errors);

    let mut c = Check::new("the relative error of a sketch over several element sets");
    for (i, (estimate, error)) in estimates.iter().zip(&errors).enumerate() {
        c.observe(&format!("trial[{i}].estimate"), estimate);
        c.observe(&format!("trial[{i}].relative_error"), error);
    }
    c.observe("hll.mean_relative_error", observed);
    c.observe("hll.standard_error", se);
    c.at_most("hll.mean_relative_error", bound, observed);
    c.note(format!(
        "{trials} sets of {exact} distinct items at p={precision}: 1.04/sqrt(2^p) is \
         {se:.5}, and the bound is three of those"
    ));
    c.note("the element sets are derived from --seed, so a failure is reproducible and a pass is not luck with one particular set of strings");
    c.finish()?;
    ctx.note(format!(
        "p={precision}, n={exact}: estimates {estimates:?}, mean relative error {observed:.4} against a bound of {bound:.4}"
    ));
    Ok(())
});

dist_test!(more_registers_is_not_worse, |ctx| {
    let small = 6u32;
    let large = 14u32;
    let exact = 2_000usize;
    let seed = ctx.seed;
    let p = ctx.prim("hll").await?;
    let mut small_errors = Vec::new();
    let mut large_errors = Vec::new();
    for trial in 0..3 {
        let tag = format!("p{seed:x}t{trial}");
        let a = count_of(p, small, &tag, exact).await?;
        let b = count_of(p, large, &tag, exact).await?;
        small_errors.push(relative_error(a, exact));
        large_errors.push(relative_error(b, exact));
    }
    let (small_mean, large_mean) = (mean(&small_errors), mean(&large_errors));
    // One trial of a 64-register sketch is very noisy, so the comparison is only asserted
    // with a whole standard error of that noise allowed on top.
    let slack = oracles::hll_standard_error(small);

    let mut c = Check::new("the same set counted at two precisions");
    c.observe(&format!("p={small}.mean_relative_error"), small_mean);
    c.observe(&format!("p={large}.mean_relative_error"), large_mean);
    c.observe(&format!("p={small}.errors"), &small_errors);
    c.observe(&format!("p={large}.errors"), &large_errors);
    c.at_most(
        &format!("p={large}.mean_relative_error"),
        small_mean + slack,
        large_mean,
    );
    c.at_most(
        &format!("p={large}.mean_relative_error"),
        oracles::hll_standard_error(large) * 3.0,
        large_mean,
    );
    c.note("more registers buy accuracy and nothing else; a sketch that gets worse with p has its index and its rank taken from overlapping bits");
    c.finish()?;
    ctx.note(format!(
        "p={small} mean error {small_mean:.4}, p={large} mean error {large_mean:.4}"
    ));
    Ok(())
});

dist_test!(init_resets, |ctx| {
    let p = ctx.prim("hll").await?;
    let before = count_of(p, 12, "reset", 3_000).await?;
    p.send("init 12").await?;
    let after = p.num("count", "estimate").await?;
    p.send("add reset-0").await?;
    let one = p.num("count", "estimate").await?;

    let mut c = Check::new("init on a sketch that already holds three thousand items");
    c.that(
        "count.estimate before the second init",
        "an estimate near three thousand",
        (2_500..=3_500).contains(&before),
        before,
    );
    c.eq("count.estimate after the second init", 0, after);
    c.eq("count.estimate after one item", 1, one);
    c.finish()
});
