//! Stage 10 — Bloom filter.
//!
//! The first stage whose answer is a distribution rather than a value. Two things are being
//! checked, and they pull in opposite directions: the filter must *never* lose an item, and
//! it must not claim to hold very much it does not. The first is absolute — a false
//! negative is a bug in the hashing or the bit setting, never a tuning decision — and the
//! second is only ever true within a bound, so every test here that measures a rate also
//! records it with `observe`, next to what `(1 - e^(-kn/m))^k` predicted.
//!
//! The slack is deliberately wide. One filter is one sample: with a few thousand probes the
//! measured rate is a binomial draw around the theoretical one, and a bound tight enough to
//! be interesting would fail one run in ten. A suite self-check that flakes is worse than no
//! check, so the bound is the published formula times 2.5 plus a small absolute allowance,
//! which is still an order of magnitude below what a broken filter produces.

use crate::assert::{Check, Failure};
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::prim::{oracles, PrimProc};
use crate::stages::{Ladder, Stage, Test};

/// Stage 10.
pub fn stage() -> Stage {
    Stage {
        number: 10,
        slug: "bloom_filter",
        name: "Bloom filter",
        ext: false,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `bloom`: k hash positions per item over m bits, all set on add",
            "`contains` is only ever 'maybe' or 'no': a false negative is a bug, never a tuning issue",
            "The measured false-positive rate must sit inside the theoretical bound for m, k and n",
            "Derive the k positions from one or two hashes; k independent hash functions are not needed",
        ],
        examples,
        tests: vec![
            Test::new("init reports back the size it was given", init_reports_the_size),
            Test::new("an item that was added is always maybe", added_items_are_always_maybe),
            Test::new("items that were never added are mostly not present", absent_items_are_mostly_absent),
            Test::new("a false positive rate inside the theoretical bound", rate_inside_the_bound),
            Test::new("set bits grow with insertions and never exceed the filter", set_bits_grow),
            Test::new("a filter with too few bits is useless but never loses an item", a_tiny_filter_still_holds_everything),
            Test::new("init a second time resets the filter", init_resets),
            Test::new("adding the same item twice changes nothing", adding_twice_changes_nothing).ext(),
            Test::new("the optimal number of hashes beats a badly chosen one", optimal_k_wins).ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example(
            "A filter that is certain about what it holds",
            "bloom",
            || {
                lines(&[
                    "init 1024 3",
                    "add alpha",
                    "add beta",
                    "contains alpha",
                    "contains gamma",
                    "stats",
                ])
            },
        )
        .request(
            "a 1024-bit filter with three positions per item, two items added, two asked about",
        )
        .response(
            "`maybe` for the item that was added; almost certainly `no` for the one that was not",
        )
        .note(
            "`contains` has two answers and neither of them is `yes`. `maybe` means every one \
             of the k bits is set, which an item that was never added can also manage once \
             the filter is full enough. `no` is the only certain answer a Bloom filter gives.",
        ),
        prim_example("What the filter will admit to", "bloom", || {
            lines(&["init 64 4", "add one", "add two", "add three", "stats"])
        })
        .request("three items in a filter far too small for them, then its own accounting")
        .response("around a dozen of the sixty-four bits set, and `items` counting the adds")
        .note(
            "set_bits is the honest measure of how full a filter is: it rises by at most k \
             per add and by less once positions start colliding. When it approaches bits, \
             every `contains` answers `maybe` and the filter has stopped saying anything.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Shared moves
// ---------------------------------------------------------------------------------------

/// Add `n` items named `<tag>-0` .. `<tag>-<n-1>`.
async fn add_items(p: &mut PrimProc, tag: &str, n: usize) -> Result<(), Failure> {
    for i in 0..n {
        let cmd = format!("add {tag}-{i}");
        let v = p.send(&cmd).await?;
        if v.get("ok").is_none() {
            return Err(p.shape(&cmd, "an add answers {\"ok\": true}"));
        }
    }
    Ok(())
}

/// Probe `n` items that were never added and count how many came back `maybe`.
async fn probe(p: &mut PrimProc, tag: &str, n: usize) -> Result<usize, Failure> {
    let mut hits = 0;
    for i in 0..n {
        let cmd = format!("contains {tag}-{i}");
        let v = p.send(&cmd).await?;
        if p.expect_bool(&v, &cmd, "maybe")? {
            hits += 1;
        }
    }
    Ok(hits)
}

/// Ask for every added item and hand back the ones the filter has forgotten.
async fn missing(p: &mut PrimProc, tag: &str, n: usize) -> Result<Vec<usize>, Failure> {
    let mut lost = Vec::new();
    for i in 0..n {
        let cmd = format!("contains {tag}-{i}");
        let v = p.send(&cmd).await?;
        if !p.expect_bool(&v, &cmd, "maybe")? {
            lost.push(i);
        }
    }
    Ok(lost)
}

/// Fill a fresh filter and measure its false-positive rate over `probes` absent items.
///
/// Returns `(measured rate, theoretical rate)`. The two tags never overlap, so every probe
/// really is an item the filter was never shown, and both carry the run's seed so that a
/// pass is not luck with one particular set of strings.
async fn measure(
    p: &mut PrimProc,
    seed: u64,
    bits: i64,
    hashes: i64,
    inserted: usize,
    probes: usize,
) -> Result<(f64, f64), Failure> {
    p.send(&format!("init {bits} {hashes}")).await?;
    add_items(p, &format!("in{seed:x}"), inserted).await?;
    let hits = probe(p, &format!("out{seed:x}"), probes).await?;
    let measured = hits as f64 / probes as f64;
    let theory = oracles::bloom_fpr(bits as f64, hashes as f64, inserted as f64);
    Ok((measured, theory))
}

/// The bound a measured rate must sit inside: the published formula, times a slack factor,
/// plus an absolute allowance because one filter is one sample.
fn allowed(theory: f64) -> f64 {
    theory * 2.5 + 0.01
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(init_reports_the_size, |ctx| {
    let p = ctx.prim("bloom").await?;
    let init = p.send("init 4096 4").await?;
    let bits = p.expect_i64(&init, "init 4096 4", "bits")?;
    let hashes = p.expect_i64(&init, "init 4096 4", "hashes")?;
    let stats = p.send("stats").await?;
    let s_bits = p.expect_i64(&stats, "stats", "bits")?;
    let s_hashes = p.expect_i64(&stats, "stats", "hashes")?;
    let s_set = p.expect_i64(&stats, "stats", "set_bits")?;
    let s_items = p.expect_i64(&stats, "stats", "items")?;
    let second = p.send("init 1024 7").await?;
    let b2 = p.expect_i64(&second, "init 1024 7", "bits")?;
    let h2 = p.expect_i64(&second, "init 1024 7", "hashes")?;

    let mut c = Check::new("what init and stats say about an untouched filter");
    c.eq("init.bits", 4096, bits);
    c.eq("init.hashes", 4, hashes);
    c.eq("stats.bits", 4096, s_bits);
    c.eq("stats.hashes", 4, s_hashes);
    c.eq("stats.set_bits", 0, s_set);
    c.eq("stats.items", 0, s_items);
    c.eq("init.bits (second)", 1024, b2);
    c.eq("init.hashes (second)", 7, h2);
    c.finish()
});

dist_test!(added_items_are_always_maybe, |ctx| {
    let p = ctx.prim("bloom").await?;
    p.send("init 4096 4").await?;
    add_items(p, "item", 200).await?;
    let lost = missing(p, "item", 200).await?;

    let mut c = Check::new("a filter answering for the items it holds");
    c.that(
        "contains(added)",
        "every added item to answer maybe",
        lost.is_empty(),
        lost,
    );
    c.note("a false negative means a bit was not set, or a position was computed differently on add and on contains");
    c.finish()
});

dist_test!(absent_items_are_mostly_absent, |ctx| {
    let seed = ctx.seed;
    let p = ctx.prim("bloom").await?;
    let (measured, theory) = measure(p, seed, 65_536, 6, 500, 2_000).await?;

    let mut c = Check::new("a filter with far more bits than it needs");
    c.observe("bloom.false_positive_rate", measured);
    c.observe("bloom.theoretical_rate", theory);
    c.at_most("bloom.false_positive_rate", 0.02, measured);
    c.note(format!(
        "500 items in 65536 bits with 6 positions: the formula predicts {theory:.2e}, so \
         anything above a rounding error means the positions are not spread over the filter"
    ));
    c.finish()?;
    ctx.note(format!(
        "65536 bits, 6 hashes, 500 items: measured {measured:.4}, predicted {theory:.2e}"
    ));
    Ok(())
});

dist_test!(rate_inside_the_bound, |ctx| {
    let seed = ctx.seed;
    let p = ctx.prim("bloom").await?;
    let tight = measure(p, seed, 4_096, 4, 700, 3_000).await?;
    let roomy = measure(p, seed, 16_384, 6, 1_200, 3_000).await?;

    let mut c = Check::new("the measured false-positive rate against the published formula");
    for (label, (measured, theory)) in [("4096/4/700", tight), ("16384/6/1200", roomy)] {
        c.observe(&format!("bloom[{label}].measured"), measured);
        c.observe(&format!("bloom[{label}].predicted"), theory);
        c.at_most(
            &format!("bloom[{label}].false_positive_rate"),
            allowed(theory),
            measured,
        );
    }
    c.note("the bound is (1 - e^(-kn/m))^k times 2.5, plus 0.01, because 3000 probes of one filter is a single noisy sample");
    c.note(format!(
        "the items and the probes are derived from --seed {seed}"
    ));
    c.finish()?;
    ctx.note(format!(
        "4096/4/700: measured {:.4} against {:.4}; 16384/6/1200: measured {:.4} against {:.4}",
        tight.0, tight.1, roomy.0, roomy.1
    ));
    Ok(())
});

dist_test!(set_bits_grow, |ctx| {
    let p = ctx.prim("bloom").await?;
    p.send("init 8192 5").await?;
    let mut counts = Vec::new();
    let mut at = 0usize;
    for target in [0usize, 50, 100, 200, 400] {
        while at < target {
            let cmd = format!("add grow-{at}");
            p.send(&cmd).await?;
            at += 1;
        }
        let stats = p.send("stats").await?;
        let set = p.expect_i64(&stats, "stats", "set_bits")?;
        let items = p.expect_i64(&stats, "stats", "items")?;
        counts.push((target, set, items));
    }

    let mut c = Check::new("how a filter fills up");
    for (target, set, items) in &counts {
        c.observe(&format!("stats.set_bits after {target} adds"), set);
        c.at_most(&format!("stats.set_bits after {target} adds"), 8192, *set);
        c.eq(
            &format!("stats.items after {target} adds"),
            *target as i64,
            *items,
        );
        // Each add sets at most k positions, and never fewer than one.
        c.at_most(
            &format!("stats.set_bits <= 5 * items after {target} adds"),
            5 * *target as i64,
            *set,
        );
    }
    for w in counts.windows(2) {
        c.that(
            &format!("stats.set_bits between {} and {} adds", w[0].0, w[1].0),
            "more bits set after more insertions",
            w[1].1 > w[0].1,
            (w[0].1, w[1].1),
        );
    }
    c.finish()
});

dist_test!(a_tiny_filter_still_holds_everything, |ctx| {
    let seed = ctx.seed;
    let p = ctx.prim("bloom").await?;
    p.send("init 32 3").await?;
    add_items(p, "cram", 60).await?;
    let lost = missing(p, "cram", 60).await?;
    let stats = p.send("stats").await?;
    let set = p.expect_i64(&stats, "stats", "set_bits")?;
    let hits = probe(p, &format!("never{seed:x}"), 200).await?;
    let rate = hits as f64 / 200.0;

    let mut c = Check::new("a filter with 32 bits and 60 items in it");
    c.that(
        "contains(added)",
        "every added item to answer maybe even in a saturated filter",
        lost.is_empty(),
        lost,
    );
    c.at_most("stats.set_bits", 32, set);
    c.observe("bloom.false_positive_rate", rate);
    c.note("a useless filter is allowed; a lossy one is not, and this is the case where the two are told apart");
    c.finish()?;
    ctx.note(format!(
        "32 bits with 60 items: {set} bits set, false-positive rate {rate:.2}"
    ));
    Ok(())
});

dist_test!(init_resets, |ctx| {
    let p = ctx.prim("bloom").await?;
    p.send("init 4096 4").await?;
    add_items(p, "before", 300).await?;
    let before = p.send("stats").await?;
    let before_set = p.expect_i64(&before, "stats", "set_bits")?;
    p.send("init 4096 4").await?;
    let after = p.send("stats").await?;
    let after_set = p.expect_i64(&after, "stats", "set_bits")?;
    let after_items = p.expect_i64(&after, "stats", "items")?;
    let survivors = 300 - missing(p, "before", 300).await?.len();

    let mut c = Check::new("init on a filter that already holds something");
    c.that(
        "stats.set_bits before the second init",
        "a filter with bits set",
        before_set > 0,
        before_set,
    );
    c.eq("stats.set_bits after the second init", 0, after_set);
    c.eq("stats.items after the second init", 0, after_items);
    c.eq("contains(old items) after the second init", 0, survivors);
    c.note("with no bits set nothing can answer maybe, so a single survivor means init reset the counters but not the bits");
    c.finish()
});

dist_test!(adding_twice_changes_nothing, |ctx| {
    let p = ctx.prim("bloom").await?;
    p.send("init 4096 4").await?;
    add_items(p, "dup", 100).await?;
    let once = p.send("stats").await?;
    let once_set = p.expect_i64(&once, "stats", "set_bits")?;
    add_items(p, "dup", 100).await?;
    let twice = p.send("stats").await?;
    let twice_set = p.expect_i64(&twice, "stats", "set_bits")?;
    let lost = missing(p, "dup", 100).await?;

    let mut c = Check::new("the same hundred items added a second time");
    c.eq("stats.set_bits after re-adding", once_set, twice_set);
    c.that(
        "contains(added)",
        "every item still present",
        lost.is_empty(),
        lost,
    );
    c.note("a Bloom filter is a set of bits, so an add is idempotent in the bits even if `items` counts the call");
    c.finish()
});

dist_test!(optimal_k_wins, |ctx| {
    let bits = 4_096i64;
    let inserted = 500usize;
    let best = oracles::bloom_optimal_k(bits as f64, inserted as f64).round() as i64;
    let worst = 20i64;
    let seed = ctx.seed;
    let p = ctx.prim("bloom").await?;
    let good = measure(p, seed, bits, best, inserted, 3_000).await?;
    let bad = measure(p, seed, bits, worst, inserted, 3_000).await?;

    let mut c = Check::new("the number of hash positions that minimises the rate");
    c.observe(&format!("bloom[k={best}].measured"), good.0);
    c.observe(&format!("bloom[k={worst}].measured"), bad.0);
    c.observe(&format!("bloom[k={best}].predicted"), good.1);
    c.observe(&format!("bloom[k={worst}].predicted"), bad.1);
    c.at_most(
        &format!("bloom[k={best}].false_positive_rate"),
        allowed(good.1),
        good.0,
    );
    c.that(
        "bloom[optimal k].false_positive_rate < bloom[k=20].false_positive_rate",
        "the optimal k to be the better of the two by a clear margin",
        good.0 < bad.0,
        (good.0, bad.0),
    );
    c.note(format!(
        "m/n * ln 2 is {best} here; 20 positions per item fills the same 4096 bits four \
         times faster and the rate gets worse, not better"
    ));
    c.finish()?;
    ctx.note(format!(
        "k={best} measured {:.4} (predicted {:.4}); k={worst} measured {:.4} (predicted {:.4})",
        good.0, good.1, bad.0, bad.1
    ));
    Ok(())
});
