//! Stage 82 — Raft in production: pre-vote, ReadIndex and leadership transfer.
//!
//! Stages 56 to 60 build Raft as the paper presents it. Every Raft that runs in production
//! also carries three things the paper leaves to section 9 and to Ongaro's dissertation,
//! and each of them fixes something that is not a bug in the algorithm but is a problem in
//! the world.
//!
//! **Pre-vote.** A node that has been partitioned away keeps timing out and keeps raising
//! its term. When the partition heals it arrives with a term higher than everyone else's,
//! which forces the perfectly healthy leader to step down for no reason at all. Pre-vote
//! makes it ask "would you vote for me?" *before* incrementing anything, so a node that
//! cannot win never disturbs a cluster that is working.
//!
//! **ReadIndex.** A leader that answers a read from its own state is guessing twice: that
//! it is still the leader, and that its state machine has caught up with its log. Neither
//! is free. ReadIndex records the commit index, confirms leadership with a heartbeat round,
//! waits for the state machine to reach that index, and only then answers — a linearizable
//! read with no log entry written.
//!
//! **Leadership transfer.** Moving leadership by stopping the leader means an election
//! timeout of downtime. Transferring it deliberately hands the term to a chosen successor,
//! which is how a node is drained for maintenance without anybody noticing.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};

/// Stage 82.
pub fn stage() -> Stage {
    Stage {
        number: 82,
        slug: "raft_reads_and_prevote",
        name: "Raft in production: pre-vote, ReadIndex and transfer",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `raft-reads`: `partition`/`heal` isolate a node, `campaign <node>` makes it \
             try for leadership, `read-index` serves a linearizable read, `transfer <to>` hands \
             leadership over",
            "Pre-vote asks for votes without raising the term, and only increments when the \
             answer would be yes — a node that cannot win leaves the cluster's term alone",
            "ReadIndex has two conditions and needs both: a heartbeat quorum proving the leader \
             is still the leader, and the state machine having applied everything committed",
            "Leadership transfer does not hold an election: the term moves once, to a named \
             successor, with no timeout in between",
        ],
        examples,
        tests: vec![
            Test::new("a fresh cluster has a leader and a term", a_fresh_cluster),
            Test::new(
                "a partitioned node cannot win an election",
                a_partitioned_node_loses,
            ),
            Test::new(
                "and with pre-vote it does not raise the term either",
                prevote_protects_the_term,
            ),
            Test::new(
                "without pre-vote, the same node disrupts a healthy cluster",
                without_prevote_it_disrupts,
            ),
            Test::new(
                "a read from the leader needs a quorum to still be there",
                read_index_needs_a_quorum,
            ),
            Test::new(
                "and needs the state machine to have caught up",
                read_index_waits_for_apply,
            ),
            Test::new(
                "a read served this way reports the index it was safe at",
                read_index_reports_its_index,
            ),
            Test::new(
                "leadership transfer moves the leader without an election",
                transfer_moves_leadership,
            ),
            Test::new(
                "transferring to an unreachable node is refused, not attempted",
                transfer_to_unreachable,
            )
            .ext(),
            Test::new(
                "a leader that lost its quorum stops serving reads",
                a_deposed_leader_stops_reading,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("What pre-vote is for", "raft-reads", || {
            lines(&[
                "init 3",
                "partition 2",
                "campaign 2",
                "state",
                "prevote off",
                "campaign 2",
                "state",
            ])
        })
        .request("An isolated node campaigns, first with pre-vote and then without")
        .response("with pre-vote the term stays 1; without it the term jumps to 2")
        .note(
            "Node 2 loses either way — it cannot reach a majority. The difference is what it \
             costs everyone else: without pre-vote it comes back from the partition with a \
             higher term and forces a working leader to step down, which is a re-election \
             caused entirely by a node that was never eligible.",
        ),
        prim_example("A linearizable read without writing anything", "raft-reads", || {
            lines(&[
                "init 3",
                "append",
                "read-index",
                "apply",
                "read-index",
            ])
        })
        .request("An entry is committed, then read before and after the state machine applies it")
        .response("the first read is refused because the state machine is behind; the second is served at index 1")
        .note(
            "Nothing was written to the log to serve this read. The cost is one heartbeat \
             round and the patience to wait for apply — which is why ReadIndex is how a \
             production Raft serves reads, rather than appending a no-op entry per read.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(a_fresh_cluster, |ctx| {
    let p = ctx.prim("raft-reads").await?;
    let init = p.send("init 3").await?;
    let state = p.send("state").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("three nodes, one leader");
    c.eq("init.term", 1, p.expect_i64(&init, "init", "term")?);
    c.eq("state.quorum", 2, p.expect_i64(&state, "state", "quorum")?);
    c.eq("state.leader", 0, p.expect_i64(&state, "state", "leader")?);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_partitioned_node_loses, |ctx| {
    let p = ctx.prim("raft-reads").await?;
    p.send("init 3").await?;
    p.send("partition 2").await?;
    let campaign = p.send("campaign 2").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("an isolated candidate");
    c.note(
        "One vote out of three is not a majority, and no amount of retrying changes that. \
         The question this stage asks is what the attempt costs the rest of the cluster.",
    );
    c.eq(
        "it did not become leader",
        false,
        p.expect_bool(&campaign, "campaign", "became_leader")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(prevote_protects_the_term, |ctx| {
    let p = ctx.prim("raft-reads").await?;
    p.send("init 3").await?;
    p.send("partition 2").await?;
    let before = p.send("state").await?;
    let campaign = p.send("campaign 2").await?;
    let after = p.send("state").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a pre-vote that fails");
    c.note(
        "The candidate asked before it acted. Nobody would have voted for it, so it never \
         incremented its term — and the cluster it will rejoin is exactly as it left it.",
    );
    c.eq(
        "campaign.term_raised",
        false,
        p.expect_bool(&campaign, "campaign", "term_raised")?,
    );
    c.eq(
        "the term is unchanged",
        p.expect_i64(&before, "state", "term")?,
        p.expect_i64(&after, "state", "term")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(without_prevote_it_disrupts, |ctx| {
    let p = ctx.prim("raft-reads").await?;
    p.send("init 3").await?;
    p.send("prevote off").await?;
    p.send("partition 2").await?;
    let before = p.send("state").await?;
    let first = p.send("campaign 2").await?;
    p.send("campaign 2").await?;
    let after = p.send("state").await?;
    let transcript = p.transcript_block();
    let start = p.expect_i64(&before, "state", "term")?;
    let end = p.expect_i64(&after, "state", "term")?;
    let mut c = Check::new("the same node without pre-vote");
    c.note(
        "Two failed campaigns, two term increments. When the partition heals, this node's \
         term is ahead of the leader's and the leader steps down — a re-election caused by a \
         node that was never able to win one. That is the disruption pre-vote exists to stop.",
    );
    c.eq(
        "it still did not become leader",
        false,
        p.expect_bool(&first, "campaign", "became_leader")?,
    );
    c.eq(
        "but the term was raised",
        true,
        p.expect_bool(&first, "campaign", "term_raised")?,
    );
    c.eq("twice over", start + 2, end);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(read_index_needs_a_quorum, |ctx| {
    let p = ctx.prim("raft-reads").await?;
    p.send("init 3").await?;
    p.send("append").await?;
    p.send("apply").await?;
    let healthy = p.send("read-index").await?;
    p.send("partition 1").await?;
    p.send("partition 2").await?;
    let isolated = p.send("read-index").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a leader that can no longer hear anyone");
    c.note(
        "A leader does not find out it has been deposed; it finds out it cannot hear a \
         quorum, which is the same thing from the inside. Answering a read here would risk \
         serving a state some other leader has already moved past.",
    );
    c.eq(
        "with a quorum",
        true,
        p.expect_bool(&healthy, "read-index", "served")?,
    );
    c.eq(
        "without one",
        false,
        p.expect_bool(&isolated, "read-index", "served")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(read_index_waits_for_apply, |ctx| {
    let p = ctx.prim("raft-reads").await?;
    p.send("init 3").await?;
    p.send("append").await?;
    let before = p.send("read-index").await?;
    p.send("apply").await?;
    let after = p.send("read-index").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a state machine behind the log");
    c.note(
        "Committed and applied are different things. The entry is safely replicated, but the \
         state machine has not run it yet, so reading from that state machine would return \
         the world as it was before a committed write — which is exactly what linearizable \
         means to rule out.",
    );
    c.eq(
        "before apply",
        false,
        p.expect_bool(&before, "read-index", "served")?,
    );
    c.eq(
        "after apply",
        true,
        p.expect_bool(&after, "read-index", "served")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(read_index_reports_its_index, |ctx| {
    let p = ctx.prim("raft-reads").await?;
    p.send("init 5").await?;
    for _ in 0..3 {
        p.send("append").await?;
    }
    p.send("apply").await?;
    let read = p.send("read-index").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("the index a read was safe at");
    c.note(
        "The index is the point of the mechanism: the read is answered as of a state that \
         includes everything committed when the read began. A client that wants to chain \
         operations can use it, and a reviewer can check it.",
    );
    c.eq(
        "read_index",
        3,
        p.expect_i64(&read, "read-index", "read_index")?,
    );
    c.eq("applied", 3, p.expect_i64(&read, "read-index", "applied")?);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(transfer_moves_leadership, |ctx| {
    let p = ctx.prim("raft-reads").await?;
    p.send("init 3").await?;
    let before = p.send("state").await?;
    let moved = p.send("transfer 2").await?;
    let after = p.send("state").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("handing leadership over");
    c.note(
        "One term increment and the leadership is somewhere else — no election timeout, no \
         window with no leader. This is how a node is taken out of service without the \
         cluster pausing to notice.",
    );
    c.eq(
        "transferred",
        true,
        p.expect_bool(&moved, "transfer", "transferred")?,
    );
    c.eq(
        "the new leader",
        2,
        p.expect_i64(&after, "state", "leader")?,
    );
    c.eq(
        "the term moved exactly once",
        p.expect_i64(&before, "state", "term")? + 1,
        p.expect_i64(&after, "state", "term")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(transfer_to_unreachable, |ctx| {
    let p = ctx.prim("raft-reads").await?;
    p.send("init 3").await?;
    p.send("partition 2").await?;
    let moved = p.send("transfer 2").await?;
    let after = p.send("state").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("transferring to a node that cannot be reached");
    c.note(
        "A transfer the target cannot complete would leave the cluster with no leader until \
         somebody times out — strictly worse than not transferring. Refusing keeps the \
         leader where it is.",
    );
    c.eq(
        "transferred",
        false,
        p.expect_bool(&moved, "transfer", "transferred")?,
    );
    c.eq(
        "the leader is unchanged",
        0,
        p.expect_i64(&after, "state", "leader")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_deposed_leader_stops_reading, |ctx| {
    let p = ctx.prim("raft-reads").await?;
    p.send("init 5").await?;
    p.send("append").await?;
    p.send("apply").await?;
    // Three of five gone: the leader is in the minority now.
    for i in 1..4 {
        p.send(&format!("partition {i}")).await?;
    }
    let read = p.send("read-index").await?;
    let appended = p.send("append").await?;
    p.send("heal 1").await?;
    p.send("heal 2").await?;
    let again = p.send("read-index").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a leader in the minority, then back");
    c.note(
        "While it cannot hear a quorum it does nothing: no reads, no commits. When enough \
         nodes come back it resumes without any special recovery, because it never did \
         anything it would have to take back.",
    );
    c.eq(
        "no read while isolated",
        false,
        p.expect_bool(&read, "read-index", "served")?,
    );
    c.eq(
        "no commit either",
        false,
        p.expect_bool(&appended, "append", "committed")?,
    );
    c.eq(
        "and it serves again once healed",
        true,
        p.expect_bool(&again, "read-index", "served")?,
    );
    c.block("transcript", transcript);
    c.finish()
});
