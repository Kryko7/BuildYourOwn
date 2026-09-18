//! Stage 14 — G-Counter and PN-Counter.
//!
//! The first CRDT, and the one where the shape of the whole family is visible. A counter
//! that replicas can increment without talking to each other cannot be a number: two
//! replicas that both added three have no way to tell, later, whether that was one update
//! seen twice or two updates seen once. Keeping one entry per replica and merging by
//! maximum removes the question — the entry is owned by exactly one writer, so the largest
//! value anyone has seen for it is the truth.
//!
//! The oracle here is exhaustive replay. Merge has to be commutative, associative and
//! idempotent, which is another way of saying that *every* order in which the same states
//! can arrive must land on the same value. So the stage builds a handful of replica states
//! and replays every permutation of them into a fresh replica, with a duplicate delivery
//! and an already-merged state thrown in, and demands one answer from all of them.

use crate::assert::{Check, Failure};
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::prim::{oracles, PrimProc};
use crate::stages::{Ctx, Ladder, Stage, Test};
use rand::Rng;
use std::collections::BTreeSet;

/// Stage 14.
pub fn stage() -> Stage {
    Stage {
        number: 14,
        slug: "counters",
        name: "G-Counter and PN-Counter",
        ext: false,
        ladder: Ladder::Primitives,
        hints: &[
            "Topics `g-counter` and `pn-counter`: one entry per replica, merged by maximum",
            "A PN-Counter is two G-Counters: increments and decrements, never one signed number",
            "Merge must be commutative, associative and idempotent — every delivery order converges",
            "A replica only ever writes its own entry; it copies others by taking the maximum",
        ],
        examples,
        tests: vec![
            Test::new("a fresh counter reads zero and then counts up", one_replica_counts_up),
            Test::new("the value is the sum over every replica", value_is_the_sum),
            Test::new("a stale merge never lowers the value", a_stale_merge_never_lowers_the_value),
            Test::new("merging the same state twice changes nothing", merging_twice_changes_nothing),
            Test::new(
                "a replica writes only its own entry, so no increment is lost",
                only_its_own_entry,
            ),
            Test::new(
                "every delivery order of the same updates converges",
                every_delivery_order_converges,
            )
            .min_timeout_ms(60_000),
            Test::new(
                "a pn-counter keeps increments and decrements apart",
                two_halves_stay_apart,
            ),
            Test::new("a pn-counter can go below zero", it_can_go_below_zero),
            Test::new(
                "every delivery order of increments and decrements converges",
                every_signed_delivery_order_converges,
            )
            .ext()
            .min_timeout_ms(60_000),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("Two replicas counting in one process", "g-counter", || {
            lines(&["inc r1 5", "inc r2 7", "value r1", "state r1"])
        })
        .request("two replicas increment their own entries, then the counter is read")
        .response("the value is the sum of the entries, and `state` hands back a shippable copy")
        .note(
            "The shape of `state` is entirely yours: the harness never looks inside it, it \
             only hands it to another replica's `merge`. What it must do is round trip, so \
             whatever a replica needs to keep has to be in there.",
        ),
        prim_example(
            "Increments and decrements are counted apart",
            "pn-counter",
            || lines(&["inc r1 10", "dec r1 3", "value r1", "dec r2 20", "value r1"]),
        )
        .request("one replica adds ten and takes three, then another takes twenty")
        .response("7, and then -13: a PN-Counter is free to be negative")
        .note(
            "Storing 7 as a single number would be the bug. Two G-Counters merge by \
             maximum on each half independently, and the difference is read at the end; \
             one signed number has no maximum that means 'whoever saw more updates'.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Shipping a replica to another replica
// ---------------------------------------------------------------------------------------

/// Read a replica's shippable state, rendered as the compact JSON a `merge` takes.
///
/// Commands are split on whitespace, so the state has to arrive as one word; that is what
/// `crate::prim::arg` is for.
async fn snapshot(p: &mut PrimProc, replica: &str) -> Result<String, Failure> {
    let command = format!("state {replica}");
    let answer = p.send(&command).await?;
    match answer.get("state") {
        Some(state) => Ok(crate::prim::arg(state)),
        None => Err(p.shape(&command, "the answer has no \"state\" field")),
    }
}

/// Merge a shipped state into a replica and read the value it reports.
async fn merge(p: &mut PrimProc, dst: &str, state: &str) -> Result<i64, Failure> {
    let command = format!("merge {dst} {state}");
    let answer = p.send(&command).await?;
    p.expect_i64(&answer, &command, "value")
}

/// Build one independent replica per update and take a snapshot of each.
///
/// Each state comes from its own process, so no replica has seen any of the others: these
/// are genuinely concurrent updates, which is the only interesting thing to permute.
async fn independent_states(
    ctx: &mut Ctx,
    topic: &'static str,
    updates: &[(String, String)],
) -> Result<Vec<String>, Failure> {
    let mut states = Vec::with_capacity(updates.len());
    for (replica, command) in updates {
        let mut p = ctx.prim_fresh(topic).await?;
        p.send(command).await?;
        let state = snapshot(&mut p, replica).await?;
        p.close().await;
        states.push(state);
    }
    Ok(states)
}

/// Replay every permutation of `states` into a fresh replica and demand one answer.
///
/// Each replay also merges `composite` — a state that is itself the merge of two others,
/// which is what associativity is about — and delivers the order's first state a second
/// time, which is what idempotence is about. The first order that disagrees is reported
/// with the order itself, because "which order broke it" is the only question worth
/// answering here.
async fn replay_every_order(
    ctx: &mut Ctx,
    topic: &'static str,
    states: &[String],
    composite: &str,
    expected: i64,
) -> Result<usize, Failure> {
    let orders = oracles::permutations(states.len());
    let mut seen: BTreeSet<i64> = BTreeSet::new();
    for order in &orders {
        let mut f = ctx.prim_fresh(topic).await?;
        for i in order {
            let Some(state) = states.get(*i) else {
                return Err(Failure::harness(
                    "the permutation names a state that is not there",
                ));
            };
            merge(&mut f, "rz", state).await?;
        }
        merge(&mut f, "rz", composite).await?;
        let first = order.first().copied().unwrap_or(0);
        let Some(again) = states.get(first) else {
            return Err(Failure::harness("the permutation is empty"));
        };
        let value = merge(&mut f, "rz", again).await?;
        let read_back = f.num("value rz", "value").await?;
        f.close().await;
        seen.insert(value);
        if value != expected || read_back != value {
            let mut c = Check::new("a replica that has merged every state once");
            c.note(format!("the states arrived in the order {order:?}"));
            c.note("then an already-merged state, then the first one a second time");
            c.eq("merge -> value", expected, value);
            c.eq("value after the last merge", value, read_back);
            c.finish()?;
        }
    }
    let mut c = Check::new("the values every merge order arrived at");
    c.eq(
        "how many distinct values the orders produced",
        1,
        seen.len(),
    );
    c.observe("the values seen", &seen);
    c.finish()?;
    Ok(orders.len())
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(one_replica_counts_up, |ctx| {
    let p = ctx.prim("g-counter").await?;
    let mut c = Check::new("a g-counter on a single replica");
    c.eq(
        "value on an untouched counter",
        0,
        p.num("value r1", "value").await?,
    );
    c.eq("inc r1 1 -> value", 1, p.num("inc r1 1", "value").await?);
    c.eq("inc r1 2 -> value", 3, p.num("inc r1 2", "value").await?);
    c.eq("inc r1 3 -> value", 6, p.num("inc r1 3", "value").await?);
    c.eq("value r1", 6, p.num("value r1", "value").await?);
    c.finish()
});

dist_test!(value_is_the_sum, |ctx| {
    let p = ctx.prim("g-counter").await?;
    p.send("inc r1 5").await?;
    p.send("inc r2 7").await?;
    let after = p.num("inc r1 2", "value").await?;
    let mut c = Check::new("a counter with two replica entries");
    c.eq("inc r1 2 -> value", 14, after);
    // The value is a property of the counter, not of whoever asks for it.
    c.eq("value r1", 14, p.num("value r1", "value").await?);
    c.eq("value r2", 14, p.num("value r2", "value").await?);
    c.finish()
});

dist_test!(a_stale_merge_never_lowers_the_value, |ctx| {
    let mut a = ctx.prim_fresh("g-counter").await?;
    a.send("inc r1 5").await?;
    let early = snapshot(&mut a, "r1").await?;
    let grown = a.num("inc r1 4", "value").await?;
    let after_stale = merge(&mut a, "r1", &early).await?;
    let read_back = a.num("value r1", "value").await?;
    a.close().await;
    let mut c = Check::new("an old copy of a replica's own state merged back into it");
    c.note("the entry for r1 was 5 when the state was taken and 9 when it came back");
    c.eq("value after inc", 9, grown);
    // 14 would mean the merge added the two entries; 5 would mean it took the newer word
    // for it. Only a maximum answers 9.
    c.eq("merge -> value", 9, after_stale);
    c.eq("value r1", 9, read_back);
    c.finish()
});

dist_test!(merging_twice_changes_nothing, |ctx| {
    let mut a = ctx.prim_fresh("g-counter").await?;
    let mut b = ctx.prim_fresh("g-counter").await?;
    a.send("inc r1 6").await?;
    let state_a = snapshot(&mut a, "r1").await?;
    b.send("inc r2 4").await?;
    let once = merge(&mut b, "r2", &state_a).await?;
    let twice = merge(&mut b, "r2", &state_a).await?;
    let thrice = merge(&mut b, "r2", &state_a).await?;
    let read_back = b.num("value r2", "value").await?;
    a.close().await;
    b.close().await;
    let mut c = Check::new("the same state delivered three times");
    c.eq("merge -> value, first delivery", 10, once);
    c.eq("merge -> value, second delivery", 10, twice);
    c.eq("merge -> value, third delivery", 10, thrice);
    c.eq("value r2", 10, read_back);
    c.finish()
});

dist_test!(only_its_own_entry, |ctx| {
    let mut a = ctx.prim_fresh("g-counter").await?;
    let mut b = ctx.prim_fresh("g-counter").await?;
    a.send("inc r1 5").await?;
    let a1 = snapshot(&mut a, "r1").await?;
    // b learns about r1, then counts four of its own.
    merge(&mut b, "r2", &a1).await?;
    let b_after_own = b.num("inc r2 4", "value").await?;
    let b1 = snapshot(&mut b, "r2").await?;
    // a moves on without hearing anything from b.
    let a_after_own = a.num("inc r1 6", "value").await?;
    let a2 = snapshot(&mut a, "r1").await?;
    // Now both directions.
    let b_final = merge(&mut b, "r2", &a2).await?;
    let a_final = merge(&mut a, "r1", &b1).await?;
    a.close().await;
    b.close().await;
    let mut c = Check::new("two replicas that each counted after hearing from the other");
    c.note("r1 counted 5 then 6, r2 counted 4, so the counter is 15");
    c.eq("b: inc r2 4 -> value", 9, b_after_own);
    c.eq("a: inc r1 6 -> value", 11, a_after_own);
    // A replica that had put its four into r1's entry instead of its own would now read
    // 11 here: the maximum would swallow its update whole.
    c.eq("b: value after merging a's newer state", 15, b_final);
    c.eq("a: value after merging b's state", 15, a_final);
    c.finish()
});

dist_test!(every_delivery_order_converges, |ctx| {
    // Six entries, drawn from the seed so a failure is reproducible. Six is 720 orders,
    // which is under a second: the point of the oracle is that it is exhaustive.
    let amounts: Vec<i64> = (0..6).map(|_| ctx.rng.random_range(1..=20i64)).collect();
    let total: i64 = amounts.iter().sum();
    let updates: Vec<(String, String)> = amounts
        .iter()
        .enumerate()
        .map(|(i, n)| (format!("r{i}"), format!("inc r{i} {n}")))
        .collect();
    let states = independent_states(ctx, "g-counter", &updates).await?;
    // A state that is itself a merge of two others: merging it must add nothing new.
    let composite = {
        let mut m = ctx.prim_fresh("g-counter").await?;
        for state in states.iter().take(2) {
            merge(&mut m, "rm", state).await?;
        }
        let s = snapshot(&mut m, "rm").await?;
        m.close().await;
        s
    };
    let orders = replay_every_order(ctx, "g-counter", &states, &composite, total).await?;
    ctx.note(format!(
        "replayed {orders} merge orders of {} concurrent replica states, each with a \
         duplicate delivery and an already-merged state",
        states.len()
    ));
    ctx.note(format!(
        "the entries were {amounts:?}, so the counter is {total}"
    ));
    Ok(())
});

dist_test!(two_halves_stay_apart, |ctx| {
    let mut a = ctx.prim_fresh("pn-counter").await?;
    a.send("inc r1 10").await?;
    // A state taken before the decrement: merging it back must not undo the decrement.
    let before_dec = snapshot(&mut a, "r1").await?;
    let after_dec = a.num("dec r1 3", "value").await?;
    let after_stale = merge(&mut a, "r1", &before_dec).await?;
    let read_back = a.num("value r1", "value").await?;
    a.close().await;
    let mut c = Check::new("a stale state merged over a decrement");
    c.eq("dec r1 3 -> value", 7, after_dec);
    // 10 would mean the increments and the decrements are one number, and the older, larger
    // number won.
    c.eq("merge -> value", 7, after_stale);
    c.eq("value r1", 7, read_back);
    c.finish()
});

dist_test!(it_can_go_below_zero, |ctx| {
    let p = ctx.prim("pn-counter").await?;
    let mut c = Check::new("a pn-counter taken below zero");
    c.eq("dec r1 4 -> value", -4, p.num("dec r1 4", "value").await?);
    c.eq("dec r2 6 -> value", -10, p.num("dec r2 6", "value").await?);
    c.eq("inc r1 1 -> value", -9, p.num("inc r1 1", "value").await?);
    c.eq("value r1", -9, p.num("value r1", "value").await?);
    c.finish()
});

dist_test!(every_signed_delivery_order_converges, |ctx| {
    // Five replicas, each with one update; some add, some take away. Five rather than six
    // orders because this stage already replays 720 of them once.
    let mut updates: Vec<(String, String)> = Vec::new();
    let mut total = 0i64;
    for i in 0..5 {
        let n: i64 = ctx.rng.random_range(1..=15);
        let down = i % 2 == 1;
        total += if down { -n } else { n };
        let verb = if down { "dec" } else { "inc" };
        updates.push((format!("r{i}"), format!("{verb} r{i} {n}")));
    }
    let states = independent_states(ctx, "pn-counter", &updates).await?;
    let composite = {
        let mut m = ctx.prim_fresh("pn-counter").await?;
        for state in states.iter().take(2) {
            merge(&mut m, "rm", state).await?;
        }
        let s = snapshot(&mut m, "rm").await?;
        m.close().await;
        s
    };
    let orders = replay_every_order(ctx, "pn-counter", &states, &composite, total).await?;
    ctx.note(format!(
        "replayed {orders} merge orders of {} concurrent replica states, two halves each",
        states.len()
    ));
    ctx.note(format!("the updates sum to {total}"));
    Ok(())
});
