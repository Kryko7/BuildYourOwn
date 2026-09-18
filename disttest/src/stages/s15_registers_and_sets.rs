//! Stage 15 — LWW-Register and OR-Set.
//!
//! Two CRDTs that look easy and are not. A last-writer-wins register has to answer "which
//! write was last" when the two writes carry the same timestamp, and it has to answer it
//! the same way on every replica, with no conversation — so the tie-break has to be part of
//! the data, which is why the writer's name travels with the value.
//!
//! The OR-Set is the sharper of the two. A set that records "x was removed" is a 2P-Set: it
//! loses an add that happened concurrently with the remove, and it can never take x back
//! afterwards. An OR-Set records *which adds* were removed, so an add nobody had seen
//! survives, and a re-add mints a tag no remove can name. The test for that is the one to
//! write first and the one to trust.
//!
//! Both stages lean on the same oracle: replay every permutation of the same states into a
//! fresh replica and demand one answer from all of them.

use crate::assert::{Check, Failure};
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::prim::{oracles, PrimProc};
use crate::stages::{Ctx, Ladder, Stage, Test};
use rand::Rng;
use serde_json::Value;

/// Stage 15.
pub fn stage() -> Stage {
    Stage {
        number: 15,
        slug: "registers_and_sets",
        name: "LWW-Register and OR-Set",
        ext: false,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `lww-register`: value plus timestamp, and a deterministic tie-break on equal timestamps",
            "Topic `or-set`: an add carries a unique tag; a remove takes away the tags it saw",
            "An add concurrent with a remove survives — that is the whole difference from a 2P-Set",
            "Re-adding after a remove must work, which is why tags cannot be reused",
        ],
        examples,
        tests: vec![
            Test::new("an untouched register reads null", untouched_register_is_null),
            Test::new("a set answers with the value it now holds", a_set_answers_with_its_value),
            Test::new(
                "the larger timestamp wins whichever order the merges arrive in",
                the_larger_timestamp_wins,
            ),
            Test::new(
                "equal timestamps break by replica name, so two replicas agree",
                equal_timestamps_break_by_name,
            ),
            Test::new(
                "every delivery order of register writes converges",
                every_register_order_converges,
            )
            .min_timeout_ms(60_000),
            Test::new("elements come back sorted and without duplicates", elements_are_sorted),
            Test::new(
                "an add concurrent with a remove survives",
                an_add_concurrent_with_a_remove_survives,
            ),
            Test::new(
                "re-adding after a remove works, and a stale state cannot undo it",
                re_adding_after_a_remove_works,
            ),
            Test::new(
                "every delivery order of a mixed add and remove exchange converges",
                every_set_order_converges,
            )
            .min_timeout_ms(60_000),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("A tie broken by the writer's name", "lww-register", || {
            lines(&["value r1", "set r1 red 7", "set r2 blue 7", "value r1"])
        })
        .request("an untouched register, then two writes carrying the same timestamp")
        .response("null at first, and then `blue`: on a tie the larger replica name wins")
        .note(
            "Wall clocks tie more often than anyone expects, and two replicas that resolve \
             a tie differently have diverged permanently. The rule has to be a total order \
             on data both sides already hold, so the name rides along with the value.",
        ),
        prim_example("An element removed and added again", "or-set", || {
            lines(&[
                "add r1 x",
                "remove r1 x",
                "elements r1",
                "add r1 x",
                "elements r1",
            ])
        })
        .request("one element added, removed, then added a second time")
        .response("an empty set, then a set holding `x` again")
        .note(
            "The second add has to mint a tag the remove never saw. A set that remembers \
             removed *elements* rather than removed *tags* answers the empty set here and \
             can never be talked out of it.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Shipping a replica to another replica
// ---------------------------------------------------------------------------------------

/// Read a replica's shippable state, rendered as the compact JSON a `merge` takes.
///
/// Commands are split on whitespace, so a state has to arrive as a single word, which is
/// what `crate::prim::arg` renders.
async fn snapshot(p: &mut PrimProc, replica: &str) -> Result<String, Failure> {
    let command = format!("state {replica}");
    let answer = p.send(&command).await?;
    match answer.get("state") {
        Some(state) => Ok(crate::prim::arg(state)),
        None => Err(p.shape(&command, "the answer has no \"state\" field")),
    }
}

/// Merge a shipped state into a replica, keeping whatever the answer was.
async fn merge(p: &mut PrimProc, dst: &str, state: &str) -> Result<Value, Failure> {
    p.send(&format!("merge {dst} {state}")).await
}

/// The register's value as a JSON value, which is `null` until somebody writes.
async fn register_value(p: &mut PrimProc, replica: &str) -> Result<Value, Failure> {
    let command = format!("value {replica}");
    let answer = p.send(&command).await?;
    Ok(answer.get("value").cloned().unwrap_or(Value::Null))
}

/// The set's elements, which the grammar says arrive sorted.
async fn set_elements(p: &mut PrimProc, replica: &str) -> Result<Vec<String>, Failure> {
    let command = format!("elements {replica}");
    let answer = p.send(&command).await?;
    p.expect_strs(&answer, &command, "elements")
}

/// Replay every permutation of `states` into a fresh replica and demand one answer.
///
/// `read` is the command that reads the replica back and `field` the part of its answer
/// that has to converge, so the same replay serves a register and a set. Every replay also
/// merges `composite`, a state that is itself the merge of two others — associativity — and
/// delivers the order's first state a second time — idempotence. The first order that
/// disagrees is reported along with the order itself, because which order broke it is the
/// only question worth answering.
async fn replay_every_order(
    ctx: &mut Ctx,
    topic: &'static str,
    states: &[String],
    composite: &str,
    read: &str,
    field: &str,
    expected: &Value,
) -> Result<usize, Failure> {
    let orders = oracles::permutations(states.len());
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
        merge(&mut f, "rz", again).await?;
        let answer = f.send(read).await?;
        let got = answer.get(field).cloned().unwrap_or(Value::Null);
        f.close().await;
        if got != *expected {
            let mut c = Check::new("a replica that has merged every state");
            c.note(format!("the states arrived in the order {order:?}"));
            c.note("then an already-merged state, then the first one a second time");
            c.json_eq(&format!("{read} -> {field}"), expected, &got);
            c.finish()?;
        }
    }
    Ok(orders.len())
}

// ---------------------------------------------------------------------------------------
// LWW-Register
// ---------------------------------------------------------------------------------------

dist_test!(untouched_register_is_null, |ctx| {
    let p = ctx.prim("lww-register").await?;
    let command = "value r1";
    let answer = p.send(command).await?;
    let ts = p.expect_i64(&answer, command, "ts")?;
    let mut c = Check::new("a register nobody has written to");
    c.json_eq(
        "value r1 -> value",
        &Value::Null,
        answer.get("value").unwrap_or(&Value::Null),
    );
    c.eq("value r1 -> ts", 0, ts);
    c.finish()
});

dist_test!(a_set_answers_with_its_value, |ctx| {
    let p = ctx.prim("lww-register").await?;
    let set = p.send("set r1 red 7").await?;
    let set_value = p.expect_str(&set, "set r1 red 7", "value")?;
    let set_ts = p.expect_i64(&set, "set r1 red 7", "ts")?;
    let read = p.send("value r1").await?;
    let read_value = p.expect_str(&read, "value r1", "value")?;
    let read_ts = p.expect_i64(&read, "value r1", "ts")?;
    let mut c = Check::new("one write to a register");
    c.eq("set -> value", "red".to_string(), set_value);
    c.eq("set -> ts", 7, set_ts);
    c.eq("value r1 -> value", "red".to_string(), read_value);
    c.eq("value r1 -> ts", 7, read_ts);
    c.finish()
});

dist_test!(the_larger_timestamp_wins, |ctx| {
    let mut a = ctx.prim_fresh("lww-register").await?;
    let mut b = ctx.prim_fresh("lww-register").await?;
    a.send("set r1 early 3").await?;
    b.send("set r2 late 9").await?;
    let state_a = snapshot(&mut a, "r1").await?;
    let state_b = snapshot(&mut b, "r2").await?;
    a.close().await;
    b.close().await;

    // The same two states, delivered in both orders, into two replicas that have never
    // heard of either write.
    let mut forwards = ctx.prim_fresh("lww-register").await?;
    merge(&mut forwards, "rz", &state_a).await?;
    merge(&mut forwards, "rz", &state_b).await?;
    let forwards_value = register_value(&mut forwards, "rz").await?;
    forwards.close().await;

    let mut backwards = ctx.prim_fresh("lww-register").await?;
    merge(&mut backwards, "rz", &state_b).await?;
    merge(&mut backwards, "rz", &state_a).await?;
    let backwards_value = register_value(&mut backwards, "rz").await?;
    backwards.close().await;

    let late = Value::String("late".to_string());
    let mut c = Check::new("a stale write and a newer one, merged in both orders");
    // Arriving second is not the same as being last: the timestamp decides, not the pipe.
    c.json_eq(
        "value after merging the old state last",
        &late,
        &backwards_value,
    );
    c.json_eq(
        "value after merging the new state last",
        &late,
        &forwards_value,
    );
    c.finish()
});

dist_test!(equal_timestamps_break_by_name, |ctx| {
    let mut a = ctx.prim_fresh("lww-register").await?;
    let mut b = ctx.prim_fresh("lww-register").await?;
    a.send("set r1 alpha 5").await?;
    b.send("set r2 beta 5").await?;
    let state_a = snapshot(&mut a, "r1").await?;
    let state_b = snapshot(&mut b, "r2").await?;
    merge(&mut a, "r1", &state_b).await?;
    merge(&mut b, "r2", &state_a).await?;
    let a_value = register_value(&mut a, "r1").await?;
    let b_value = register_value(&mut b, "r2").await?;
    a.close().await;
    b.close().await;
    let beta = Value::String("beta".to_string());
    let mut c = Check::new("two writes carrying the same timestamp");
    c.note("r1 wrote alpha and r2 wrote beta, both at 5; the larger name is r2");
    c.json_eq("r1's value after the exchange", &beta, &a_value);
    c.json_eq("r2's value after the exchange", &beta, &b_value);
    c.finish()
});

dist_test!(every_register_order_converges, |ctx| {
    // Small timestamps on purpose: ties are the interesting case, so make them likely. Six
    // writes is 720 delivery orders, which is under a second and leaves nothing untried.
    let stamps: Vec<i64> = (0..6).map(|_| ctx.rng.random_range(1..=4i64)).collect();
    let mut states = Vec::new();
    for (i, ts) in stamps.iter().enumerate() {
        let mut p = ctx.prim_fresh("lww-register").await?;
        p.send(&format!("set r{i} v{i} {ts}")).await?;
        states.push(snapshot(&mut p, &format!("r{i}")).await?);
        p.close().await;
    }
    // The oracle: the largest (timestamp, replica name) pair, which is the whole
    // specification of a last-writer-wins register.
    let winner = stamps
        .iter()
        .enumerate()
        .max_by_key(|(i, ts)| (**ts, format!("r{i}")))
        .map(|(i, _)| i)
        .unwrap_or(0);
    let expected = Value::String(format!("v{winner}"));

    let composite = {
        let mut m = ctx.prim_fresh("lww-register").await?;
        for state in states.iter().take(2) {
            merge(&mut m, "rm", state).await?;
        }
        let s = snapshot(&mut m, "rm").await?;
        m.close().await;
        s
    };
    let orders = replay_every_order(
        ctx,
        "lww-register",
        &states,
        &composite,
        "value rz",
        "value",
        &expected,
    )
    .await?;
    ctx.note(format!(
        "replayed {orders} merge orders of {} concurrent writes, each with a duplicate \
         delivery and an already-merged state",
        states.len()
    ));
    ctx.note(format!(
        "the timestamps were {stamps:?}, so r{winner} holds the register"
    ));
    Ok(())
});

// ---------------------------------------------------------------------------------------
// OR-Set
// ---------------------------------------------------------------------------------------

dist_test!(elements_are_sorted, |ctx| {
    let p = ctx.prim("or-set").await?;
    for element in ["pear", "apple", "fig", "apple"] {
        p.send(&format!("add r1 {element}")).await?;
    }
    let command = "elements r1";
    let answer = p.send(command).await?;
    let elements = p.expect_strs(&answer, command, "elements")?;
    let mut c = Check::new("four adds of three distinct elements");
    // Adding twice is two tags for one element, not two elements.
    c.eq(
        "elements r1",
        vec!["apple".to_string(), "fig".into(), "pear".into()],
        elements,
    );
    c.finish()
});

dist_test!(an_add_concurrent_with_a_remove_survives, |ctx| {
    let mut a = ctx.prim_fresh("or-set").await?;
    let mut b = ctx.prim_fresh("or-set").await?;
    a.send("add r1 x").await?;
    a.send("add r1 keep").await?;
    let first = snapshot(&mut a, "r1").await?;
    // r2 sees that add and takes it away.
    merge(&mut b, "r2", &first).await?;
    b.send("remove r2 x").await?;
    // Meanwhile r1, which has heard nothing, adds x a second time.
    a.send("add r1 x").await?;
    let state_a = snapshot(&mut a, "r1").await?;
    let state_b = snapshot(&mut b, "r2").await?;
    merge(&mut a, "r1", &state_b).await?;
    merge(&mut b, "r2", &state_a).await?;
    let a_elements = set_elements(&mut a, "r1").await?;
    let b_elements = set_elements(&mut b, "r2").await?;
    a.close().await;
    b.close().await;
    let expected = vec!["keep".to_string(), "x".into()];
    let mut c = Check::new("an add that happened while a remove was in flight");
    c.note("r2 removed the first add of x; r1 added x again without having seen the remove");
    // A 2P-Set answers ["keep"] here: it remembers that x was removed, not which add was.
    c.eq(
        "r1's elements after the exchange",
        expected.clone(),
        a_elements,
    );
    c.eq("r2's elements after the exchange", expected, b_elements);
    c.finish()
});

dist_test!(re_adding_after_a_remove_works, |ctx| {
    let mut a = ctx.prim_fresh("or-set").await?;
    a.send("add r1 x").await?;
    // A state taken before the remove: merging it back must not resurrect the add it holds,
    // because that add is exactly the one the remove named.
    let before_remove = snapshot(&mut a, "r1").await?;
    a.send("remove r1 x").await?;
    let after_remove = set_elements(&mut a, "r1").await?;
    merge(&mut a, "r1", &before_remove).await?;
    let after_stale_merge = set_elements(&mut a, "r1").await?;
    a.send("add r1 x").await?;
    let after_re_add = set_elements(&mut a, "r1").await?;
    merge(&mut a, "r1", &before_remove).await?;
    let still_there = set_elements(&mut a, "r1").await?;
    a.close().await;
    let empty: Vec<String> = Vec::new();
    let one = vec!["x".to_string()];
    let mut c = Check::new("an element removed, then added again");
    c.eq("elements after the remove", empty.clone(), after_remove);
    c.eq(
        "elements after merging the pre-remove state",
        empty,
        after_stale_merge,
    );
    c.eq("elements after adding x again", one.clone(), after_re_add);
    c.eq(
        "elements after merging the pre-remove state again",
        one,
        still_there,
    );
    c.finish()
});

dist_test!(every_set_order_converges, |ctx| {
    // One replica seeds the set; four others each see that seed and then do one thing of
    // their own, and a sixth has never heard of any of it. Three of those things are removes
    // and one is a concurrent re-add, so the answer is neither "everything anyone added" nor
    // "everything nobody removed".
    let mut seed = ctx.prim_fresh("or-set").await?;
    for element in ["a", "b", "c"] {
        seed.send(&format!("add r0 {element}")).await?;
    }
    let base = snapshot(&mut seed, "r0").await?;
    seed.close().await;

    let mut states = vec![base.clone()];
    for (i, command) in [
        (1usize, "remove r1 a"),
        (2, "remove r2 b"),
        (3, "add r3 a"),
        (5, "remove r5 c"),
    ] {
        let mut p = ctx.prim_fresh("or-set").await?;
        merge(&mut p, &format!("r{i}"), &base).await?;
        p.send(command).await?;
        states.push(snapshot(&mut p, &format!("r{i}")).await?);
        p.close().await;
    }
    let mut lone = ctx.prim_fresh("or-set").await?;
    lone.send("add r4 d").await?;
    states.push(snapshot(&mut lone, "r4").await?);
    lone.close().await;

    // r1 removed the only add of `a` it had seen, but r3 added `a` again afterwards without
    // seeing the remove, so `a` lives. `b` and `c` were removed and nobody added them again,
    // so they are gone however late the removes arrive.
    let expected = serde_json::json!(["a", "d"]);

    let composite = {
        let mut m = ctx.prim_fresh("or-set").await?;
        for state in states.iter().skip(1).take(2) {
            merge(&mut m, "rm", state).await?;
        }
        let s = snapshot(&mut m, "rm").await?;
        m.close().await;
        s
    };
    let orders = replay_every_order(
        ctx,
        "or-set",
        &states,
        &composite,
        "elements rz",
        "elements",
        &expected,
    )
    .await?;
    ctx.note(format!(
        "replayed {orders} merge orders of {} states — a seed, three removes, a concurrent \
         re-add and an unrelated replica",
        states.len()
    ));
    ctx.note("every order has to read back [a, d]: the concurrent re-add of a survives");
    Ok(())
});
