//! Stage 81 — Chain replication.
//!
//! Leader-based replication puts every read and every write through one node. Chain
//! replication splits the job: writes enter at the **head** and travel down the chain,
//! reads are answered by the **tail** and nobody else. A value the tail holds has, by
//! construction, been applied by every node before it — so the tail can answer immediately,
//! with no quorum to collect and no round trip to anyone.
//!
//! What that buys is throughput and a very short read path. What it costs is exposure to
//! the chain's length — a write is not committed until it has walked the whole thing — and
//! a failure story that depends on *where* the failure was. Losing the head loses only
//! un-forwarded writes. Losing the tail promotes a node that is, by the invariant, at least
//! as up to date. Losing a middle node is the interesting one: its successor must be
//! caught up from its predecessor, which still holds everything it sent.
//!
//! The stage makes propagation explicit, so the window between "accepted at the head" and
//! "committed at the tail" is a thing the test can stand inside and look around.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;

/// Stage 81.
pub fn stage() -> Stage {
    Stage {
        number: 81,
        slug: "chain_replication",
        name: "Chain replication: writes at the head, reads at the tail",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `chain`: `write <key> <value>` is accepted at the head only, `read <key>` is \
             answered by the tail only, `propagate` moves one hop, `fail <node>` closes the gap",
            "The invariant is a prefix relationship: every node holds a prefix of what the node \
             before it holds, so the tail's state is the committed state by definition",
            "`read-at <node> <key>` shows a node mid-chain, which may hold writes the tail has \
             never heard of — those are not committed and a client must not be shown them",
            "When a middle node fails its successor is missing whatever that node had not yet \
             forwarded, and its predecessor is the one that still has it",
        ],
        examples,
        tests: vec![
            Test::new("a fresh chain names its head and tail", a_fresh_chain),
            Test::new("a write is accepted at the head", write_at_the_head),
            Test::new(
                "and is not committed until it reaches the tail",
                not_committed_until_the_tail,
            ),
            Test::new("the tail answers reads once it has it", the_tail_answers),
            Test::new(
                "a middle node may be ahead of the tail, and that is not committed",
                the_middle_is_ahead,
            ),
            Test::new(
                "losing the tail promotes a node that is at least as current",
                losing_the_tail,
            ),
            Test::new(
                "losing the head loses only what it had not sent",
                losing_the_head,
            ),
            Test::new(
                "losing a middle node does not lose what it had already taken",
                losing_the_middle,
            ),
            Test::new("a chain of one is head and tail at once", a_chain_of_one).ext(),
            Test::new(
                "twenty seeded writes arrive at the tail in order",
                a_long_seeded_run,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("The window between accepted and committed", "chain", || {
            lines(&[
                "init 3",
                "write k 7",
                "read k",
                "read-at 0 k",
                "propagate",
                "read k",
            ])
        })
        .request("A write at the head, read from the tail before and after it propagates")
        .response("the tail reads 0, node 0 already has 7 with `committed: false`, and after propagation the tail reads 7")
        .note(
            "Both answers are correct at the moment they are given. The head has the write \
             and the tail does not, and the rule that reads come from the tail is what turns \
             that into a consistent story rather than a race.",
        ),
        prim_example("Losing the middle of the chain", "chain", || {
            lines(&[
                "init 3",
                "write k 7",
                "propagate",
                "write k 9",
                "fail 1",
                "propagate",
                "read k",
            ])
        })
        .request("A committed write, a second in flight, and then the middle node dies")
        .response("the chain closes to head 0 and tail 2, and the tail ends up with 9")
        .note(
            "The second write had reached node 1 and no further. Node 1's predecessor still \
             holds it, so closing the chain is enough to finish delivering it — no consensus \
             round, no reconciliation, just the prefix invariant doing the work.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(a_fresh_chain, |ctx| {
    let p = ctx.prim("chain").await?;
    let init = p.send("init 3").await?;
    let state = p.send("state").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a chain of three");
    c.eq("init.head", 0, p.expect_i64(&init, "init", "head")?);
    c.eq("init.tail", 2, p.expect_i64(&init, "init", "tail")?);
    c.eq("state.head", 0, p.expect_i64(&state, "state", "head")?);
    c.eq("state.tail", 2, p.expect_i64(&state, "state", "tail")?);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(write_at_the_head, |ctx| {
    let p = ctx.prim("chain").await?;
    p.send("init 3").await?;
    let wrote = p.send("write k 7").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a write entering the chain");
    c.note(
        "Every write enters at one place. That is what makes the order of writes a property \
         of the chain rather than something that has to be agreed on.",
    );
    c.eq(
        "write.accepted",
        true,
        p.expect_bool(&wrote, "write", "accepted")?,
    );
    c.eq("write.at", 0, p.expect_i64(&wrote, "write", "at")?);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(not_committed_until_the_tail, |ctx| {
    let p = ctx.prim("chain").await?;
    p.send("init 3").await?;
    p.send("write k 7").await?;
    let before = p.send("read k").await?;
    p.send("propagate").await?;
    let middle = p.send("read k").await?;
    p.send("propagate").await?;
    let after = p.send("read k").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a write walking the chain");
    c.note(
        "One hop per `propagate`. The tail is two hops from the head, so it takes two — and \
         until then the honest answer to a read is the old value.",
    );
    c.eq(
        "before any propagation",
        0,
        p.expect_i64(&before, "read", "value")?,
    );
    c.eq("after one hop", 0, p.expect_i64(&middle, "read", "value")?);
    c.eq("after two", 7, p.expect_i64(&after, "read", "value")?);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(the_tail_answers, |ctx| {
    let p = ctx.prim("chain").await?;
    p.send("init 3").await?;
    p.send("write k 7").await?;
    p.send("propagate").await?;
    p.send("propagate").await?;
    let read = p.send("read k").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("the tail serving a committed read");
    c.note(
        "No quorum is collected and nobody is asked anything: the tail answers out of its \
         own state. It can do that because anything it holds, every node before it also \
         holds — the prefix invariant is the proof.",
    );
    c.eq("read.served", true, p.expect_bool(&read, "read", "served")?);
    c.eq("read.value", 7, p.expect_i64(&read, "read", "value")?);
    c.eq("read.from", 2, p.expect_i64(&read, "read", "from")?);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(the_middle_is_ahead, |ctx| {
    let p = ctx.prim("chain").await?;
    p.send("init 3").await?;
    p.send("write k 7").await?;
    p.send("propagate").await?;
    let at_one = p.send("read-at 1 k").await?;
    let at_tail = p.send("read k").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a node in the middle of the chain");
    c.note(
        "Node 1 has the write and the tail does not. Serving a client from node 1 would hand \
         out a value that is not committed and could still be lost — which is the whole \
         reason reads are pinned to the tail rather than sent to whoever is nearest.",
    );
    c.eq(
        "node 1 has it",
        7,
        p.expect_i64(&at_one, "read-at", "value")?,
    );
    c.eq(
        "but not as committed",
        false,
        p.expect_bool(&at_one, "read-at", "committed")?,
    );
    c.eq(
        "and the tail does not have it",
        0,
        p.expect_i64(&at_tail, "read", "value")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(losing_the_tail, |ctx| {
    let p = ctx.prim("chain").await?;
    p.send("init 3").await?;
    p.send("write k 7").await?;
    p.send("propagate").await?;
    p.send("propagate").await?;
    let failed = p.send("fail 2").await?;
    let read = p.send("read k").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("the tail failing");
    c.note(
        "The new tail is node 1, which by the invariant holds everything the old tail held \
         and possibly more. Promoting it can only make more writes committed, never fewer — \
         so no acknowledged read is ever contradicted.",
    );
    c.eq(
        "it was the tail",
        true,
        p.expect_bool(&failed, "fail", "was_tail")?,
    );
    c.eq("the new tail", 1, p.expect_i64(&failed, "fail", "tail")?);
    c.eq(
        "and the committed value survives",
        7,
        p.expect_i64(&read, "read", "value")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(losing_the_head, |ctx| {
    let p = ctx.prim("chain").await?;
    p.send("init 3").await?;
    p.send("write k 7").await?;
    p.send("propagate").await?;
    p.send("propagate").await?;
    // A second write reaches the head and goes no further before it dies.
    p.send("write k 9").await?;
    let failed = p.send("fail 0").await?;
    p.send("propagate").await?;
    let read = p.send("read k").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("the head failing with a write in hand");
    c.note(
        "The write of 9 was accepted but never acknowledged to any client, so losing it is \
         allowed. What must not happen is losing 7, which was committed — and it is on every \
         remaining node.",
    );
    c.eq(
        "it was the head",
        true,
        p.expect_bool(&failed, "fail", "was_head")?,
    );
    c.eq("the new head", 1, p.expect_i64(&failed, "fail", "head")?);
    c.eq(
        "the committed value is intact",
        7,
        p.expect_i64(&read, "read", "value")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(losing_the_middle, |ctx| {
    let p = ctx.prim("chain").await?;
    p.send("init 3").await?;
    p.send("write k 7").await?;
    p.send("propagate").await?;
    p.send("propagate").await?;
    // The second write gets one hop in — node 1 has it, the tail does not.
    p.send("write k 9").await?;
    p.send("propagate").await?;
    p.send("fail 1").await?;
    p.send("propagate").await?;
    let read = p.send("read k").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a middle node failing mid-flight");
    c.note(
        "Node 1 held a write the tail had not seen. Its predecessor still holds it too, so \
         closing the chain and propagating again finishes the delivery. Nothing had to be \
         agreed on, because the prefix invariant already said who had what.",
    );
    c.eq(
        "the tail ends up with the later write",
        9,
        p.expect_i64(&read, "read", "value")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_chain_of_one, |ctx| {
    let p = ctx.prim("chain").await?;
    let init = p.send("init 1").await?;
    p.send("write k 7").await?;
    let read = p.send("read k").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a chain with one node in it");
    c.note(
        "Head and tail are the same node, so a write is committed the moment it is accepted. \
         It is a degenerate case and it still has to answer correctly — which is the \
         cheapest way to find out whether head and tail are computed or assumed.",
    );
    c.eq("head", 0, p.expect_i64(&init, "init", "head")?);
    c.eq("tail", 0, p.expect_i64(&init, "init", "tail")?);
    c.eq(
        "the read is immediate",
        7,
        p.expect_i64(&read, "read", "value")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_long_seeded_run, |ctx| {
    let mut writes: Vec<i64> = Vec::new();
    for _ in 0..20 {
        writes.push(ctx.rng.random_range(1..1000));
    }
    let mut seen: Vec<i64> = Vec::new();
    {
        let p = ctx.prim("chain").await?;
        p.send("init 4").await?;
        for v in &writes {
            p.send(&format!("write k {v}")).await?;
            // Enough hops to walk the whole chain, so every write commits before the next.
            for _ in 0..3 {
                p.send("propagate").await?;
            }
            let r = p.send("read k").await?;
            seen.push(p.expect_i64(&r, "read", "value")?);
        }
    }
    let mut c = Check::new("twenty writes, each read back at the tail");
    c.note(
        "One write at a time, each allowed to walk the chain before the next. The tail must \
         show them in exactly the order the head took them: a chain has no reordering in it, \
         which is most of why it needs no agreement protocol.",
    );
    c.eq("the values the tail showed", writes, seen);
    c.finish()
});
