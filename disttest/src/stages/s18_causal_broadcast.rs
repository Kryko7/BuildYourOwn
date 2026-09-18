//! Stage 18 — Causal broadcast delivery order.
//!
//! Vector clocks were stage 2's answer to "did this happen before that". This stage spends
//! them: a receiver holds a message back until everything it depends on has arrived, so a
//! reply can never be delivered before the message it replies to, however the network
//! reorders them on the way.
//!
//! The oracle is [`causally_deliverable`], the rule itself: a message from `s` may be
//! delivered when its clock says it is exactly the next one from `s`, and every other entry
//! is one the receiver has already seen. No test here compares the delivery order against a
//! list written by hand — the orders that satisfy causality are many, and picking one of
//! them would fail a correct program. Every test replays the order the program chose
//! through the rule instead, which accepts all of them and no others.

use crate::assert::{Check, Failure};
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::prim::oracles::{causally_deliverable, Clock};
use crate::prim::PrimProc;
use crate::stages::{Ladder, Stage, Test};
use rand::seq::SliceRandom;
use rand::Rng;
use serde_json::Value;
use std::collections::BTreeMap;

/// Stage 18.
pub fn stage() -> Stage {
    Stage {
        number: 18,
        slug: "causal_broadcast",
        name: "Causal broadcast delivery order",
        ext: false,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `causal-broadcast`: a message carries the sender's vector clock",
            "Deliver only when the message is the sender's next, and its other entries are already seen",
            "Anything else waits in a buffer and is re-checked after every delivery",
            "Delivering a message out of causal order is the failure this stage looks for",
        ],
        examples,
        tests: vec![
            Test::new(
                "a message whose dependency has not arrived is buffered, not delivered",
                a_gap_holds_a_message_back,
            ),
            Test::new(
                "filling the gap releases the buffered message at once",
                filling_the_gap_releases_it,
            ),
            Test::new(
                "a sender's own messages are never delivered out of order",
                one_sender_in_order,
            ),
            Test::new(
                "every delivery order satisfies the causal rule",
                the_order_satisfies_the_rule,
            ),
            Test::new(
                "a message that has already been delivered is not delivered twice",
                no_message_is_delivered_twice,
            ),
            Test::new(
                "a seeded reordering of a causal chain still delivers in causal order",
                a_seeded_reordering,
            ),
            Test::new(
                "a broadcast stamps the sender's own entry and nothing else",
                a_broadcast_stamps_one_entry,
            ),
            Test::new(
                "two concurrent messages may be delivered in either order",
                concurrent_messages_need_not_wait,
            )
            .ext(),
            Test::new(
                "a chain delivered in the order it was created never buffers anything",
                a_chain_in_order_never_buffers,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example(
            "A reply that overtakes what it replies to",
            "causal-broadcast",
            || {
                lines(&[
                    "init a,b,c",
                    "bcast a m1",
                    "recv b a m1 {\"a\":1}",
                    "bcast b m2",
                    "recv c b m2 {\"a\":1,\"b\":1}",
                    "recv c a m1 {\"a\":1}",
                    "delivered c",
                ])
            },
        )
        .request("`b` answers `a`, and `c` is handed the answer before the question")
        .response("m2 waits in the buffer, then both are delivered the moment m1 arrives")
        .note(
            "The clock on m2 is {\"a\":1,\"b\":1}: it says `b` had seen one message from `a` \
             before it spoke. `c` has seen none, so the entry for `a` is the gap, and the \
             buffer is what a receiver does with a message it is not yet allowed to read.",
        ),
        prim_example(
            "Two senders who never heard each other",
            "causal-broadcast",
            || {
                lines(&[
                    "init a,b,c",
                    "bcast a m1",
                    "bcast b m2",
                    "recv c b m2 {\"b\":1}",
                    "recv c a m1 {\"a\":1}",
                    "delivered c",
                ])
            },
        )
        .request("`a` and `b` broadcast without seeing each other, and `c` hears `b` first")
        .response("both are delivered straight away, in the order they arrived")
        .note(
            "Causal order is a partial order, not a queue. m1 and m2 are concurrent, so \
             either order is correct and a test that demands one of them is wrong. Holding \
             m2 back until m1 arrives would be the opposite mistake: needless blocking.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Reading clocks and delivery lists
// ---------------------------------------------------------------------------------------

/// Read a clock out of an answer, dropping zero entries: `{}` and `{"a":0}` are the same
/// clock written two ways.
fn clock_of(p: &PrimProc, v: &Value, command: &str, field: &str) -> Result<Clock, Failure> {
    let Some(value) = v.get(field) else {
        return Err(p.shape(command, &format!("the answer has no {field:?} field")));
    };
    let Some(map) = value.as_object() else {
        return Err(p.shape(
            command,
            &format!("{field} should be an object, got {value}"),
        ));
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

/// A clock as a command-line argument: compact JSON, because commands split on whitespace.
fn as_arg(c: &Clock) -> String {
    let object: serde_json::Map<String, Value> = c
        .iter()
        .map(|(name, count)| (name.clone(), Value::from(*count)))
        .collect();
    crate::prim::arg(&Value::Object(object))
}

/// One message of a history: who sent it, what it is called, and the clock it carries.
#[derive(Clone)]
struct Message {
    from: String,
    name: String,
    vc: Clock,
}

/// Broadcast a message and take down the clock the program stamped on it.
async fn bcast(p: &mut PrimProc, from: &str, name: &str) -> Result<Message, Failure> {
    let command = format!("bcast {from} {name}");
    let v = p.send(&command).await?;
    Ok(Message {
        from: from.to_string(),
        name: name.to_string(),
        vc: clock_of(p, &v, &command, "vc")?,
    })
}

/// Hand a message to a process, and report what that released and what is still waiting.
async fn deliver_to(
    p: &mut PrimProc,
    to: &str,
    m: &Message,
) -> Result<(Vec<String>, i64), Failure> {
    let command = format!("recv {to} {} {} {}", m.from, m.name, as_arg(&m.vc));
    let v = p.send(&command).await?;
    let delivered = p.expect_strs(&v, &command, "delivered")?;
    let buffered = p.expect_i64(&v, &command, "buffered")?;
    Ok((delivered, buffered))
}

/// The messages a process has delivered, in delivery order.
async fn delivered(p: &mut PrimProc, who: &str) -> Result<Vec<String>, Failure> {
    let command = format!("delivered {who}");
    let v = p.send(&command).await?;
    p.expect_strs(&v, &command, "messages")
}

/// Replay a delivery order through the rule itself.
///
/// Starting from an empty clock, every message in turn must have been deliverable at the
/// moment it was delivered. This accepts every causally correct order — and there are many,
/// because concurrent messages may come in either order — and rejects every other one.
fn check_causal_order(
    c: &mut Check,
    path: &str,
    order: &[String],
    history: &BTreeMap<String, Message>,
) {
    let mut seen = Clock::new();
    for (i, name) in order.iter().enumerate() {
        let Some(m) = history.get(name) else {
            c.that(
                &format!("{path}[{i}]"),
                "a message this test broadcast",
                false,
                name.clone(),
            );
            return;
        };
        if !causally_deliverable(&seen, &m.from, &m.vc) {
            c.that(
                &format!("{path}[{i}] = {name}"),
                "a message whose causal dependencies had all been delivered",
                false,
                format!(
                    "{name} from {} carrying {}, delivered at a process whose clock was {}",
                    m.from,
                    as_arg(&m.vc),
                    as_arg(&seen)
                ),
            );
            return;
        }
        *seen.entry(m.from.clone()).or_insert(0) += 1;
    }
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(a_gap_holds_a_message_back, |ctx| {
    let p = ctx.prim("causal-broadcast").await?;
    p.send("init a,b,c").await?;
    let m1 = bcast(p, "a", "m1").await?;
    deliver_to(p, "b", &m1).await?;
    let m2 = bcast(p, "b", "m2").await?;
    // `c` is handed the reply first. Its clock says `b` had seen m1, which `c` has not.
    let (released, buffered) = deliver_to(p, "c", &m2).await?;
    let log = delivered(p, "c").await?;
    let mut c = Check::new("a message that arrived before what it depends on");
    c.eq("recv(c, m2).delivered", Vec::<String>::new(), released);
    c.eq("recv(c, m2).buffered", 1, buffered);
    c.eq("delivered(c).messages", Vec::<String>::new(), log);
    c.eq("bcast(b, m2).vc.a", Some(1), m2.vc.get("a").copied());
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(filling_the_gap_releases_it, |ctx| {
    let p = ctx.prim("causal-broadcast").await?;
    p.send("init a,b,c").await?;
    let m1 = bcast(p, "a", "m1").await?;
    deliver_to(p, "b", &m1).await?;
    let m2 = bcast(p, "b", "m2").await?;
    deliver_to(p, "c", &m2).await?;
    // The gap closes, and both messages become deliverable in the same breath.
    let (released, buffered) = deliver_to(p, "c", &m1).await?;
    let log = delivered(p, "c").await?;
    let mut c = Check::new("the delivery that closes a gap");
    c.eq(
        "recv(c, m1).delivered",
        vec!["m1".to_string(), "m2".to_string()],
        released,
    );
    c.eq("recv(c, m1).buffered", 0, buffered);
    c.eq(
        "delivered(c).messages",
        vec!["m1".to_string(), "m2".to_string()],
        log,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(one_sender_in_order, |ctx| {
    let p = ctx.prim("causal-broadcast").await?;
    p.send("init a,b").await?;
    let m1 = bcast(p, "a", "m1").await?;
    let m2 = bcast(p, "a", "m2").await?;
    let m3 = bcast(p, "a", "m3").await?;
    // Handed to `b` backwards: the first two have to wait for the one they follow.
    let (r3, b3) = deliver_to(p, "b", &m3).await?;
    let (r2, b2) = deliver_to(p, "b", &m2).await?;
    let (r1, b1) = deliver_to(p, "b", &m1).await?;
    let log = delivered(p, "b").await?;
    let mut c = Check::new("three messages from one sender, delivered backwards");
    c.eq("recv(b, m3).delivered", Vec::<String>::new(), r3);
    c.eq("recv(b, m3).buffered", 1, b3);
    c.eq("recv(b, m2).delivered", Vec::<String>::new(), r2);
    c.eq("recv(b, m2).buffered", 2, b2);
    c.eq(
        "recv(b, m1).delivered",
        vec!["m1".to_string(), "m2".to_string(), "m3".to_string()],
        r1,
    );
    c.eq("recv(b, m1).buffered", 0, b1);
    c.eq(
        "delivered(b).messages",
        vec!["m1".to_string(), "m2".to_string(), "m3".to_string()],
        log,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_order_satisfies_the_rule, |ctx| {
    let p = ctx.prim("causal-broadcast").await?;
    p.send("init a,b,c,d").await?;
    // A history with two chains and one pair of concurrent messages in it.
    let m1 = bcast(p, "a", "m1").await?;
    deliver_to(p, "b", &m1).await?;
    let m2 = bcast(p, "b", "m2").await?;
    let m3 = bcast(p, "c", "m3").await?;
    deliver_to(p, "a", &m3).await?;
    let m4 = bcast(p, "a", "m4").await?;
    let history: BTreeMap<String, Message> = [&m1, &m2, &m3, &m4]
        .into_iter()
        .map(|m| (m.name.clone(), m.clone()))
        .collect();
    // `d` hears everything in an order the network chose, not the order it happened in.
    for m in [&m4, &m2, &m3, &m1] {
        deliver_to(p, "d", m).await?;
    }
    let log = delivered(p, "d").await?;
    let mut c = Check::new("the order `d` delivered a four-message history in");
    check_causal_order(&mut c, "delivered(d).messages", &log, &history);
    c.eq("delivered(d).messages.len()", 4, log.len());
    c.observe("delivered(d).messages", log.clone());
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(no_message_is_delivered_twice, |ctx| {
    let p = ctx.prim("causal-broadcast").await?;
    p.send("init a,b").await?;
    let m1 = bcast(p, "a", "m1").await?;
    let (first, _) = deliver_to(p, "b", &m1).await?;
    // The same message again: a retransmission, or a gossip round that came the long way.
    let (again, buffered) = deliver_to(p, "b", &m1).await?;
    let log = delivered(p, "b").await?;
    let mut c = Check::new("a message delivered to the same process twice");
    c.eq(
        "the first recv(b, m1).delivered",
        vec!["m1".to_string()],
        first,
    );
    c.eq(
        "the second recv(b, m1).delivered",
        Vec::<String>::new(),
        again,
    );
    c.eq("delivered(b).messages", vec!["m1".to_string()], log);
    c.observe("the second recv(b, m1).buffered", buffered);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_seeded_reordering, |ctx| {
    let names = ["a", "b", "c"];
    // Twelve broadcasts, each preceded by the sender catching up on a seeded prefix of
    // what has been said so far. A prefix of the creation order is causally closed, so
    // every clock in the history is one a real run could have produced.
    let mut plan: Vec<(usize, usize)> = Vec::new();
    for step in 0..12 {
        plan.push((
            ctx.rng.random_range(0..names.len()),
            ctx.rng.random_range(0..=step),
        ));
    }
    let mut order: Vec<usize> = (0..plan.len()).collect();
    order.shuffle(&mut ctx.rng);
    let seed = ctx.seed;

    let p = ctx.prim("causal-broadcast").await?;
    p.send("init a,b,c,d").await?;
    let mut history: Vec<Message> = Vec::new();
    let mut caught_up = vec![0usize; names.len()];
    for (step, (who, prefix)) in plan.into_iter().enumerate() {
        while caught_up[who] < prefix.min(history.len()) {
            let m = history[caught_up[who]].clone();
            caught_up[who] += 1;
            if m.from != names[who] {
                deliver_to(p, names[who], &m).await?;
            }
        }
        history.push(bcast(p, names[who], &format!("m{step}")).await?);
    }
    let by_name: BTreeMap<String, Message> = history
        .iter()
        .map(|m| (m.name.clone(), m.clone()))
        .collect();

    // `d` has said nothing and heard nothing; it now hears the whole history shuffled.
    let mut buffered_at_the_end = 0;
    for index in order {
        let (_, buffered) = deliver_to(p, "d", &history[index]).await?;
        buffered_at_the_end = buffered;
    }
    let log = delivered(p, "d").await?;
    let mut c = Check::new("a causal history delivered in a seeded order");
    c.note(format!("seed {seed}"));
    check_causal_order(&mut c, "delivered(d).messages", &log, &by_name);
    c.eq("delivered(d).messages.len()", history.len(), log.len());
    c.eq(
        "the buffer once everything has arrived",
        0,
        buffered_at_the_end,
    );
    c.observe("delivered(d).messages", log.clone());
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_broadcast_stamps_one_entry, |ctx| {
    let p = ctx.prim("causal-broadcast").await?;
    p.send("init a,b").await?;
    let m1 = bcast(p, "a", "m1").await?;
    let m2 = bcast(p, "a", "m2").await?;
    let log = delivered(p, "a").await?;
    let mut c = Check::new("what a broadcast does to the sender's own clock");
    c.eq("bcast(a, m1).vc.a", Some(1), m1.vc.get("a").copied());
    c.eq("bcast(a, m1).vc.b", None, m1.vc.get("b").copied());
    c.eq("bcast(a, m2).vc.a", Some(2), m2.vc.get("a").copied());
    // A sender has seen its own messages, so they are in its delivered order from the off.
    c.eq(
        "delivered(a).messages",
        vec!["m1".to_string(), "m2".to_string()],
        log,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(concurrent_messages_need_not_wait, |ctx| {
    let p = ctx.prim("causal-broadcast").await?;
    p.send("init a,b,c").await?;
    // Neither sender has heard the other, so neither message depends on anything.
    let m1 = bcast(p, "a", "m1").await?;
    let m2 = bcast(p, "b", "m2").await?;
    let (first, buffered_first) = deliver_to(p, "c", &m2).await?;
    let (second, buffered_second) = deliver_to(p, "c", &m1).await?;
    let log = delivered(p, "c").await?;
    let history: BTreeMap<String, Message> = [&m1, &m2]
        .into_iter()
        .map(|m| (m.name.clone(), m.clone()))
        .collect();
    let mut c = Check::new("two messages neither of which depends on the other");
    c.eq("recv(c, m2).delivered", vec!["m2".to_string()], first);
    c.eq("recv(c, m2).buffered", 0, buffered_first);
    c.eq("recv(c, m1).delivered", vec!["m1".to_string()], second);
    c.eq("recv(c, m1).buffered", 0, buffered_second);
    check_causal_order(&mut c, "delivered(c).messages", &log, &history);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_chain_in_order_never_buffers, |ctx| {
    let p = ctx.prim("causal-broadcast").await?;
    p.send("init a,b,c,d").await?;
    let senders = ["a", "b", "c"];
    let mut history: Vec<Message> = Vec::new();
    let mut buffers = Vec::new();
    for step in 0..9 {
        let who = senders[step % senders.len()];
        // Every sender catches up before it speaks, so each message depends on all of the
        // ones before it: one chain, nine links long.
        for m in &history {
            if m.from != who {
                deliver_to(p, who, m).await?;
            }
        }
        history.push(bcast(p, who, &format!("m{step}")).await?);
    }
    for m in &history {
        let (_, buffered) = deliver_to(p, "d", m).await?;
        buffers.push(buffered);
    }
    let log = delivered(p, "d").await?;
    let expected: Vec<String> = history.iter().map(|m| m.name.clone()).collect();
    let mut c = Check::new("a chain delivered in the order it was created");
    c.eq("delivered(d).messages", expected, log);
    c.that(
        "recv(d, ..).buffered",
        "a buffer that never holds anything, because nothing ever arrives early",
        buffers.iter().all(|b| *b == 0),
        buffers.clone(),
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});
