//! Stage 87 — Bounded clock uncertainty, and the wait that buys external consistency.
//!
//! Stage 04 built a hybrid logical clock: physical time you can still order. This stage is
//! about the other approach, the one Spanner took — keep the physical clock, but make the
//! *uncertainty explicit* and then wait it out.
//!
//! A node cannot read the true time. What it can have is an interval: "now is somewhere in
//! [t − ε, t + ε]". Given that, a transaction takes the **latest** possible time as its
//! commit timestamp, so the timestamp is not in the past for anybody. That alone is not
//! enough: a second transaction starting immediately afterwards could read a clock at the
//! other end of its own interval and pick a smaller timestamp, and an outside observer who
//! saw the first commit before starting the second would see the timestamps disagree with
//! reality.
//!
//! **Commit-wait** closes it. After choosing the timestamp, hold the commit until the
//! *earliest* possible now is past it — wait out 2ε — and only then tell anybody. By the
//! time the world learns a transaction committed, every clock in the system is certainly
//! beyond its timestamp, so anything that starts later gets a larger one. The cost is
//! latency proportional to the clock uncertainty, which is why the interesting engineering
//! is in making ε small rather than in the protocol.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};

/// Stage 87.
pub fn stage() -> Stage {
    Stage {
        number: 87,
        slug: "commit_wait",
        name: "Clock uncertainty and commit-wait",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `commit-wait`: `init <epsilon>` sets the uncertainty, `now` returns the \
             interval, `commit <name>` takes a timestamp, `tick <ms>` moves time on",
            "The timestamp is the *latest* the clock could be, so it is never in the past for \
             any node in the system",
            "Commit-wait holds the commit until the *earliest* possible now is past that \
             timestamp — a wait of 2ε, not ε",
            "`external-order` asks the question that matters: does the timestamp order agree \
             with the order an outside observer saw the commits happen",
        ],
        examples,
        tests: vec![
            Test::new(
                "a clock is an interval, not a number",
                the_clock_is_an_interval,
            ),
            Test::new(
                "a commit timestamp is the latest possible now",
                the_timestamp_is_the_latest,
            ),
            Test::new(
                "committing waits out the uncertainty",
                the_wait_is_two_epsilon,
            ),
            Test::new(
                "two commits in a row are ordered by their timestamps",
                consecutive_commits_are_ordered,
            ),
            Test::new(
                "without the wait, two commits can share a timestamp",
                without_the_wait_order_breaks,
            ),
            Test::new(
                "and an outside observer can see the contradiction",
                the_observer_sees_it,
            ),
            Test::new(
                "a tighter clock means a shorter wait",
                tighter_clock_shorter_wait,
            ),
            Test::new(
                "a perfect clock needs no wait at all",
                perfect_clock_no_wait,
            ),
            Test::new("time never runs backwards", time_moves_forward).ext(),
            Test::new("ten commits in a row keep external order", a_run_of_commits).ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("Waiting out the uncertainty", "commit-wait", || {
            lines(&["init 5", "now", "commit t1", "state"])
        })
        .request("A clock uncertain by ±5, and one transaction committing")
        .response("the timestamp is the top of the interval, and the commit is released 2ε later")
        .note(
            "The transaction is done well before it is announced. That gap is the price of \
             being able to compare timestamps across machines that have never spoken to each \
             other — and it is why the engineering effort goes into shrinking ε with GPS and \
             atomic clocks rather than into the algorithm.",
        ),
        prim_example(
            "The same two commits without the wait",
            "commit-wait",
            || {
                lines(&[
                    "init 5",
                    "commit-wait off",
                    "commit t1",
                    "commit t2",
                    "external-order",
                ])
            },
        )
        .request("Two transactions committing back to back with commit-wait disabled")
        .response("both take the same timestamp and `external-order` reports a violation")
        .note(
            "t2 began after t1 had finished, and their timestamps say they are simultaneous. \
             Nothing inside the system notices; an observer who watched t1 commit and then \
             started t2 sees a database that disagrees with the order they watched happen.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(the_clock_is_an_interval, |ctx| {
    let p = ctx.prim("commit-wait").await?;
    p.send("init 7").await?;
    let now = p.send("now").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("what a node can know about the time");
    c.note(
        "The width is the whole design. A system that treats `now()` as a number has an ε it \
         has not measured and cannot reason about; one that returns an interval can at least \
         say what it does not know.",
    );
    c.eq(
        "the interval is 2ε wide",
        14,
        p.expect_i64(&now, "now", "latest")? - p.expect_i64(&now, "now", "earliest")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(the_timestamp_is_the_latest, |ctx| {
    let p = ctx.prim("commit-wait").await?;
    p.send("init 5").await?;
    let now = p.send("now").await?;
    let committed = p.send("commit t1").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("which end of the interval a commit takes");
    c.note(
        "Taking the earliest would produce a timestamp that is already in the past for a \
         node whose clock runs ahead, and a write in the past is how a snapshot read misses \
         a committed transaction. The top of the interval is safe for everybody.",
    );
    c.eq(
        "the timestamp is the top of the interval",
        p.expect_i64(&now, "now", "latest")?,
        p.expect_i64(&committed, "commit", "ts")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(the_wait_is_two_epsilon, |ctx| {
    let p = ctx.prim("commit-wait").await?;
    p.send("init 5").await?;
    let before = p.send("state").await?;
    let committed = p.send("commit t1").await?;
    let after = p.send("state").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("how long the commit is held");
    c.note(
        "From the moment the timestamp is chosen at now + ε, the wait runs until the \
         earliest possible now is past it — another 2ε of real time. Halving ε halves the \
         latency of every read-write transaction, which is the entire argument for expensive \
         clocks.",
    );
    c.eq(
        "the clock advanced by 2ε",
        10,
        p.expect_i64(&after, "state", "now")? - p.expect_i64(&before, "state", "now")?,
    );
    c.eq(
        "and the commit was released after its timestamp",
        true,
        p.expect_i64(&committed, "commit", "released_at")?
            >= p.expect_i64(&committed, "commit", "ts")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(consecutive_commits_are_ordered, |ctx| {
    let p = ctx.prim("commit-wait").await?;
    p.send("init 5").await?;
    let first = p.send("commit t1").await?;
    let second = p.send("commit t2").await?;
    let order = p.send("external-order").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("two transactions, one after the other");
    c.note(
        "t2 could only start after t1 was released, and by then every clock is past t1's \
         timestamp — so t2's timestamp, taken from any of those clocks, is larger. That is \
         external consistency: the database's order and the world's order agree.",
    );
    c.eq(
        "the second timestamp is larger",
        true,
        p.expect_i64(&second, "commit", "ts")? > p.expect_i64(&first, "commit", "ts")?,
    );
    c.eq(
        "external order holds",
        true,
        p.expect_bool(&order, "external-order", "holds")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(without_the_wait_order_breaks, |ctx| {
    let p = ctx.prim("commit-wait").await?;
    p.send("init 5").await?;
    p.send("commit-wait off").await?;
    let first = p.send("commit t1").await?;
    let second = p.send("commit t2").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("the same two commits with the wait removed");
    c.note(
        "Nothing waited, so no time passed, so both transactions read the same interval and \
         took the same top of it. Two transactions that certainly happened one after the \
         other now carry timestamps that say otherwise.",
    );
    c.eq(
        "the timestamps are equal",
        p.expect_i64(&first, "commit", "ts")?,
        p.expect_i64(&second, "commit", "ts")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(the_observer_sees_it, |ctx| {
    let p = ctx.prim("commit-wait").await?;
    p.send("init 5").await?;
    p.send("commit-wait off").await?;
    p.send("commit t1").await?;
    p.send("commit t2").await?;
    let order = p.send("external-order").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("what an outside observer notices");
    c.note(
        "This is the failure that makes clock uncertainty a correctness problem rather than \
         a tidiness one. A client that saw t1 commit, then started t2, can query at a \
         timestamp between them and get an answer that contradicts what it watched happen.",
    );
    c.eq(
        "external order holds",
        false,
        p.expect_bool(&order, "external-order", "holds")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(tighter_clock_shorter_wait, |ctx| {
    let mut waits = Vec::new();
    for e in [1, 2, 5, 10] {
        let mut p = ctx.prim_fresh("commit-wait").await?;
        p.send(&format!("init {e}")).await?;
        let before = p.send("state").await?;
        p.send("commit t").await?;
        let after = p.send("state").await?;
        waits.push(p.expect_i64(&after, "state", "now")? - p.expect_i64(&before, "state", "now")?);
    }
    let mut c = Check::new("the wait at four clock qualities");
    c.note(
        "Linear in ε, which is the whole reason Spanner's engineering is about GPS receivers \
         and atomic clocks in every datacentre. The protocol does not get cleverer as the \
         clock gets better; it just waits less.",
    );
    c.eq("waits at ε = 1, 2, 5, 10", vec![2, 4, 10, 20], waits);
    c.finish()
});

dist_test!(perfect_clock_no_wait, |ctx| {
    let p = ctx.prim("commit-wait").await?;
    p.send("init 0").await?;
    let before = p.send("state").await?;
    let first = p.send("commit t1").await?;
    let after = p.send("state").await?;
    let order = p.send("external-order").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a clock with no uncertainty at all");
    c.note(
        "With ε = 0 the interval is a point, the wait is nothing, and external consistency \
         is free. No such clock exists, which is exactly why the mechanism does.",
    );
    c.eq(
        "no time passed",
        0,
        p.expect_i64(&after, "state", "now")? - p.expect_i64(&before, "state", "now")?,
    );
    c.eq(
        "and the commit is immediate",
        0,
        p.expect_i64(&first, "commit", "waited")?,
    );
    c.eq(
        "external order still holds",
        true,
        p.expect_bool(&order, "external-order", "holds")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(time_moves_forward, |ctx| {
    let p = ctx.prim("commit-wait").await?;
    p.send("init 3").await?;
    let backwards = p.send("tick -5").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("an attempt to move the clock backwards");
    c.note(
        "A clock that can jump backwards breaks every timestamp-ordered guarantee at once, \
         which is why production systems slew rather than step and why NTP stepping a server \
         backwards is a well-known way to corrupt a database.",
    );
    c.eq(
        "refused",
        false,
        backwards["ok"] == serde_json::Value::Bool(true),
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_run_of_commits, |ctx| {
    let p = ctx.prim("commit-wait").await?;
    p.send("init 4").await?;
    let mut stamps = Vec::new();
    for i in 0..10 {
        let v = p.send(&format!("commit t{i}")).await?;
        stamps.push(p.expect_i64(&v, "commit", "ts")?);
    }
    let order = p.send("external-order").await?;
    let transcript = p.transcript_block();
    let ascending = stamps.windows(2).all(|w| w[1] > w[0]);
    let mut c = Check::new("ten transactions in a row");
    c.note(
        "Every one of them is released before the next begins, so the timestamps must be \
         strictly increasing. A single equal pair anywhere in this sequence would be a \
         violation an observer could catch.",
    );
    c.eq("strictly increasing", true, ascending);
    c.eq(
        "external order holds",
        true,
        p.expect_bool(&order, "external-order", "holds")?,
    );
    c.block("transcript", transcript);
    c.finish()
});
