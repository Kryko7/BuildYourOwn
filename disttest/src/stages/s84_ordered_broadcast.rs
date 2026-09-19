//! Stage 84 — Ordered delivery: FIFO, causal, total.
//!
//! Reliable broadcast says everyone gets the message. It says nothing about *when*, and two
//! processes that deliver the same set in different orders will reach different states from
//! the same input. Three orderings fix that, each strictly stronger than the last.
//!
//! **FIFO**: messages from the same sender are delivered in the order that sender sent them.
//! A sequence number per sender and a hold-back queue is the whole implementation.
//!
//! **Causal**: if sending m2 could have been influenced by delivering m1, then m1 is
//! delivered first everywhere. This is FIFO plus the rest of the happens-before relation,
//! and a vector clock is the usual way to carry it (stage 02 built the clock; this is what
//! it is for).
//!
//! **Total**: every correct process delivers every message in the *same* order, whether or
//! not the messages are related at all. That last one is not a small step: total-order
//! broadcast and consensus are reducible to each other, so anything that implements it has
//! implemented consensus, with everything that implies about failure detectors and majorities.
//! A sequencer is the shortest way to get it, and the reason a sequencer needs failover is
//! the reason it is consensus in disguise.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};

/// Stage 84.
pub fn stage() -> Stage {
    Stage {
        number: 84,
        slug: "ordered_broadcast",
        name: "Ordered delivery: FIFO, causal, total",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `ordering`: `order <fifo|causal|total>`, `send <from>`, `offer <id> <to>` to \
             try a delivery, `log <p>` for what a process has delivered",
            "A message that may not be delivered yet is *held back*, not refused — it waits in a \
             queue until what it depends on has arrived",
            "FIFO needs one sequence number per sender; causal needs what the sender had already \
             delivered, which is exactly a vector clock",
            "Total order means every process's log is identical, which is why it cannot be done \
             without agreement — it is consensus wearing a different name",
        ],
        examples,
        tests: vec![
            Test::new(
                "with nothing held back, order is whatever arrives",
                the_unordered_case,
            ),
            Test::new(
                "FIFO holds back a message that overtook its predecessor",
                fifo_holds_back,
            ),
            Test::new("and releases it once the gap is filled", fifo_releases),
            Test::new(
                "FIFO says nothing about two different senders",
                fifo_is_per_sender,
            ),
            Test::new(
                "causal holds back a reply until the message it replies to arrives",
                causal_holds_back_a_reply,
            ),
            Test::new(
                "and lets concurrent messages through in any order",
                causal_allows_concurrency,
            ),
            Test::new(
                "total order gives every process the same log",
                total_order_agrees,
            ),
            Test::new(
                "which FIFO and causal both allow to differ",
                weaker_orders_allow_divergence,
            ),
            Test::new("a message is never delivered twice", no_duplicates).ext(),
            Test::new(
                "under total order, a gap blocks everything behind it",
                total_order_blocks_on_a_gap,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("A reply that overtakes its cause", "ordering", || {
            lines(&[
                "init 3",
                "order causal",
                "send 0",
                "offer 1 1",
                "send 1",
                "offer 2 2",
                "log 2",
                "offer 1 2",
                "offer 2 2",
                "log 2",
            ])
        })
        .request("0 sends, 1 delivers it and sends a reply, and the reply reaches 2 first")
        .response("the reply is held back, and is delivered only after the message it replies to")
        .note(
            "Process 2 sees the answer before the question — over a network that reorders, \
             which is every network. Causal order is the machinery that makes that \
             impossible to observe, and the vector clock on the message is what carries the \
             information needed to spot it.",
        ),
        prim_example("Total order means identical logs", "ordering", || {
            lines(&[
                "init 3",
                "order total",
                "send 0",
                "send 1",
                "offer 2 1",
                "offer 1 1",
                "offer 2 1",
                "log 1",
            ])
        })
        .request("Two messages from different senders, offered to one process out of order")
        .response("the second message is held back until the first has been delivered")
        .note(
            "Under FIFO or causal these two are unrelated and either order would be fine. \
             Total order refuses anyway, because every process has to end up with the same \
             log — and that agreement is the part that cannot be done without consensus.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

/// What one process has delivered, in order.
async fn log_of(
    p: &mut crate::prim::PrimProc,
    who: usize,
) -> Result<Vec<i64>, crate::stages::Failure> {
    let v = p.send(&format!("log {who}")).await?;
    Ok(v["log"]
        .as_array()
        .map(|a| a.iter().filter_map(serde_json::Value::as_i64).collect())
        .unwrap_or_default())
}

dist_test!(the_unordered_case, |ctx| {
    let p = ctx.prim("ordering").await?;
    p.send("init 2").await?;
    p.send("order fifo").await?;
    p.send("send 0").await?;
    p.send("offer 1 1").await?;
    let log = log_of(p, 1).await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("one message, nothing to order it against");
    c.note("The uninteresting case, which every ordering has to get right first.");
    c.eq("the log", vec![1], log);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(fifo_holds_back, |ctx| {
    let p = ctx.prim("ordering").await?;
    p.send("init 2").await?;
    p.send("order fifo").await?;
    p.send("send 0").await?;
    p.send("send 0").await?;
    // The second one arrives first.
    let second = p.send("offer 2 1").await?;
    let log = log_of(p, 1).await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a message that overtook its predecessor");
    c.note(
        "Holding back is not an error path. The message is fine, the network is fine, and \
         the receiver simply cannot deliver it yet without breaking the promise it made.",
    );
    c.eq(
        "it was held back",
        false,
        p.expect_bool(&second, "offer", "delivered")?,
    );
    c.eq("and nothing is in the log", Vec::<i64>::new(), log);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(fifo_releases, |ctx| {
    let p = ctx.prim("ordering").await?;
    p.send("init 2").await?;
    p.send("order fifo").await?;
    p.send("send 0").await?;
    p.send("send 0").await?;
    p.send("offer 2 1").await?;
    p.send("offer 1 1").await?;
    p.send("offer 2 1").await?;
    let log = log_of(p, 1).await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("the gap filled");
    c.note(
        "Once the missing message arrives, everything queued behind it becomes deliverable. \
         An implementation that drops held-back messages instead of queueing them will pass \
         the previous test and fail this one.",
    );
    c.eq("the log, in sending order", vec![1, 2], log);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(fifo_is_per_sender, |ctx| {
    let p = ctx.prim("ordering").await?;
    p.send("init 3").await?;
    p.send("order fifo").await?;
    p.send("send 0").await?;
    p.send("send 1").await?;
    // Message 2 is from a different sender, so nothing holds it back.
    let out_of_order = p.send("offer 2 2").await?;
    let log = log_of(p, 2).await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("two senders, one receiver");
    c.note(
        "FIFO is a promise about each sender separately. Messages from different senders may \
         be delivered in any order at all, which is why FIFO alone is not enough to stop a \
         reply arriving before the thing it replies to.",
    );
    c.eq(
        "the later message went straight through",
        true,
        p.expect_bool(&out_of_order, "offer", "delivered")?,
    );
    c.eq("the log", vec![2], log);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(causal_holds_back_a_reply, |ctx| {
    let p = ctx.prim("ordering").await?;
    p.send("init 3").await?;
    p.send("order causal").await?;
    p.send("send 0").await?;
    // Process 1 delivers it and then sends its own message — a reply, causally after it.
    p.send("offer 1 1").await?;
    p.send("send 1").await?;
    // The reply reaches process 2 first.
    let early = p.send("offer 2 2").await?;
    let before = log_of(p, 2).await?;
    p.send("offer 1 2").await?;
    p.send("offer 2 2").await?;
    let after = log_of(p, 2).await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a reply arriving before its cause");
    c.note(
        "Process 1 had delivered message 1 before it sent message 2, so message 2 carries \
         that knowledge in its clock. Process 2 can see it is missing something and waits, \
         which is the difference between a system that merely delivers everything and one \
         that never shows anybody an effect before its cause.",
    );
    c.eq(
        "the reply is held back",
        false,
        p.expect_bool(&early, "offer", "delivered")?,
    );
    c.eq("so nothing is delivered yet", Vec::<i64>::new(), before);
    c.eq("and then both, in causal order", vec![1, 2], after);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(causal_allows_concurrency, |ctx| {
    let p = ctx.prim("ordering").await?;
    p.send("init 3").await?;
    p.send("order causal").await?;
    // Neither sender has heard from the other, so the two messages are concurrent.
    p.send("send 0").await?;
    p.send("send 1").await?;
    let second_first = p.send("offer 2 2").await?;
    let first_second = p.send("offer 1 2").await?;
    let log = log_of(p, 2).await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("two messages that are not causally related");
    c.note(
        "Causal order constrains only what happens-before requires. Two concurrent messages \
         may be delivered in either order, and two processes may choose differently — which \
         is exactly what total order rules out and causal does not.",
    );
    c.eq(
        "the later one went first",
        true,
        p.expect_bool(&second_first, "offer", "delivered")?,
    );
    c.eq(
        "and the earlier one after it",
        true,
        p.expect_bool(&first_second, "offer", "delivered")?,
    );
    c.eq("the log, in arrival order", vec![2, 1], log);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(total_order_agrees, |ctx| {
    let p = ctx.prim("ordering").await?;
    p.send("init 3").await?;
    p.send("order total").await?;
    p.send("send 0").await?;
    p.send("send 1").await?;
    p.send("send 2").await?;
    // Offer them to each process in a different arrival order.
    for (who, order) in [(0usize, [1, 2, 3]), (1, [3, 1, 2]), (2, [2, 3, 1])] {
        for _ in 0..3 {
            for id in order {
                p.send(&format!("offer {id} {who}")).await?;
            }
        }
    }
    let logs = [
        log_of(p, 0).await?,
        log_of(p, 1).await?,
        log_of(p, 2).await?,
    ];
    let transcript = p.transcript_block();
    let mut c = Check::new("three processes, three arrival orders");
    c.note(
        "Every process was offered the messages in a different order and every process \
         delivered them in the same one. That is the whole promise, and it is why a system \
         that has it can run a replicated state machine: same input sequence, same state.",
    );
    c.eq(
        "all three logs are identical",
        vec![vec![1, 2, 3], vec![1, 2, 3], vec![1, 2, 3]],
        logs.to_vec(),
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(weaker_orders_allow_divergence, |ctx| {
    let p = ctx.prim("ordering").await?;
    p.send("init 3").await?;
    p.send("order causal").await?;
    p.send("send 0").await?;
    p.send("send 1").await?;
    // Concurrent messages, offered to the two receivers in opposite orders.
    p.send("offer 1 2").await?;
    p.send("offer 2 2").await?;
    p.send("offer 2 0").await?;
    p.send("offer 1 0").await?;
    let logs = [log_of(p, 0).await?, log_of(p, 2).await?];
    let transcript = p.transcript_block();
    let mut c = Check::new("causal order with concurrent messages");
    c.note(
        "Two correct processes, the same two messages, two different orders — and causal \
         order is perfectly satisfied, because nothing happened-before anything. If the \
         messages were state machine commands these two processes have now diverged, which \
         is the argument for paying for total order.",
    );
    c.eq(
        "the logs differ",
        vec![vec![2, 1], vec![1, 2]],
        logs.to_vec(),
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(no_duplicates, |ctx| {
    let p = ctx.prim("ordering").await?;
    p.send("init 2").await?;
    p.send("order fifo").await?;
    p.send("send 0").await?;
    p.send("offer 1 1").await?;
    let again = p.send("offer 1 1").await?;
    let log = log_of(p, 1).await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("the same message offered twice");
    c.note(
        "A hold-back queue makes re-offering normal: the network retries, the sender relays, \
         and the same message arrives again. Delivering it twice would corrupt every state \
         machine built on top.",
    );
    c.eq(
        "the second offer delivers nothing",
        false,
        p.expect_bool(&again, "offer", "delivered")?,
    );
    c.eq("the log", vec![1], log);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(total_order_blocks_on_a_gap, |ctx| {
    let p = ctx.prim("ordering").await?;
    p.send("init 2").await?;
    p.send("order total").await?;
    p.send("send 0").await?;
    p.send("send 0").await?;
    p.send("send 1").await?;
    // The third message is available; the first is not.
    let third = p.send("offer 3 1").await?;
    let second = p.send("offer 2 1").await?;
    let log = log_of(p, 1).await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a gap in the total order");
    c.note(
        "Nothing after the gap may be delivered, however long the wait. That is the cost of \
         the guarantee: total order turns one slow or lost message into a stall for \
         everything behind it, which is why systems that can live with causal order usually \
         do.",
    );
    c.eq(
        "the third is held",
        false,
        p.expect_bool(&third, "offer", "delivered")?,
    );
    c.eq(
        "and so is the second",
        false,
        p.expect_bool(&second, "offer", "delivered")?,
    );
    c.eq("nothing delivered", Vec::<i64>::new(), log);
    c.block("transcript", transcript);
    c.finish()
});
