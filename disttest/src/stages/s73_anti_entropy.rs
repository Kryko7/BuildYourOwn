//! Stage 73 — Anti-entropy: read repair and hinted handoff.
//!
//! A replicated store that writes to whoever is reachable will drift. Three mechanisms pull
//! it back together, and they are not interchangeable. Read repair fixes what somebody
//! happens to read, which is cheap and covers the hot keys and nothing else. Hinted handoff
//! fixes what was written while a replica was away, which covers the cold keys too, but only
//! while the stand-in holding the hint stays alive. Anti-entropy — a full comparison between
//! two replicas — fixes everything, slowly, and is the only one of the three that is a
//! guarantee rather than an optimisation.
//!
//! The oracle is the tester's own copy of every replica. It knows which replicas were behind
//! before each read, so it can assert that a repair touched exactly those and no others; it
//! knows which hints were taken and where they were put, so it can assert a handoff to a
//! replica whose stand-in has died delivers nothing. The seeded run at the end diverges three
//! replicas at random and asserts that a round of syncs lands all of them on the version the
//! tester computed as the maximum.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;
use std::collections::BTreeMap;

/// Stage 73.
pub fn stage() -> Stage {
    Stage {
        number: 73,
        slug: "anti_entropy",
        name: "Anti-entropy: read repair and hinted handoff",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `anti-entropy`: a read takes the highest version any live replica holds",
            "Repair exactly the replicas that were behind — repairing the current ones is wasted work",
            "A write to a replica that is down leaves a hint on the first live replica instead",
            "A hint dies with its stand-in; only `sync` is a guarantee, and it compares everything",
        ],
        examples,
        tests: vec![
            Test::new(
                "a read where every replica agrees repairs nothing",
                agreement_needs_no_repair,
            ),
            Test::new(
                "a read repairs exactly the replicas that were behind",
                read_repair_is_targeted,
            ),
            Test::new(
                "a read of a key nobody holds answers nothing",
                a_key_nobody_holds,
            ),
            Test::new(
                "a write while a replica is down takes a hint",
                a_hint_is_taken,
            ),
            Test::new(
                "a handoff delivers the hint once and then forgets it",
                a_handoff_delivers_once,
            ),
            Test::new(
                "a handoff to a replica that is still down keeps the hint",
                a_handoff_waits,
            ),
            Test::new(
                "a sync moves only the keys that were behind",
                a_sync_moves_only_what_differs,
            ),
            Test::new(
                "neither a repair nor a handoff carries a version backwards",
                versions_never_regress,
            ),
            Test::new(
                "a hint whose stand-in dies is lost, and only a sync repairs the replica",
                a_hint_dies_with_its_standin,
            )
            .ext(),
            Test::new(
                "seeded divergence converges once every pair has synced",
                a_seeded_divergence_converges,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("A read that repairs one replica", "anti-entropy", || {
            lines(&[
                "init [\"r1\",\"r2\",\"r3\"]",
                "put r1 k new 5",
                "put r2 k old 2",
                "read k",
                "read k",
            ])
        })
        .request("three replicas, two of them behind, and the same read issued twice")
        .response("the first read repairs r2 and r3; the second finds nothing to do")
        .note(
            "The repair list must be the stale list and nothing more. Writing the winning \
             value back to every replica works, costs three writes instead of two, and hides \
             the very thing the field is there to show you: how far out of step the replicas \
             had drifted.",
        ),
        prim_example("A hint that outlives its holder", "anti-entropy", || {
            lines(&[
                "init [\"r1\",\"r2\",\"r3\"]",
                "down r3",
                "write k v 1",
                "down r1",
                "up r3",
                "handoff",
                "sync r2 r3",
            ])
        })
        .request("a write while r3 is away, then the stand-in holding its hint dies too")
        .response("the handoff delivers nothing; the sync is what finally repairs r3")
        .note(
            "Hinted handoff is a latency optimisation wearing the costume of a durability \
             mechanism. The hint is one more copy on one more machine, and when that machine \
             goes, so does it. Only a full comparison between two replicas is a guarantee.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

/// The three replicas every test in this stage starts with.
const INIT: &str = "init [\"r1\",\"r2\",\"r3\"]";

/// The version a replica holds for a key, or zero when it holds nothing.
fn version_of(state: &serde_json::Value, key: &str) -> i64 {
    state["keys"][key]["version"].as_i64().unwrap_or(0)
}

/// The value a replica holds for a key, or the empty string.
fn value_of(state: &serde_json::Value, key: &str) -> String {
    state["keys"][key]["value"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

dist_test!(agreement_needs_no_repair, |ctx| {
    let p = ctx.prim("anti-entropy").await?;
    p.send(INIT).await?;
    for r in ["r1", "r2", "r3"] {
        p.send(&format!("put {r} k same 4")).await?;
    }
    let r = p.send("read k").await?;
    let mut c = Check::new("a read of a key every replica already agrees about");
    c.eq(
        "read.value",
        "same".to_string(),
        p.expect_str(&r, "read k", "value")?,
    );
    c.eq("read.version", 4, p.expect_i64(&r, "read k", "version")?);
    c.eq(
        "read.stale",
        Vec::<String>::new(),
        p.expect_strs(&r, "read k", "stale")?,
    );
    c.eq(
        "read.repaired",
        Vec::<String>::new(),
        p.expect_strs(&r, "read k", "repaired")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(read_repair_is_targeted, |ctx| {
    let p = ctx.prim("anti-entropy").await?;
    p.send(INIT).await?;
    p.send("put r1 k new 5").await?;
    p.send("put r2 k old 2").await?;
    // r3 has never seen the key at all, which counts as version zero.
    let first = p.send("read k").await?;
    let r3 = p.send("state r3").await?;
    let second = p.send("read k").await?;
    let mut c = Check::new("a read across replicas that had drifted apart");
    c.eq(
        "read.value",
        "new".to_string(),
        p.expect_str(&first, "read k", "value")?,
    );
    c.eq(
        "read.version",
        5,
        p.expect_i64(&first, "read k", "version")?,
    );
    c.eq(
        "read.stale",
        vec!["r2".to_string(), "r3".to_string()],
        p.expect_strs(&first, "read k", "stale")?,
    );
    // Repairing r1 as well would be two extra writes and would hide how far apart they were.
    c.eq(
        "read.repaired",
        vec!["r2".to_string(), "r3".to_string()],
        p.expect_strs(&first, "read k", "repaired")?,
    );
    c.eq("state(r3).keys.k.version", 5, version_of(&r3, "k"));
    c.eq(
        "the second read.repaired",
        Vec::<String>::new(),
        p.expect_strs(&second, "read k", "repaired")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_key_nobody_holds, |ctx| {
    let p = ctx.prim("anti-entropy").await?;
    p.send(INIT).await?;
    p.send("put r1 other v 1").await?;
    let r = p.send("read missing").await?;
    let mut c = Check::new("a read of a key that was never written");
    c.eq("read.value", &serde_json::Value::Null, &r["value"]);
    c.eq(
        "read.version",
        0,
        p.expect_i64(&r, "read missing", "version")?,
    );
    c.eq(
        "read.stale",
        Vec::<String>::new(),
        p.expect_strs(&r, "read missing", "stale")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_hint_is_taken, |ctx| {
    let p = ctx.prim("anti-entropy").await?;
    p.send(INIT).await?;
    p.send("down r3").await?;
    let w = p.send("write k v 1").await?;
    let hints = p.send("hints r3").await?;
    let r3 = p.send("state r3").await?;
    let mut c = Check::new("a write issued while one replica is unreachable");
    c.eq(
        "write.stored",
        vec!["r1".to_string(), "r2".to_string()],
        p.expect_strs(&w, "write k v 1", "stored")?,
    );
    c.eq(
        "write.hinted",
        &serde_json::json!([{ "for": "r3", "on": "r1" }]),
        &w["hinted"],
    );
    c.eq(
        "hints(r3).keys",
        vec!["k".to_string()],
        p.expect_strs(&hints, "hints r3", "keys")?,
    );
    // The hint is a note on somebody else's machine, not a copy on r3.
    c.eq("state(r3).keys.k.version", 0, version_of(&r3, "k"));
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_handoff_delivers_once, |ctx| {
    let p = ctx.prim("anti-entropy").await?;
    p.send(INIT).await?;
    p.send("down r3").await?;
    p.send("write k v 1").await?;
    p.send("up r3").await?;
    let first = p.send("handoff").await?;
    let r3 = p.send("state r3").await?;
    let second = p.send("handoff").await?;
    let hints = p.send("hints r3").await?;
    let mut c = Check::new("a handoff once the absent replica is back");
    c.eq(
        "the first handoff.delivered",
        1,
        p.expect_i64(&first, "handoff", "delivered")?,
    );
    c.eq(
        "the first handoff.remaining",
        0,
        p.expect_i64(&first, "handoff", "remaining")?,
    );
    c.eq(
        "state(r3).keys.k.value",
        "v".to_string(),
        value_of(&r3, "k"),
    );
    // A hint that has been handed off is forgotten; replaying it would be at best wasted
    // work and at worst a resurrection of a value that has since been deleted.
    c.eq(
        "the second handoff.delivered",
        0,
        p.expect_i64(&second, "handoff", "delivered")?,
    );
    c.eq(
        "hints(r3).keys",
        Vec::<String>::new(),
        p.expect_strs(&hints, "hints r3", "keys")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_handoff_waits, |ctx| {
    let p = ctx.prim("anti-entropy").await?;
    p.send(INIT).await?;
    p.send("down r3").await?;
    p.send("write k v 1").await?;
    let early = p.send("handoff").await?;
    let hints = p.send("hints r3").await?;
    let mut c = Check::new("a handoff attempted while the owner is still away");
    c.eq(
        "handoff.delivered",
        0,
        p.expect_i64(&early, "handoff", "delivered")?,
    );
    c.eq(
        "handoff.remaining",
        1,
        p.expect_i64(&early, "handoff", "remaining")?,
    );
    c.eq(
        "hints(r3).keys",
        vec!["k".to_string()],
        p.expect_strs(&hints, "hints r3", "keys")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_sync_moves_only_what_differs, |ctx| {
    let p = ctx.prim("anti-entropy").await?;
    p.send(INIT).await?;
    for r in ["r1", "r2"] {
        p.send(&format!("put {r} a va 1")).await?;
        p.send(&format!("put {r} b vb 1")).await?;
    }
    p.send("put r1 c vc 3").await?;
    let s = p.send("sync r1 r2").await?;
    let again = p.send("sync r1 r2").await?;
    let r2 = p.send("state r2").await?;
    let mut c = Check::new("a sync between two replicas that differ in one key");
    // A sync that ships everything has learnt nothing: the comparison is the whole point.
    c.eq(
        "sync.transferred",
        1,
        p.expect_i64(&s, "sync r1 r2", "transferred")?,
    );
    c.eq(
        "sync.keys",
        vec!["c".to_string()],
        p.expect_strs(&s, "sync r1 r2", "keys")?,
    );
    c.eq("state(r2).keys.c.version", 3, version_of(&r2, "c"));
    c.eq(
        "the second sync.transferred",
        0,
        p.expect_i64(&again, "sync r1 r2", "transferred")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(versions_never_regress, |ctx| {
    let p = ctx.prim("anti-entropy").await?;
    p.send(INIT).await?;
    p.send("down r3").await?;
    p.send("write k old 1").await?;
    p.send("up r3").await?;
    // r3 came back and was written directly, so the hint it is owed is already stale.
    p.send("put r3 k newer 9").await?;
    let h = p.send("handoff").await?;
    let r3 = p.send("state r3").await?;
    let read = p.send("read k").await?;
    let r1 = p.send("state r1").await?;
    let mut c = Check::new("a hint and a repair that both carry an older version");
    c.eq(
        "handoff.delivered",
        1,
        p.expect_i64(&h, "handoff", "delivered")?,
    );
    // Delivered, but not applied: the version guard is what stops a delayed hint from
    // undoing a write that happened while it was queued.
    c.eq(
        "state(r3).keys.k.value",
        "newer".to_string(),
        value_of(&r3, "k"),
    );
    c.eq("state(r3).keys.k.version", 9, version_of(&r3, "k"));
    c.eq("read.version", 9, p.expect_i64(&read, "read k", "version")?);
    c.eq(
        "state(r1).keys.k.version after the repair",
        9,
        version_of(&r1, "k"),
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_hint_dies_with_its_standin, |ctx| {
    let p = ctx.prim("anti-entropy").await?;
    p.send(INIT).await?;
    p.send("down r3").await?;
    p.send("write k v 1").await?;
    // The stand-in holding r3's hint now dies as well. The hint was one extra copy on one
    // extra machine, and both the write and the note about it are gone with it.
    p.send("down r1").await?;
    p.send("up r3").await?;
    let h = p.send("handoff").await?;
    let orphaned = p.send("state r3").await?;
    let s = p.send("sync r2 r3").await?;
    let repaired = p.send("state r3").await?;
    let mut c = Check::new("a handoff whose stand-in is no longer there");
    c.eq(
        "handoff.delivered",
        0,
        p.expect_i64(&h, "handoff", "delivered")?,
    );
    c.eq(
        "handoff.remaining",
        1,
        p.expect_i64(&h, "handoff", "remaining")?,
    );
    c.eq(
        "state(r3).keys.k.version before the sync",
        0,
        version_of(&orphaned, "k"),
    );
    // Anti-entropy is the guarantee; hinted handoff was only ever the shortcut.
    c.eq(
        "sync.transferred",
        1,
        p.expect_i64(&s, "sync r2 r3", "transferred")?,
    );
    c.eq(
        "state(r3).keys.k.value after the sync",
        "v".to_string(),
        value_of(&repaired, "k"),
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_seeded_divergence_converges, |ctx| {
    // Three replicas are driven apart at random and the tester keeps its own copy of what
    // the highest version of each key is. Syncing every pair once must land all three on
    // exactly that, whatever order the divergence happened in.
    let replicas = ["r1", "r2", "r3"];
    let keys = ["a", "b", "c", "d"];
    // `put` is the fixture that creates the divergence, so it writes whatever it is told,
    // and a later put with a lower version really does move that replica backwards. The
    // tester therefore keeps a copy of every replica rather than a running maximum.
    let mut store: BTreeMap<(String, String), i64> = BTreeMap::new();
    let mut plan = Vec::new();
    for _ in 0..40 {
        let r = replicas[ctx.rng.random_range(0..replicas.len())];
        let k = keys[ctx.rng.random_range(0..keys.len())];
        let version = ctx.rng.random_range(1..20);
        // The value is a function of the version, so two replicas that agree about the
        // version can never disagree about the value and a tie is harmless.
        plan.push(format!("put {r} {k} v{version} {version}"));
        store.insert((r.to_string(), k.to_string()), version);
    }
    let mut best: BTreeMap<String, i64> = BTreeMap::new();
    for ((_, key), version) in &store {
        let slot = best.entry(key.clone()).or_insert(0);
        *slot = (*slot).max(*version);
    }
    let seed = ctx.seed;
    let p = ctx.prim("anti-entropy").await?;
    p.send(INIT).await?;
    for command in &plan {
        p.send(command).await?;
    }
    for pair in ["sync r1 r2", "sync r2 r3", "sync r1 r2"] {
        p.send(pair).await?;
    }
    let mut states = Vec::new();
    for r in replicas {
        states.push((r, p.send(&format!("state {r}")).await?));
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("forty seeded writes, then one round of syncs");
    c.note(format!("seed {seed}, {} writes", plan.len()));
    for (r, state) in &states {
        for (key, version) in &best {
            c.eq(
                &format!("state({r}).keys.{key}.version"),
                *version,
                version_of(state, key),
            );
            c.eq(
                &format!("state({r}).keys.{key}.value"),
                format!("v{version}"),
                value_of(state, key),
            );
        }
        if !c.ok() {
            break;
        }
    }
    c.block("transcript", transcript);
    c.finish()
});
