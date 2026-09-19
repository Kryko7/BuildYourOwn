//! Stage 86 — Byzantine quorums: what changes when a replica may lie.
//!
//! Every quorum system in this track so far assumes a failed node goes quiet. A node that
//! keeps answering but answers wrongly — corrupted memory, a bad disk, buggy code, or an
//! actual adversary — breaks the arithmetic, and the fix is arithmetic too.
//!
//! With crash faults, two majorities share at least one node, and that shared node's answer
//! is the truth. With Byzantine faults the shared node might be one of the liars, so
//! quorums have to overlap by **more than f** nodes: 2q − n > f, which gives q > (n + f)/2.
//! Availability wants a quorum to be reachable while f are silent, so q ≤ n − f. Put the two
//! together and n > 3f falls out — the reason every Byzantine protocol is sized 3f + 1 and
//! not 2f + 1.
//!
//! The same counting governs what a client may believe. One reply proves nothing. **f + 1
//! matching replies** is the threshold, because f of them could be lies and the remaining
//! one is then honest.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};

/// Stage 86.
pub fn stage() -> Stage {
    Stage {
        number: 86,
        slug: "byzantine_quorums",
        name: "Byzantine quorums: sizing for replicas that lie",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `byzantine`: `init <nodes> <faults>`, `quorum`, `intersect`, `safe`, and \
             `decide <reply>...` for what a client may believe",
            "Two quorums must share more than f nodes so that at least one shared node is \
             honest: q > (n + f) / 2",
            "A quorum must also be reachable while f nodes are silent, so q ≤ n − f; the two \
             bounds together require n > 3f",
            "With f = 0 the rule collapses to the ordinary majority, which is why crash-fault \
             systems are sized 2f + 1 and Byzantine ones 3f + 1",
        ],
        examples,
        tests: vec![
            Test::new(
                "with no Byzantine faults it is a plain majority",
                crash_faults_only,
            ),
            Test::new(
                "one liar in four needs three, not three of five",
                one_fault_in_four,
            ),
            Test::new(
                "two quorums always share an honest node",
                intersection_has_an_honest_node,
            ),
            Test::new("three nodes cannot tolerate one liar", three_is_not_enough),
            Test::new(
                "but four can, and seven can tolerate two",
                the_sizes_that_work,
            ),
            Test::new(
                "a single reply is never enough to believe",
                one_reply_proves_nothing,
            ),
            Test::new("f + 1 matching replies are", f_plus_one_is_enough),
            Test::new(
                "a majority of replies is not the same as f + 1 of them",
                majority_is_the_wrong_threshold,
            ),
            Test::new(
                "quorum size grows with f, not with n alone",
                quorum_grows_with_f,
            )
            .ext(),
            Test::new("the three-way trade-off, tabulated", the_whole_table).ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("Why 3f + 1 and not 2f + 1", "byzantine", || {
            lines(&["init 3 1", "safe", "intersect", "init 4 1", "safe", "intersect"])
        })
        .request("One Byzantine fault, first with three nodes and then with four")
        .response("three nodes is not safe; four gives a quorum of three sharing two nodes, one of them certainly honest")
        .note(
            "With three nodes and one liar, a quorum would have to be all three — and then \
             nothing is available while one node is merely slow. Four is the first size where \
             a quorum is both reachable and guaranteed to overlap another quorum in an \
             honest node.",
        ),
        prim_example("What a client may believe", "byzantine", || {
            lines(&[
                "init 4 1",
                "decide red red blue",
                "decide red blue green",
            ])
        })
        .request("Three replies that mostly agree, then three that do not")
        .response("the first decides `red` on two matching replies; the second decides nothing")
        .note(
            "Two matching replies with f = 1 means at least one honest node said it, which \
             is enough. Three different answers means at most one of them came from an \
             honest node and there is no way to tell which — so the client must wait for \
             more replies rather than pick the most popular.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(crash_faults_only, |ctx| {
    let p = ctx.prim("byzantine").await?;
    let init = p.send("init 5 0").await?;
    let inter = p.send("intersect").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("five nodes, no liars");
    c.note(
        "With f = 0 the Byzantine bound is the majority bound: three of five, overlapping in \
         one node, and that one node's answer is the truth because nobody lies.",
    );
    c.eq("quorum", 3, p.expect_i64(&init, "init", "quorum")?);
    c.eq(
        "shared between two quorums",
        1,
        p.expect_i64(&inter, "intersect", "shared")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(one_fault_in_four, |ctx| {
    let p = ctx.prim("byzantine").await?;
    let init = p.send("init 4 1").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("four nodes, one of them lying");
    c.note(
        "q > (4 + 1) / 2 means q ≥ 3. A plain majority would be three as well here, which is \
         a coincidence of the numbers — at n = 7, f = 2 the majority is four and the \
         Byzantine quorum is five.",
    );
    c.eq("quorum", 3, p.expect_i64(&init, "init", "quorum")?);
    c.eq("safe", true, p.expect_bool(&init, "init", "safe")?);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(intersection_has_an_honest_node, |ctx| {
    let mut rows = Vec::new();
    for (n, f) in [(4, 1), (7, 2), (10, 3), (13, 4)] {
        let mut p = ctx.prim_fresh("byzantine").await?;
        p.send(&format!("init {n} {f}")).await?;
        let inter = p.send("intersect").await?;
        rows.push((
            p.expect_i64(&inter, "intersect", "shared")?,
            p.expect_i64(&inter, "intersect", "honest_shared")?,
        ));
    }
    let mut c = Check::new("the overlap at four safe sizes");
    c.note(
        "In every case the two quorums share more than f nodes, so at least one shared node \
         is honest. That honest node is what carries information from one quorum to the \
         next, and it is the only reason a Byzantine protocol can conclude anything at all.",
    );
    c.eq(
        "(shared, honest_shared) at 4/1, 7/2, 10/3, 13/4",
        vec![(2, 1), (3, 1), (4, 1), (5, 1)],
        rows,
    );
    c.finish()
});

dist_test!(three_is_not_enough, |ctx| {
    let p = ctx.prim("byzantine").await?;
    p.send("init 3 1").await?;
    let safe = p.send("safe").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("three nodes against one liar");
    c.note(
        "A quorum here would have to be all three, and then a single slow node stops \
         everything — the system is safe and never available. This is why PBFT, Tendermint \
         and every other Byzantine protocol starts at four replicas for one fault.",
    );
    c.eq("safe", false, p.expect_bool(&safe, "safe", "safe")?);
    c.eq("nodes needed", 4, p.expect_i64(&safe, "safe", "needed")?);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(the_sizes_that_work, |ctx| {
    let mut rows = Vec::new();
    for (n, f) in [(3, 1), (4, 1), (6, 2), (7, 2), (9, 3), (10, 3)] {
        let mut p = ctx.prim_fresh("byzantine").await?;
        let init = p.send(&format!("init {n} {f}")).await?;
        rows.push(p.expect_bool(&init, "init", "safe")?);
    }
    let mut c = Check::new("sizes either side of the 3f + 1 line");
    c.note(
        "3f is never enough and 3f + 1 always is. The line is sharp, which is worth seeing \
         directly rather than taking on trust — every one of these systems is either \
         provably fine or provably broken, with nothing in between.",
    );
    c.eq(
        "3/1, 4/1, 6/2, 7/2, 9/3, 10/3",
        vec![false, true, false, true, false, true],
        rows,
    );
    c.finish()
});

dist_test!(one_reply_proves_nothing, |ctx| {
    let p = ctx.prim("byzantine").await?;
    p.send("init 4 1").await?;
    let one = p.send("decide red").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a client with one reply in hand");
    c.note(
        "The one node that answered may be the liar. With crash faults a single reply from a \
         live node is authoritative; with Byzantine faults it carries no information at all, \
         which changes how every client is written.",
    );
    c.eq("decided", false, p.expect_bool(&one, "decide", "decided")?);
    c.eq("replies needed", 2, p.expect_i64(&one, "decide", "needed")?);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(f_plus_one_is_enough, |ctx| {
    let p = ctx.prim("byzantine").await?;
    p.send("init 7 2").await?;
    let two = p.send("decide red red").await?;
    let three = p.send("decide red red red").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("matching replies at f = 2");
    c.note(
        "Two liars can produce two identical wrong answers between them, so two matching \
         replies mean nothing. Three cannot all be lies, so at least one honest node said \
         it — and an honest node only says what it holds.",
    );
    c.eq(
        "two matching",
        false,
        p.expect_bool(&two, "decide", "decided")?,
    );
    c.eq(
        "three matching",
        true,
        p.expect_bool(&three, "decide", "decided")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(majority_is_the_wrong_threshold, |ctx| {
    let p = ctx.prim("byzantine").await?;
    p.send("init 7 2").await?;
    // Two matching is a plurality of three replies, and still not enough.
    let plurality = p.send("decide red red blue").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("the most popular answer");
    c.note(
        "Taking the most common reply is the natural thing to write and it is wrong here: \
         two of these three could both be lies told by the same two faulty nodes. The \
         threshold is f + 1 matching, not a majority of whatever turned up.",
    );
    c.eq(
        "decided",
        false,
        p.expect_bool(&plurality, "decide", "decided")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(quorum_grows_with_f, |ctx| {
    let mut quorums = Vec::new();
    for f in 0..4 {
        let mut p = ctx.prim_fresh("byzantine").await?;
        let init = p.send(&format!("init 13 {f}")).await?;
        quorums.push(p.expect_i64(&init, "init", "quorum")?);
    }
    let mut c = Check::new("thirteen nodes, tolerating zero to three liars");
    c.note(
        "The node count is fixed and the quorum grows anyway. Tolerating liars is paid for \
         in quorum size — more nodes to hear from on every operation — which is the running \
         cost of Byzantine fault tolerance rather than a one-off.",
    );
    c.eq("quorum at f = 0, 1, 2, 3", vec![7, 8, 8, 9], quorums);
    c.finish()
});

dist_test!(the_whole_table, |ctx| {
    let mut rows = Vec::new();
    for f in 1..4 {
        let n = 3 * f + 1;
        let mut p = ctx.prim_fresh("byzantine").await?;
        let init = p.send(&format!("init {n} {f}")).await?;
        let inter = p.send("intersect").await?;
        rows.push((
            n,
            f,
            p.expect_i64(&init, "init", "quorum")?,
            p.expect_i64(&inter, "intersect", "honest_shared")?,
        ));
    }
    let mut c = Check::new("the minimum safe configuration for one, two and three liars");
    c.note(
        "At the minimum size the quorum is always 2f + 1 and the honest overlap is always \
         exactly one. The system is correct with nothing to spare, which is what \"minimum\" \
         means and why running at 3f + 1 exactly leaves no room for a simultaneous crash.",
    );
    c.eq(
        "(nodes, faults, quorum, honest overlap)",
        vec![(4, 1, 3, 1), (7, 2, 5, 1), (10, 3, 7, 1)],
        rows,
    );
    c.finish()
});
