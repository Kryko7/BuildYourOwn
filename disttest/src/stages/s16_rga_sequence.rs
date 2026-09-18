//! Stage 16 — RGA: a replicated sequence.
//!
//! A counter merges by maximum and a set merges by union, and neither has to answer the
//! question a sequence does: *where*. Two replicas that both typed a character at position
//! three cannot both be right about position three, and there is no position three anyway
//! once a third replica has deleted position one.
//!
//! RGA answers it by never using positions. Every element is given an id and remembers the
//! id it was inserted after, so the sequence is a tree read depth first, and a merge is the
//! union of two sets of elements — commutative, associative and idempotent for free. Two
//! things follow, and both are tested here. Siblings inserted after the same element are
//! ordered by their ids, largest first, so concurrent inserts at one spot land the same way
//! on every replica without anyone asking. And a delete may only set a flag, because an
//! insert that has not arrived yet may still point at the element being removed; drop the
//! element and that insert has nowhere to go.
//!
//! The convergence test here deliberately does not pin which of two concurrent characters
//! comes first. It pins that every delivery order reads back the *same* string, and that
//! the string holds the right characters — which is what the merge laws actually promise.

use crate::assert::{Check, Failure};
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::prim::{oracles, PrimProc};
use crate::stages::{Ctx, Ladder, Stage, Test};

/// Stage 16.
pub fn stage() -> Stage {
    Stage {
        number: 16,
        slug: "rga_sequence",
        name: "RGA: a replicated sequence",
        ext: true,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `rga`: every element has an id and points at the element it was inserted after",
            "Concurrent inserts at the same position are ordered by id, consistently on every replica",
            "A delete leaves a tombstone: later inserts may still point at it",
            "Every delivery order of the same operations must read back the same sequence",
        ],
        examples,
        tests: vec![
            Test::new("a sequence builds up as it is typed", a_sequence_builds_up),
            Test::new("inserting after root prepends", inserting_after_root_prepends),
            Test::new("a delete takes the character out of the text", a_delete_hides_a_character),
            Test::new(
                "an insert after a deleted element still lands in its place",
                an_insert_after_a_tombstone_lands_in_place,
            ),
            Test::new(
                "two replicas inserting at the same position converge",
                two_replicas_at_one_position_converge,
            ),
            Test::new(
                "both replicas read back the same sequence after a longer exchange",
                a_longer_exchange_converges,
            ),
            Test::new(
                "a tombstone survives a state that predates the delete",
                a_tombstone_is_not_resurrected,
            )
            .ext(),
            Test::new(
                "every delivery order of inserts and deletes converges",
                every_delivery_order_converges,
            )
            .ext()
            .min_timeout_ms(60_000),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("Typing, and then typing at the front", "rga", || {
            lines(&[
                "insert r1 root a1 h",
                "insert r1 a1 a2 i",
                "text r1",
                "insert r1 root a3 o",
                "text r1",
            ])
        })
        .request("two characters typed in order, then a third inserted after `root`")
        .response("`hi`, and then `ohi`: `root` is the start of the sequence, not the end")
        .note(
            "`a3` and `a1` were both inserted after `root`, so they are siblings, and the \
             larger id goes first. That single rule is what makes two replicas that never \
             spoke agree on the order of characters they typed at the same spot.",
        ),
        prim_example("A delete leaves something behind", "rga", || {
            lines(&[
                "insert r1 root a1 h",
                "insert r1 a1 a2 i",
                "delete r1 a2",
                "text r1",
                "insert r1 a2 a3 o",
                "text r1",
            ])
        })
        .request("the second character is deleted, then something is inserted after it")
        .response("`h`, and then `ho`: the insert still found the place it pointed at")
        .note(
            "The deleted element has to stay in the structure with a flag on it. A replica \
             that erases it cannot place `a3` at all when this arrives from elsewhere, and \
             the two replicas have diverged over a character nobody can see.",
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

/// Merge a shipped state into a replica and read the text it reports.
async fn merge(p: &mut PrimProc, dst: &str, state: &str) -> Result<String, Failure> {
    let command = format!("merge {dst} {state}");
    let answer = p.send(&command).await?;
    p.expect_str(&answer, &command, "text")
}

/// The sequence a replica reads back.
async fn text(p: &mut PrimProc, replica: &str) -> Result<String, Failure> {
    let command = format!("text {replica}");
    let answer = p.send(&command).await?;
    p.expect_str(&answer, &command, "text")
}

/// The characters of a string, sorted — the part of a sequence that is not up to the
/// implementation's tie-break.
fn characters(s: &str) -> String {
    let mut chars: Vec<char> = s.chars().collect();
    chars.sort_unstable();
    chars.into_iter().collect()
}

/// Replay every permutation of `states` into a fresh replica and demand one answer.
///
/// Every replay also merges `composite`, a state that is itself the merge of two others —
/// associativity — and delivers the order's first state a second time — idempotence. The
/// first order that disagrees is reported along with the order itself, because which order
/// broke it is the only question worth answering.
async fn replay_every_order(
    ctx: &mut Ctx,
    states: &[String],
    composite: &str,
    expected: &str,
) -> Result<usize, Failure> {
    let orders = oracles::permutations(states.len());
    for order in &orders {
        let mut f = ctx.prim_fresh("rga").await?;
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
        let after_duplicate = merge(&mut f, "rz", again).await?;
        let read_back = text(&mut f, "rz").await?;
        f.close().await;
        if read_back != expected || after_duplicate != read_back {
            let mut c = Check::new("a replica that has merged every state");
            c.note(format!("the states arrived in the order {order:?}"));
            c.note("then an already-merged state, then the first one a second time");
            c.eq("text rz", expected.to_string(), read_back.clone());
            c.eq(
                "the text the last merge answered with",
                read_back,
                after_duplicate,
            );
            c.finish()?;
        }
    }
    Ok(orders.len())
}

/// Type a chain of characters, each inserted after the one before it.
///
/// Ids are `<prefix>1`, `<prefix>2`, ... and the answer is the id of each character, so a
/// test can point a later insert or a delete at whichever one it means.
async fn type_chain(
    p: &mut PrimProc,
    replica: &str,
    prefix: &str,
    word: &str,
) -> Result<Vec<String>, Failure> {
    let mut ids = Vec::new();
    let mut after = "root".to_string();
    for (i, ch) in word.chars().enumerate() {
        let id = format!("{prefix}{}", i + 1);
        p.send(&format!("insert {replica} {after} {id} {ch}"))
            .await?;
        after = id.clone();
        ids.push(id);
    }
    Ok(ids)
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(a_sequence_builds_up, |ctx| {
    let p = ctx.prim("rga").await?;
    let command = "insert r1 root a1 h";
    let answer = p.send(command).await?;
    let after_first = p.expect_str(&answer, command, "text")?;
    let command = "insert r1 a1 a2 e";
    let answer = p.send(command).await?;
    let after_second = p.expect_str(&answer, command, "text")?;
    p.send("insert r1 a2 a3 l").await?;
    p.send("insert r1 a3 a4 l").await?;
    let command = "insert r1 a4 a5 o";
    let answer = p.send(command).await?;
    let after_last = p.expect_str(&answer, command, "text")?;
    let read_back = text(p, "r1").await?;
    let mut c = Check::new("five characters typed in order");
    // Every insert answers with the whole sequence, not with what it inserted.
    c.eq(
        "the text after the first insert",
        "h".to_string(),
        after_first,
    );
    c.eq(
        "the text after the second insert",
        "he".to_string(),
        after_second,
    );
    c.eq(
        "the text the last insert answered with",
        "hello".to_string(),
        after_last,
    );
    c.eq("text r1", "hello".to_string(), read_back);
    c.finish()
});

dist_test!(inserting_after_root_prepends, |ctx| {
    let p = ctx.prim("rga").await?;
    type_chain(p, "r1", "a", "bc").await?;
    let built = text(p, "r1").await?;
    // `a3` and `a1` are siblings, both inserted after root, and the larger id goes first.
    p.send("insert r1 root a3 a").await?;
    let prepended = text(p, "r1").await?;
    let mut c = Check::new("an insert pointing at the start of the sequence");
    c.note("root is the start of the sequence, so its newest child is the first character");
    c.eq("text after typing bc", "bc".to_string(), built);
    c.eq(
        "text after inserting a after root",
        "abc".to_string(),
        prepended,
    );
    c.finish()
});

dist_test!(a_delete_hides_a_character, |ctx| {
    let p = ctx.prim("rga").await?;
    let ids = type_chain(p, "r1", "a", "hello").await?;
    let Some(second) = ids.get(1) else {
        return Err(Failure::harness(
            "typing hello produced no second character",
        ));
    };
    let command = format!("delete r1 {second}");
    let answer = p.send(&command).await?;
    let after_delete = p.expect_str(&answer, &command, "text")?;
    let read_back = text(p, "r1").await?;
    let mut c = Check::new("one character deleted out of the middle");
    c.eq(
        "the text the delete answered with",
        "hllo".to_string(),
        after_delete,
    );
    c.eq("text r1", "hllo".to_string(), read_back);
    c.finish()
});

dist_test!(an_insert_after_a_tombstone_lands_in_place, |ctx| {
    let p = ctx.prim("rga").await?;
    let ids = type_chain(p, "r1", "a", "hello").await?;
    let Some(second) = ids.get(1).cloned() else {
        return Err(Failure::harness(
            "typing hello produced no second character",
        ));
    };
    p.send(&format!("delete r1 {second}")).await?;
    // The insert points at an element nobody can see any more. It still knows where it
    // belongs, because the element is still there with a flag on it.
    p.send(&format!("insert r1 {second} a9 E")).await?;
    let after = text(p, "r1").await?;
    let mut c = Check::new("an insert pointing at a deleted element");
    c.note("E was inserted after the deleted e, so it takes the place e had");
    c.eq("text r1", "hEllo".to_string(), after);
    c.finish()
});

dist_test!(two_replicas_at_one_position_converge, |ctx| {
    let mut a = ctx.prim_fresh("rga").await?;
    let mut b = ctx.prim_fresh("rga").await?;
    type_chain(&mut a, "r1", "a", "hi").await?;
    let base = snapshot(&mut a, "r1").await?;
    merge(&mut b, "r2", &base).await?;
    // Both replicas type one character after the same element, neither having seen the
    // other. There is no right answer to which comes first; there is only one answer.
    a.send("insert r1 a2 a7 X").await?;
    b.send("insert r2 a2 b7 Y").await?;
    let state_a = snapshot(&mut a, "r1").await?;
    let state_b = snapshot(&mut b, "r2").await?;
    merge(&mut a, "r1", &state_b).await?;
    merge(&mut b, "r2", &state_a).await?;
    let a_text = text(&mut a, "r1").await?;
    let b_text = text(&mut b, "r2").await?;
    a.close().await;
    b.close().await;
    let mut c = Check::new("two characters typed concurrently at one position");
    c.eq("the two replicas' texts", a_text.clone(), b_text);
    c.eq(
        "the characters in the sequence",
        "XYhi".to_string(),
        characters(&a_text),
    );
    c.that(
        "text",
        "the two characters that were already there, still in order",
        a_text.starts_with("hi"),
        a_text,
    );
    c.finish()
});

dist_test!(a_longer_exchange_converges, |ctx| {
    let mut a = ctx.prim_fresh("rga").await?;
    let mut b = ctx.prim_fresh("rga").await?;
    let ids = type_chain(&mut a, "r1", "a", "abcd").await?;
    let base = snapshot(&mut a, "r1").await?;
    merge(&mut b, "r2", &base).await?;
    let Some(first) = ids.first().cloned() else {
        return Err(Failure::harness("typing abcd produced no characters"));
    };
    let Some(third) = ids.get(2).cloned() else {
        return Err(Failure::harness("typing abcd produced no third character"));
    };
    // Each replica edits in a place the other is not looking at, and each deletes one
    // character; the two deletes overlap on nothing.
    a.send(&format!("insert r1 {first} a8 P")).await?;
    a.send(&format!("delete r1 {third}")).await?;
    b.send("insert r2 root b8 Q").await?;
    b.send(&format!("insert r2 {third} b9 R")).await?;
    let state_a = snapshot(&mut a, "r1").await?;
    let state_b = snapshot(&mut b, "r2").await?;
    merge(&mut a, "r1", &state_b).await?;
    merge(&mut b, "r2", &state_a).await?;
    let a_text = text(&mut a, "r1").await?;
    let b_text = text(&mut b, "r2").await?;
    a.close().await;
    b.close().await;
    let mut c = Check::new("two replicas editing the same sequence in different places");
    c.eq("the two replicas' texts", a_text.clone(), b_text);
    // The deleted `c` is gone on both sides; R, which was inserted after it, is not.
    c.eq(
        "the characters in the sequence",
        characters("QaPbdR"),
        characters(&a_text),
    );
    c.finish()
});

dist_test!(a_tombstone_is_not_resurrected, |ctx| {
    let mut a = ctx.prim_fresh("rga").await?;
    let ids = type_chain(&mut a, "r1", "a", "hi").await?;
    let Some(first) = ids.first().cloned() else {
        return Err(Failure::harness("typing hi produced no characters"));
    };
    // A state taken while both characters were alive.
    let before_delete = snapshot(&mut a, "r1").await?;
    a.send(&format!("delete r1 {first}")).await?;
    let after_delete = text(&mut a, "r1").await?;
    let after_stale = merge(&mut a, "r1", &before_delete).await?;

    // And the other way round: a replica that only knows the older state has to take the
    // delete when it finally hears about it.
    let mut b = ctx.prim_fresh("rga").await?;
    merge(&mut b, "r2", &before_delete).await?;
    let b_before = text(&mut b, "r2").await?;
    let state_a = snapshot(&mut a, "r1").await?;
    let b_after = merge(&mut b, "r2", &state_a).await?;
    a.close().await;
    b.close().await;

    let mut c = Check::new("a delete and a state that predates it");
    c.eq("text after the delete", "i".to_string(), after_delete);
    c.eq(
        "text after merging the older state back",
        "i".to_string(),
        after_stale,
    );
    c.eq(
        "the other replica before it heard",
        "hi".to_string(),
        b_before,
    );
    c.eq("the other replica after it heard", "i".to_string(), b_after);
    c.finish()
});

dist_test!(every_delivery_order_converges, |ctx| {
    // One replica types `hi`; four others each see that and do one thing of their own, and a
    // sixth never heard any of it. Two of the four delete a character another one has just
    // inserted after, which is the case a structure without tombstones cannot merge. Six
    // states is 720 delivery orders, and every one of them is replayed.
    let mut seed = ctx.prim_fresh("rga").await?;
    type_chain(&mut seed, "r0", "a", "hi").await?;
    let base = snapshot(&mut seed, "r0").await?;
    seed.close().await;

    let mut states = vec![base.clone()];
    for (i, command) in [
        (1usize, "insert r1 a2 c1 X"),
        (2, "insert r2 a1 c2 Y"),
        (3, "delete r3 a1"),
        (5, "delete r5 a2"),
    ] {
        let mut p = ctx.prim_fresh("rga").await?;
        merge(&mut p, &format!("r{i}"), &base).await?;
        p.send(command).await?;
        states.push(snapshot(&mut p, &format!("r{i}")).await?);
        p.close().await;
    }
    let mut lone = ctx.prim_fresh("rga").await?;
    lone.send("insert r4 root c4 Z").await?;
    states.push(snapshot(&mut lone, "r4").await?);
    lone.close().await;

    // The oracle is not "this exact string": which of two concurrent siblings comes first
    // is the implementation's own consistent choice. It is that every order reads back the
    // same string, and that the string holds the characters it should.
    let composite = {
        let mut m = ctx.prim_fresh("rga").await?;
        for state in states.iter().skip(1).take(2) {
            merge(&mut m, "rm", state).await?;
        }
        let s = snapshot(&mut m, "rm").await?;
        m.close().await;
        s
    };
    let reference = {
        let mut r = ctx.prim_fresh("rga").await?;
        for state in &states {
            merge(&mut r, "rr", state).await?;
        }
        merge(&mut r, "rr", &composite).await?;
        let t = text(&mut r, "rr").await?;
        r.close().await;
        t
    };

    let mut c = Check::new("the sequence every replica has to end up with");
    c.note("h and i were deleted; X, Y and Z were typed concurrently in three places");
    c.eq(
        "the characters of the merged sequence",
        characters("XYZ"),
        characters(&reference),
    );
    c.finish()?;

    let orders = replay_every_order(ctx, &states, &composite, &reference).await?;
    ctx.note(format!(
        "replayed {orders} merge orders of {} states — a seed, two concurrent inserts, two \
         deletes and an unrelated replica",
        states.len()
    ));
    ctx.note(format!(
        "every order read back {reference:?}, with a duplicate delivery and an \
         already-merged state thrown in"
    ));
    Ok(())
});
