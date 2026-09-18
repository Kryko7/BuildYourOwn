//! Stage 01 — Lamport clocks.
//!
//! The first rung of the whole track, and the smallest possible statement of the idea the
//! rest of it depends on: a counter that only ever moves forward, and a receive rule that
//! makes an effect outrank its cause. One integer per process, three rules, and a `+1`
//! that everyone forgets once.
//!
//! The oracle is the rules themselves. Nothing here asks the program what it thinks the
//! answer is: the tester keeps its own counters, replays `max(local, received) + 1` by
//! hand, and compares. The long randomized exchange at the end of the stage is the same
//! replay over sixty seeded steps, which is where an implementation that increments after
//! stamping a message, or that lets a stale message drag a clock backwards, finally shows.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;

/// Stage 01.
pub fn stage() -> Stage {
    Stage {
        number: 1,
        slug: "lamport_clocks",
        name: "Lamport clocks",
        ext: false,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `lamport`: keep one counter per process and hand it out with `send`",
            "A local event and a send both increment the counter before it is used",
            "On receive the counter becomes max(local, received) + 1 — the +1 is the whole point",
            "Counters never go backwards, even when a message arrives from the past",
        ],
        examples,
        tests: vec![
            Test::new(
                "a fresh process has not ticked yet",
                a_fresh_process_is_at_zero,
            ),
            Test::new(
                "a local event moves the clock on by one",
                a_local_event_ticks,
            ),
            Test::new(
                "a send stamps the counter it has just moved",
                a_send_stamps_the_new_value,
            ),
            Test::new(
                "a receive takes the maximum plus one",
                a_receive_takes_the_maximum_plus_one,
            ),
            Test::new(
                "a receive never moves the clock backwards",
                a_stale_message_never_rewinds,
            ),
            Test::new("reading the clock does not move it", a_read_does_not_tick),
            Test::new(
                "each process keeps a counter of its own",
                processes_are_independent,
            ),
            Test::new(
                "an exchange orders cause before effect",
                an_exchange_orders_cause_before_effect,
            ),
            Test::new(
                "a message from far in the future drags the clock with it",
                a_far_future_message,
            )
            .ext(),
            Test::new(
                "a long seeded exchange matches the tester's own replay",
                a_long_exchange,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("Two processes exchanging one message", "lamport", || {
            lines(&["send a", "recv b 7", "get b", "get a"])
        })
        .request("a send on `a`, a message stamped 7 delivered to `b`, and both clocks read back")
        .response("one JSON object per line; the receive answers max(local, 7) + 1")
        .note(
            "The trap is the +1. `b` had never ticked, so the merge alone would leave it at 7 \
             — the same instant as the send it is supposed to follow. The extra tick is what \
             makes the receive strictly later than the send, and `get` must not add another.",
        ),
        prim_example("A message that arrives from the past", "lamport", || {
            lines(&["local a", "local a", "local a", "recv a 1", "get a"])
        })
        .request("a clock pushed to 3, then a message stamped 1 delivered to it")
        .response("4, not 2: the maximum is the local value, and it still ticks")
        .note(
            "A Lamport clock is monotonic per process, so an old message can never pull one \
             back. Writing `ts + 1` instead of `max(local, ts) + 1` passes every test where \
             messages happen to arrive in order and fails the moment one is late.",
        ),
    ]
}

dist_test!(a_fresh_process_is_at_zero, |ctx| {
    let p = ctx.prim("lamport").await?;
    let a = p.num("get a", "ts").await?;
    let b = p.num("get b", "ts").await?;
    let first = p.num("local a", "ts").await?;
    let mut c = Check::new("the clock of a process that has done nothing");
    c.eq("get(a).ts", 0, a);
    c.eq("get(b).ts", 0, b);
    c.eq("local(a).ts", 1, first);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_local_event_ticks, |ctx| {
    let p = ctx.prim("lamport").await?;
    let mut seen = Vec::new();
    for _ in 0..4 {
        seen.push(p.num("local a", "ts").await?);
    }
    let read_back = p.num("get a", "ts").await?;
    let mut c = Check::new("four local events in a row");
    c.eq("the timestamps of local(a)", vec![1, 2, 3, 4], seen.clone());
    c.eq("get(a).ts", 4, read_back);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_send_stamps_the_new_value, |ctx| {
    let p = ctx.prim("lamport").await?;
    let first = p.num("send a", "ts").await?;
    let after_first = p.num("get a", "ts").await?;
    let second = p.num("send a", "ts").await?;
    let mut c = Check::new("the timestamp a send puts on its message");
    // A send that stamps the old value and increments afterwards answers 0 here, which is
    // the same mistake as stamping a message with the instant before it was sent.
    c.eq("send(a).ts", 1, first);
    c.eq("get(a).ts after the send", 1, after_first);
    c.eq("the second send(a).ts", 2, second);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_receive_takes_the_maximum_plus_one, |ctx| {
    let p = ctx.prim("lamport").await?;
    let sent = p.num("send a", "ts").await?;
    let received = p.num("recv b 40", "ts").await?;
    let read_back = p.num("get b", "ts").await?;
    let mut c = Check::new("a receive of a timestamp from the future");
    c.eq("recv(b, 40).ts", 41, received);
    c.eq("get(b).ts", 41, read_back);
    c.observe("send(a).ts", sent);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_stale_message_never_rewinds, |ctx| {
    let p = ctx.prim("lamport").await?;
    for _ in 0..5 {
        p.num("local a", "ts").await?;
    }
    let stale = p.num("recv a 2", "ts").await?;
    let older_still = p.num("recv a 0", "ts").await?;
    let read_back = p.num("get a", "ts").await?;
    let mut c = Check::new("two messages that were stamped before the receiver's own clock");
    // max(5, 2) + 1, then max(6, 0) + 1: the local value wins the maximum both times, and
    // the tick happens anyway.
    c.eq("recv(a, 2).ts", 6, stale);
    c.eq("recv(a, 0).ts", 7, older_still);
    c.eq("get(a).ts", 7, read_back);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_read_does_not_tick, |ctx| {
    let p = ctx.prim("lamport").await?;
    p.num("local a", "ts").await?;
    p.num("local a", "ts").await?;
    let mut reads = Vec::new();
    for _ in 0..3 {
        reads.push(p.num("get a", "ts").await?);
    }
    let after = p.num("local a", "ts").await?;
    let mut c = Check::new("three reads between two local events");
    c.eq("the timestamps of get(a)", vec![2, 2, 2], reads.clone());
    c.eq("local(a).ts after the reads", 3, after);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(processes_are_independent, |ctx| {
    let p = ctx.prim("lamport").await?;
    for _ in 0..3 {
        p.num("local a", "ts").await?;
    }
    let b_before = p.num("get b", "ts").await?;
    let b_first = p.num("local b", "ts").await?;
    let a_after = p.num("get a", "ts").await?;
    let c_fresh = p.num("get c", "ts").await?;
    let mut c = Check::new("three processes sharing one program but not one counter");
    c.eq("get(b).ts while only a has ticked", 0, b_before);
    c.eq("local(b).ts", 1, b_first);
    c.eq("get(a).ts", 3, a_after);
    c.eq("get(c).ts", 0, c_fresh);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(an_exchange_orders_cause_before_effect, |ctx| {
    let p = ctx.prim("lamport").await?;
    // a does some work, sends to b; b answers; a receives the answer. Every step of that
    // chain happened after the one before it, so every timestamp has to be larger.
    let a_work = p.num("local a", "ts").await?;
    let a_send = p.num("send a", "ts").await?;
    let b_recv = p.num(&format!("recv b {a_send}"), "ts").await?;
    let b_work = p.num("local b", "ts").await?;
    let b_send = p.num("send b", "ts").await?;
    let a_recv = p.num(&format!("recv a {b_send}"), "ts").await?;
    let chain = [a_work, a_send, b_recv, b_work, b_send, a_recv];
    let mut c = Check::new("a request and its answer, timestamped along the way");
    c.eq("local(a).ts", 1, a_work);
    c.eq("send(a).ts", 2, a_send);
    c.eq("recv(b, 2).ts", 3, b_recv);
    c.eq("local(b).ts", 4, b_work);
    c.eq("send(b).ts", 5, b_send);
    c.eq("recv(a, 5).ts", 6, a_recv);
    for (i, w) in chain.windows(2).enumerate() {
        c.that(
            &format!("chain[{i}] < chain[{}]", i + 1),
            "a strictly later timestamp, because each step caused the next",
            w[1] > w[0],
            (w[0], w[1]),
        );
    }
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_far_future_message, |ctx| {
    let p = ctx.prim("lamport").await?;
    p.num("local a", "ts").await?;
    let jumped = p.num("recv a 1000000", "ts").await?;
    let after = p.num("local a", "ts").await?;
    let b_untouched = p.num("get b", "ts").await?;
    let mut c = Check::new("one message stamped a million");
    c.eq("recv(a, 1000000).ts", 1_000_001, jumped);
    c.eq("local(a).ts", 1_000_002, after);
    c.eq("get(b).ts", 0, b_untouched);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_long_exchange, |ctx| {
    // The whole conversation is worked out here first — every command and the timestamp it
    // must answer with — by replaying Lamport's three rules over the seeded plan. The
    // program is then asked the same questions and never consulted about the answers.
    let names = ["a", "b", "c"];
    let mut clocks = [0i64; 3];
    let mut in_flight: Vec<i64> = Vec::new();
    let mut script: Vec<(String, i64)> = Vec::new();
    for _ in 0..60 {
        let who = ctx.rng.random_range(0..names.len());
        let choice = ctx.rng.random_range(0..3);
        if choice == 2 && !in_flight.is_empty() {
            let idx = ctx.rng.random_range(0..in_flight.len());
            let ts = in_flight.remove(idx);
            clocks[who] = clocks[who].max(ts) + 1;
            script.push((format!("recv {} {}", names[who], ts), clocks[who]));
        } else if choice == 1 {
            clocks[who] += 1;
            in_flight.push(clocks[who]);
            script.push((format!("send {}", names[who]), clocks[who]));
        } else {
            clocks[who] += 1;
            script.push((format!("local {}", names[who]), clocks[who]));
        }
    }
    let seed = ctx.seed;
    let p = ctx.prim("lamport").await?;
    let mut answers = Vec::with_capacity(script.len());
    for (command, _) in &script {
        answers.push(p.num(command, "ts").await?);
    }
    let mut finals = Vec::new();
    for name in names {
        finals.push(p.num(&format!("get {name}"), "ts").await?);
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("sixty seeded events replayed against the tester's own counters");
    c.note(format!("seed {seed}, {} events", script.len()));
    for (i, ((command, expected), actual)) in script.iter().zip(&answers).enumerate() {
        c.eq(&format!("step[{i}] {command}"), *expected, *actual);
        if !c.ok() {
            break;
        }
    }
    for (i, name) in names.iter().enumerate() {
        c.eq(&format!("get({name}).ts"), clocks[i], finals[i]);
    }
    c.block("transcript", transcript);
    c.finish()
});
