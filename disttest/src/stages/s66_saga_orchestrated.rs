//! Stage 66 — Orchestrated saga.
//!
//! A long-lived business transaction that no lock can span: reserve the stock, charge the
//! card, ship the parcel. There is no rollback here, because every step committed in a
//! database of its own; what there is instead is a compensation per step and a rule about
//! the order they run in. An orchestrator drives the forward steps and, when one of them
//! fails, walks back down the ones that succeeded.
//!
//! The oracle is that rule, written out: a saga that fails at step k must leave the ledger
//! `do:1 … do:k-1` followed by `undo:k-1 … undo:1`, and nothing else is admissible. Nothing
//! here asks the program which order it chose. The seeded replay drives a six-step saga
//! with seeded compensation failures, which is where an implementation that compensates the
//! step that failed, or that skips a compensation whose first attempt did not take, shows.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;

/// Stage 66.
pub fn stage() -> Stage {
    Stage {
        number: 66,
        slug: "saga_orchestrated",
        name: "Orchestrated saga",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `saga`: remember the steps that completed, and undo them newest first",
            "A failure at step k compensates k-1 … 1; the step that failed never completed",
            "Compensations are retried, so running one twice must leave one undo behind",
            "A compensation that fails stays at the head of the queue until it succeeds",
        ],
        examples,
        tests: vec![
            Test::new(
                "a saga with no failure commits every step",
                a_saga_with_no_failure_commits,
            ),
            Test::new(
                "a failure at the last step undoes the rest in reverse",
                a_failure_undoes_in_reverse,
            ),
            Test::new(
                "the step that failed is never compensated",
                the_failed_step_is_not_compensated,
            ),
            Test::new(
                "a failure at the first step compensates nothing",
                a_failure_at_the_first_step,
            ),
            Test::new(
                "a retried compensation leaves one undo, not two",
                a_retried_compensation_is_idempotent,
            ),
            Test::new(
                "a compensation that fails stays at the head of the queue",
                a_failed_compensation_is_retried,
            ),
            Test::new(
                "the phase and the outcome follow the saga through",
                the_phase_follows_the_saga,
            ),
            Test::new(
                "a compensation for a step that never ran is refused",
                an_unrun_step_cannot_be_compensated,
            )
            .ext(),
            Test::new(
                "driving step by step reaches the same ledger as one run",
                step_by_step_agrees_with_run,
            )
            .ext(),
            Test::new(
                "a seeded saga with seeded compensation failures matches the replay",
                a_seeded_saga,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example(
            "A saga that fails at the third of five steps",
            "saga",
            || {
                lines(&[
                    r#"init ["reserve","charge","pack","ship","notify"]"#,
                    "run 3",
                    "ledger",
                ])
            },
        )
        .request("a five-step saga driven in one command, failing at step three")
        .response("two steps executed, the same two compensated newest first")
        .note(
            "The ledger is the whole specification of this stage: `do:reserve`, \
             `do:charge`, `undo:charge`, `undo:reserve`. Note that `pack` appears nowhere. \
             It failed, so it never completed, and compensating a step that did not happen \
             is how a saga refunds money it never took.",
        ),
        prim_example("A compensation retried after it failed", "saga", || {
            lines(&[
                r#"init ["reserve","charge","ship"]"#,
                "step reserve ok",
                "step charge fail",
                "compensate reserve fail",
                "compensate reserve ok",
                "compensate reserve ok",
                "ledger",
            ])
        })
        .request("one compensation that fails, then succeeds, then is retried once more")
        .response("the failed attempt leaves the queue alone; the ledger holds one undo")
        .note(
            "Both halves matter. A compensation that fails must stay at the head of the \
             queue, because skipping it leaves the reservation held for ever; and the \
             retry that eventually lands must be idempotent, because the orchestrator \
             cannot tell a lost answer from a step that never ran.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// The tester's own model. Nothing below consults the program for what the answer is.
// ---------------------------------------------------------------------------------------

/// A three-step saga, the one most of the stage uses.
const THREE: &str = r#"init ["reserve","charge","ship"]"#;
/// Its step names.
const THREE_NAMES: [&str; 3] = ["reserve", "charge", "ship"];
/// A five-step saga, long enough that a wrong compensation order is unmistakable.
const FIVE: &str = r#"init ["reserve","charge","pack","ship","notify"]"#;
/// Its step names.
const FIVE_NAMES: [&str; 5] = ["reserve", "charge", "pack", "ship", "notify"];

/// The ledger the rules demand for a saga over `names` that fails at `fail_at`.
///
/// `fail_at` is 1-based; 0 means every step succeeded. This is the specification, not a
/// second implementation: the steps that completed are `names[..fail_at - 1]`, and their
/// compensations run in the opposite order.
fn expected_ledger(names: &[&str], fail_at: usize) -> Vec<String> {
    let done = if fail_at == 0 {
        names.len()
    } else {
        fail_at - 1
    };
    let mut out: Vec<String> = names[..done].iter().map(|n| format!("do:{n}")).collect();
    if fail_at != 0 {
        out.extend(names[..done].iter().rev().map(|n| format!("undo:{n}")));
    }
    out
}

/// The compensations still owed after `done` steps have completed, newest first.
fn expected_pending(names: &[&str], done: usize) -> Vec<String> {
    names[..done]
        .iter()
        .rev()
        .map(|n| (*n).to_string())
        .collect()
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(a_saga_with_no_failure_commits, |ctx| {
    let p = ctx.prim("saga").await?;
    p.send(THREE).await?;
    let run = p.send("run 0").await?;
    let ledger = p.send("ledger").await?;
    let state = p.send("state").await?;
    let mut c = Check::new("a saga in which every step succeeded");
    c.eq(
        "run(0).executed",
        THREE_NAMES.map(str::to_string).to_vec(),
        p.expect_strs(&run, "run 0", "executed")?,
    );
    c.eq(
        "run(0).compensated",
        Vec::<String>::new(),
        p.expect_strs(&run, "run 0", "compensated")?,
    );
    c.eq(
        "run(0).outcome",
        "committed".to_string(),
        p.expect_str(&run, "run 0", "outcome")?,
    );
    c.eq(
        "ledger.entries",
        expected_ledger(&THREE_NAMES, 0),
        p.expect_strs(&ledger, "ledger", "entries")?,
    );
    c.eq(
        "state.phase",
        "done".to_string(),
        p.expect_str(&state, "state", "phase")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_failure_undoes_in_reverse, |ctx| {
    let p = ctx.prim("saga").await?;
    p.send(FIVE).await?;
    // Four steps complete and the fifth fails, so four compensations run. With five steps a
    // compensation order that is merely "some order" cannot pass by luck.
    let run = p.send("run 5").await?;
    let ledger = p.send("ledger").await?;
    let mut c = Check::new("a five-step saga that failed at the last step");
    c.eq(
        "run(5).executed",
        FIVE_NAMES[..4]
            .iter()
            .map(|n| (*n).to_string())
            .collect::<Vec<String>>(),
        p.expect_strs(&run, "run 5", "executed")?,
    );
    c.eq(
        "run(5).compensated",
        expected_pending(&FIVE_NAMES, 4),
        p.expect_strs(&run, "run 5", "compensated")?,
    );
    c.eq(
        "run(5).outcome",
        "compensated".to_string(),
        p.expect_str(&run, "run 5", "outcome")?,
    );
    c.eq(
        "ledger.entries",
        expected_ledger(&FIVE_NAMES, 5),
        p.expect_strs(&ledger, "ledger", "entries")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_failed_step_is_not_compensated, |ctx| {
    let p = ctx.prim("saga").await?;
    p.send(FIVE).await?;
    p.send("step reserve ok").await?;
    p.send("step charge ok").await?;
    let failed = p.send("step pack fail").await?;
    let queued = p.send("state").await?;
    p.send("compensate charge ok").await?;
    p.send("compensate reserve ok").await?;
    let ledger = p.send("ledger").await?;
    let entries = p.expect_strs(&ledger, "ledger", "entries")?;
    let mut c = Check::new("the step that failed, and what it owes");
    c.eq(
        "step(pack, fail).phase",
        "compensating".to_string(),
        p.expect_str(&failed, "step pack fail", "phase")?,
    );
    c.eq(
        "step(pack, fail).completed",
        vec!["reserve".to_string(), "charge".to_string()],
        p.expect_strs(&failed, "step pack fail", "completed")?,
    );
    // `pack` never completed: there is no effect to undo, so it must not be queued, and
    // compensating it would undo something that never happened — a refund for a charge
    // that never landed.
    c.eq(
        "state.pending after the failure",
        expected_pending(&FIVE_NAMES, 2),
        p.expect_strs(&queued, "state", "pending")?,
    );
    c.that(
        "ledger.entries",
        "no entry naming the step that failed",
        entries.iter().all(|e| !e.contains("pack")),
        &entries,
    );
    c.eq("ledger.entries", expected_ledger(&FIVE_NAMES, 3), entries);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_failure_at_the_first_step, |ctx| {
    let p = ctx.prim("saga").await?;
    p.send(THREE).await?;
    let run = p.send("run 1").await?;
    let ledger = p.send("ledger").await?;
    let mut c = Check::new("a saga whose very first step failed");
    c.eq(
        "run(1).executed",
        Vec::<String>::new(),
        p.expect_strs(&run, "run 1", "executed")?,
    );
    c.eq(
        "run(1).compensated",
        Vec::<String>::new(),
        p.expect_strs(&run, "run 1", "compensated")?,
    );
    // Nothing happened, and yet the saga did not commit: "compensated" with an empty ledger
    // is the correct terminal state, not "running" and not "committed".
    c.eq(
        "run(1).outcome",
        "compensated".to_string(),
        p.expect_str(&run, "run 1", "outcome")?,
    );
    c.eq(
        "ledger.entries",
        Vec::<String>::new(),
        p.expect_strs(&ledger, "ledger", "entries")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_retried_compensation_is_idempotent, |ctx| {
    let p = ctx.prim("saga").await?;
    p.send(THREE).await?;
    p.send("step reserve ok").await?;
    p.send("step charge ok").await?;
    p.send("step ship fail").await?;
    let first = p.send("compensate charge ok").await?;
    let again = p.send("compensate charge ok").await?;
    let third = p.send("compensate charge ok").await?;
    let ledger = p.send("ledger").await?;
    let mut c = Check::new("one compensation run three times");
    let pending_after = vec!["reserve".to_string()];
    c.eq(
        "the first compensate(charge).pending",
        pending_after.clone(),
        p.expect_strs(&first, "compensate charge ok", "pending")?,
    );
    // The orchestrator cannot tell a lost answer from a step that never ran, so it retries;
    // a compensation that is not idempotent then refunds the customer twice.
    c.eq(
        "the second compensate(charge).pending",
        pending_after.clone(),
        p.expect_strs(&again, "compensate charge ok", "pending")?,
    );
    c.eq(
        "the third compensate(charge).pending",
        pending_after,
        p.expect_strs(&third, "compensate charge ok", "pending")?,
    );
    c.eq(
        "ledger.entries",
        vec![
            "do:reserve".to_string(),
            "do:charge".to_string(),
            "undo:charge".to_string(),
        ],
        p.expect_strs(&ledger, "ledger", "entries")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_failed_compensation_is_retried, |ctx| {
    let p = ctx.prim("saga").await?;
    p.send(THREE).await?;
    p.send("step reserve ok").await?;
    p.send("step charge ok").await?;
    p.send("step ship fail").await?;
    let failed = p.send("compensate charge fail").await?;
    let failed_twice = p.send("compensate charge fail").await?;
    let landed = p.send("compensate charge ok").await?;
    let ledger = p.send("ledger").await?;
    let mut c = Check::new("a compensation whose first two attempts did not take");
    let both = vec!["charge".to_string(), "reserve".to_string()];
    // Skipping a compensation that failed leaves the reservation held for ever, which is
    // worse than the failure that started the compensation in the first place.
    c.eq(
        "the first compensate(charge, fail).pending",
        both.clone(),
        p.expect_strs(&failed, "compensate charge fail", "pending")?,
    );
    c.eq(
        "the second compensate(charge, fail).pending",
        both,
        p.expect_strs(&failed_twice, "compensate charge fail", "pending")?,
    );
    c.eq(
        "compensate(charge, ok).pending",
        vec!["reserve".to_string()],
        p.expect_strs(&landed, "compensate charge ok", "pending")?,
    );
    c.eq(
        "ledger.entries",
        vec![
            "do:reserve".to_string(),
            "do:charge".to_string(),
            "undo:charge".to_string(),
        ],
        p.expect_strs(&ledger, "ledger", "entries")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_phase_follows_the_saga, |ctx| {
    let p = ctx.prim("saga").await?;
    p.send(THREE).await?;
    let start = p.send("state").await?;
    p.send("step reserve ok").await?;
    let mid = p.send("state").await?;
    p.send("step charge fail").await?;
    let turning = p.send("state").await?;
    p.send("compensate reserve ok").await?;
    let end = p.send("state").await?;
    let mut c = Check::new("the phase and outcome at four points of one saga");
    c.eq(
        "state.phase before anything ran",
        "forward".to_string(),
        p.expect_str(&start, "state", "phase")?,
    );
    c.eq(
        "state.outcome before anything ran",
        "running".to_string(),
        p.expect_str(&start, "state", "outcome")?,
    );
    c.eq(
        "state.phase after one step",
        "forward".to_string(),
        p.expect_str(&mid, "state", "phase")?,
    );
    c.eq(
        "state.phase after the failure",
        "compensating".to_string(),
        p.expect_str(&turning, "state", "phase")?,
    );
    c.eq(
        "state.pending after the failure",
        expected_pending(&THREE_NAMES, 1),
        p.expect_strs(&turning, "state", "pending")?,
    );
    c.eq(
        "state.phase once the queue is empty",
        "done".to_string(),
        p.expect_str(&end, "state", "phase")?,
    );
    c.eq(
        "state.outcome once the queue is empty",
        "compensated".to_string(),
        p.expect_str(&end, "state", "outcome")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(an_unrun_step_cannot_be_compensated, |ctx| {
    let p = ctx.prim("saga").await?;
    p.send(THREE).await?;
    p.send("step reserve ok").await?;
    p.send("step charge fail").await?;
    // `ship` never ran, and `charge` failed; neither owes a compensation, so neither may be
    // taken off the queue ahead of `reserve`.
    let ship = p.send("compensate ship ok").await?;
    let charge = p.send("compensate charge ok").await?;
    let ledger = p.send("ledger").await?;
    let mut c = Check::new("compensating steps the saga never completed");
    c.eq(
        "compensate(ship).ok",
        false,
        p.expect_bool(&ship, "compensate ship ok", "ok")?,
    );
    c.eq(
        "compensate(charge).ok",
        false,
        p.expect_bool(&charge, "compensate charge ok", "ok")?,
    );
    c.eq(
        "ledger.entries",
        vec!["do:reserve".to_string()],
        p.expect_strs(&ledger, "ledger", "entries")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(step_by_step_agrees_with_run, |ctx| {
    let p = ctx.prim("saga").await?;
    let mut rows: Vec<(usize, Vec<String>, Vec<String>)> = Vec::new();
    for fail_at in 0..=FIVE_NAMES.len() {
        // Driven one command at a time.
        p.send(FIVE).await?;
        for (i, name) in FIVE_NAMES.iter().enumerate() {
            let ok = fail_at == 0 || i + 1 != fail_at;
            p.send(&format!("step {name} {}", if ok { "ok" } else { "fail" }))
                .await?;
            if !ok {
                break;
            }
        }
        for name in expected_pending(&FIVE_NAMES, fail_at.saturating_sub(1)) {
            p.send(&format!("compensate {name} ok")).await?;
        }
        let manual = p.send("ledger").await?;
        let manual = p.expect_strs(&manual, "ledger", "entries")?;
        // Driven in one command.
        p.send(FIVE).await?;
        p.send(&format!("run {fail_at}")).await?;
        let batched = p.send("ledger").await?;
        let batched = p.expect_strs(&batched, "ledger", "entries")?;
        rows.push((fail_at, manual, batched));
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("every failure point, driven both ways");
    for (fail_at, manual, batched) in rows {
        let want = expected_ledger(&FIVE_NAMES, fail_at);
        c.eq(
            &format!("step-by-step ledger for fail_at {fail_at}"),
            want.clone(),
            manual,
        );
        c.eq(&format!("run({fail_at}) ledger"), want, batched);
        if !c.ok() {
            break;
        }
    }
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_seeded_saga, |ctx| {
    // The plan is worked out first — which step fails, and how many times each compensation
    // fails before it lands — and the ledger it must produce follows from the rules alone.
    let names = ["a", "b", "c", "d", "e", "f"];
    let fail_at = ctx.rng.random_range(1..=names.len());
    let retries: Vec<usize> = (0..names.len())
        .map(|_| ctx.rng.random_range(0..3))
        .collect();
    let seed = ctx.seed;
    let want_ledger = expected_ledger(&names, fail_at);
    let p = ctx.prim("saga").await?;
    p.send(&format!(
        "init [{}]",
        names
            .iter()
            .map(|n| format!("\"{n}\""))
            .collect::<Vec<String>>()
            .join(",")
    ))
    .await?;
    for (i, name) in names.iter().enumerate() {
        let ok = i + 1 != fail_at;
        p.send(&format!("step {name} {}", if ok { "ok" } else { "fail" }))
            .await?;
        if !ok {
            break;
        }
    }
    let queue = expected_pending(&names, fail_at - 1);
    let mut seen: Vec<(String, Vec<String>)> = Vec::new();
    for (i, name) in queue.iter().enumerate() {
        // Every failed attempt must leave the queue exactly as it was.
        for _ in 0..retries[i] {
            let r = p.send(&format!("compensate {name} fail")).await?;
            seen.push((
                format!("compensate {name} fail"),
                p.expect_strs(&r, "compensate", "pending")?,
            ));
        }
        let r = p.send(&format!("compensate {name} ok")).await?;
        seen.push((
            format!("compensate {name} ok"),
            p.expect_strs(&r, "compensate", "pending")?,
        ));
    }
    let ledger = p.send("ledger").await?;
    let ledger = p.expect_strs(&ledger, "ledger", "entries")?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a seeded six-step saga with seeded compensation failures");
    c.note(format!(
        "seed {seed}, fail_at {fail_at}, retries {retries:?}"
    ));
    let mut left = queue.clone();
    for (command, pending) in seen {
        if command.ends_with("ok") && !left.is_empty() {
            left.remove(0);
        }
        c.eq(&format!("{command} → pending"), left.clone(), pending);
        if !c.ok() {
            break;
        }
    }
    c.eq("ledger.entries", want_ledger, ledger);
    c.block("transcript", transcript);
    c.finish()
});
