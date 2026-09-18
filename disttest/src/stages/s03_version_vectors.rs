//! Stage 03 — Version vectors and sibling detection.
//!
//! A vector clock says two writes were concurrent. A version vector is what a store does
//! about it: it keeps both, calls them siblings, and refuses to guess which one the
//! application meant. This is the stage where "eventually consistent" stops being a slogan
//! and becomes a data structure with a shape that can be checked.
//!
//! The oracle is exhaustive rather than clever. For every state this stage reaches the
//! tester knows the whole sibling set — each value and the exact vector stamped on it — and
//! compares the program's answer against that set, in order, entry for entry. Where two
//! writes are supposed to be siblings the tester also asks [`compare_clocks`] whether their
//! vectors really are concurrent, so a stage that keeps two values for the wrong reason
//! fails as loudly as one that keeps only one.

use crate::assert::{Check, Failure};
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::prim::oracles::{clock, compare_clocks, Clock, Relation};
use crate::prim::PrimProc;
use crate::stages::{Ladder, Stage, Test};
use serde_json::Value;

/// Stage 03.
pub fn stage() -> Stage {
    Stage {
        number: 3,
        slug: "version_vectors",
        name: "Version vectors and sibling detection",
        ext: true,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `version-vector`: a value carries the vector of the replica that wrote it",
            "A write that does not dominate an existing value creates a sibling instead of replacing it",
            "Sync merges both replicas' sets and drops any value dominated by another",
            "Siblings come back sorted so two replicas that agree answer identically",
        ],
        examples,
        tests: vec![
            Test::new(
                "a first write stamps the writing replica, and nothing else",
                a_first_write_names_its_replica,
            ),
            Test::new("a second write on the same replica replaces the first", a_later_write_replaces),
            Test::new("two replicas writing apart fork into siblings", a_fork_makes_siblings),
            Test::new("a sync leaves both replicas holding the same set", a_sync_converges),
            Test::new("siblings come back sorted and deduplicated", siblings_are_sorted_and_deduplicated),
            Test::new("a write that has seen every sibling replaces them all", a_dominating_write_collapses),
            Test::new("put-ctx naming the whole context collapses the siblings", put_ctx_resolves_a_conflict),
            Test::new("put-ctx naming a stale context forks again", put_ctx_with_a_stale_context),
            Test::new("a fresh process shares nothing with the one before it", state_is_per_process),
            Test::new("a three-way fork keeps all three values", a_three_way_fork).ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example(
            "Two replicas writing at the same time",
            "version-vector",
            || {
                lines(&[
                    "put r1 cart red",
                    "put r2 cart blue",
                    "sync r1 r2",
                    "read r1 cart",
                    "read r2 cart",
                ])
            },
        )
        .request("one write on each replica, no communication between them, then a sync")
        .response("both replicas answer with the same two siblings, sorted by value")
        .note(
            "Neither vector dominates the other — {\"r1\":1} and {\"r2\":1} are concurrent — so \
             the sync has no basis for dropping either. Picking the later arrival, or the \
             larger replica name, silently loses a write; that is the bug this whole stage \
             exists to catch.",
        ),
        prim_example(
            "Resolving the siblings with put-ctx",
            "version-vector",
            || {
                lines(&[
                    "put r1 cart red",
                    "put r2 cart blue",
                    "sync r1 r2",
                    "put-ctx r1 cart purple {\"r1\":1,\"r2\":1}",
                    "read r1 cart",
                ])
            },
        )
        .request("a write that names, as its context, the merge of both siblings' vectors")
        .response("one sibling left, stamped {\"r1\":2,\"r2\":1}")
        .note(
            "The context is the claim \"I read both of these\". Increment the writer's own \
             entry on top of it and the result dominates both siblings, so both are dropped. \
             A write that ignores the context it was given cannot collapse anything, and the \
             siblings accumulate for ever.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Reading siblings
// ---------------------------------------------------------------------------------------

/// One value and the vector the write that produced it was stamped with.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Sibling {
    /// The stored value.
    value: String,
    /// The version vector it carries.
    vv: Clock,
}

/// Build the sibling the tester expects to see.
fn sibling(value: &str, vv: &[(&str, i64)]) -> Sibling {
    Sibling {
        value: value.to_string(),
        vv: clock(vv),
    }
}

/// Read a clock out of one field of one object, dropping zero entries: `{}` and `{"a":0}`
/// are the same vector written two ways.
fn clock_field(p: &PrimProc, owner: &Value, field: &str, command: &str) -> Result<Clock, Failure> {
    let Some(map) = owner.get(field) else {
        return Err(p.shape(command, &format!("{owner} has no {field:?} field")));
    };
    let Some(map) = map.as_object() else {
        return Err(p.shape(command, &format!("{field} should be an object, got {map}")));
    };
    let mut out = Clock::new();
    for (name, count) in map {
        let n = match count {
            Value::Number(n) => n.as_i64(),
            Value::String(s) => s.parse().ok(),
            _ => None,
        };
        match n {
            Some(0) => {}
            Some(n) => {
                out.insert(name.clone(), n);
            }
            None => {
                return Err(p.shape(command, &format!("{field}.{name} is not a number: {count}")))
            }
        }
    }
    Ok(out)
}

/// A vector as a command-line argument: compact JSON, because commands split on whitespace.
fn as_arg(c: &Clock) -> String {
    let object: serde_json::Map<String, Value> = c
        .iter()
        .map(|(name, count)| (name.clone(), Value::from(*count)))
        .collect();
    crate::prim::arg(&Value::Object(object))
}

/// Write, and read back the vector the write was stamped with.
async fn put(p: &mut PrimProc, command: &str) -> Result<Clock, Failure> {
    let v = p.send(command).await?;
    clock_field(p, &v, "vv", command)
}

/// Read one key's siblings, exactly as the program ordered them.
async fn read(p: &mut PrimProc, replica: &str, key: &str) -> Result<Vec<Sibling>, Failure> {
    let command = format!("read {replica} {key}");
    let v = p.send(&command).await?;
    let Some(items) = v.get("siblings") else {
        return Err(p.shape(&command, "the answer has no \"siblings\" field"));
    };
    let Some(items) = items.as_array() else {
        return Err(p.shape(
            &command,
            &format!("siblings should be an array, got {items}"),
        ));
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Some(value) = item.get("value").and_then(Value::as_str) else {
            return Err(p.shape(&command, &format!("{item} has no string \"value\"")));
        };
        out.push(Sibling {
            value: value.to_string(),
            vv: clock_field(p, item, "vv", &command)?,
        });
    }
    Ok(out)
}

/// Exchange everything between two replicas.
async fn sync(p: &mut PrimProc, a: &str, b: &str) -> Result<(), Failure> {
    let command = format!("sync {a} {b}");
    let v = p.send(&command).await?;
    match v.get("ok") {
        Some(Value::Bool(true)) => Ok(()),
        _ => Err(p.shape(&command, "a sync answers {\"ok\": true}")),
    }
}

/// Assert that every pair of a sibling set really is concurrent — that the set is a set of
/// conflicts and not just a list the program forgot to prune.
fn check_pairwise_concurrent(c: &mut Check, path: &str, siblings: &[Sibling]) {
    for (i, a) in siblings.iter().enumerate() {
        for (j, b) in siblings.iter().enumerate() {
            if i >= j {
                continue;
            }
            c.eq(
                &format!("compare({path}[{i}].vv, {path}[{j}].vv)"),
                Relation::Concurrent,
                compare_clocks(&a.vv, &b.vv),
            );
        }
    }
}

/// The componentwise maximum of every sibling's vector: the context a reader has seen.
fn merged_context(siblings: &[Sibling]) -> Clock {
    let mut out = Clock::new();
    for s in siblings {
        for (name, count) in &s.vv {
            let slot = out.entry(name.clone()).or_insert(0);
            *slot = (*slot).max(*count);
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(a_first_write_names_its_replica, |ctx| {
    let p = ctx.prim("version-vector").await?;
    let vv = put(p, "put r1 cart red").await?;
    let here = read(p, "r1", "cart").await?;
    // A write reaches one replica and one key until something moves it. A replica that has
    // heard nothing, and a key nobody has written, both answer with an empty set rather
    // than an error or a null.
    let other_replica = read(p, "r2", "cart").await?;
    let other_key = read(p, "r1", "basket").await?;
    let mut c = Check::new("the first write a replica makes");
    c.eq("put.vv", clock(&[("r1", 1)]), vv);
    c.eq(
        "read(r1, cart).siblings",
        vec![sibling("red", &[("r1", 1)])],
        here,
    );
    c.eq(
        "read(r2, cart).siblings",
        Vec::<Sibling>::new(),
        other_replica,
    );
    c.eq(
        "read(r1, basket).siblings",
        Vec::<Sibling>::new(),
        other_key,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_later_write_replaces, |ctx| {
    let p = ctx.prim("version-vector").await?;
    put(p, "put r1 cart red").await?;
    let second = put(p, "put r1 cart green").await?;
    let third = put(p, "put r1 cart blue").await?;
    let siblings = read(p, "r1", "cart").await?;
    let mut c = Check::new("three writes in a row on one replica");
    // Each write has seen everything the replica holds, so each dominates the last: one
    // value, and a counter that has moved three times.
    c.eq("the second put.vv", clock(&[("r1", 2)]), second);
    c.eq("the third put.vv", clock(&[("r1", 3)]), third);
    c.eq(
        "read(r1, cart).siblings",
        vec![sibling("blue", &[("r1", 3)])],
        siblings,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_fork_makes_siblings, |ctx| {
    let p = ctx.prim("version-vector").await?;
    let first = put(p, "put r1 cart red").await?;
    let second = put(p, "put r2 cart blue").await?;
    let before_sync = read(p, "r1", "cart").await?;
    sync(p, "r1", "r2").await?;
    let after_sync = read(p, "r1", "cart").await?;
    let transcript = p.transcript_block();
    let expected = vec![sibling("blue", &[("r2", 1)]), sibling("red", &[("r1", 1)])];
    let mut c = Check::new("two replicas that wrote without hearing from each other");
    c.eq("put(r1).vv", clock(&[("r1", 1)]), first);
    c.eq("put(r2).vv", clock(&[("r2", 1)]), second);
    c.eq(
        "read(r1, cart).siblings before the sync",
        vec![sibling("red", &[("r1", 1)])],
        before_sync,
    );
    c.eq(
        "read(r1, cart).siblings after the sync",
        expected,
        after_sync.clone(),
    );
    check_pairwise_concurrent(&mut c, "read(r1, cart).siblings", &after_sync);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_sync_converges, |ctx| {
    let p = ctx.prim("version-vector").await?;
    put(p, "put r1 cart red").await?;
    put(p, "put r2 cart blue").await?;
    put(p, "put r1 list one").await?;
    sync(p, "r1", "r2").await?;
    let cart_here = read(p, "r1", "cart").await?;
    let cart_there = read(p, "r2", "cart").await?;
    let list_here = read(p, "r1", "list").await?;
    let list_there = read(p, "r2", "list").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("both replicas after one sync");
    c.eq(
        "read(r2, cart).siblings",
        cart_here.clone(),
        cart_there.clone(),
    );
    c.eq(
        "read(r1, cart).siblings",
        vec![sibling("blue", &[("r2", 1)]), sibling("red", &[("r1", 1)])],
        cart_here,
    );
    // A key only one replica ever wrote is not a conflict; it just has to arrive.
    c.eq(
        "read(r1, list).siblings",
        vec![sibling("one", &[("r1", 1)])],
        list_here,
    );
    c.eq(
        "read(r2, list).siblings",
        vec![sibling("one", &[("r1", 1)])],
        list_there,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(siblings_are_sorted_and_deduplicated, |ctx| {
    let p = ctx.prim("version-vector").await?;
    // Written in an order that is not the sorted one, so a program that answers in arrival
    // order is caught here rather than by luck.
    put(p, "put r1 cart zebra").await?;
    put(p, "put r2 cart mango").await?;
    put(p, "put r3 cart apple").await?;
    sync(p, "r1", "r2").await?;
    sync(p, "r1", "r3").await?;
    let once = read(p, "r1", "cart").await?;
    // Syncing again exchanges values both replicas already hold: a set stays a set.
    sync(p, "r1", "r2").await?;
    sync(p, "r1", "r2").await?;
    sync(p, "r1", "r3").await?;
    let twice = read(p, "r1", "cart").await?;
    let transcript = p.transcript_block();
    let expected = vec![
        sibling("apple", &[("r3", 1)]),
        sibling("mango", &[("r2", 1)]),
        sibling("zebra", &[("r1", 1)]),
    ];
    let mut c = Check::new("three concurrent values, synced repeatedly");
    c.eq("read(r1, cart).siblings", expected.clone(), once);
    c.eq(
        "read(r1, cart).siblings after three more syncs",
        expected,
        twice.clone(),
    );
    let mut values: Vec<&String> = twice.iter().map(|s| &s.value).collect();
    let sorted = {
        let mut v = values.clone();
        v.sort();
        v
    };
    c.eq(
        "the sibling values in the order they arrived",
        sorted,
        values.clone(),
    );
    values.dedup();
    c.eq("the number of distinct values", values.len(), twice.len());
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_dominating_write_collapses, |ctx| {
    let p = ctx.prim("version-vector").await?;
    put(p, "put r1 cart red").await?;
    put(p, "put r2 cart blue").await?;
    sync(p, "r1", "r2").await?;
    // r1 now holds both siblings, so its next plain write has seen both of them.
    let resolved = put(p, "put r1 cart purple").await?;
    let here = read(p, "r1", "cart").await?;
    let there = read(p, "r2", "cart").await?;
    sync(p, "r1", "r2").await?;
    let there_after = read(p, "r2", "cart").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a write made by a replica that is holding siblings");
    c.eq("put(r1).vv", clock(&[("r1", 2), ("r2", 1)]), resolved);
    c.eq(
        "read(r1, cart).siblings",
        vec![sibling("purple", &[("r1", 2), ("r2", 1)])],
        here,
    );
    c.eq(
        "read(r2, cart).siblings before the second sync",
        vec![sibling("blue", &[("r2", 1)]), sibling("red", &[("r1", 1)])],
        there,
    );
    c.eq(
        "read(r2, cart).siblings after the second sync",
        vec![sibling("purple", &[("r1", 2), ("r2", 1)])],
        there_after,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(put_ctx_resolves_a_conflict, |ctx| {
    let p = ctx.prim("version-vector").await?;
    put(p, "put r1 cart red").await?;
    put(p, "put r2 cart blue").await?;
    sync(p, "r1", "r2").await?;
    let siblings = read(p, "r1", "cart").await?;
    // The context is worked out here, from what the read actually returned, and is the
    // merge of both siblings' vectors: the claim "I have seen both of these".
    let context = merged_context(&siblings);
    let command = format!("put-ctx r1 cart purple {}", as_arg(&context));
    let stamped = put(p, &command).await?;
    let here = read(p, "r1", "cart").await?;
    sync(p, "r1", "r2").await?;
    let there = read(p, "r2", "cart").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a write that names both siblings as its context");
    c.eq(
        "the context read out of the siblings",
        clock(&[("r1", 1), ("r2", 1)]),
        context,
    );
    c.eq("put-ctx.vv", clock(&[("r1", 2), ("r2", 1)]), stamped);
    c.eq(
        "read(r1, cart).siblings",
        vec![sibling("purple", &[("r1", 2), ("r2", 1)])],
        here,
    );
    c.eq(
        "read(r2, cart).siblings after the sync",
        vec![sibling("purple", &[("r1", 2), ("r2", 1)])],
        there,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(put_ctx_with_a_stale_context, |ctx| {
    let p = ctx.prim("version-vector").await?;
    put(p, "put r1 cart red").await?;
    put(p, "put r2 cart blue").await?;
    sync(p, "r1", "r2").await?;
    // A client that read `red` before the sync and only now writes back: its context
    // covers r1's write and knows nothing of r2's.
    let stamped = put(p, "put-ctx r1 cart crimson {\"r1\":1}").await?;
    let here = read(p, "r1", "cart").await?;
    let transcript = p.transcript_block();
    let expected = vec![
        sibling("blue", &[("r2", 1)]),
        sibling("crimson", &[("r1", 2)]),
    ];
    let mut c = Check::new("a write whose context covers one sibling but not the other");
    c.eq("put-ctx.vv", clock(&[("r1", 2)]), stamped);
    c.eq("read(r1, cart).siblings", expected, here.clone());
    check_pairwise_concurrent(&mut c, "read(r1, cart).siblings", &here);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(state_is_per_process, |ctx| {
    let p = ctx.prim("version-vector").await?;
    put(p, "put r1 cart red").await?;
    sync(p, "r1", "r2").await?;
    let first = read(p, "r2", "cart").await?;
    let first_transcript = p.transcript_block();
    // A second process is a second cluster: it has never heard of this key, and a program
    // keeping its state in a file rather than in memory is caught here.
    let mut other = ctx.prim_fresh("version-vector").await?;
    let untouched = read(&mut other, "r1", "cart").await?;
    let fresh_write = put(&mut other, "put r1 cart green").await?;
    let after = read(&mut other, "r1", "cart").await?;
    let second_transcript = other.transcript_block();
    other.close().await;
    let mut c = Check::new("two processes that cannot see each other");
    c.eq(
        "read(r2, cart).siblings in the first process",
        vec![sibling("red", &[("r1", 1)])],
        first,
    );
    c.eq(
        "read(r1, cart).siblings in the second process",
        Vec::<Sibling>::new(),
        untouched,
    );
    c.eq(
        "put(r1).vv in the second process",
        clock(&[("r1", 1)]),
        fresh_write,
    );
    c.eq(
        "read(r1, cart).siblings after that write",
        vec![sibling("green", &[("r1", 1)])],
        after,
    );
    c.block("transcript of the first process", first_transcript);
    c.block("transcript of the second process", second_transcript);
    c.finish()
});

dist_test!(a_three_way_fork, |ctx| {
    let p = ctx.prim("version-vector").await?;
    put(p, "put r1 cart red").await?;
    put(p, "put r2 cart blue").await?;
    put(p, "put r3 cart green").await?;
    // Gossip, not a broadcast: r3 only ever talks to r2.
    sync(p, "r1", "r2").await?;
    sync(p, "r2", "r3").await?;
    sync(p, "r1", "r2").await?;
    let one = read(p, "r1", "cart").await?;
    let two = read(p, "r2", "cart").await?;
    let three = read(p, "r3", "cart").await?;
    let transcript = p.transcript_block();
    let expected = vec![
        sibling("blue", &[("r2", 1)]),
        sibling("green", &[("r3", 1)]),
        sibling("red", &[("r1", 1)]),
    ];
    let mut c = Check::new("three concurrent writes gossiped round a chain of replicas");
    c.eq("read(r1, cart).siblings", expected.clone(), one.clone());
    c.eq("read(r2, cart).siblings", expected.clone(), two);
    c.eq("read(r3, cart).siblings", expected, three);
    check_pairwise_concurrent(&mut c, "read(r1, cart).siblings", &one);
    c.block("transcript", transcript);
    c.finish()
});
