//! Stage 05 — Consistent hashing with virtual nodes.
//!
//! The tester cannot predict which node a key hashes to, because the hash is yours. So
//! nothing here asserts a placement. What it asserts is every property that makes the
//! scheme worth having and that holds whatever the hash is: `locate` is a function, it only
//! ever names a node that is actually on the ring, a single node owns everything, and the
//! load over a few thousand keys is even *because* of the virtual nodes rather than in
//! spite of them.
//!
//! The load test is a statistical envelope, so the measured imbalance always goes into the
//! report — passing or failing, the run says what it saw. Imbalance is the largest
//! deviation from an even split as a fraction of the mean, computed by
//! [`oracles::imbalance`], which is the tester's own arithmetic over the counts your `stats`
//! reported.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::prim::{oracles, PrimProc};
use crate::stages::{Ctx, Ladder, Stage, Test};
use serde_json::Value;
use std::collections::BTreeMap;

/// Stage 05.
pub fn stage() -> Stage {
    Stage {
        number: 5,
        slug: "consistent_hashing",
        name: "Consistent hashing with virtual nodes",
        ext: false,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `consistent-hash`: place `vnodes` points per node on a ring and walk clockwise",
            "`locate` is a function: the same key must always answer the same node",
            "Virtual nodes are what makes the load even — one point per node is visibly lumpy",
            "`stats` reports how many of the sampled keys landed on each node",
        ],
        examples,
        tests: vec![
            Test::new("a key always lands on the same node", locate_is_a_function),
            Test::new(
                "a key only ever lands on a node that is on the ring",
                only_names_a_real_node,
            ),
            Test::new("one node owns every key", one_node_owns_everything),
            Test::new(
                "the answer does not drift while the ring stands still",
                no_drift_while_the_ring_stands_still,
            ),
            Test::new(
                "virtual nodes spread the keys evenly",
                virtual_nodes_spread_the_load,
            ),
            Test::new(
                "one point per node is visibly lumpier",
                one_point_per_node_is_lumpy,
            )
            .ext(),
            Test::new("stats counts every sampled key exactly once", stats_adds_up),
            Test::new(
                "an empty ring answers instead of crashing",
                an_empty_ring_answers,
            ),
            Test::new(
                "removing a node that was never added leaves the ring alone",
                removing_a_stranger_changes_nothing,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("A two-node ring and one key", "consistent-hash", || {
            lines(&[
                "add n1 128",
                "add n2 128",
                "locate user-42",
                "locate user-42",
            ])
        })
        .request("two nodes with the default 128 points each, then the same key located twice")
        .response("the node count after each add, then the same node name both times")
        .note(
            "Which node answers is your hash's business and no test here predicts it. That \
             the second answer equals the first is not: `locate` is a pure function of the \
             key and the current membership.",
        ),
        prim_example("What the virtual nodes are for", "consistent-hash", || {
            lines(&[
                "add n1 128",
                "add n2 128",
                "add n3 128",
                "add n4 128",
                "stats 4000",
            ])
        })
        .request("four nodes, then where `key0`..`key3999` land")
        .response("a count per node, all four within a few per cent of a quarter of the sample")
        .note(
            "Run the same thing with `add n1 1` and the counts stop looking like quarters: \
             four random points on a ring cut it into four very unequal arcs, and the arc is \
             the share of the key space. That is the whole reason for the points.",
        ),
    ]
}

/// The keys this stage samples, `s05-k0`..`s05-k<n-1>`.
fn sample_keys(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("s05-k{i}")).collect()
}

/// Locate every key, returning key → node.
async fn locate_all(
    p: &mut PrimProc,
    keys: &[String],
) -> Result<BTreeMap<String, String>, crate::assert::Failure> {
    let mut out = BTreeMap::new();
    for key in keys {
        let command = format!("locate {key}");
        let answer = p.send(&command).await?;
        out.insert(key.clone(), p.expect_str(&answer, &command, "node")?);
    }
    Ok(out)
}

/// Read a `stats` answer as node → count.
fn stats_counts(
    p: &PrimProc,
    answer: &Value,
    command: &str,
) -> Result<BTreeMap<String, i64>, crate::assert::Failure> {
    let Some(Value::Object(map)) = answer.get("nodes") else {
        return Err(p.shape(command, "the answer has no \"nodes\" object"));
    };
    let mut out = BTreeMap::new();
    for (node, value) in map {
        let count = match value {
            Value::Number(n) => n.as_i64(),
            Value::String(s) => s.parse().ok(),
            _ => None,
        };
        let Some(count) = count else {
            return Err(p.shape(command, &format!("nodes.{node} is not a whole number")));
        };
        out.insert(node.clone(), count);
    }
    Ok(out)
}

/// Add `count` nodes named `n1`.. with the given number of points each.
async fn add_nodes(
    p: &mut PrimProc,
    count: usize,
    vnodes: i64,
) -> Result<(), crate::assert::Failure> {
    for i in 1..=count {
        p.send(&format!("add n{i} {vnodes}")).await?;
    }
    Ok(())
}

/// Measure the imbalance of a `stats` sample over `nodes` nodes of `vnodes` points each.
async fn measure_imbalance(
    ctx: &mut Ctx,
    nodes: usize,
    vnodes: i64,
    sample: i64,
) -> Result<(f64, BTreeMap<String, i64>), crate::assert::Failure> {
    let mut p = ctx.prim_fresh("consistent-hash").await?;
    add_nodes(&mut p, nodes, vnodes).await?;
    let command = format!("stats {sample}");
    let answer = p.send(&command).await?;
    let counts = stats_counts(&p, &answer, &command)?;
    let values: Vec<usize> = counts.values().map(|c| (*c).max(0) as usize).collect();
    let measured = oracles::imbalance(&values);
    p.close().await;
    Ok((measured, counts))
}

dist_test!(locate_is_a_function, |ctx| {
    let p = ctx.prim("consistent-hash").await?;
    p.send("add n1 128").await?;
    p.send("add n2 128").await?;
    p.send("add n3 128").await?;
    let first = p.send("locate user-42").await?;
    let node = p.expect_str(&first, "locate user-42", "node")?;
    let mut seen = Vec::new();
    for _ in 0..5 {
        let again = p.send("locate user-42").await?;
        seen.push(p.expect_str(&again, "locate user-42", "node")?);
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("locating one key six times over an unchanged ring");
    for (i, answer) in seen.iter().enumerate() {
        c.eq(&format!("locate(user-42)[{}].node", i + 1), &node, answer);
    }
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(only_names_a_real_node, |ctx| {
    let keys = sample_keys(200);
    let p = ctx.prim("consistent-hash").await?;
    add_nodes(p, 4, 128).await?;
    let placement = locate_all(p, &keys).await?;
    let transcript = p.transcript_block();
    let members: Vec<String> = (1..=4).map(|i| format!("n{i}")).collect();
    let strangers: Vec<(&String, &String)> = placement
        .iter()
        .filter(|(_, node)| !members.contains(node))
        .take(3)
        .collect();
    let mut c = Check::new("200 keys over a four-node ring");
    c.that(
        "locate.node",
        "a node that is on the ring",
        strangers.is_empty(),
        strangers,
    );
    c.eq("locate answers", keys.len(), placement.len());
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(one_node_owns_everything, |ctx| {
    let keys = sample_keys(50);
    let p = ctx.prim("consistent-hash").await?;
    p.send("add only 128").await?;
    let placement = locate_all(p, &keys).await?;
    let command = "stats 500";
    let answer = p.send(command).await?;
    let counts = stats_counts(p, &answer, command)?;
    let transcript = p.transcript_block();
    let elsewhere: Vec<(&String, &String)> = placement
        .iter()
        .filter(|(_, node)| node.as_str() != "only")
        .take(3)
        .collect();
    let mut c = Check::new("a ring holding exactly one node");
    c.that(
        "locate.node",
        "the only node there is",
        elsewhere.is_empty(),
        elsewhere,
    );
    c.eq("stats.nodes.only", Some(&500i64), counts.get("only"));
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(no_drift_while_the_ring_stands_still, |ctx| {
    // Ten keys located in one order, then in the reverse order: an implementation that
    // caches the last answer, or that stirs its hash state as it goes, comes apart here
    // while a pure function does not notice.
    let keys = sample_keys(10);
    let p = ctx.prim("consistent-hash").await?;
    add_nodes(p, 5, 64).await?;
    let forwards = locate_all(p, &keys).await?;
    let mut reversed = keys.clone();
    reversed.reverse();
    let backwards = locate_all(p, &reversed).await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("the same ten keys located in both directions");
    for key in &keys {
        c.eq(
            &format!("locate({key}).node"),
            forwards.get(key),
            backwards.get(key),
        );
    }
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(virtual_nodes_spread_the_load, |ctx| {
    // Eight nodes of 128 points each over 10 000 keys. The vnode count sets the spread:
    // the relative deviation of a node's share falls off like 1/sqrt(vnodes), so 128 points
    // leaves a few per cent of noise and 0.35 is a wide, forgiving band around it.
    let (measured, counts) = measure_imbalance(ctx, 8, 128, 10_000).await?;
    ctx.note(format!(
        "imbalance over 10000 keys, 8 nodes, 128 points each: {measured:.3}"
    ));
    let total: i64 = counts.values().sum();
    let mut c = Check::new("the spread of 10000 keys over eight nodes");
    c.observe("stats.imbalance", format!("{measured:.3}"));
    c.observe("stats.nodes", format!("{counts:?}"));
    c.eq("stats.nodes.len()", 8, counts.len());
    c.eq("sum of stats.nodes", 10_000i64, total);
    c.at_most("stats.imbalance", 0.35, measured);
    c.finish()
});

dist_test!(one_point_per_node_is_lumpy, |ctx| {
    // The point of the previous test, shown from the other side. Eight single points cut
    // the ring into eight arcs whose lengths are a uniform random partition, and the arc is
    // the share: one node holding twice its share is the common case, not bad luck.
    let (lumpy, counts) = measure_imbalance(ctx, 8, 1, 10_000).await?;
    let (smooth, _) = measure_imbalance(ctx, 8, 128, 10_000).await?;
    ctx.note(format!(
        "imbalance with one point per node: {lumpy:.3}, with 128 points: {smooth:.3}"
    ));
    let mut c = Check::new("eight nodes with one ring point each");
    c.observe("stats(vnodes=1).imbalance", format!("{lumpy:.3}"));
    c.observe("stats(vnodes=128).imbalance", format!("{smooth:.3}"));
    c.observe("stats(vnodes=1).nodes", format!("{counts:?}"));
    c.at_least("stats(vnodes=1).imbalance", 0.10, lumpy);
    c.finish()
});

dist_test!(stats_adds_up, |ctx| {
    let p = ctx.prim("consistent-hash").await?;
    add_nodes(p, 3, 128).await?;
    let command = "stats 1000";
    let answer = p.send(command).await?;
    let counts = stats_counts(p, &answer, command)?;
    let transcript = p.transcript_block();
    let total: i64 = counts.values().sum();
    let negative: Vec<(&String, &i64)> = counts.iter().filter(|(_, c)| **c < 0).collect();
    let strangers: Vec<&String> = counts
        .keys()
        .filter(|n| !["n1", "n2", "n3"].contains(&n.as_str()))
        .collect();
    let mut c = Check::new("stats over 1000 keys on a three-node ring");
    c.eq("sum of stats.nodes", 1000i64, total);
    c.that(
        "stats.nodes",
        "no negative counts",
        negative.is_empty(),
        negative,
    );
    c.that(
        "stats.nodes",
        "only nodes that are on the ring",
        strangers.is_empty(),
        strangers,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(an_empty_ring_answers, |ctx| {
    // Every node gone is a real state, not an impossible one, and the process has to
    // survive it: an error object or an answer naming nobody, then business as usual once a
    // node is back.
    let p = ctx.prim("consistent-hash").await?;
    p.send("add n1 128").await?;
    p.send("remove n1").await?;
    let empty = p.send("locate user-42").await?;
    let claimed = empty
        .get("node")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    p.send("add n1 128").await?;
    let back = p.send("locate user-42").await?;
    let node = p.expect_str(&back, "locate user-42", "node")?;
    let transcript = p.transcript_block();
    let mut c = Check::new("locating a key on a ring with no nodes");
    c.that(
        "locate.node",
        "no node at all, because there is none to name",
        claimed.is_none(),
        claimed,
    );
    c.eq("locate.node after the node is back", "n1".to_string(), node);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(removing_a_stranger_changes_nothing, |ctx| {
    let keys = sample_keys(40);
    let p = ctx.prim("consistent-hash").await?;
    add_nodes(p, 3, 128).await?;
    let before = locate_all(p, &keys).await?;
    // Whether this answers an error or a plain no-op is the implementation's choice; what
    // it may not do is disturb the ring or stop answering.
    p.send("remove n9").await?;
    let after = locate_all(p, &keys).await?;
    let transcript = p.transcript_block();
    let moved: Vec<&String> = keys
        .iter()
        .filter(|k| before.get(*k) != after.get(*k))
        .collect();
    let mut c = Check::new("removing a node the ring never had");
    c.that(
        "locate.node",
        "every key exactly where it was",
        moved.is_empty(),
        moved,
    );
    c.block("transcript", transcript);
    c.finish()
});
