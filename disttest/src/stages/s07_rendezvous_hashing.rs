//! Stage 07 — Rendezvous (HRW) hashing.
//!
//! No ring, no points, no sorted structure: score every node for the key and take the
//! largest. The whole scheme is one function, which makes the properties sharper than the
//! ring's. A ring only promises that a departure moves the leaving node's keys; rendezvous
//! promises where each of them goes, because the ranking of the surviving nodes was already
//! fixed before the node left.
//!
//! As in stage 05 nothing here predicts a score. Every test is a property of the ranking —
//! it is total, it is stable, it is a superset of `locate`, and it only changes at the
//! entries that joined or left.

use crate::assert::{Check, Failure};
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::prim::{oracles, PrimProc};
use crate::stages::{Ladder, Stage, Test};
use serde_json::Value;
use std::collections::BTreeMap;

/// Stage 07.
pub fn stage() -> Stage {
    Stage {
        number: 7,
        slug: "rendezvous_hashing",
        name: "Rendezvous (HRW) hashing",
        ext: true,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `rendezvous`: score every node for the key and take the highest",
            "No ring and no virtual nodes: the score function does all the work",
            "`locate-k` returns the top k in score order, and its head is what `locate` answers",
            "Removing a node only moves the keys it owned, exactly as a ring does",
        ],
        examples,
        tests: vec![
            Test::new(
                "the head of locate-k is what locate answers",
                the_head_is_locate,
            ),
            Test::new(
                "locate-k ranks every node once, with no repeats",
                the_ranking_is_strict,
            ),
            Test::new(
                "asking for more nodes than there are returns every one",
                k_beyond_the_membership,
            ),
            Test::new(
                "the same key ranks the nodes the same way every time",
                the_ranking_is_stable,
            ),
            Test::new("one node owns every key", one_node_owns_everything),
            Test::new(
                "adding a node only takes keys, it never shuffles them",
                adding_only_takes_keys,
            ),
            Test::new(
                "removing a node only moves the keys it owned",
                removing_only_moves_its_own,
            ),
            Test::new(
                "removing the best node promotes the runner-up",
                the_runner_up_is_promoted,
            )
            .ext(),
            Test::new(
                "removing a node leaves the others in the same order",
                the_rest_of_the_ranking_holds,
            )
            .ext(),
            Test::new(
                "locating with no nodes answers instead of crashing",
                no_nodes_at_all,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("Scoring one key against three nodes", "rendezvous", || {
            lines(&[
                "add n1",
                "add n2",
                "add n3",
                "locate-k user-42 3",
                "locate user-42",
            ])
        })
        .request("three nodes, the whole ranking for one key, then the plain lookup")
        .response("three distinct nodes in descending score order, then the first of them")
        .note(
            "`locate` is not a second implementation. It is `locate-k <key> 1` with the \
             brackets taken off, and the suite checks exactly that: a head that disagrees \
             with the ranking means two different score functions are in the program.",
        ),
        prim_example("A node leaves", "rendezvous", || {
            lines(&[
                "add n1",
                "add n2",
                "add n3",
                "locate-k user-42 3",
                "remove n1",
                "locate-k user-42 3",
                "locate user-42",
            ])
        })
        .request("the ranking for a key, then the same ranking without `n1`")
        .response("the second ranking is the first with `n1` deleted — the rest keep their order")
        .note(
            "This is what rendezvous buys over a ring: not only do the survivors keep their \
             keys, the *replicas* keep their order, so a key whose owner died already knows \
             which node takes over without asking anybody.",
        ),
    ]
}

/// The keys this stage samples, `s07-k0`..`s07-k<n-1>`.
fn sample_keys(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("s07-k{i}")).collect()
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

/// Rank every key, returning key → the top `k` nodes in order.
async fn rank_all(
    p: &mut PrimProc,
    keys: &[String],
    k: usize,
) -> Result<BTreeMap<String, Vec<String>>, Failure> {
    let mut out = BTreeMap::new();
    for key in keys {
        let command = format!("locate-k {key} {k}");
        let answer = p.send(&command).await?;
        out.insert(key.clone(), p.expect_strs(&answer, &command, "nodes")?);
    }
    Ok(out)
}

/// Add nodes `n1`.. and hand back their names.
async fn add_nodes(p: &mut PrimProc, count: usize) -> Result<Vec<String>, Failure> {
    let mut names = Vec::new();
    for i in 1..=count {
        let name = format!("n{i}");
        p.send(&format!("add {name}")).await?;
        names.push(name);
    }
    Ok(names)
}

dist_test!(the_head_is_locate, |ctx| {
    let keys = sample_keys(60);
    let p = ctx.prim("rendezvous").await?;
    add_nodes(p, 5).await?;
    let ranking = rank_all(p, &keys, 3).await?;
    let placement = locate_all(p, &keys).await?;
    let disagreeing: Vec<(&String, Option<&String>, Option<&String>)> = keys
        .iter()
        .filter(|k| ranking.get(*k).and_then(|r| r.first()) != placement.get(*k))
        .map(|k| (k, ranking.get(k).and_then(|r| r.first()), placement.get(k)))
        .take(5)
        .collect();
    let mut c = Check::new("locate against the head of locate-k, over 60 keys");
    c.that(
        "(key, locate-k.nodes[0], locate.node)",
        "the same node from both commands",
        disagreeing.is_empty(),
        disagreeing,
    );
    c.eq("keys ranked", keys.len(), ranking.len());
    c.finish()
});

dist_test!(the_ranking_is_strict, |ctx| {
    let keys = sample_keys(60);
    let p = ctx.prim("rendezvous").await?;
    let members = add_nodes(p, 6).await?;
    let ranking = rank_all(p, &keys, 6).await?;
    let mut short: Vec<(&String, usize)> = Vec::new();
    let mut repeated: Vec<(&String, Vec<String>)> = Vec::new();
    let mut strangers: Vec<(&String, String)> = Vec::new();
    for (key, ranked) in &ranking {
        if ranked.len() != members.len() {
            short.push((key, ranked.len()));
        }
        let mut sorted = ranked.clone();
        sorted.sort();
        sorted.dedup();
        if sorted.len() != ranked.len() {
            repeated.push((key, ranked.clone()));
        }
        for node in ranked {
            if !members.contains(node) {
                strangers.push((key, node.clone()));
            }
        }
    }
    short.truncate(3);
    repeated.truncate(3);
    strangers.truncate(3);
    let mut c = Check::new("locate-k over all six nodes, for 60 keys");
    c.that(
        "locate-k.nodes.len()",
        "six nodes, one entry per member",
        short.is_empty(),
        short,
    );
    c.that(
        "locate-k.nodes",
        "no node twice: a ranking is an order, not a multiset",
        repeated.is_empty(),
        repeated,
    );
    c.that(
        "locate-k.nodes",
        "only nodes that were added",
        strangers.is_empty(),
        strangers,
    );
    c.finish()
});

dist_test!(k_beyond_the_membership, |ctx| {
    // Asking for ten replicas of a key when four nodes exist is not an error: it is what a
    // preference list does when the cluster is small, and the honest answer is everybody.
    let p = ctx.prim("rendezvous").await?;
    let members = add_nodes(p, 4).await?;
    let command = "locate-k user-42 10";
    let answer = p.send(command).await?;
    let ranked = p.expect_strs(&answer, command, "nodes")?;
    let exact = p.send("locate-k user-42 4").await?;
    let four = p.expect_strs(&exact, "locate-k user-42 4", "nodes")?;
    let transcript = p.transcript_block();
    let mut sorted = ranked.clone();
    sorted.sort();
    let mut expected = members.clone();
    expected.sort();
    let mut c = Check::new("asking for ten of four nodes");
    c.eq("locate-k(k=10).nodes.len()", members.len(), ranked.len());
    c.eq("locate-k(k=10).nodes, sorted", expected, sorted);
    c.eq("locate-k(k=10).nodes", four, ranked);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(the_ranking_is_stable, |ctx| {
    let p = ctx.prim("rendezvous").await?;
    add_nodes(p, 5).await?;
    let command = "locate-k user-42 5";
    let first = p.send(command).await?;
    let ranked = p.expect_strs(&first, command, "nodes")?;
    let mut repeats = Vec::new();
    for _ in 0..4 {
        // Other keys in between: a score function that carries state from one call to the
        // next gives itself away here.
        p.send("locate-k someone-else 5").await?;
        p.send("locate another-key").await?;
        let again = p.send(command).await?;
        repeats.push(p.expect_strs(&again, command, "nodes")?);
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("one key ranked five times, with other work in between");
    for (i, again) in repeats.iter().enumerate() {
        c.eq(
            &format!("locate-k(user-42)[{}].nodes", i + 1),
            &ranked,
            again,
        );
    }
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(one_node_owns_everything, |ctx| {
    let keys = sample_keys(40);
    let p = ctx.prim("rendezvous").await?;
    p.send("add only").await?;
    let placement = locate_all(p, &keys).await?;
    let command = "locate-k s07-k0 3";
    let answer = p.send(command).await?;
    let ranked = p.expect_strs(&answer, command, "nodes")?;
    let elsewhere: Vec<(&String, &String)> = placement
        .iter()
        .filter(|(_, node)| node.as_str() != "only")
        .take(3)
        .collect();
    let mut c = Check::new("a single-node membership");
    c.that(
        "locate.node",
        "the only node there is",
        elsewhere.is_empty(),
        elsewhere,
    );
    c.eq("locate-k(k=3).nodes", vec!["only".to_string()], ranked);
    c.finish()
});

dist_test!(adding_only_takes_keys, |ctx| {
    let keys = sample_keys(300);
    let p = ctx.prim("rendezvous").await?;
    let before_nodes = add_nodes(p, 4).await?;
    let before = locate_all(p, &keys).await?;
    p.send("add n5").await?;
    let after = locate_all(p, &keys).await?;
    let mut after_nodes = before_nodes.clone();
    after_nodes.push("n5".to_string());
    let m = oracles::movement(&before, &after, &before_nodes, &after_nodes);
    let sample: Vec<&String> = m.moved_between_survivors.iter().take(5).collect();
    let mut c = Check::new("300 keys across a join, from four nodes to five");
    c.observe("movement.stayed", m.stayed);
    c.observe("movement.moved_to_new", m.moved_to_new);
    c.eq(
        "keys shuffled between nodes that stayed",
        0,
        m.moved_between_survivors.len(),
    );
    c.that(
        "the first of them",
        "no key at all: a join may only take keys",
        sample.is_empty(),
        sample,
    );
    c.at_least("keys that moved to n5", 1, m.moved_to_new);
    c.finish()
});

dist_test!(removing_only_moves_its_own, |ctx| {
    let keys = sample_keys(300);
    let p = ctx.prim("rendezvous").await?;
    let before_nodes = add_nodes(p, 5).await?;
    let before = locate_all(p, &keys).await?;
    p.send("remove n4").await?;
    let after = locate_all(p, &keys).await?;
    let after_nodes: Vec<String> = before_nodes
        .iter()
        .filter(|n| *n != "n4")
        .cloned()
        .collect();
    let m = oracles::movement(&before, &after, &before_nodes, &after_nodes);
    let wrongly_moved: Vec<(&String, &String, &String)> = keys
        .iter()
        .filter_map(|k| match (before.get(k), after.get(k)) {
            (Some(was), Some(now)) if was != now && was != "n4" => Some((k, was, now)),
            _ => None,
        })
        .take(5)
        .collect();
    let mut c = Check::new("300 keys across a departure, from five nodes to four");
    c.observe("movement.stayed", m.stayed);
    c.observe("movement.moved_off_gone", m.moved_off_gone);
    c.that(
        "(key, before, after)",
        "no movement at all, because only n4's keys had reason to move",
        wrongly_moved.is_empty(),
        wrongly_moved,
    );
    c.eq(
        "keys shuffled between nodes that stayed",
        0,
        m.moved_between_survivors.len(),
    );
    c.at_least("keys that moved off n4", 1, m.moved_off_gone);
    c.finish()
});

dist_test!(the_runner_up_is_promoted, |ctx| {
    // The property a preference list rests on: the node that takes over from a dead owner
    // is the one that was already second, and every client works that out alone.
    let keys = sample_keys(150);
    let p = ctx.prim("rendezvous").await?;
    add_nodes(p, 5).await?;
    let before = rank_all(p, &keys, 2).await?;
    p.send("remove n2").await?;
    let after = locate_all(p, &keys).await?;
    let mut wrong: Vec<(&String, String, Option<&String>)> = Vec::new();
    let mut promoted = 0usize;
    for key in &keys {
        let Some(ranked) = before.get(key) else {
            continue;
        };
        let expected = match ranked.first().map(String::as_str) {
            Some("n2") => {
                promoted += 1;
                ranked.get(1).cloned()
            }
            _ => ranked.first().cloned(),
        };
        if expected.as_ref() != after.get(key) {
            wrong.push((key, format!("{ranked:?}"), after.get(key)));
        }
    }
    wrong.truncate(5);
    let mut c = Check::new("the owner of 150 keys removed, against the runner-up from before");
    c.observe("keys whose owner was n2", promoted);
    c.that(
        "(key, locate-k before, locate.node after)",
        "the node that was second before n2 left",
        wrong.is_empty(),
        wrong,
    );
    c.at_least("keys whose owner was n2", 1, promoted);
    c.finish()
});

dist_test!(the_rest_of_the_ranking_holds, |ctx| {
    let keys = sample_keys(100);
    let p = ctx.prim("rendezvous").await?;
    add_nodes(p, 6).await?;
    let before = rank_all(p, &keys, 6).await?;
    p.send("remove n3").await?;
    let after = rank_all(p, &keys, 5).await?;
    let mut wrong: Vec<(&String, String, String)> = Vec::new();
    for key in &keys {
        let expected: Vec<String> = before
            .get(key)
            .map(|r| r.iter().filter(|n| n.as_str() != "n3").cloned().collect())
            .unwrap_or_default();
        let actual = after.get(key).cloned().unwrap_or_default();
        if expected != actual {
            wrong.push((key, format!("{expected:?}"), format!("{actual:?}")));
        }
    }
    wrong.truncate(5);
    let mut c = Check::new("the full ranking of 100 keys before and after a removal");
    c.that(
        "(key, expected ranking, locate-k.nodes)",
        "the ranking from before with n3 deleted and nothing else touched",
        wrong.is_empty(),
        wrong,
    );
    c.finish()
});

dist_test!(no_nodes_at_all, |ctx| {
    let p = ctx.prim("rendezvous").await?;
    p.send("add n1").await?;
    p.send("remove n1").await?;
    let located = p.send("locate user-42").await?;
    let claimed = located
        .get("node")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let ranked = p.send("locate-k user-42 3").await?;
    let listed = ranked
        .get("nodes")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    p.send("add n1").await?;
    let back = p.send("locate user-42").await?;
    let node = p.expect_str(&back, "locate user-42", "node")?;
    let transcript = p.transcript_block();
    let mut c = Check::new("locating when every node has been removed");
    c.that(
        "locate.node",
        "no node at all, because there is none to name",
        claimed.is_none(),
        claimed,
    );
    c.eq("locate-k.nodes.len()", 0, listed);
    c.eq("locate.node once n1 is back", "n1".to_string(), node);
    c.block("transcript", transcript);
    c.finish()
});
