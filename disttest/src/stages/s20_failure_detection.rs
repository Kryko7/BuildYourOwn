//! Stage 20 — Backoff, phi-accrual and SWIM suspicion.
//!
//! Three small pieces of timing that every later stage leans on: how long to wait before
//! retrying, how suspicious to be of silence, and when to stop waiting altogether. All
//! three take their time and their randomness as arguments, which is the only reason a
//! failure detector can be tested at all — the same script has to give the same answers
//! twice, on a slow machine as on a fast one.
//!
//! Two of the oracles are exact: [`full_jitter`] is the published formula, and
//! [`suspicion`] is two comparisons. Phi is not, because there is more than one reasonable
//! distribution to fit to the heartbeat intervals, so this stage asserts the shape of the
//! curve instead — near zero while the heartbeats keep coming, over four after ten silent
//! intervals, never falling as the silence grows — and puts the measured values in the
//! report either way.

use crate::assert::{Check, Failure};
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::prim::oracles::{full_jitter, suspicion};
use crate::prim::PrimProc;
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;

/// Stage 20.
pub fn stage() -> Stage {
    Stage {
        number: 20,
        slug: "failure_detection",
        name: "Backoff, phi-accrual and SWIM suspicion",
        ext: true,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `backoff`: full jitter is uniform(0, min(cap, base * 2^attempt))",
            "Topic `phi-accrual`: phi rises with the time since the last heartbeat, smoothly",
            "Topic `swim`: alive, then suspect, then dead — two deadlines, both sharp",
            "The sampled value arrives with the command, so a jittered delay is still reproducible",
        ],
        examples,
        tests: vec![
            Test::new(
                "a sweep of attempts and samples matches the full jitter formula",
                the_jitter_formula,
            ),
            Test::new(
                "the cap holds however many attempts have failed",
                the_cap_holds,
            ),
            Test::new(
                "a seeded sample is never negative and never above the ceiling",
                a_delay_stays_inside_its_ceiling,
            )
            .ext(),
            Test::new("phi never falls as the silence grows", phi_never_falls),
            Test::new(
                "phi stays well under one while the heartbeats keep coming",
                phi_is_small_after_one_interval,
            ),
            Test::new(
                "phi passes four once ten intervals have gone by",
                phi_is_large_after_ten_intervals,
            ),
            Test::new(
                "swim moves from alive to suspect to dead as the oracle says",
                swim_matches_the_oracle,
            ),
            Test::new(
                "both deadlines are sharp at the exact instant they fall",
                the_deadlines_are_sharp,
            ),
            Test::new(
                "an ack moves a suspect node back to alive",
                an_ack_revives_a_suspect,
            ),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example(
            "A jittered delay, and where the cap bites",
            "backoff",
            || {
                lines(&[
                    "init 100 1000",
                    "next 0 1",
                    "next 3 1",
                    "next 5 1",
                    "next 3 0.5",
                    "next 3 0",
                ])
            },
        )
        .request("a 100 ms base capped at a second, sampled at four attempts")
        .response("100, 800, 1000 — the cap — then 400 and 0 for the smaller samples")
        .note(
            "The sample `u` arrives with the command rather than being drawn inside, so a \
             jittered delay is still exactly reproducible. A real sample comes from [0, 1); \
             the 1 here is only to show where the ceiling is. Full jitter spreads over the \
             whole interval from zero, and a retry that always waits the ceiling is not \
             jittered at all — a fleet of them then retries in lockstep.",
        ),
        prim_example("Silence, and how suspicious it is", "phi-accrual", || {
            lines(&[
                "heartbeat 0",
                "heartbeat 1000",
                "heartbeat 2000",
                "phi 2000",
                "phi 3000",
                "phi 12000",
            ])
        })
        .request("a heartbeat a second, then phi asked at the last beat, a second on, and ten")
        .response("phi near zero, then a fraction, then a number well past four")
        .note(
            "Phi is not a verdict but a number: how unlikely this much silence is, on a log \
             scale, given how the heartbeats have been arriving. A detector that compares \
             the gap with a fixed timeout cannot adapt to a link that is simply slower.",
        ),
        prim_example("Two deadlines from the last ack", "swim", || {
            lines(&[
                "init 1000 3000",
                "ack n2 0",
                "status n2 999",
                "status n2 1000",
                "status n2 3000",
                "ack n2 3000",
                "status n2 3100",
            ])
        })
        .request(
            "a node acked at 0 ms, asked about either side of both deadlines, then acked again",
        )
        .response("alive, suspect, dead — and alive again once the ack lands")
        .note(
            "Both deadlines are measured from the last ack and both are sharp: at exactly \
             1000 ms the node is already suspect. Suspicion is not a verdict of death, \
             which is why the third state exists at all.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Asking the program
// ---------------------------------------------------------------------------------------

/// How far a delay may sit from the formula. Anything larger is a different calculation.
const TOLERANCE: f64 = 1e-6;

/// One backoff delay.
async fn delay(p: &mut PrimProc, attempt: u32, u: f64) -> Result<f64, Failure> {
    let command = format!("next {attempt} {u}");
    let v = p.send(&command).await?;
    p.expect_f64(&v, &command, "delay")
}

/// Phi at one instant.
async fn phi(p: &mut PrimProc, t_ms: i64) -> Result<f64, Failure> {
    let command = format!("phi {t_ms}");
    let v = p.send(&command).await?;
    p.expect_f64(&v, &command, "phi")
}

/// What a node's state is said to be at one instant.
async fn state(p: &mut PrimProc, node: &str, t_ms: i64) -> Result<String, Failure> {
    let command = format!("status {node} {t_ms}");
    let v = p.send(&command).await?;
    p.expect_str(&v, &command, "state")
}

/// Send a heartbeat and read the number of samples the detector says it holds.
async fn heartbeat(p: &mut PrimProc, t_ms: i64) -> Result<i64, Failure> {
    let command = format!("heartbeat {t_ms}");
    let v = p.send(&command).await?;
    p.expect_i64(&v, &command, "samples")
}

/// A heartbeat a second, from 0 to `beats - 1` seconds; the last one is at the instant
/// returned.
async fn heartbeat_train(p: &mut PrimProc, beats: i64) -> Result<(i64, Vec<i64>), Failure> {
    let mut samples = Vec::new();
    let mut last = 0;
    for beat in 0..beats {
        last = beat * 1000;
        samples.push(heartbeat(p, last).await?);
    }
    Ok((last, samples))
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(the_jitter_formula, |ctx| {
    let base = 100.0;
    let cap = 10_000.0;
    let p = ctx.prim("backoff").await?;
    p.send("init 100 10000").await?;
    let mut c = Check::new("full jitter over a sweep of attempts and samples");
    // The sample is inclusive at zero and exclusive at one, so both ends are asked for:
    // zero must give exactly nothing, and a sample just short of one the whole ceiling.
    for u in [0.0, 0.25, 0.5, 0.75, 0.999_999] {
        for attempt in 0..8u32 {
            let got = delay(p, attempt, u).await?;
            let want = full_jitter(base, cap, attempt, u);
            c.within(
                &format!("next({attempt}, {u}).delay"),
                want - TOLERANCE,
                want + TOLERANCE,
                got,
            );
        }
    }
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_cap_holds, |ctx| {
    let base = 50.0;
    let cap = 400.0;
    let u = 0.999_999;
    let p = ctx.prim("backoff").await?;
    p.send("init 50 400").await?;
    let mut c = Check::new("a base that doubles past its cap");
    // 50, 100, 200, then 400 for ever: doubling without a cap reaches half a minute by
    // the twelfth attempt, which is how a retry storm becomes an outage.
    for attempt in 0..12u32 {
        let got = delay(p, attempt, u).await?;
        let want = full_jitter(base, cap, attempt, u);
        c.within(
            &format!("next({attempt}, {u}).delay"),
            want - TOLERANCE,
            want + TOLERANCE,
            got,
        );
        c.at_most(&format!("next({attempt}, {u}).delay <= cap"), cap, got);
    }
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_delay_stays_inside_its_ceiling, |ctx| {
    let base = 20.0f64;
    let cap = 5_000.0f64;
    let samples: Vec<(u32, f64)> = (0..24)
        .map(|_| {
            (
                ctx.rng.random_range(0..12u32),
                ctx.rng.random_range(0.0..1.0f64),
            )
        })
        .collect();
    let seed = ctx.seed;
    let p = ctx.prim("backoff").await?;
    p.send("init 20 5000").await?;
    let mut c = Check::new("seeded samples of a jittered delay");
    c.note(format!("seed {seed}"));
    for (attempt, u) in samples {
        let got = delay(p, attempt, u).await?;
        let ceiling = cap.min(base * 2f64.powi(attempt as i32));
        c.within(
            &format!("next({attempt}, {u:.6}).delay"),
            -TOLERANCE,
            ceiling + TOLERANCE,
            got,
        );
        let want = full_jitter(base, cap, attempt, u);
        c.within(
            &format!("next({attempt}, {u:.6}).delay against the formula"),
            want - TOLERANCE,
            want + TOLERANCE,
            got,
        );
    }
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(phi_never_falls, |ctx| {
    let p = ctx.prim("phi-accrual").await?;
    let (last, samples) = heartbeat_train(p, 11).await?;
    let mut measured = Vec::new();
    for step in 0..=10 {
        measured.push((step, phi(p, last + step * 1000).await?));
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("phi as the silence lengthens");
    for pair in measured.windows(2) {
        let ((before, earlier), (after, later)) = (pair[0], pair[1]);
        c.that(
            &format!("phi({after} intervals) >= phi({before} intervals)"),
            "a suspicion that does not fall while nothing is heard",
            later + TOLERANCE >= earlier,
            (earlier, later),
        );
    }
    c.that(
        "phi(0 intervals)",
        "a phi at or above zero at the instant of the last heartbeat",
        measured[0].1 >= -TOLERANCE,
        measured[0].1,
    );
    // Eleven heartbeats are ten intervals; a detector may count either.
    c.within("heartbeat.samples after 11 beats", 10, 11, samples[10]);
    c.that(
        "heartbeat.samples",
        "a sample count that never falls",
        samples.windows(2).all(|w| w[1] >= w[0]),
        samples.clone(),
    );
    c.observe("phi per interval of silence", format!("{measured:?}"));
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(phi_is_small_after_one_interval, |ctx| {
    let p = ctx.prim("phi-accrual").await?;
    // Ten intervals of exactly a second, so one interval of silence is entirely ordinary.
    let (last, _) = heartbeat_train(p, 11).await?;
    let at_the_beat = phi(p, last).await?;
    let one_interval = phi(p, last + 1000).await?;
    let transcript = p.transcript_block();
    ctx.note(format!(
        "phi at the last heartbeat {at_the_beat:.3}, one interval later {one_interval:.3}"
    ));
    let mut c = Check::new("phi one interval after a regular heartbeat train");
    c.within(
        "phi at the instant of the last heartbeat",
        0.0,
        1.0,
        at_the_beat,
    );
    c.within("phi one interval later", 0.0, 1.0, one_interval);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(phi_is_large_after_ten_intervals, |ctx| {
    let p = ctx.prim("phi-accrual").await?;
    let (last, _) = heartbeat_train(p, 11).await?;
    let ten_intervals = phi(p, last + 10_000).await?;
    let transcript = p.transcript_block();
    ctx.note(format!(
        "phi ten intervals after the last heartbeat {ten_intervals:.3}"
    ));
    let mut c = Check::new("phi ten intervals after the last heartbeat");
    // Ten times the usual gap is not a slow link, and a detector that still reports a
    // fraction here will never raise a suspicion at all.
    c.at_least("phi ten intervals later", 4.0, ten_intervals);
    c.that(
        "phi ten intervals later",
        "a finite number",
        ten_intervals.is_finite(),
        ten_intervals,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(swim_matches_the_oracle, |ctx| {
    let suspect_ms = 1000;
    let dead_ms = 3000;
    let p = ctx.prim("swim").await?;
    p.send("init 1000 3000").await?;
    p.send("ack n1 0").await?;
    let mut c = Check::new("a node's state on both sides of both deadlines");
    for t in [0, 1, 500, 999, 1000, 1001, 2000, 2999, 3000, 3001, 9000] {
        let got = state(p, "n1", t).await?;
        let want = suspicion(t, suspect_ms, dead_ms).as_str().to_string();
        c.eq(&format!("status(n1, {t}).state"), want, got);
    }
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_deadlines_are_sharp, |ctx| {
    let p = ctx.prim("swim").await?;
    p.send("init 1000 3000").await?;
    // The deadlines are measured from the last ack, not from zero, so the instants that
    // matter here are 1500 and 3500.
    p.send("ack n2 500").await?;
    let mut c = Check::new("the exact instant each deadline falls");
    for t in [1499, 1500, 3499, 3500] {
        let got = state(p, "n2", t).await?;
        let want = suspicion(t - 500, 1000, 3000).as_str().to_string();
        c.eq(&format!("status(n2, {t}).state"), want, got);
    }
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(an_ack_revives_a_suspect, |ctx| {
    let p = ctx.prim("swim").await?;
    p.send("init 1000 3000").await?;
    p.send("ack n3 0").await?;
    let suspected = state(p, "n3", 1500).await?;
    // The node answers late rather than never: suspicion is a question, and this is the
    // answer to it.
    p.send("ack n3 1500").await?;
    let revived = state(p, "n3", 1600).await?;
    let still_alive = state(p, "n3", 2400).await?;
    let suspect_again = state(p, "n3", 2600).await?;
    let mut c = Check::new("a suspect node that answers");
    c.eq("status(n3, 1500).state", "suspect".to_string(), suspected);
    c.eq(
        "status(n3, 1600).state after an ack",
        "alive".to_string(),
        revived,
    );
    c.eq("status(n3, 2400).state", "alive".to_string(), still_alive);
    c.eq(
        "status(n3, 2600).state",
        suspicion(2600 - 1500, 1000, 3000).as_str().to_string(),
        suspect_again,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});
