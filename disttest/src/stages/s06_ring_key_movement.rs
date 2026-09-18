//! Stage 06 — Key movement when the ring changes.
//!
//! Stage 05 asked where the keys are. This one asks what happens to them when the
//! membership moves, which is the only reason anyone builds a hash ring instead of taking
//! `hash(key) % n`.
//!
//! The method is the same throughout: snapshot `locate` for a few hundred keys, change the
//! ring, snapshot again, and hand both maps to [`oracles::movement`], which sorts every key
//! into stayed, moved to a node that is new, moved off a node that is gone, or — the one
//! that must never happen — moved between two nodes that were there the whole time. A
//! modulo placement fails the very first test here and passes almost nothing else, which is
//! the point.

use crate::assert::{Check, Failure};
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::prim::{oracles, PrimProc};
use crate::stages::{Ladder, Stage, Test};
use std::collections::BTreeMap;

/// Stage 06.
pub fn stage() -> Stage {
    Stage {
        number: 6,
        slug: "ring_key_movement",
        name: "Key movement when the ring changes",
        ext: true,
        ladder: Ladder::Primitives,
        hints: &[
            "Adding a node may only take keys, never shuffle keys between two nodes that stayed",
            "Removing a node may only give its keys away; everything else stays put",
            "Roughly 1/N of the keys move when the Nth node joins — that is the whole point",
            "Re-adding a node that was removed must restore the mapping exactly",
        ],
        examples,
        tests: vec![
            Test::new(
                "adding a node never shuffles keys between the nodes that stayed",
                adding_never_shuffles_survivors,
            ),
            Test::new(
                "every key that moves when a node joins moves to that node",
                movers_move_to_the_newcomer,
            ),
            Test::new(
                "removing a node only moves the keys it owned",
                removing_only_moves_its_own,
            ),
            Test::new(
                "roughly one key in N moves when the Nth node joins",
                about_one_in_n_moves,
            ),
            Test::new(
                "removing a node and adding it back restores the mapping exactly",
                a_round_trip_restores_the_mapping,
            ),
            Test::new(
                "three joins in a row still never shuffle a key between survivors",
                repeated_joins_never_shuffle,
            )
            .ext(),
            Test::new(
                "a node leaving and a node joining only touch their own keys",
                a_swap_only_touches_the_pair,
            )
            .ext(),
            Test::new(
                "the order the nodes joined in does not change where a key lands",
                membership_order_does_not_matter,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("A fourth node joins", "consistent-hash", || {
            lines(&[
                "add n1 128",
                "add n2 128",
                "add n3 128",
                "locate user-42",
                "add n4 128",
                "locate user-42",
            ])
        })
        .request("three nodes, a key located, a fourth node, the same key located again")
        .response("the two `locate` answers are either the same node or the new one, never a third")
        .note(
            "A key may only stay where it is or move to `n4`. If `user-42` had been on `n1` \
             and answered `n2` after the join, the ring is not consistent hashing — it is \
             `hash(key) % n` wearing a costume, and every key in the store would be in the \
             wrong place after a join.",
        ),
        prim_example("A node leaves and comes back", "consistent-hash", || {
            lines(&[
                "add n1 128",
                "add n2 128",
                "add n3 128",
                "locate user-42",
                "remove n2",
                "locate user-42",
                "add n2 128",
                "locate user-42",
            ])
        })
        .request("a key located before a removal, during it, and after the node is back")
        .response("the first and third answers are identical")
        .note(
            "The points a node owns have to be derived from its name, not from when it \
             joined or from a counter. Seed a random generator per `add` and this last \
             `locate` answers something else, which means every key that node held is lost \
             after a restart.",
        ),
    ]
}

/// The keys this stage samples, `s06-k0`..`s06-k<n-1>`.
fn sample_keys(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("s06-k{i}")).collect()
}

/// Locate every key, returning key → node.
async fn locate_all(
    p: &mut PrimProc,
    keys: &[String],
) -> Result<BTreeMap<String, String>, Failure> {
    let mut out = BTreeMap::new();
    for key in keys {
        let command = format!("locate {key}");
        let answer = p.send(&command).await?;
        out.insert(key.clone(), p.expect_str(&answer, &command, "node")?);
    }
    Ok(out)
}

/// Add nodes `n1`.. with 128 points each.
async fn add_nodes(p: &mut PrimProc, count: usize) -> Result<Vec<String>, Failure> {
    let mut names = Vec::new();
    for i in 1..=count {
        let name = format!("n{i}");
        p.send(&format!("add {name} 128")).await?;
        names.push(name);
    }
    Ok(names)
}

/// The keys whose node changed between two snapshots.
fn movers<'a>(
    before: &'a BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> Vec<&'a String> {
    before
        .iter()
        .filter(|(k, was)| after.get(*k) != Some(*was))
        .map(|(k, _)| k)
        .collect()
}

/// Record the oracle's verdict in the report whether or not it fails anything.
fn observe_movement(c: &mut Check, m: &oracles::Movement) {
    c.observe("movement.stayed", m.stayed);
    c.observe("movement.moved_to_new", m.moved_to_new);
    c.observe("movement.moved_off_gone", m.moved_off_gone);
    c.observe(
        "movement.moved_between_survivors",
        m.moved_between_survivors.len(),
    );
}

dist_test!(adding_never_shuffles_survivors, |ctx| {
    let keys = sample_keys(500);
    let p = ctx.prim("consistent-hash").await?;
    let before_nodes = add_nodes(p, 4).await?;
    let before = locate_all(p, &keys).await?;
    p.send("add n5 128").await?;
    let after = locate_all(p, &keys).await?;
    let mut after_nodes = before_nodes.clone();
    after_nodes.push("n5".to_string());
    let m = oracles::movement(&before, &after, &before_nodes, &after_nodes);
    let stderr = p.stderr();
    let sample: Vec<&String> = m.moved_between_survivors.iter().take(5).collect();
    let mut c = Check::new("500 keys across a join, from four nodes to five");
    observe_movement(&mut c, &m);
    c.eq(
        "keys shuffled between nodes that stayed",
        0,
        m.moved_between_survivors.len(),
    );
    c.that(
        "the first of them",
        "no key at all: a join may only take keys, never redistribute them",
        sample.is_empty(),
        sample,
    );
    if !stderr.is_empty() {
        c.block("stderr", stderr);
    }
    c.finish()
});

dist_test!(movers_move_to_the_newcomer, |ctx| {
    let keys = sample_keys(500);
    let p = ctx.prim("consistent-hash").await?;
    let before_nodes = add_nodes(p, 3).await?;
    let before = locate_all(p, &keys).await?;
    p.send("add n4 128").await?;
    let after = locate_all(p, &keys).await?;
    let mut after_nodes = before_nodes.clone();
    after_nodes.push("n4".to_string());
    let m = oracles::movement(&before, &after, &before_nodes, &after_nodes);
    let elsewhere: Vec<(&String, &String)> = movers(&before, &after)
        .into_iter()
        .filter(|k| after.get(*k).map(String::as_str) != Some("n4"))
        .filter_map(|k| after.get(k).map(|now| (k, now)))
        .take(5)
        .collect();
    let mut c = Check::new("where the keys that moved on a join ended up");
    observe_movement(&mut c, &m);
    c.that(
        "locate.node after the join",
        "n4, because nothing else changed",
        elsewhere.is_empty(),
        elsewhere,
    );
    c.eq(
        "keys that moved off a node that is gone",
        0,
        m.moved_off_gone,
    );
    c.eq(
        "stayed + moved_to_new",
        keys.len(),
        m.stayed + m.moved_to_new,
    );
    c.finish()
});

dist_test!(removing_only_moves_its_own, |ctx| {
    let keys = sample_keys(500);
    let p = ctx.prim("consistent-hash").await?;
    let before_nodes = add_nodes(p, 5).await?;
    let before = locate_all(p, &keys).await?;
    p.send("remove n3").await?;
    let after = locate_all(p, &keys).await?;
    let after_nodes: Vec<String> = before_nodes
        .iter()
        .filter(|n| *n != "n3")
        .cloned()
        .collect();
    let m = oracles::movement(&before, &after, &before_nodes, &after_nodes);
    let on_n3 = before.values().filter(|n| n.as_str() == "n3").count();
    let wrongly_moved: Vec<(&String, &String)> = movers(&before, &after)
        .into_iter()
        .filter(|k| before.get(*k).map(String::as_str) != Some("n3"))
        .filter_map(|k| before.get(k).map(|was| (k, was)))
        .take(5)
        .collect();
    let still_there: Vec<&String> = after
        .values()
        .filter(|n| n.as_str() == "n3")
        .take(3)
        .collect();
    let mut c = Check::new("500 keys across a departure, from five nodes to four");
    observe_movement(&mut c, &m);
    c.observe("keys that had been on n3", on_n3);
    c.that(
        "the key that moved",
        "a key that had been on n3, because no other key had reason to move",
        wrongly_moved.is_empty(),
        wrongly_moved,
    );
    c.eq(
        "keys shuffled between nodes that stayed",
        0,
        m.moved_between_survivors.len(),
    );
    c.that(
        "locate.node",
        "never n3 again, since it left the ring",
        still_there.is_empty(),
        still_there,
    );
    c.eq("keys that moved to a node that is new", 0, m.moved_to_new);
    c.finish()
});

dist_test!(about_one_in_n_moves, |ctx| {
    // The fifth node should end up owning about a fifth of the key space, and the keys it
    // takes are exactly the ones that move. The band is wide on purpose: with 128 points
    // per node the share of any one node still wobbles by a few per cent, and a sample of
    // 500 keys adds a couple more. What the band rules out is 0 (a join that took nothing)
    // and anything near 1 (a rehash of the whole key space).
    let keys = sample_keys(500);
    let p = ctx.prim("consistent-hash").await?;
    add_nodes(p, 4).await?;
    let before = locate_all(p, &keys).await?;
    p.send("add n5 128").await?;
    let after = locate_all(p, &keys).await?;
    let moved = movers(&before, &after).len();
    let fraction = moved as f64 / keys.len() as f64;
    ctx.note(format!(
        "{moved} of {} keys moved when the fifth node joined ({:.1} %, the ideal is 20 %)",
        keys.len(),
        fraction * 100.0
    ));
    let mut c = Check::new("how much of the key space a join takes");
    c.observe("keys that moved", moved);
    c.observe("fraction that moved", format!("{fraction:.3}"));
    c.within("fraction that moved", 0.08, 0.40, fraction);
    c.finish()
});

dist_test!(a_round_trip_restores_the_mapping, |ctx| {
    // The trap is placing a node's points with a fresh random seed on every `add`. That
    // looks fine until a node restarts, at which point it owns a different slice of the
    // ring and every key it held is now somewhere else.
    let keys = sample_keys(400);
    let p = ctx.prim("consistent-hash").await?;
    add_nodes(p, 4).await?;
    let before = locate_all(p, &keys).await?;
    p.send("remove n2").await?;
    let without = locate_all(p, &keys).await?;
    p.send("add n2 128").await?;
    let after = locate_all(p, &keys).await?;
    let changed: Vec<(&String, &String, &String)> = keys
        .iter()
        .filter_map(|k| match (before.get(k), after.get(k)) {
            (Some(was), Some(now)) if was != now => Some((k, was, now)),
            _ => None,
        })
        .take(5)
        .collect();
    let moved_while_away = movers(&before, &without).len();
    let mut c = Check::new("a node removed and added again with the same point count");
    c.observe("keys that moved while n2 was away", moved_while_away);
    c.that(
        "locate.node after n2 came back",
        "exactly what it was before n2 left",
        changed.is_empty(),
        changed,
    );
    c.at_least("keys that moved while n2 was away", 1, moved_while_away);
    c.finish()
});

dist_test!(repeated_joins_never_shuffle, |ctx| {
    let keys = sample_keys(300);
    let p = ctx.prim("consistent-hash").await?;
    let mut nodes = add_nodes(p, 3).await?;
    let mut snapshot = locate_all(p, &keys).await?;
    let mut totals = Vec::new();
    let mut offenders: Vec<String> = Vec::new();
    for i in 4..=6 {
        let joining = format!("n{i}");
        p.send(&format!("add {joining} 128")).await?;
        let after = locate_all(p, &keys).await?;
        let mut next_nodes = nodes.clone();
        next_nodes.push(joining.clone());
        let m = oracles::movement(&snapshot, &after, &nodes, &next_nodes);
        totals.push((joining, m.stayed, m.moved_to_new));
        offenders.extend(m.moved_between_survivors.iter().take(3).cloned());
        nodes = next_nodes;
        snapshot = after;
    }
    let mut c = Check::new("three joins in a row, checked after each one");
    c.observe("stayed and moved after each join", format!("{totals:?}"));
    c.that(
        "keys shuffled between nodes that stayed",
        "none, after any of the three joins",
        offenders.is_empty(),
        offenders,
    );
    c.finish()
});

dist_test!(a_swap_only_touches_the_pair, |ctx| {
    let keys = sample_keys(400);
    let p = ctx.prim("consistent-hash").await?;
    let before_nodes = add_nodes(p, 5).await?;
    let before = locate_all(p, &keys).await?;
    p.send("remove n2").await?;
    p.send("add n6 128").await?;
    let after = locate_all(p, &keys).await?;
    let mut after_nodes: Vec<String> = before_nodes
        .iter()
        .filter(|n| *n != "n2")
        .cloned()
        .collect();
    after_nodes.push("n6".to_string());
    let m = oracles::movement(&before, &after, &before_nodes, &after_nodes);
    let mut c = Check::new("one node replaced by another");
    observe_movement(&mut c, &m);
    c.eq(
        "keys shuffled between nodes that stayed",
        0,
        m.moved_between_survivors.len(),
    );
    c.at_least("keys that moved off n2", 1, m.moved_off_gone);
    c.at_least("keys that moved onto n6", 1, m.moved_to_new);
    c.finish()
});

dist_test!(membership_order_does_not_matter, |ctx| {
    // Two conversations that cannot see each other, given the same four nodes in opposite
    // orders. A ring is a set of points, so the placement has to be identical; anything
    // that depends on insertion order — a list walked in arrival order, an index assigned
    // on join — differs here.
    let keys = sample_keys(200);
    let forwards = {
        let p = ctx.prim("consistent-hash").await?;
        add_nodes(p, 4).await?;
        locate_all(p, &keys).await?
    };
    let mut other = ctx.prim_fresh("consistent-hash").await?;
    for i in (1..=4).rev() {
        other.send(&format!("add n{i} 128")).await?;
    }
    let backwards = locate_all(&mut other, &keys).await?;
    other.close().await;
    let differing: Vec<(&String, &String, &String)> = keys
        .iter()
        .filter_map(|k| match (forwards.get(k), backwards.get(k)) {
            (Some(a), Some(b)) if a != b => Some((k, a, b)),
            _ => None,
        })
        .take(5)
        .collect();
    let mut c = Check::new("the same four nodes added in opposite orders");
    c.that(
        "locate.node",
        "the same node from both rings",
        differing.is_empty(),
        differing,
    );
    c.eq("keys located", keys.len(), backwards.len());
    c.finish()
});
