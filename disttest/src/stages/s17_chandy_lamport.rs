//! Stage 17 — Chandy-Lamport snapshots.
//!
//! A distributed snapshot has to be taken without stopping the system, which means no
//! process ever sees the global state it is helping to record. What the algorithm promises
//! instead is that the pieces add up: the balances the processes recorded, plus the
//! messages the channels recorded, equal the money the system holds. That sum is the only
//! thing worth asserting, and it is what nearly every test here checks.
//!
//! The oracle is arithmetic, not a second implementation. Money is conserved by
//! construction — a `send` takes it out of one process and a `deliver` puts it into
//! another — so the system total is known from the initial balances alone, whatever order
//! the harness drives the interleaving in. A snapshot that counts a message twice, or
//! loses one between the sender's balance and the channel, misses that total.

use crate::assert::{Check, Failure};
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::prim::PrimProc;
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;
use serde_json::Value;
use std::collections::BTreeMap;

/// Stage 17.
pub fn stage() -> Stage {
    Stage {
        number: 17,
        slug: "chandy_lamport",
        name: "Chandy-Lamport snapshots",
        ext: true,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `snapshot`: a process records its own state when it first sees a marker",
            "After that it records every message arriving on a channel until that channel's marker",
            "The snapshot's total must equal the system's total, whatever the interleaving",
            "A process sends a marker on every outgoing channel immediately after recording",
        ],
        examples,
        tests: vec![
            Test::new(
                "a snapshot of a quiet system is the state the system is in",
                a_quiet_system,
            ),
            Test::new(
                "a snapshot taken while money is in flight still totals correctly",
                money_in_flight_still_totals,
            ),
            Test::new(
                "a process records its own state the first time it sees a marker and not again",
                a_process_records_itself_once,
            ),
            Test::new(
                "a message arriving after its channel's marker is not recorded",
                after_the_marker_is_not_recorded,
            ),
            Test::new(
                "a snapshot is complete only once every process and channel has reported",
                complete_needs_everyone,
            ),
            Test::new(
                "a seeded interleaving snapshots the same total every time",
                seeded_interleavings,
            ),
            Test::new("two snapshots in a row both total correctly", two_snapshots),
            Test::new(
                "a marker started anywhere goes round a one-way ring",
                a_one_way_ring,
            )
            .ext(),
            Test::new(
                "the reported total is the recorded balances plus the recorded channels",
                the_total_is_the_sum_of_its_parts,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("A snapshot with money in the channel", "snapshot", || {
            lines(&[
                "init p1 100",
                "init p2 50",
                "channel p1 p2",
                "channel p2 p1",
                "send p1 p2 30",
                "snapshot p2",
                "deliver p2 p1",
                "deliver p1 p2",
                "deliver p1 p2",
                "result",
            ])
        })
        .request(
            "`p1` sends 30, then `p2` starts the snapshot while that 30 is still in the channel",
        )
        .response(
            "balances 70 and 50, the channel `p1->p2` holding the 30, and a total of 150 — \
             the money the system started with",
        )
        .note(
            "The 30 is in neither balance at the instant of the snapshot: it has left `p1` \
             and not reached `p2`. A snapshot that records only balances loses it, which is \
             exactly why the algorithm records channels as well.",
        ),
        prim_example("What the marker separates", "snapshot", || {
            lines(&[
                "init p1 100",
                "init p2 50",
                "channel p1 p2",
                "channel p2 p1",
                "snapshot p1",
                "deliver p1 p2",
                "send p1 p2 25",
                "deliver p1 p2",
                "deliver p2 p1",
                "result",
            ])
        })
        .request("a marker delivered to `p2`, and only then a payment down the same channel")
        .response("a complete snapshot whose channels are both empty, still totalling 150")
        .note(
            "The 25 was sent after the marker, so it belongs to the future of the snapshot, \
             not to its past. Recording it would count it twice: once in the balance `p1` \
             recorded before it left, and once in the channel.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Reading what `result` answers
// ---------------------------------------------------------------------------------------

/// What one `result` command answered with.
struct Recorded {
    /// Whether every process and channel has reported.
    complete: bool,
    /// The total the program claims the snapshot holds.
    total: i64,
    /// Recorded balance per process.
    balances: BTreeMap<String, i64>,
    /// Recorded contents per channel, keyed however the program spells a channel.
    channels: BTreeMap<String, i64>,
}

impl Recorded {
    /// What one channel recorded, whichever way its two ends are joined in the key.
    fn channel(&self, from: &str, to: &str) -> Option<i64> {
        self.channels
            .iter()
            .find(|(key, _)| key.starts_with(from) && key.ends_with(to))
            .map(|(_, amount)| *amount)
    }

    /// The recorded balances plus the recorded channels: what `total` has to be.
    fn parts(&self) -> i64 {
        self.balances.values().sum::<i64>() + self.channels.values().sum::<i64>()
    }
}

/// An amount, whether it arrived as a number, as a JSON string, or as the list of messages
/// a channel held.
fn amount(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        Value::Array(items) => items.iter().map(amount).sum::<Option<i64>>(),
        _ => None,
    }
}

/// Read a name-to-amount object out of an answer.
fn amounts(
    p: &PrimProc,
    v: &Value,
    command: &str,
    field: &str,
) -> Result<BTreeMap<String, i64>, Failure> {
    let Some(value) = v.get(field) else {
        return Err(p.shape(command, &format!("the answer has no {field:?} field")));
    };
    let Some(map) = value.as_object() else {
        return Err(p.shape(
            command,
            &format!("{field} should be an object, got {value}"),
        ));
    };
    let mut out = BTreeMap::new();
    for (name, entry) in map {
        match amount(entry) {
            Some(n) => {
                out.insert(name.clone(), n);
            }
            None => {
                return Err(p.shape(
                    command,
                    &format!("{field}.{name} is not an amount: {entry}"),
                ))
            }
        }
    }
    Ok(out)
}

/// Ask for the snapshot.
async fn recorded(p: &mut PrimProc) -> Result<Recorded, Failure> {
    let command = "result";
    let v = p.send(command).await?;
    let complete = p.expect_bool(&v, command, "complete")?;
    let total = p.expect_i64(&v, command, "total")?;
    let balances = amounts(p, &v, command, "balances")?;
    let channels = amounts(p, &v, command, "channels")?;
    Ok(Recorded {
        complete,
        total,
        balances,
        channels,
    })
}

// ---------------------------------------------------------------------------------------
// Driving the system
// ---------------------------------------------------------------------------------------

/// Both directions of every pair of names: the channels of a fully connected system.
fn mesh(names: &[&'static str]) -> Vec<(&'static str, &'static str)> {
    let mut out = Vec::new();
    for from in names {
        for to in names {
            if from != to {
                out.push((*from, *to));
            }
        }
    }
    out
}

/// Start the processes and declare the channels.
async fn wire(
    p: &mut PrimProc,
    processes: &[(&str, i64)],
    channels: &[(&str, &str)],
) -> Result<(), Failure> {
    for (name, balance) in processes {
        p.send(&format!("init {name} {balance}")).await?;
    }
    for (from, to) in channels {
        p.send(&format!("channel {from} {to}")).await?;
    }
    Ok(())
}

/// Deliver everything every channel holds until the snapshot says it is complete.
///
/// A `deliver` on a channel that happens to be empty is answered with an error object and
/// ignored here: the test cannot see how many markers are in flight, so it simply keeps
/// draining until nothing moves. The answer to a delivery that did happen carries a
/// `balance`, which is how a round knows it made progress.
async fn drain(p: &mut PrimProc, channels: &[(&str, &str)]) -> Result<bool, Failure> {
    for _ in 0..channels.len() + 8 {
        let mut moved = false;
        for (from, to) in channels {
            let v = p.send(&format!("deliver {from} {to}")).await?;
            moved |= v.get("balance").is_some();
        }
        let v = p.send("result").await?;
        if v.get("complete").and_then(Value::as_bool).unwrap_or(false) {
            return Ok(true);
        }
        if !moved {
            break;
        }
    }
    Ok(false)
}

/// The check every test in this stage makes: complete, and adding up to the money the
/// system was started with.
fn check_conserved(c: &mut Check, label: &str, total: i64, complete: bool, r: &Recorded) {
    c.that(
        &format!("{label}.complete"),
        "a snapshot every process and every channel has reported",
        complete && r.complete,
        r.complete,
    );
    c.eq(&format!("{label}.total"), total, r.total);
    c.eq(
        &format!("{label}.balances + {label}.channels"),
        r.total,
        r.parts(),
    );
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(a_quiet_system, |ctx| {
    let channels = mesh(&["p1", "p2"]);
    let p = ctx.prim("snapshot").await?;
    wire(p, &[("p1", 100), ("p2", 50)], &channels).await?;
    p.send("snapshot p1").await?;
    let complete = drain(p, &channels).await?;
    let r = recorded(p).await?;
    let mut c = Check::new("a snapshot of a system with nothing in flight");
    check_conserved(&mut c, "result", 150, complete, &r);
    c.eq(
        "result.balances.p1",
        Some(100),
        r.balances.get("p1").copied(),
    );
    c.eq(
        "result.balances.p2",
        Some(50),
        r.balances.get("p2").copied(),
    );
    c.eq("result.channels[p1->p2]", Some(0), r.channel("p1", "p2"));
    c.eq("result.channels[p2->p1]", Some(0), r.channel("p2", "p1"));
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(money_in_flight_still_totals, |ctx| {
    let channels = mesh(&["p1", "p2"]);
    let p = ctx.prim("snapshot").await?;
    wire(p, &[("p1", 100), ("p2", 50)], &channels).await?;
    // The 30 leaves `p1` and is still in the channel when `p2` starts the snapshot, so it
    // is in neither balance and has to be recorded by the channel instead.
    p.send("send p1 p2 30").await?;
    p.send("snapshot p2").await?;
    let complete = drain(p, &channels).await?;
    let r = recorded(p).await?;
    let mut c = Check::new("a snapshot taken with a message in the channel");
    check_conserved(&mut c, "result", 150, complete, &r);
    c.eq(
        "result.balances.p1",
        Some(70),
        r.balances.get("p1").copied(),
    );
    c.eq(
        "result.balances.p2",
        Some(50),
        r.balances.get("p2").copied(),
    );
    c.eq("result.channels[p1->p2]", Some(30), r.channel("p1", "p2"));
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_process_records_itself_once, |ctx| {
    let names = ["p1", "p2", "p3"];
    let channels = mesh(&names);
    let p = ctx.prim("snapshot").await?;
    wire(p, &[("p1", 100), ("p2", 50), ("p3", 50)], &channels).await?;
    p.send("send p3 p2 10").await?;
    p.send("snapshot p1").await?;
    // `p2` records 50 when the marker from `p1` arrives; the 10 from `p3` lands after
    // that, and the second marker must not make `p2` record its state again.
    p.send("deliver p1 p2").await?;
    p.send("deliver p3 p2").await?;
    p.send("deliver p1 p3").await?;
    p.send("deliver p3 p2").await?;
    let complete = drain(p, &channels).await?;
    let r = recorded(p).await?;
    let mut c = Check::new("a process that sees two markers");
    check_conserved(&mut c, "result", 200, complete, &r);
    c.eq(
        "result.balances.p2",
        Some(50),
        r.balances.get("p2").copied(),
    );
    c.eq("result.channels[p3->p2]", Some(10), r.channel("p3", "p2"));
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(after_the_marker_is_not_recorded, |ctx| {
    let channels = mesh(&["p1", "p2"]);
    let p = ctx.prim("snapshot").await?;
    wire(p, &[("p1", 100), ("p2", 50)], &channels).await?;
    p.send("snapshot p1").await?;
    p.send("deliver p1 p2").await?;
    // Sent after `p2` saw the marker on this channel: it belongs to the snapshot's future.
    p.send("send p1 p2 25").await?;
    p.send("deliver p1 p2").await?;
    let complete = drain(p, &channels).await?;
    let r = recorded(p).await?;
    let mut c = Check::new("a payment sent after the marker on the same channel");
    check_conserved(&mut c, "result", 150, complete, &r);
    c.eq("result.channels[p1->p2]", Some(0), r.channel("p1", "p2"));
    c.eq(
        "result.balances.p1",
        Some(100),
        r.balances.get("p1").copied(),
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(complete_needs_everyone, |ctx| {
    let channels = mesh(&["p1", "p2"]);
    let p = ctx.prim("snapshot").await?;
    wire(p, &[("p1", 100), ("p2", 50)], &channels).await?;
    p.send("snapshot p1").await?;
    let started = recorded(p).await?;
    p.send("deliver p1 p2").await?;
    // Both processes have recorded now, but the channel back to `p1` has not seen its
    // marker, so nobody yet knows what was in it.
    let halfway = recorded(p).await?;
    p.send("deliver p2 p1").await?;
    let done = recorded(p).await?;
    let mut c = Check::new("when a snapshot becomes complete");
    c.eq(
        "result.complete just after snapshot p1",
        false,
        started.complete,
    );
    c.eq(
        "result.complete with one channel still open",
        false,
        halfway.complete,
    );
    c.eq(
        "result.complete once both markers have landed",
        true,
        done.complete,
    );
    c.eq("result.total", 150, done.total);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(seeded_interleavings, |ctx| {
    let names = ["p1", "p2", "p3"];
    let channels = mesh(&names);
    let processes = [("p1", 100), ("p2", 100), ("p3", 100)];
    let mut c = Check::new("a snapshot taken part way through a seeded interleaving");
    for trial in 0..4 {
        // Every choice comes from the seed, and every payment is a single unit, so no
        // process can ever be overdrawn however the twelve steps fall out.
        let starter = names[ctx.rng.random_range(0..names.len())];
        let snapshot_at = ctx.rng.random_range(0..12);
        let mut plan = Vec::new();
        for step in 0..12 {
            if step == snapshot_at {
                plan.push(format!("snapshot {starter}"));
            }
            let (from, to) = channels[ctx.rng.random_range(0..channels.len())];
            if ctx.rng.random_bool(0.5) {
                plan.push(format!("send {from} {to} 1"));
            } else {
                plan.push(format!("deliver {from} {to}"));
            }
        }
        let mut p = ctx.prim_fresh("snapshot").await?;
        wire(&mut p, &processes, &channels).await?;
        for command in &plan {
            p.send(command).await?;
        }
        let complete = drain(&mut p, &channels).await?;
        let r = recorded(&mut p).await?;
        let before = c.failed();
        check_conserved(&mut c, &format!("trial[{trial}].result"), 300, complete, &r);
        if c.failed() > before {
            c.note(format!("trial {trial} started its snapshot at {starter}"));
            c.block(format!("trial {trial} transcript"), p.transcript_block());
        }
        p.close().await;
    }
    c.finish()
});

dist_test!(two_snapshots, |ctx| {
    let channels = mesh(&["p1", "p2"]);
    let p = ctx.prim("snapshot").await?;
    wire(p, &[("p1", 100), ("p2", 50)], &channels).await?;
    p.send("snapshot p1").await?;
    let first_complete = drain(p, &channels).await?;
    let first = recorded(p).await?;
    // The system keeps running between the two snapshots, and the second one has to start
    // from nothing the first one left behind.
    p.send("send p1 p2 40").await?;
    p.send("snapshot p2").await?;
    let second_complete = drain(p, &channels).await?;
    let second = recorded(p).await?;
    let mut c = Check::new("two snapshots of the same system");
    check_conserved(&mut c, "the first result", 150, first_complete, &first);
    check_conserved(&mut c, "the second result", 150, second_complete, &second);
    c.eq(
        "the second result.channels[p1->p2]",
        Some(40),
        second.channel("p1", "p2"),
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_one_way_ring, |ctx| {
    // Nothing says the channels have to come in pairs. In a ring the marker travels the
    // long way round, and every process records when it arrives.
    let channels = [("p1", "p2"), ("p2", "p3"), ("p3", "p1")];
    let p = ctx.prim("snapshot").await?;
    wire(p, &[("p1", 10), ("p2", 20), ("p3", 30)], &channels).await?;
    p.send("send p1 p2 5").await?;
    p.send("snapshot p2").await?;
    let complete = drain(p, &channels).await?;
    let r = recorded(p).await?;
    let mut c = Check::new("a snapshot started half way round a ring");
    check_conserved(&mut c, "result", 60, complete, &r);
    c.eq("result.balances.p1", Some(5), r.balances.get("p1").copied());
    c.eq(
        "result.balances.p2",
        Some(20),
        r.balances.get("p2").copied(),
    );
    c.eq(
        "result.balances.p3",
        Some(30),
        r.balances.get("p3").copied(),
    );
    c.eq("result.channels[p1->p2]", Some(5), r.channel("p1", "p2"));
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_total_is_the_sum_of_its_parts, |ctx| {
    let names = ["p1", "p2", "p3"];
    let channels = mesh(&names);
    let p = ctx.prim("snapshot").await?;
    wire(p, &[("p1", 100), ("p2", 100), ("p3", 100)], &channels).await?;
    p.send("send p1 p2 10").await?;
    p.send("send p3 p1 20").await?;
    p.send("snapshot p2").await?;
    let complete = drain(p, &channels).await?;
    let r = recorded(p).await?;
    let mut c = Check::new("the arithmetic inside one answer");
    check_conserved(&mut c, "result", 300, complete, &r);
    c.observe("result.balances", format!("{:?}", r.balances));
    c.observe("result.channels", format!("{:?}", r.channels));
    c.eq("result.balances.len()", 3, r.balances.len());
    c.block("transcript", p.transcript_block());
    c.finish()
});
