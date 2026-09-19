//! Stage 80 — ABD: a linearizable register from quorums alone.
//!
//! Consensus is expensive and, for a single register, unnecessary. Attiya, Bar-Noy and
//! Dolev showed in 1990 that read/write registers can be made linearizable with nothing but
//! majority quorums and a timestamp — no leader, no log, no term, and no agreement
//! protocol anywhere. Every write is two phases: ask a quorum for the highest timestamp,
//! then put (highest + 1, value) on a quorum. Every read is two phases too, and the second
//! one is the part everybody leaves out.
//!
//! **A read must write back what it found.** Without that, a write that reached only part
//! of a quorum before its client died leaves the register in a state where one read returns
//! the new value and the next returns the old one — two reads, no writes in between, and
//! the value went backwards. Writing back makes the first read's answer durable on a
//! quorum, so the second read cannot miss it.
//!
//! That is the whole stage: the cheapest correct register there is, and the one line of it
//! that is easy to skip.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;

/// Stage 80.
pub fn stage() -> Stage {
    Stage {
        number: 80,
        slug: "abd_register",
        name: "ABD: a linearizable register without consensus",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `abd`: `write <writer> <value>` and `read` are both two-phase; `poke` puts a \
             tagged value on one replica, which is how a half-finished write is arranged",
            "Values are tagged (timestamp, writer) and ordered by that pair, so two writers at \
             the same timestamp still have a deterministic winner",
            "A write reads a quorum for the highest timestamp, then writes timestamp + 1 to a \
             quorum; a replica never accepts a tag lower than the one it already holds",
            "A read must write back what it found before returning it — that second phase is \
             what makes a later read unable to see anything older",
        ],
        examples,
        tests: vec![
            Test::new("a fresh register reads zero", a_fresh_register),
            Test::new("a write is visible to the next read", write_then_read),
            Test::new("timestamps go up, one per write", timestamps_increase),
            Test::new("a minority of failures changes nothing", minority_failure),
            Test::new("without a quorum, nothing is served", no_quorum_no_answer),
            Test::new("a read writes back what it found", a_read_writes_back),
            Test::new(
                "which is what stops a later read going backwards",
                writeback_prevents_going_backwards,
            ),
            Test::new(
                "and without it, two reads disagree with no write between them",
                without_writeback_it_breaks,
            ),
            Test::new(
                "a higher tag is never overwritten by a lower one",
                tags_only_move_forward,
            )
            .ext(),
            Test::new(
                "thirty seeded writes and reads never go backwards",
                a_long_seeded_run,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("The half-finished write, repaired by a read", "abd", || {
            lines(&["init 3", "poke 0 5 1 42", "read", "state"])
        })
        .request("Replica 0 alone holds (ts 5, 42) — a writer died mid-write — then someone reads")
        .response("the read returns 42 and `wrote_back` is true; two replicas now hold ts 5")
        .note(
            "The read did not just observe the register, it repaired it. Before the read, 42 \
             existed on one replica out of three and could still have been lost; afterwards \
             it is on a majority and cannot be. This is why the second phase is not an \
             optimisation.",
        ),
        prim_example(
            "The same story with the second phase removed",
            "abd",
            || {
                lines(&[
                    "init 3",
                    "writeback off",
                    "poke 0 5 1 42",
                    "read",
                    "down 0",
                    "read",
                ])
            },
        )
        .request("Write-back disabled, then the replica holding the value goes away")
        .response("the first read returns 42 and the second returns 0")
        .note(
            "Two reads, no write between them, and the value went backwards. Nothing failed \
             that a majority-based system is not allowed to survive — one replica of three. \
             The history is simply not linearizable, and the missing write-back is the whole \
             reason.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(a_fresh_register, |ctx| {
    let p = ctx.prim("abd").await?;
    let init = p.send("init 3").await?;
    let read = p.send("read").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a register nobody has written");
    c.eq("init.quorum", 2, p.expect_i64(&init, "init", "quorum")?);
    c.eq("read.served", true, p.expect_bool(&read, "read", "served")?);
    c.eq("read.value", 0, p.expect_i64(&read, "read", "value")?);
    c.eq("read.ts", 0, p.expect_i64(&read, "read", "ts")?);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(write_then_read, |ctx| {
    let p = ctx.prim("abd").await?;
    p.send("init 3").await?;
    let wrote = p.send("write 1 42").await?;
    let read = p.send("read").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a write and the read after it");
    c.eq(
        "write.committed",
        true,
        p.expect_bool(&wrote, "write", "committed")?,
    );
    c.eq("read.value", 42, p.expect_i64(&read, "read", "value")?);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(timestamps_increase, |ctx| {
    let p = ctx.prim("abd").await?;
    p.send("init 5").await?;
    let mut seen = Vec::new();
    for v in 1..=4 {
        let wrote = p.send(&format!("write 1 {}", v * 10)).await?;
        seen.push(p.expect_i64(&wrote, "write", "ts")?);
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("four writes in a row");
    c.note(
        "The first phase of a write exists to learn the highest timestamp in use. A writer \
         that keeps its own counter instead works perfectly until a second writer appears.",
    );
    c.eq("the timestamps written", vec![1, 2, 3, 4], seen);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(minority_failure, |ctx| {
    let p = ctx.prim("abd").await?;
    p.send("init 5").await?;
    p.send("write 1 42").await?;
    p.send("down 0").await?;
    p.send("down 1").await?;
    let read = p.send("read").await?;
    let wrote = p.send("write 2 43").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("two replicas of five gone");
    c.note(
        "Three of five is still a quorum, so the register carries on. No leader has to be \
         elected and no reconfiguration happens — there is nothing to reconfigure.",
    );
    c.eq(
        "the read is served",
        true,
        p.expect_bool(&read, "read", "served")?,
    );
    c.eq(
        "and the write commits",
        true,
        p.expect_bool(&wrote, "write", "committed")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(no_quorum_no_answer, |ctx| {
    let p = ctx.prim("abd").await?;
    p.send("init 3").await?;
    p.send("write 1 42").await?;
    p.send("down 0").await?;
    p.send("down 1").await?;
    let read = p.send("read").await?;
    let wrote = p.send("write 2 43").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a minority left standing");
    c.note(
        "One replica of three cannot know whether it is the survivor or the stranded one. \
         Answering from it would be a guess, so neither operation is served — availability \
         given up for consistency, exactly where the theorem says the choice is.",
    );
    c.eq(
        "the read is refused",
        false,
        p.expect_bool(&read, "read", "served")?,
    );
    c.eq(
        "the write is refused",
        false,
        p.expect_bool(&wrote, "write", "committed")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_read_writes_back, |ctx| {
    let p = ctx.prim("abd").await?;
    p.send("init 3").await?;
    // One replica alone holds a tagged value: a writer that died between its phases.
    p.send("poke 0 5 1 42").await?;
    let read = p.send("read").await?;
    let state = p.send("state").await?;
    let transcript = p.transcript_block();
    let holding: usize = state["replicas"]
        .as_array()
        .map(|a| a.iter().filter(|r| r["ts"].as_i64() == Some(5)).count())
        .unwrap_or(0);
    let mut c = Check::new("a read over a half-finished write");
    c.note(
        "Before the read, 42 is on one replica of three and could still vanish. The read \
         finds it, returns it — and puts it on a quorum on the way out.",
    );
    c.eq("read.value", 42, p.expect_i64(&read, "read", "value")?);
    c.eq(
        "read.wrote_back",
        true,
        p.expect_bool(&read, "read", "wrote_back")?,
    );
    c.at_least("replicas now holding ts 5", 2, holding);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(writeback_prevents_going_backwards, |ctx| {
    let p = ctx.prim("abd").await?;
    p.send("init 3").await?;
    p.send("poke 0 5 1 42").await?;
    let first = p.send("read").await?;
    // The replica that originally held it now fails. The write-back is what saves the value.
    p.send("down 0").await?;
    let second = p.send("read").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("two reads with a failure between them");
    c.note(
        "The only replica that held 42 at the start is gone by the second read. If the first \
         read had not written back, there would be nothing left to find.",
    );
    c.eq("the first read", 42, p.expect_i64(&first, "read", "value")?);
    c.eq(
        "the second read",
        42,
        p.expect_i64(&second, "read", "value")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(without_writeback_it_breaks, |ctx| {
    let p = ctx.prim("abd").await?;
    p.send("init 3").await?;
    p.send("writeback off").await?;
    p.send("poke 0 5 1 42").await?;
    let first = p.send("read").await?;
    p.send("down 0").await?;
    let second = p.send("read").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("the same two reads without the second phase");
    c.note(
        "This history is the reason the second phase exists. Two reads, no write between \
         them, and the register went from 42 to 0 — no total order of these operations reads \
         like a register, which is precisely the definition stage 78 works with.",
    );
    c.eq("the first read", 42, p.expect_i64(&first, "read", "value")?);
    c.eq(
        "the second read",
        0,
        p.expect_i64(&second, "read", "value")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(tags_only_move_forward, |ctx| {
    let p = ctx.prim("abd").await?;
    p.send("init 3").await?;
    p.send("poke 0 9 1 99").await?;
    p.send("poke 1 9 1 99").await?;
    // A writer that learned an old timestamp tries to write under it.
    p.send("poke 2 1 2 11").await?;
    let read = p.send("read").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a stale tag meeting a fresher one");
    c.note(
        "Replicas keep the highest tag they have seen and ignore anything below it. Without \
         that rule a slow message could undo a newer write simply by arriving late.",
    );
    c.eq(
        "the read finds the higher tag",
        99,
        p.expect_i64(&read, "read", "value")?,
    );
    c.eq("at its timestamp", 9, p.expect_i64(&read, "read", "ts")?);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_long_seeded_run, |ctx| {
    enum Step {
        Write(i64, i64),
        Read,
        Down(usize),
        Up(usize),
    }
    let mut plan = Vec::new();
    for _ in 0..30 {
        plan.push(match ctx.rng.random_range(0..4) {
            0 => Step::Write(ctx.rng.random_range(1..=2), ctx.rng.random_range(1..100)),
            1 | 2 => Step::Read,
            _ => {
                let i = ctx.rng.random_range(0..5usize);
                if ctx.rng.random_bool(0.5) {
                    Step::Down(i)
                } else {
                    Step::Up(i)
                }
            }
        });
    }
    let mut backwards: Vec<String> = Vec::new();
    {
        let p = ctx.prim("abd").await?;
        p.send("init 5").await?;
        // Once a value has been returned by a read or acknowledged by a write, no later read
        // may return an older timestamp: that is linearizability for a register, checked
        // without building the whole history.
        let mut floor = 0i64;
        for step in plan {
            match step {
                Step::Write(w, v) => {
                    let r = p.send(&format!("write {w} {v}")).await?;
                    if p.expect_bool(&r, "write", "committed")? {
                        floor = floor.max(p.expect_i64(&r, "write", "ts")?);
                    }
                }
                Step::Read => {
                    let r = p.send("read").await?;
                    if p.expect_bool(&r, "read", "served")? {
                        let ts = p.expect_i64(&r, "read", "ts")?;
                        if ts < floor {
                            backwards.push(format!("a read returned ts {ts} after ts {floor}"));
                        }
                        floor = floor.max(ts);
                    }
                }
                Step::Down(i) => {
                    p.send(&format!("down {i}")).await?;
                }
                Step::Up(i) => {
                    p.send(&format!("up {i}")).await?;
                }
            }
        }
    }
    let mut c = Check::new("thirty seeded operations with replicas coming and going");
    c.note(
        "Writes, reads and failures in a seeded order over five replicas. Nothing here \
         requires a quorum to be available at every moment — only that when an answer is \
         given, it never goes back on one already given.",
    );
    c.eq("reads that went backwards", Vec::<String>::new(), backwards);
    c.finish()
});
