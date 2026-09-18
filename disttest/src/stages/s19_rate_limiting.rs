//! Stage 19 — Token bucket and leaky bucket.
//!
//! Two limiters that look alike and answer different questions. A token bucket asks "may
//! this go now", and lets a burst through after an idle period because the tokens were
//! there all along. A leaky bucket asks "is there room for this", and smooths a burst into
//! a steady drain. Both are three lines of arithmetic, and both are wrong in the same two
//! ways: a refill that is computed from the program's own clock, and one that is truncated
//! to whole tokens.
//!
//! The oracles here are exact. [`TokenBucket`] and [`LeakyBucket`] are the closed form the
//! specification pins down, so every test drives the program and the oracle side by side
//! and compares both the verdict and the level after it, one command at a time. A limiter
//! that drifts by a thousandth of a token is caught on the step it drifts.

use crate::assert::{Check, Failure};
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::prim::oracles::{LeakyBucket, TokenBucket};
use crate::prim::PrimProc;
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;

/// Stage 19.
pub fn stage() -> Stage {
    Stage {
        number: 19,
        slug: "rate_limiting",
        name: "Token bucket and leaky bucket",
        ext: false,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `token-bucket`: refill rate * elapsed, capped at the burst, then spend",
            "Topic `leaky-bucket`: drain rate * elapsed, then refuse whatever would overflow",
            "Time arrives with every command: never read a clock of your own",
            "A long idle period fills the token bucket to the burst and no further",
        ],
        examples,
        tests: vec![
            Test::new(
                "a full bucket allows exactly the burst at time zero",
                the_burst_and_no_more,
            ),
            Test::new(
                "the bucket refills at its rate and no faster",
                it_refills_at_its_rate,
            ),
            Test::new(
                "a bucket that has idled holds exactly the burst",
                an_idle_bucket_holds_the_burst,
            ),
            Test::new(
                "a seeded sequence of takes matches the oracle step by step",
                a_seeded_sequence_of_takes,
            ),
            Test::new(
                "fractional refills accumulate instead of being truncated",
                fractions_accumulate,
            ),
            Test::new(
                "the same instant twice means no time has passed",
                the_same_instant_twice,
            ),
            Test::new(
                "a timestamp that goes backwards adds no tokens",
                time_that_goes_backwards,
            )
            .ext(),
            Test::new(
                "the leaky bucket refuses what would overflow and takes it once it has drained",
                the_leaky_bucket_refuses_the_overflow,
            ),
            Test::new(
                "a seeded sequence of offers matches the leaky bucket oracle step by step",
                a_seeded_sequence_of_offers,
            ),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("A burst, then the refill", "token-bucket", || {
            lines(&[
                "init 10 5",
                "take 0 5",
                "take 0 1",
                "take 100 1",
                "take 100 1",
                "take 100000 5",
            ])
        })
        .request("ten tokens a second with a burst of five, spent at 0 ms, 100 ms and much later")
        .response("the burst, then a refusal, then one token per 100 ms, then the burst again")
        .note(
            "100 ms at ten a second is exactly one token, and the take at 100 000 ms finds \
             five rather than a thousand: the cap is what makes a bucket a bucket. Note \
             that the two takes at 100 ms answer differently — the clock has not moved, so \
             nothing has been added between them.",
        ),
        prim_example(
            "A queue that drains rather than fills",
            "leaky-bucket",
            || {
                lines(&[
                    "init 10 3",
                    "offer 0 3",
                    "offer 0 1",
                    "offer 200 2",
                    "offer 200 2",
                ])
            },
        )
        .request("a queue of three draining at ten a second, offered work at 0 ms and 200 ms")
        .response(
            "three accepted, the fourth refused, then two accepted once 200 ms have drained \
             two — and nothing more at that same instant",
        )
        .note(
            "The leaky bucket refuses rather than delays: there is no queue of pending \
             answers here, only a level that says whether the next unit fits. Accepting \
             work and reporting a queue longer than the capacity is the mistake to watch \
             for.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Driving the program and the oracle together
// ---------------------------------------------------------------------------------------

/// How far a reported level may sit from the oracle's. Anything larger is a different
/// calculation, not rounding.
const TOLERANCE: f64 = 1e-6;

/// One `take`, checked against the oracle: the verdict and the level it leaves behind.
async fn take(
    p: &mut PrimProc,
    c: &mut Check,
    oracle: &mut TokenBucket,
    label: &str,
    t_ms: i64,
    n: i64,
) -> Result<bool, Failure> {
    let command = format!("take {t_ms} {n}");
    let v = p.send(&command).await?;
    let allowed = p.expect_bool(&v, &command, "allowed")?;
    let tokens = p.expect_f64(&v, &command, "tokens")?;
    let expected = oracle.take(t_ms as f64, n as f64);
    c.eq(&format!("{label}.allowed"), expected, allowed);
    c.within(
        &format!("{label}.tokens"),
        oracle.tokens - TOLERANCE,
        oracle.tokens + TOLERANCE,
        tokens,
    );
    Ok(allowed)
}

/// One `offer`, checked against the oracle the same way.
async fn offer(
    p: &mut PrimProc,
    c: &mut Check,
    oracle: &mut LeakyBucket,
    label: &str,
    t_ms: i64,
    n: i64,
) -> Result<bool, Failure> {
    let command = format!("offer {t_ms} {n}");
    let v = p.send(&command).await?;
    let accepted = p.expect_bool(&v, &command, "accepted")?;
    let queue = p.expect_f64(&v, &command, "queue")?;
    let expected = oracle.offer(t_ms as f64, n as f64);
    c.eq(&format!("{label}.accepted"), expected, accepted);
    c.within(
        &format!("{label}.queue"),
        oracle.queued - TOLERANCE,
        oracle.queued + TOLERANCE,
        queue,
    );
    Ok(accepted)
}

/// Start a token bucket, and the oracle that shadows it.
async fn token_bucket(p: &mut PrimProc, rate: f64, burst: f64) -> Result<TokenBucket, Failure> {
    p.send(&format!("init {rate} {burst}")).await?;
    Ok(TokenBucket::new(rate, burst))
}

/// Start a leaky bucket, and the oracle that shadows it.
async fn leaky_bucket(p: &mut PrimProc, rate: f64, capacity: f64) -> Result<LeakyBucket, Failure> {
    p.send(&format!("init {rate} {capacity}")).await?;
    Ok(LeakyBucket::new(rate, capacity))
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(the_burst_and_no_more, |ctx| {
    let p = ctx.prim("token-bucket").await?;
    let mut oracle = token_bucket(p, 10.0, 5.0).await?;
    let mut c = Check::new("a full bucket at time zero");
    let mut allowed = Vec::new();
    for i in 0..7 {
        allowed.push(take(p, &mut c, &mut oracle, &format!("take[{i}]"), 0, 1).await?);
    }
    // Five and only five: no time has passed, so there is nothing to add to the bucket.
    c.eq(
        "the takes that were allowed at t=0",
        5,
        allowed.iter().filter(|a| **a).count(),
    );
    c.eq(
        "the first five takes",
        vec![true, true, true, true, true],
        allowed[..5].to_vec(),
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(it_refills_at_its_rate, |ctx| {
    let p = ctx.prim("token-bucket").await?;
    let mut oracle = token_bucket(p, 10.0, 5.0).await?;
    let mut c = Check::new("a bucket refilling at ten tokens a second");
    take(p, &mut c, &mut oracle, "the burst", 0, 5).await?;
    // Half a token, then just under a whole one: ten a second and not one faster.
    take(p, &mut c, &mut oracle, "take at 50 ms", 50, 1).await?;
    take(p, &mut c, &mut oracle, "take at 99 ms", 99, 1).await?;
    take(p, &mut c, &mut oracle, "take at 150 ms", 150, 1).await?;
    take(p, &mut c, &mut oracle, "the second take at 150 ms", 150, 1).await?;
    take(p, &mut c, &mut oracle, "take at 1000 ms", 1000, 5).await?;
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(an_idle_bucket_holds_the_burst, |ctx| {
    let p = ctx.prim("token-bucket").await?;
    let mut oracle = token_bucket(p, 10.0, 4.0).await?;
    let mut c = Check::new("a bucket that has been idle for a thousand seconds");
    take(p, &mut c, &mut oracle, "the burst", 0, 4).await?;
    // A thousand seconds at ten a second is ten thousand tokens, and the bucket holds four.
    let big = take(p, &mut c, &mut oracle, "the burst again", 1_000_000, 4).await?;
    let more = take(p, &mut c, &mut oracle, "one more", 1_000_000, 1).await?;
    c.eq("the take of a whole burst after an idle period", true, big);
    c.eq("one token past the burst", false, more);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_seeded_sequence_of_takes, |ctx| {
    // Forty takes at seeded instants and sizes, each compared with the oracle before the
    // next one is sent: a divergence is reported on the step it happens, not at the end.
    let steps: Vec<(i64, i64)> = (0..40)
        .map(|_| (ctx.rng.random_range(0..4000), ctx.rng.random_range(1..4)))
        .collect();
    let seed = ctx.seed;
    let p = ctx.prim("token-bucket").await?;
    let mut oracle = token_bucket(p, 7.0, 6.0).await?;
    let mut c = Check::new("a seeded sequence of takes against the oracle");
    c.note(format!("seed {seed}"));
    let mut t = 0i64;
    for (step, (gap, n)) in steps.into_iter().enumerate() {
        t += gap;
        take(p, &mut c, &mut oracle, &format!("take[{step}]"), t, n).await?;
    }
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(fractions_accumulate, |ctx| {
    let p = ctx.prim("token-bucket").await?;
    let mut oracle = token_bucket(p, 10.0, 5.0).await?;
    let mut c = Check::new("three tenths of a token, four times over");
    take(p, &mut c, &mut oracle, "the burst", 0, 5).await?;
    // Each step adds 0.3 of a token. A refill that truncates to whole tokens adds nothing
    // at any of these instants, and the bucket never refills again.
    let mut allowed = Vec::new();
    for t in [30, 60, 90, 120] {
        allowed.push(take(p, &mut c, &mut oracle, &format!("take at {t} ms"), t, 1).await?);
    }
    c.eq(
        "the takes at 30, 60, 90 and 120 ms",
        vec![false, false, false, true],
        allowed,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_same_instant_twice, |ctx| {
    let p = ctx.prim("token-bucket").await?;
    // A thousand tokens a second: a program that refills from a clock of its own gains a
    // token for every millisecond it spends answering, and these commands all claim to
    // happen at the same instant.
    let mut oracle = token_bucket(p, 1000.0, 10.0).await?;
    let mut c = Check::new("ten commands that all claim the same instant");
    take(p, &mut c, &mut oracle, "the burst", 5000, 10).await?;
    let mut allowed = Vec::new();
    for i in 0..8 {
        allowed.push(take(p, &mut c, &mut oracle, &format!("repeat[{i}]"), 5000, 1).await?);
    }
    c.that(
        "the takes at the same instant as the burst",
        "every one refused, because no time has passed",
        allowed.iter().all(|a| !*a),
        allowed.clone(),
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(time_that_goes_backwards, |ctx| {
    let p = ctx.prim("token-bucket").await?;
    let mut oracle = token_bucket(p, 10.0, 5.0).await?;
    let mut c = Check::new("a timestamp older than the last one");
    take(p, &mut c, &mut oracle, "the burst at 1000 ms", 1000, 5).await?;
    // Elapsed time is never negative, so a step backwards neither adds nor removes tokens.
    let backwards = take(p, &mut c, &mut oracle, "a take at 500 ms", 500, 1).await?;
    take(p, &mut c, &mut oracle, "a take at 1000 ms", 1000, 1).await?;
    c.eq("the take at 500 ms, after one at 1000 ms", false, backwards);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_leaky_bucket_refuses_the_overflow, |ctx| {
    let p = ctx.prim("leaky-bucket").await?;
    let mut oracle = leaky_bucket(p, 10.0, 3.0).await?;
    let mut c = Check::new("a queue of three draining at ten a second");
    let fills = offer(p, &mut c, &mut oracle, "offer 3 at 0 ms", 0, 3).await?;
    let overflow = offer(p, &mut c, &mut oracle, "offer 1 more at 0 ms", 0, 1).await?;
    // 200 ms at ten a second drains two, and two is what now fits.
    let after_draining = offer(p, &mut c, &mut oracle, "offer 2 at 200 ms", 200, 2).await?;
    let again = offer(p, &mut c, &mut oracle, "offer 1 at 200 ms", 200, 1).await?;
    let emptied = offer(p, &mut c, &mut oracle, "offer 3 at 500 ms", 500, 3).await?;
    c.eq("the first offer of a whole capacity", true, fills);
    c.eq("the offer that would overflow", false, overflow);
    c.eq(
        "the offer that fits what drained away",
        true,
        after_draining,
    );
    c.eq("the offer that would overflow again", false, again);
    c.eq("the offer once the queue has emptied", true, emptied);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_seeded_sequence_of_offers, |ctx| {
    let steps: Vec<(i64, i64)> = (0..30)
        .map(|_| (ctx.rng.random_range(0..500), ctx.rng.random_range(1..5)))
        .collect();
    let seed = ctx.seed;
    let p = ctx.prim("leaky-bucket").await?;
    let mut oracle = leaky_bucket(p, 20.0, 8.0).await?;
    let mut c = Check::new("a seeded sequence of offers against the oracle");
    c.note(format!("seed {seed}"));
    let mut t = 0i64;
    for (step, (gap, n)) in steps.into_iter().enumerate() {
        t += gap;
        offer(p, &mut c, &mut oracle, &format!("offer[{step}]"), t, n).await?;
    }
    c.block("transcript", p.transcript_block());
    c.finish()
});
