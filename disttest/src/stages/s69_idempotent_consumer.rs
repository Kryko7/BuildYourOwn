//! Stage 69 — Idempotent consumer.
//!
//! The other half of stage 68. A broker that never loses a message will sometimes deliver
//! one twice, because the acknowledgement is the thing that goes missing, and no protocol
//! removes that. What removes the *harm* is the consumer: it remembers the ids it has
//! already applied and applies each effect once. A table of ids, a lookup before the
//! effect, and the whole problem moves from the network to a piece of state.
//!
//! The oracle is that table and its bound. A duplicate must not move the running total; two
//! different ids must both move it; and — the point of the stage — the table is finite, so
//! an id it has forgotten is applied a second time and the guarantee quietly stops holding.
//! That is why the honest name is effectively-once: at-least-once delivery plus idempotent
//! application, with a guarantee no more durable than the dedup table behind it.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;

/// Stage 69.
pub fn stage() -> Stage {
    Stage {
        number: 69,
        slug: "idempotent_consumer",
        name: "Idempotent consumer",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `dedup`: look the message id up before applying the effect, not after",
            "The table is a queue of first sightings: the oldest id is always the one evicted",
            "A redelivery is not a refresh — an entry ages from when it was first seen",
            "Exactly-once holds only while the table still remembers every id it applied",
        ],
        examples,
        tests: vec![
            Test::new("the same message id applies once", the_same_id_applies_once),
            Test::new("two different ids both apply", two_ids_both_apply),
            Test::new(
                "a duplicate does not move the running total",
                a_duplicate_does_not_move_the_total,
            ),
            Test::new(
                "the table forgets the oldest id first",
                the_oldest_id_is_forgotten_first,
            ),
            Test::new(
                "an id the table has forgotten is applied a second time",
                a_forgotten_id_is_applied_again,
            ),
            Test::new(
                "an unbounded table is exactly-once and a finite one is not",
                the_guarantee_follows_the_table,
            ),
            Test::new(
                "forgetting an id that was never seen changes nothing",
                forgetting_an_unknown_id_is_a_no_op,
            )
            .ext(),
            Test::new(
                "a redelivery does not refresh an entry",
                a_redelivery_does_not_refresh,
            )
            .ext(),
            Test::new(
                "a seeded stream with duplicates matches the tester's window model",
                a_seeded_stream,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("One message, delivered twice", "dedup", || {
            lines(&[
                "init 0",
                "deliver m1 100",
                "deliver m1 100",
                "deliver m2 5",
                "state",
            ])
        })
        .request("an unbounded table, one id delivered twice and another once")
        .response("the second copy applies nothing and the total stays at 100, then 105")
        .note(
            "The lookup has to happen before the effect and in the same transaction as it, \
             or a crash between the two turns the dedup table into a second copy of the \
             original problem. Recording the id afterwards is the most common way to get \
             this wrong and the hardest to notice.",
        ),
        prim_example("The window running out", "dedup", || {
            lines(&[
                "init 3",
                "deliver m1 10",
                "deliver m2 10",
                "deliver m3 10",
                "deliver m4 10",
                "deliver m1 10",
                "guarantee",
            ])
        })
        .request("a table of three ids, and a fifth delivery of an id it has evicted")
        .response("m1 is applied a second time and the guarantee reports itself broken")
        .note(
            "Nothing is wrong with the consumer here: it is doing exactly what it was \
             built to do. The guarantee is simply bounded by how long the table remembers, \
             so the window has to outlive the broker's retry horizon. This is why \
             'exactly-once' is a property of a system's configuration and not of a protocol.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// The tester's own model of the table. Nothing below consults the program for an answer.
// ---------------------------------------------------------------------------------------

/// A finite queue of first sightings, and the effect it guards.
struct Model {
    window: usize,
    ids: Vec<String>,
    total: i64,
    forgotten: bool,
}

impl Model {
    fn new(window: usize) -> Model {
        Model {
            window,
            ids: Vec::new(),
            total: 0,
            forgotten: false,
        }
    }

    /// Returns whether the message was applied.
    fn deliver(&mut self, id: &str, amount: i64) -> bool {
        if self.ids.iter().any(|x| x == id) {
            return false;
        }
        self.total += amount;
        self.ids.push(id.to_string());
        if self.window > 0 && self.ids.len() > self.window {
            self.ids.remove(0);
            self.forgotten = true;
        }
        true
    }
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(the_same_id_applies_once, |ctx| {
    let p = ctx.prim("dedup").await?;
    p.send("init 0").await?;
    let first = p.send("deliver m1 100").await?;
    let second = p.send("deliver m1 100").await?;
    let state = p.send("state").await?;
    let mut c = Check::new("one message delivered twice");
    c.eq(
        "the first deliver(m1).applied",
        true,
        p.expect_bool(&first, "deliver m1 100", "applied")?,
    );
    c.eq(
        "the first deliver(m1).duplicate",
        false,
        p.expect_bool(&first, "deliver m1 100", "duplicate")?,
    );
    c.eq(
        "the second deliver(m1).applied",
        false,
        p.expect_bool(&second, "deliver m1 100", "applied")?,
    );
    c.eq(
        "the second deliver(m1).duplicate",
        true,
        p.expect_bool(&second, "deliver m1 100", "duplicate")?,
    );
    c.eq(
        "state.applied",
        1,
        p.expect_i64(&state, "state", "applied")?,
    );
    c.eq("state.total", 100, p.expect_i64(&state, "state", "total")?);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(two_ids_both_apply, |ctx| {
    let p = ctx.prim("dedup").await?;
    p.send("init 0").await?;
    let first = p.send("deliver m1 100").await?;
    let second = p.send("deliver m2 100").await?;
    let state = p.send("state").await?;
    let mut c = Check::new("two different messages carrying the same amount");
    // Deduplicating on the payload rather than the id would swallow the second one here,
    // and two customers paying the same price is not a duplicate.
    c.eq(
        "deliver(m1).applied",
        true,
        p.expect_bool(&first, "deliver m1 100", "applied")?,
    );
    c.eq(
        "deliver(m2).applied",
        true,
        p.expect_bool(&second, "deliver m2 100", "applied")?,
    );
    c.eq("state.total", 200, p.expect_i64(&state, "state", "total")?);
    c.eq("state.seen", 2, p.expect_i64(&state, "state", "seen")?);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_duplicate_does_not_move_the_total, |ctx| {
    let p = ctx.prim("dedup").await?;
    p.send("init 0").await?;
    p.send("deliver m1 7").await?;
    let mut totals = Vec::new();
    for _ in 0..4 {
        let r = p.send("deliver m1 7").await?;
        totals.push(p.expect_i64(&r, "deliver m1 7", "total")?);
    }
    p.send("deliver m2 3").await?;
    let state = p.send("state").await?;
    let mut c = Check::new("four redeliveries between two real messages");
    c.eq(
        "the totals through the duplicates",
        vec![7, 7, 7, 7],
        totals,
    );
    c.eq("state.total", 10, p.expect_i64(&state, "state", "total")?);
    c.eq(
        "state.applied",
        2,
        p.expect_i64(&state, "state", "applied")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_oldest_id_is_forgotten_first, |ctx| {
    let p = ctx.prim("dedup").await?;
    p.send("init 3").await?;
    for id in ["m1", "m2", "m3"] {
        p.send(&format!("deliver {id} 1")).await?;
    }
    let full = p.send("state").await?;
    p.send("deliver m4 1").await?;
    let after = p.send("state").await?;
    let mut c = Check::new("a table of three taking a fourth id");
    c.eq(
        "state.ids while the table is exactly full",
        vec!["m1".to_string(), "m2".to_string(), "m3".to_string()],
        p.expect_strs(&full, "state", "ids")?,
    );
    c.eq(
        "state.ids after one more",
        vec!["m2".to_string(), "m3".to_string(), "m4".to_string()],
        p.expect_strs(&after, "state", "ids")?,
    );
    c.eq("state.seen", 3, p.expect_i64(&after, "state", "seen")?);
    // The effect is untouched by eviction: four messages were applied, and the table only
    // says which of them it could still recognise.
    c.eq(
        "state.applied",
        4,
        p.expect_i64(&after, "state", "applied")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_forgotten_id_is_applied_again, |ctx| {
    let p = ctx.prim("dedup").await?;
    p.send("init 3").await?;
    for id in ["m1", "m2", "m3", "m4"] {
        p.send(&format!("deliver {id} 10")).await?;
    }
    // m1 was applied and then evicted. The broker has no idea about any of that, and if it
    // redelivers now the effect happens a second time.
    let again = p.send("deliver m1 10").await?;
    let state = p.send("state").await?;
    let mut c = Check::new("a redelivery of an id the table has evicted");
    c.eq(
        "deliver(m1).applied",
        true,
        p.expect_bool(&again, "deliver m1 10", "applied")?,
    );
    c.eq(
        "deliver(m1).duplicate",
        false,
        p.expect_bool(&again, "deliver m1 10", "duplicate")?,
    );
    c.eq("state.total", 50, p.expect_i64(&state, "state", "total")?);
    c.eq(
        "state.applied",
        5,
        p.expect_i64(&state, "state", "applied")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_guarantee_follows_the_table, |ctx| {
    let p = ctx.prim("dedup").await?;
    p.send("init 0").await?;
    for id in ["m1", "m2", "m3", "m4", "m5"] {
        p.send(&format!("deliver {id} 1")).await?;
    }
    let unbounded = p.send("guarantee").await?;
    p.send("init 3").await?;
    for id in ["m1", "m2", "m3"] {
        p.send(&format!("deliver {id} 1")).await?;
    }
    let still_whole = p.send("guarantee").await?;
    p.send("deliver m4 1").await?;
    let broken = p.send("guarantee").await?;
    let mut c = Check::new("the guarantee before and after the table loses an id");
    c.eq(
        "guarantee.exactly_once with an unbounded table",
        true,
        p.expect_bool(&unbounded, "guarantee", "exactly_once")?,
    );
    c.eq(
        "guarantee.reason with an unbounded table",
        "every delivered id is still remembered".to_string(),
        p.expect_str(&unbounded, "guarantee", "reason")?,
    );
    // A finite table that has not overflowed yet is still whole: the bound is what matters,
    // not the fact that there is one.
    c.eq(
        "guarantee.exactly_once with a full but intact table",
        true,
        p.expect_bool(&still_whole, "guarantee", "exactly_once")?,
    );
    c.eq(
        "guarantee.exactly_once after an eviction",
        false,
        p.expect_bool(&broken, "guarantee", "exactly_once")?,
    );
    c.eq(
        "guarantee.reason after an eviction",
        "a delivered id has been forgotten".to_string(),
        p.expect_str(&broken, "guarantee", "reason")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(forgetting_an_unknown_id_is_a_no_op, |ctx| {
    let p = ctx.prim("dedup").await?;
    p.send("init 0").await?;
    p.send("deliver m1 4").await?;
    let unknown = p.send("forget zz").await?;
    let intact = p.send("guarantee").await?;
    let known = p.send("forget m1").await?;
    let broken = p.send("guarantee").await?;
    let mut c = Check::new("forgetting an id that is there, and one that is not");
    c.eq(
        "forget(zz).ok",
        false,
        p.expect_bool(&unknown, "forget zz", "ok")?,
    );
    c.eq(
        "forget(zz).seen",
        1,
        p.expect_i64(&unknown, "forget zz", "seen")?,
    );
    // Forgetting nothing cannot weaken a guarantee, and reporting otherwise would make the
    // consumer look broken every time a housekeeping job ran over an empty table.
    c.eq(
        "guarantee.exactly_once after forgetting nothing",
        true,
        p.expect_bool(&intact, "guarantee", "exactly_once")?,
    );
    c.eq(
        "forget(m1).ok",
        true,
        p.expect_bool(&known, "forget m1", "ok")?,
    );
    c.eq(
        "forget(m1).seen",
        0,
        p.expect_i64(&known, "forget m1", "seen")?,
    );
    c.eq(
        "guarantee.exactly_once after forgetting a real id",
        false,
        p.expect_bool(&broken, "guarantee", "exactly_once")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_redelivery_does_not_refresh, |ctx| {
    let p = ctx.prim("dedup").await?;
    p.send("init 3").await?;
    for id in ["a", "b", "c"] {
        p.send(&format!("deliver {id} 1")).await?;
    }
    // A redelivery of the oldest entry. Under a least-recently-used table this would move
    // `a` to the back and evict `b` next; the table is a queue of first sightings, so `a`
    // is still the one that goes.
    p.send("deliver a 1").await?;
    p.send("deliver d 1").await?;
    let state = p.send("state").await?;
    let mut c = Check::new("a duplicate arriving just before an eviction");
    c.eq(
        "state.ids",
        vec!["b".to_string(), "c".to_string(), "d".to_string()],
        p.expect_strs(&state, "state", "ids")?,
    );
    c.eq("state.total", 4, p.expect_i64(&state, "state", "total")?);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_seeded_stream, |ctx| {
    // A stream of eighty deliveries drawn from a small pool of ids, so duplicates are
    // frequent and evictions are frequent too. What each one does follows from the rules.
    let window = 4usize;
    let pool: Vec<String> = (0..9).map(|i| format!("m{i}")).collect();
    let mut model = Model::new(window);
    let mut script: Vec<(String, bool, i64, Vec<String>)> = Vec::new();
    for _ in 0..80 {
        let id = pool[ctx.rng.random_range(0..pool.len())].clone();
        let amount = ctx.rng.random_range(1..10);
        let applied = model.deliver(&id, amount);
        script.push((
            format!("deliver {id} {amount}"),
            applied,
            model.total,
            model.ids.clone(),
        ));
    }
    let seed = ctx.seed;
    let want_guarantee = !model.forgotten;
    let p = ctx.prim("dedup").await?;
    p.send(&format!("init {window}")).await?;
    let mut actual: Vec<(bool, i64, Vec<String>)> = Vec::new();
    for (command, ..) in &script {
        let r = p.send(command).await?;
        let applied = p.expect_bool(&r, command, "applied")?;
        let total = p.expect_i64(&r, command, "total")?;
        let s = p.send("state").await?;
        actual.push((applied, total, p.expect_strs(&s, "state", "ids")?));
    }
    let guarantee = p.send("guarantee").await?;
    let got_guarantee = p.expect_bool(&guarantee, "guarantee", "exactly_once")?;
    let transcript = p.transcript_block();
    let mut c = Check::new("eighty seeded deliveries over a window of four");
    c.note(format!(
        "seed {seed}, window {window}, {} messages",
        script.len()
    ));
    for (i, ((command, applied, total, ids), (got_a, got_t, got_i))) in
        script.iter().zip(&actual).enumerate()
    {
        c.eq(&format!("step[{i}] {command} → applied"), *applied, *got_a);
        c.eq(&format!("step[{i}] {command} → total"), *total, *got_t);
        c.eq(
            &format!("step[{i}] {command} → ids"),
            ids.clone(),
            got_i.clone(),
        );
        if !c.ok() {
            break;
        }
    }
    c.eq("guarantee.exactly_once", want_guarantee, got_guarantee);
    c.block("transcript", transcript);
    c.finish()
});
