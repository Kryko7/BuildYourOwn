//! Stage 04 — Hybrid logical clocks.
//!
//! A Lamport clock orders events but means nothing to a human; a wall clock means something
//! to a human but goes backwards whenever NTP feels like it. An HLC is the smallest thing
//! that does both: a physical part that tracks the wall clock upwards only, and a counter
//! that does the ordering whenever the physical part cannot.
//!
//! The wall clock arrives in the command, which is what makes any of this testable: the
//! tester decides what time it is, and can therefore decide what the answer must be. Every
//! expectation in this stage is the HLC update rule replayed here — `l = max(l, l_msg,
//! wall)`, and a counter chosen from whichever source the maximum came from — and the
//! seeded sequences at the end check the property the rule exists for: `(l, c)` strictly
//! increases, whatever the wall clock does underneath it.

use crate::assert::{Check, Failure};
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::prim::PrimProc;
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;

/// Stage 04.
pub fn stage() -> Stage {
    Stage {
        number: 4,
        slug: "hybrid_logical_clocks",
        name: "Hybrid logical clocks",
        ext: true,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `hlc`: a timestamp is (l, c) — a physical part and a counter that breaks ties",
            "`now` takes l = max(l, wall); when l did not move, c increments, otherwise c resets to 0",
            "`recv` takes l = max(l, l_local, l_msg) and picks c from whichever source it matched",
            "The result must be strictly greater than both the local clock and the message's",
        ],
        examples,
        tests: vec![
            Test::new("a fresh clock takes the wall clock it is handed", a_fresh_clock_takes_the_wall),
            Test::new(
                "the counter counts a stall and resets when the physical part moves",
                the_counter_counts_a_stall,
            ),
            Test::new("a wall clock that goes backwards never drags the clock back", a_backwards_wall_clock),
            Test::new("reading the clock does not move it", a_read_does_not_tick),
            Test::new("a receive adopts a message from the future", a_receive_adopts_the_future),
            Test::new("a receive of a stale message keeps the local physical part", a_receive_of_a_stale_message),
            Test::new("equal physical parts take the larger counter plus one", equal_physical_parts),
            Test::new("each process carries its own pair", processes_are_independent),
            Test::new("a receive dominates both the local clock and the message", a_receive_dominates_both).ext(),
            Test::new("a seeded sequence is strictly increasing in (l, c)", a_seeded_sequence).ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example(
            "A wall clock that stalls, then jumps, then slips",
            "hlc",
            || {
                lines(&[
                    "now a 100",
                    "now a 100",
                    "now a 100",
                    "now a 200",
                    "now a 150",
                ])
            },
        )
        .request("five readings of a clock whose physical part stands still and then goes back")
        .response("(100,0), (100,1), (100,2), (200,0), then (200,1) — never a smaller pair")
        .note(
            "The counter is not a count of events; it is what orders events the physical \
             part cannot separate. It resets the moment `l` moves, which is what keeps it \
             small and bounded rather than growing for the lifetime of the process.",
        ),
        prim_example("A message from a node whose clock is ahead", "hlc", || {
            lines(&["recv b 10 500 3", "get b", "now b 100"])
        })
        .request("a message stamped (500, 3) delivered to a process whose wall clock reads 10")
        .response("(500,4): the message's physical part wins, so the counter comes from it")
        .note(
            "The receive has to beat the message, not merely match it, so the counter that \
             came with the message is incremented. Taking `max(c_local, c_msg)` without the \
             +1 gives a timestamp equal to one already used elsewhere, and two different \
             events then share an instant.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Reading timestamps
// ---------------------------------------------------------------------------------------

/// Send one command and read the `(l, c)` pair it answers with.
async fn hlc(p: &mut PrimProc, command: &str) -> Result<(i64, i64), Failure> {
    let v = p.send(command).await?;
    let l = p.expect_i64(&v, command, "l")?;
    let c = p.expect_i64(&v, command, "c")?;
    Ok((l, c))
}

/// The HLC receive rule, replayed by the tester: `l` is the largest of the three, and the
/// counter comes from whichever source that maximum matched.
fn replay_recv(local: (i64, i64), wall: i64, message: (i64, i64)) -> (i64, i64) {
    let (lo, co) = local;
    let (lm, cm) = message;
    let l = lo.max(lm).max(wall);
    let c = if l == lo && l == lm {
        co.max(cm) + 1
    } else if l == lo {
        co + 1
    } else if l == lm {
        cm + 1
    } else {
        0
    };
    (l, c)
}

/// The HLC local rule: the physical part never falls, and the counter carries a stall.
fn replay_now(local: (i64, i64), wall: i64) -> (i64, i64) {
    let (lo, co) = local;
    let l = lo.max(wall);
    if l == lo {
        (l, co + 1)
    } else {
        (l, 0)
    }
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(a_fresh_clock_takes_the_wall, |ctx| {
    let p = ctx.prim("hlc").await?;
    let before = hlc(p, "get a").await?;
    let first = hlc(p, "now a 100").await?;
    let read_back = hlc(p, "get a").await?;
    let mut c = Check::new("the first reading of a clock that has never been read");
    c.eq("get(a)", (0, 0), before);
    c.eq("now(a, 100)", (100, 0), first);
    c.eq("get(a)", (100, 0), read_back);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_counter_counts_a_stall, |ctx| {
    let p = ctx.prim("hlc").await?;
    let mut stalled = Vec::new();
    for _ in 0..4 {
        stalled.push(hlc(p, "now a 100").await?);
    }
    // One millisecond is enough to reset it: the counter exists only to separate events
    // the physical part cannot, so it is not a count of anything that outlives one tick.
    let moved = hlc(p, "now a 101").await?;
    let stalled_again = hlc(p, "now a 101").await?;
    let jumped = hlc(p, "now a 5000").await?;
    let mut c = Check::new("a physical part that stands still and then moves on");
    c.eq(
        "the answers to now(a, 100)",
        vec![(100, 0), (100, 1), (100, 2), (100, 3)],
        stalled,
    );
    c.eq("now(a, 101)", (101, 0), moved);
    c.eq("the second now(a, 101)", (101, 1), stalled_again);
    c.eq("now(a, 5000)", (5000, 0), jumped);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_backwards_wall_clock, |ctx| {
    let p = ctx.prim("hlc").await?;
    let ahead = hlc(p, "now a 2000").await?;
    // The machine's clock is stepped back half a second, twice.
    let slipped = hlc(p, "now a 1500").await?;
    let slipped_again = hlc(p, "now a 1000").await?;
    let recovered = hlc(p, "now a 2001").await?;
    let mut c = Check::new("a wall clock corrected backwards under a running process");
    c.eq("now(a, 2000)", (2000, 0), ahead);
    c.eq("now(a, 1500)", (2000, 1), slipped);
    c.eq("now(a, 1000)", (2000, 2), slipped_again);
    c.eq("now(a, 2001)", (2001, 0), recovered);
    let order = [ahead, slipped, slipped_again, recovered];
    for (i, w) in order.windows(2).enumerate() {
        c.that(
            &format!("now[{i}] < now[{}]", i + 1),
            "a strictly larger (l, c), whatever the wall clock did",
            w[1] > w[0],
            (w[0], w[1]),
        );
    }
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_read_does_not_tick, |ctx| {
    let p = ctx.prim("hlc").await?;
    hlc(p, "now a 300").await?;
    hlc(p, "now a 300").await?;
    let mut reads = Vec::new();
    for _ in 0..3 {
        reads.push(hlc(p, "get a").await?);
    }
    let after = hlc(p, "now a 300").await?;
    let mut c = Check::new("three reads between two readings");
    c.eq("the answers to get(a)", vec![(300, 1); 3], reads);
    c.eq("now(a, 300)", (300, 2), after);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_receive_adopts_the_future, |ctx| {
    let p = ctx.prim("hlc").await?;
    let received = hlc(p, "recv b 10 500 3").await?;
    let read_back = hlc(p, "get b").await?;
    let later = hlc(p, "now b 100").await?;
    let mut c = Check::new("a message from a node whose clock is far ahead");
    c.eq(
        "recv(b, 10, 500, 3)",
        replay_recv((0, 0), 10, (500, 3)),
        received,
    );
    c.eq("get(b)", (500, 4), read_back);
    // The local wall clock is still behind, so the counter carries on from the message.
    c.eq("now(b, 100)", (500, 5), later);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_receive_of_a_stale_message, |ctx| {
    let p = ctx.prim("hlc").await?;
    let local = hlc(p, "now a 900").await?;
    let received = hlc(p, "recv a 0 100 9").await?;
    let read_back = hlc(p, "get a").await?;
    let mut c = Check::new("a message stamped long before the receiver's own clock");
    c.eq("now(a, 900)", (900, 0), local);
    // The message loses the maximum, so its counter is irrelevant: 9 must not appear.
    c.eq(
        "recv(a, 0, 100, 9)",
        replay_recv((900, 0), 0, (100, 9)),
        received,
    );
    c.eq("get(a)", (900, 1), read_back);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(equal_physical_parts, |ctx| {
    let p = ctx.prim("hlc").await?;
    for _ in 0..3 {
        hlc(p, "now a 300").await?;
    }
    let local = hlc(p, "get a").await?;
    // Same millisecond on both sides, and the message's counter is the larger one.
    let bigger = hlc(p, "recv a 300 300 5").await?;
    // Same again, but now the local counter is the larger one.
    let smaller = hlc(p, "recv a 300 300 1").await?;
    let mut c = Check::new("two clocks inside the same physical millisecond");
    c.eq("get(a)", (300, 2), local);
    c.eq(
        "recv(a, 300, 300, 5)",
        replay_recv((300, 2), 300, (300, 5)),
        bigger,
    );
    c.eq("recv(a, 300, 300, 1)", (300, 7), smaller);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(processes_are_independent, |ctx| {
    let p = ctx.prim("hlc").await?;
    hlc(p, "now a 400").await?;
    hlc(p, "now a 400").await?;
    let b_before = hlc(p, "get b").await?;
    let b_first = hlc(p, "now b 250").await?;
    let a_after = hlc(p, "get a").await?;
    let mut c = Check::new("two processes inside one program");
    c.eq("get(b) while only a has been read", (0, 0), b_before);
    c.eq("now(b, 250)", (250, 0), b_first);
    c.eq("get(a)", (400, 1), a_after);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_receive_dominates_both, |ctx| {
    // Every case of the receive rule, in a seeded order: the message ahead, the message
    // behind, both in the same millisecond, and the local wall clock ahead of both.
    let mut plan: Vec<(i64, i64, i64)> = Vec::new();
    for _ in 0..24 {
        let wall = ctx.rng.random_range(0..400) * 10;
        let lm = ctx.rng.random_range(0..400) * 10;
        let cm = ctx.rng.random_range(0..4);
        plan.push((wall, lm, cm));
    }
    let seed = ctx.seed;
    let p = ctx.prim("hlc").await?;
    let mut local = (0i64, 0i64);
    let mut steps = Vec::new();
    for (wall, lm, cm) in &plan {
        let command = format!("recv a {wall} {lm} {cm}");
        let answer = hlc(p, &command).await?;
        let expected = replay_recv(local, *wall, (*lm, *cm));
        steps.push((command, local, (*lm, *cm), expected, answer));
        local = expected;
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("twenty-four seeded receives replayed against the update rule");
    c.note(format!("seed {seed}"));
    for (command, before, message, expected, answer) in &steps {
        c.eq(command, *expected, *answer);
        if !c.ok() {
            break;
        }
        c.that(
            &format!("{command} > the local clock {before:?}"),
            "a strictly larger (l, c) than the clock it started from",
            answer > before,
            *answer,
        );
        c.that(
            &format!("{command} > the message {message:?}"),
            "a strictly larger (l, c) than the message that was received",
            answer > message,
            *answer,
        );
    }
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_seeded_sequence, |ctx| {
    // A wall clock that mostly creeps forward, sometimes stalls and sometimes slips back,
    // with messages from a peer whose own clock wanders the same way. Everything is worked
    // out here before the program is asked anything.
    let mut wall = 1_000i64;
    let mut peer = (0i64, 0i64);
    let mut plan: Vec<(bool, i64, (i64, i64))> = Vec::new();
    for _ in 0..50 {
        wall += ctx.rng.random_range(-40..60);
        let receive = ctx.rng.random_range(0..3) == 0;
        if receive {
            let peer_wall = wall + ctx.rng.random_range(-200..200);
            peer = replay_now(peer, peer_wall);
        }
        plan.push((receive, wall, peer));
    }
    let seed = ctx.seed;
    let p = ctx.prim("hlc").await?;
    let mut local = (0i64, 0i64);
    let mut steps = Vec::new();
    for (receive, wall, message) in &plan {
        let (command, expected) = if *receive {
            (
                format!("recv a {} {} {}", wall, message.0, message.1),
                replay_recv(local, *wall, *message),
            )
        } else {
            (format!("now a {wall}"), replay_now(local, *wall))
        };
        let answer = hlc(p, &command).await?;
        steps.push((command, expected, answer));
        local = expected;
    }
    let final_read = hlc(p, "get a").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("fifty seeded readings and receives under a wandering wall clock");
    c.note(format!("seed {seed}"));
    for (command, expected, answer) in &steps {
        c.eq(command, *expected, *answer);
        if !c.ok() {
            break;
        }
    }
    let answers: Vec<(i64, i64)> = steps.iter().map(|(_, _, a)| *a).collect();
    for (i, w) in answers.windows(2).enumerate() {
        c.that(
            &format!("step[{i}] < step[{}]", i + 1),
            "a strictly larger (l, c) in lexicographic order",
            w[1] > w[0],
            (w[0], w[1]),
        );
    }
    c.eq("get(a)", local, final_read);
    c.block("transcript", transcript);
    c.finish()
});
