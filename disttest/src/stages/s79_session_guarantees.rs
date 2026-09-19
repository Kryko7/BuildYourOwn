//! Stage 79 — Session guarantees over lagging replicas.
//!
//! Between "linearizable" and "eventually consistent" there is a set of guarantees that
//! cost almost nothing and remove almost all of the surprise: the four session guarantees
//! from the Bayou work. **Read-your-writes** — you never fail to see your own completed
//! write. **Monotonic reads** — your reads never go backwards in time. **Monotonic
//! writes** — your writes are applied in the order you issued them. **Writes-follow-reads**
//! — a write you make after reading something is ordered after what you read.
//!
//! None of them is a property of the store. They are properties of a *session*: the store
//! remembers what this client has already seen, and refuses to serve it from a replica that
//! is behind that. The system stays eventually consistent and the client stops seeing
//! impossible things, which is why almost every production system that offers "eventual
//! consistency" quietly offers these as well.
//!
//! The stage makes the lag explicit. The tester decides which replica takes each write and
//! when each replica catches up, so the failure mode is not a race to reproduce — it is the
//! normal case, arranged on purpose.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;

/// Stage 79.
pub fn stage() -> Stage {
    Stage {
        number: 79,
        slug: "session_guarantees",
        name: "Session guarantees: read-your-writes and monotonic reads",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `session`: `init <replicas>`, `write <client> <replica> <key> <value>`, \
             `replicate <replica>`, `read <client> <replica> <key>`",
            "Each session remembers the highest version it has written and the highest it has \
             read; a read from a replica below that floor must not be served",
            "`served: false` is the right answer for a read that would go backwards — a real \
             client retries elsewhere or waits, and neither is the store's decision",
            "`guarantees off` turns the session tracking off, which is how the stage shows what \
             the guarantees were buying",
        ],
        examples,
        tests: vec![
            Test::new("a fresh store has replicas at version zero", a_fresh_store),
            Test::new("a write lands on the replica it was sent to", a_write_lands),
            Test::new("and the other replicas do not have it yet", lag_is_real),
            Test::new(
                "read-your-writes: a lagging replica must not serve the writer",
                read_your_writes,
            ),
            Test::new(
                "and serves it once that replica has caught up",
                catching_up_unblocks_the_read,
            ),
            Test::new(
                "monotonic reads: a session's reads never go backwards",
                monotonic_reads,
            ),
            Test::new(
                "another client's session is unaffected by this one",
                sessions_are_per_client,
            ),
            Test::new(
                "with the guarantees off, the same read goes backwards",
                without_guarantees_it_breaks,
            ),
            Test::new(
                "a read of a key nobody has written is zero, not a refusal",
                unknown_key_reads_zero,
            )
            .ext(),
            Test::new(
                "thirty seeded interleavings never serve a read below the session floor",
                a_long_seeded_run,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("Read-your-writes, arranged on purpose", "session", || {
            lines(&[
                "init 2",
                "write c1 0 k 7",
                "read c1 1 k",
                "replicate 1",
                "read c1 1 k",
            ])
        })
        .request("c1 writes to replica 0, then reads from replica 1 before and after it catches up")
        .response("the first read is refused with `served: false`; the second returns 7")
        .note(
            "The replica is not broken and the write is not lost — replica 1 simply has not \
             been told yet. Serving that read would show c1 an older world than the one it \
             just changed, which is the single most confusing thing an eventually consistent \
             store can do to a user.",
        ),
        prim_example("What the guarantee was buying", "session", || {
            lines(&[
                "init 2",
                "guarantees off",
                "write c1 0 k 7",
                "read c1 1 k",
                "state",
            ])
        })
        .request("The same story with session tracking turned off")
        .response("the read is served and returns 0")
        .note(
            "Same store, same lag, same commands: the only change is that nothing is \
             remembering what c1 has already seen. The value 0 is not corruption — it is \
             what replica 1 honestly holds — and that is exactly why the guarantee has to \
             live in the session rather than in the data.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(a_fresh_store, |ctx| {
    let p = ctx.prim("session").await?;
    let init = p.send("init 3").await?;
    let s = p.send("state").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a store nobody has written to");
    c.eq("init.replicas", 3, p.expect_i64(&init, "init", "replicas")?);
    c.eq(
        "every replica is at version 0",
        vec![0, 0, 0],
        s["replicas"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|r| r["version"].as_i64().unwrap_or(-1))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_write_lands, |ctx| {
    let p = ctx.prim("session").await?;
    p.send("init 2").await?;
    let wrote = p.send("write c1 0 k 7").await?;
    let read = p.send("read c1 0 k").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a write and a read on the same replica");
    c.eq(
        "write.version",
        1,
        p.expect_i64(&wrote, "write", "version")?,
    );
    c.eq("read.served", true, p.expect_bool(&read, "read", "served")?);
    c.eq("read.value", 7, p.expect_i64(&read, "read", "value")?);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(lag_is_real, |ctx| {
    let p = ctx.prim("session").await?;
    p.send("init 2").await?;
    p.send("write c1 0 k 7").await?;
    let s = p.send("state").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("the replica that was not written to");
    c.note(
        "Nothing propagates until the test says so. That is the point of the topic: the lag \
         a real system has for milliseconds is held open here for as long as the stage needs.",
    );
    let versions: Vec<i64> = s["replicas"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|r| r["version"].as_i64().unwrap_or(-1))
                .collect()
        })
        .unwrap_or_default();
    c.eq("replica versions", vec![1, 0], versions);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(read_your_writes, |ctx| {
    let p = ctx.prim("session").await?;
    p.send("init 2").await?;
    p.send("write c1 0 k 7").await?;
    let read = p.send("read c1 1 k").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("the writer reading from a lagging replica");
    c.note(
        "c1 has an acknowledged write at version 1 and replica 1 is at version 0. Serving \
         this read would tell c1 its own write never happened.",
    );
    c.eq(
        "read.served",
        false,
        p.expect_bool(&read, "read", "served")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(catching_up_unblocks_the_read, |ctx| {
    let p = ctx.prim("session").await?;
    p.send("init 2").await?;
    p.send("write c1 0 k 7").await?;
    let before = p.send("read c1 1 k").await?;
    p.send("replicate 1").await?;
    let after = p.send("read c1 1 k").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("the same read, after the replica catches up");
    c.note(
        "The guarantee is about staleness relative to the session, not about which replica \
         answers. Once replica 1 has the write, it is a perfectly good place to read from.",
    );
    c.eq(
        "before replication",
        false,
        p.expect_bool(&before, "read", "served")?,
    );
    c.eq(
        "after replication",
        true,
        p.expect_bool(&after, "read", "served")?,
    );
    c.eq(
        "and the value is the one written",
        7,
        p.expect_i64(&after, "read", "value")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(monotonic_reads, |ctx| {
    let p = ctx.prim("session").await?;
    p.send("init 2").await?;
    // c2 does the writing, so c1 has no writes of its own — only reads.
    p.send("write c2 0 k 1").await?;
    p.send("write c2 0 k 2").await?;
    p.send("replicate 1").await?;
    p.send("write c2 0 k 3").await?;
    let fresh = p.send("read c1 0 k").await?;
    let stale = p.send("read c1 1 k").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a reader moving to a staler replica");
    c.note(
        "c1 has written nothing, so read-your-writes has nothing to say here. What stops the \
         second read is monotonic reads: having already seen version 3, this session must \
         not be shown a replica that is still at version 2.",
    );
    c.eq(
        "the first read is served",
        true,
        p.expect_bool(&fresh, "read", "served")?,
    );
    c.eq(
        "it saw the newest value",
        3,
        p.expect_i64(&fresh, "read", "value")?,
    );
    c.eq(
        "the second read is refused",
        false,
        p.expect_bool(&stale, "read", "served")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(sessions_are_per_client, |ctx| {
    let p = ctx.prim("session").await?;
    p.send("init 2").await?;
    p.send("write c1 0 k 7").await?;
    let mine = p.send("read c1 1 k").await?;
    let theirs = p.send("read c2 1 k").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a second client reading the same lagging replica");
    c.note(
        "c2 has seen nothing, so nothing it could be shown would go backwards *for it*. The \
         guarantees are per-session by definition; making them global would be a different, \
         far more expensive promise.",
    );
    c.eq(
        "the writer is refused",
        false,
        p.expect_bool(&mine, "read", "served")?,
    );
    c.eq(
        "the other client is served",
        true,
        p.expect_bool(&theirs, "read", "served")?,
    );
    c.eq(
        "and sees the old value",
        0,
        p.expect_i64(&theirs, "read", "value")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(without_guarantees_it_breaks, |ctx| {
    let p = ctx.prim("session").await?;
    p.send("init 2").await?;
    p.send("guarantees off").await?;
    p.send("write c1 0 k 7").await?;
    let read = p.send("read c1 1 k").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("the same sequence with session tracking off");
    c.note(
        "The contrast is the lesson. Nothing about the replicas changed; only the bookkeeping \
         did, and c1 is now told that the value it just wrote is not there.",
    );
    c.eq(
        "the read is served",
        true,
        p.expect_bool(&read, "read", "served")?,
    );
    c.eq(
        "and returns the stale value",
        0,
        p.expect_i64(&read, "read", "value")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(unknown_key_reads_zero, |ctx| {
    let p = ctx.prim("session").await?;
    p.send("init 1").await?;
    let read = p.send("read c1 0 missing").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a key nobody has written");
    c.note(
        "An empty register reads as 0 and the read is served: there is nothing stale about \
         it. Refusing here would confuse \"I have not caught up\" with \"there is nothing \
         to catch up on\".",
    );
    c.eq("read.served", true, p.expect_bool(&read, "read", "served")?);
    c.eq("read.value", 0, p.expect_i64(&read, "read", "value")?);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_long_seeded_run, |ctx| {
    // The plan is drawn first and the connection opened afterwards, so the randomness and
    // the program do not fight over the context — and so a failure can print the exact
    // sequence that produced it.
    enum Step {
        Write {
            client: String,
            replica: usize,
            value: i64,
        },
        Replicate {
            replica: usize,
        },
        Read {
            client: String,
            replica: usize,
        },
    }
    let mut plan: Vec<Step> = Vec::new();
    for _ in 0..30 {
        let client = format!("c{}", ctx.rng.random_range(1..=3));
        let replica = ctx.rng.random_range(0..3usize);
        plan.push(match ctx.rng.random_range(0..3) {
            0 => Step::Write {
                client,
                replica,
                value: ctx.rng.random_range(1..100),
            },
            1 => Step::Replicate { replica },
            _ => Step::Read { client, replica },
        });
    }

    let mut violations: Vec<String> = Vec::new();
    let mut seen: std::collections::BTreeMap<String, i64> = Default::default();
    {
        let p = ctx.prim("session").await?;
        p.send("init 3").await?;
        for step in &plan {
            match step {
                Step::Write {
                    client,
                    replica,
                    value,
                } => {
                    let v = p
                        .send(&format!("write {client} {replica} k {value}"))
                        .await?;
                    let version = p.expect_i64(&v, "write", "version")?;
                    let e = seen.entry(client.clone()).or_insert(0);
                    *e = (*e).max(version);
                }
                Step::Replicate { replica } => {
                    p.send(&format!("replicate {replica}")).await?;
                }
                Step::Read { client, replica } => {
                    let v = p.send(&format!("read {client} {replica} k")).await?;
                    if p.expect_bool(&v, "read", "served")? {
                        let rv = p.expect_i64(&v, "read", "replica_version")?;
                        let floor = seen.get(client).copied().unwrap_or(0);
                        if rv < floor {
                            violations.push(format!(
                                "{client} was served from a replica at version {rv} after \
                                 having already seen {floor}"
                            ));
                        }
                        let e = seen.entry(client.clone()).or_insert(0);
                        *e = (*e).max(rv);
                    }
                }
            }
        }
    }
    let mut c = Check::new("thirty seeded operations");
    c.note(
        "Writes, catch-ups and reads in a seeded order across three clients and three \
         replicas. The tester tracks what each session has been shown and checks every \
         served read against it; a single violation is printed with the session that \
         suffered it.",
    );
    c.eq(
        "reads that went backwards",
        Vec::<String>::new(),
        violations,
    );
    c.finish()
});
