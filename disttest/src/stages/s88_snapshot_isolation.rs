//! Stage 88 — Snapshot isolation, built the Percolator way.
//!
//! Stage 63 committed a transaction across participants with two-phase commit. Stage 28 read
//! and wrote at a revision with MVCC. Percolator is what you get when you put those two
//! together on a plain key/value store, and it is how a great many distributed databases
//! actually work.
//!
//! A transaction takes a **start timestamp** and reads the world as of that instant, so it
//! sees a consistent snapshot however long it runs and whatever anybody else commits
//! meanwhile. Its writes are buffered. At **prewrite** it takes a lock on every key it is
//! about to write and checks for two things: somebody else's lock, and any version
//! committed after its own start timestamp. Either is a write-write conflict and the
//! transaction aborts. Only then does it take a **commit timestamp** and make the writes
//! visible.
//!
//! What this gives is snapshot isolation: no dirty reads, no lost updates, repeatable
//! reads — and one famous hole. Two transactions that read the same rows and write
//! *different* ones never conflict at any key, so both commit, and a constraint that
//! spanned both rows can end up violated by a pair of transactions either of which alone
//! would have preserved it. That is write skew, and the last test builds it.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};

/// Stage 88.
pub fn stage() -> Stage {
    Stage {
        number: 88,
        slug: "snapshot_isolation",
        name: "Snapshot isolation, and the skew it allows",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `snapshot`: `begin <txn>` takes a start timestamp, `read`/`write` are against \
             it, `prewrite` takes the locks and finds conflicts, `commit` makes the writes visible",
            "A read is always as of the transaction's start timestamp, so a long transaction sees \
             a consistent world no matter what commits while it runs — plus its own writes",
            "Prewrite aborts on two things: a key locked by somebody else, and a version \
             committed after this transaction's start timestamp",
            "Writes become visible only at the commit timestamp, all at once — a reader at a \
             timestamp between start and commit sees none of them",
        ],
        examples,
        tests: vec![
            Test::new(
                "a transaction reads the world as of its start",
                reads_at_start,
            ),
            Test::new(
                "and sees its own writes before anyone else does",
                sees_its_own_writes,
            ),
            Test::new(
                "a concurrent commit is invisible to a transaction already running",
                the_snapshot_is_stable,
            ),
            Test::new(
                "uncommitted writes are invisible to everybody else",
                no_dirty_reads,
            ),
            Test::new(
                "two transactions writing the same key: one of them aborts",
                write_write_conflict,
            ),
            Test::new(
                "a lock held by another transaction blocks prewrite",
                a_held_lock,
            ),
            Test::new(
                "an aborted transaction leaves no lock behind",
                abort_releases_locks,
            ),
            Test::new(
                "committing without prewriting is refused",
                commit_needs_prewrite,
            ),
            Test::new(
                "writes become visible all at once, at the commit timestamp",
                atomic_visibility,
            )
            .ext(),
            Test::new(
                "write skew: two transactions that never touch the same key",
                write_skew,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example(
            "A stable snapshot while the world moves",
            "snapshot",
            || {
                lines(&[
                    "init",
                    "begin reader",
                    "begin writer",
                    "write writer k 99",
                    "prewrite writer",
                    "commit writer",
                    "read reader k",
                ])
            },
        )
        .request("A long-running reader, and a writer that commits underneath it")
        .response("the reader still sees 0 — the value as of its own start timestamp")
        .note(
            "The reader is not stale by accident, it is stale on purpose and consistently \
             so: every key it reads comes from the same instant. That is what makes a report \
             running for ten minutes produce numbers that add up.",
        ),
        prim_example("Two writers, one key", "snapshot", || {
            lines(&[
                "init",
                "begin a",
                "begin b",
                "write a k 1",
                "write b k 2",
                "prewrite a",
                "commit a",
                "prewrite b",
            ])
        })
        .request("Two concurrent transactions writing the same key, a committing first")
        .response("b's prewrite fails with a write conflict")
        .note(
            "b's snapshot predates a's commit, so letting b write would silently discard a's \
             update — the lost update anomaly. Snapshot isolation catches it at prewrite by \
             refusing any key with a version newer than the transaction's own start.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(reads_at_start, |ctx| {
    let p = ctx.prim("snapshot").await?;
    p.send("init").await?;
    p.send("begin setup").await?;
    p.send("write setup k 7").await?;
    p.send("prewrite setup").await?;
    p.send("commit setup").await?;
    p.send("begin t").await?;
    let read = p.send("read t k").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a transaction reading committed data");
    c.eq("the value", 7, p.expect_i64(&read, "read", "value")?);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(sees_its_own_writes, |ctx| {
    let p = ctx.prim("snapshot").await?;
    p.send("init").await?;
    p.send("begin t").await?;
    p.send("write t k 5").await?;
    let mine = p.send("read t k").await?;
    p.send("begin other").await?;
    let theirs = p.send("read other k").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a write that has not been committed yet");
    c.note(
        "A transaction has to see its own work or the simplest read-modify-write is wrong. \
         Nobody else may see it until commit — which means the buffered writes have to be \
         consulted on read, and are not part of the store.",
    );
    c.eq(
        "the writer sees 5",
        5,
        p.expect_i64(&mine, "read", "value")?,
    );
    c.eq(
        "everyone else sees 0",
        0,
        p.expect_i64(&theirs, "read", "value")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(the_snapshot_is_stable, |ctx| {
    let p = ctx.prim("snapshot").await?;
    p.send("init").await?;
    p.send("begin reader").await?;
    let before = p.send("read reader k").await?;
    p.send("begin writer").await?;
    p.send("write writer k 99").await?;
    p.send("prewrite writer").await?;
    p.send("commit writer").await?;
    let after = p.send("read reader k").await?;
    p.send("begin fresh").await?;
    let fresh = p.send("read fresh k").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a commit underneath a running transaction");
    c.note(
        "The same read, twice, with a committed write in between, and the same answer both \
         times — that is repeatable read. A transaction that started afterwards sees the new \
         value, which is how the snapshot stays a snapshot rather than becoming a freeze.",
    );
    c.eq(
        "the reader before",
        0,
        p.expect_i64(&before, "read", "value")?,
    );
    c.eq("and after", 0, p.expect_i64(&after, "read", "value")?);
    c.eq(
        "a newer transaction sees it",
        99,
        p.expect_i64(&fresh, "read", "value")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(no_dirty_reads, |ctx| {
    let p = ctx.prim("snapshot").await?;
    p.send("init").await?;
    p.send("begin writer").await?;
    p.send("write writer k 42").await?;
    p.send("prewrite writer").await?;
    // Prewritten, locked, and not committed: the value must still be invisible.
    p.send("begin reader").await?;
    let read = p.send("read reader k").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a prewritten but uncommitted value");
    c.note(
        "Prewrite has written the value and taken the lock, so it is physically in the \
         store. It is not committed, so it does not exist — and a reader that consults the \
         lock table instead of the commit timestamps will read it and be wrong.",
    );
    c.eq(
        "the reader sees nothing",
        0,
        p.expect_i64(&read, "read", "value")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(write_write_conflict, |ctx| {
    let p = ctx.prim("snapshot").await?;
    p.send("init").await?;
    p.send("begin a").await?;
    p.send("begin b").await?;
    p.send("write a k 1").await?;
    p.send("write b k 2").await?;
    p.send("prewrite a").await?;
    p.send("commit a").await?;
    let b = p.send("prewrite b").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("two transactions writing one key");
    c.note(
        "b read the key as of before a committed, so b's write is based on a world that no \
         longer exists. Allowing it would lose a's update entirely — first-committer-wins is \
         what snapshot isolation offers instead.",
    );
    c.eq(
        "b's prewrite fails",
        false,
        p.expect_bool(&b, "prewrite", "prewritten")?,
    );
    c.eq(
        "with a write conflict",
        "write conflict",
        b["reason"].as_str().unwrap_or(""),
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_held_lock, |ctx| {
    let p = ctx.prim("snapshot").await?;
    p.send("init").await?;
    p.send("begin a").await?;
    p.send("begin b").await?;
    p.send("write a k 1").await?;
    p.send("write b k 2").await?;
    p.send("prewrite a").await?;
    // a has the lock and has not committed: b cannot take it.
    let b = p.send("prewrite b").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a key already locked by a live transaction");
    c.note(
        "This is the other half of prewrite's job. The conflict is not yet in the committed \
         data — a has committed nothing — so only the lock reveals it. A real system also \
         has to decide whether to wait for the lock or to clean it up if its owner died, \
         which is what Percolator's primary lock is for.",
    );
    c.eq(
        "b's prewrite fails",
        false,
        p.expect_bool(&b, "prewrite", "prewritten")?,
    );
    c.eq(
        "because the key is locked",
        "locked",
        b["reason"].as_str().unwrap_or(""),
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(abort_releases_locks, |ctx| {
    let p = ctx.prim("snapshot").await?;
    p.send("init").await?;
    p.send("begin a").await?;
    p.send("write a k 1").await?;
    p.send("prewrite a").await?;
    p.send("abort a").await?;
    p.send("begin b").await?;
    p.send("write b k 2").await?;
    let b = p.send("prewrite b").await?;
    let state = p.send("state").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a key whose transaction gave up");
    c.note(
        "An abandoned lock blocks the key for everybody, so releasing on abort is not \
         housekeeping — it is availability. The hard case, which this topic does not model, \
         is an abort nobody performed because the client vanished.",
    );
    c.eq(
        "b can take the key",
        true,
        p.expect_bool(&b, "prewrite", "prewritten")?,
    );
    c.eq(
        "and a's lock is gone",
        1,
        state["locks"].as_array().map(|a| a.len()).unwrap_or(0),
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(commit_needs_prewrite, |ctx| {
    let p = ctx.prim("snapshot").await?;
    p.send("init").await?;
    p.send("begin t").await?;
    p.send("write t k 1").await?;
    let committed = p.send("commit t").await?;
    p.send("begin reader").await?;
    let read = p.send("read reader k").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a commit with no prewrite before it");
    c.note(
        "The two phases are not decorative. Prewrite is where every conflict is found, so a \
         commit that skips it is a commit that has checked nothing — and the store would \
         accept a write built on a stale snapshot.",
    );
    c.eq(
        "refused",
        false,
        p.expect_bool(&committed, "commit", "committed")?,
    );
    c.eq(
        "and nothing was written",
        0,
        p.expect_i64(&read, "read", "value")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(atomic_visibility, |ctx| {
    let p = ctx.prim("snapshot").await?;
    p.send("init").await?;
    p.send("begin t").await?;
    p.send("write t a 1").await?;
    p.send("write t b 1").await?;
    p.send("prewrite t").await?;
    p.send("begin mid").await?;
    let mid_a = p.send("read mid a").await?;
    let mid_b = p.send("read mid b").await?;
    p.send("commit t").await?;
    p.send("begin after").await?;
    let after_a = p.send("read after a").await?;
    let after_b = p.send("read after b").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a two-key transaction, observed either side of its commit");
    c.note(
        "Both keys change at the same timestamp, so no reader can ever see one without the \
         other. That is the only reason a transfer between two accounts can be written as \
         two writes — atomicity here is a property of the timestamp, not of a latch.",
    );
    c.eq(
        "before the commit: neither",
        (0, 0),
        (
            p.expect_i64(&mid_a, "read", "value")?,
            p.expect_i64(&mid_b, "read", "value")?,
        ),
    );
    c.eq(
        "after it: both",
        (1, 1),
        (
            p.expect_i64(&after_a, "read", "value")?,
            p.expect_i64(&after_b, "read", "value")?,
        ),
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(write_skew, |ctx| {
    let p = ctx.prim("snapshot").await?;
    p.send("init").await?;
    // Two doctors on call: the rule is that at least one must stay. Both start at 1.
    p.send("begin setup").await?;
    p.send("write setup alice 1").await?;
    p.send("write setup bob 1").await?;
    p.send("prewrite setup").await?;
    p.send("commit setup").await?;

    // Each doctor checks that the *other* is on call, then takes themselves off.
    p.send("begin t1").await?;
    p.send("begin t2").await?;
    let t1_sees_bob = p.send("read t1 bob").await?;
    let t2_sees_alice = p.send("read t2 alice").await?;
    p.send("write t1 alice 0").await?;
    p.send("write t2 bob 0").await?;
    let p1 = p.send("prewrite t1").await?;
    p.send("commit t1").await?;
    let p2 = p.send("prewrite t2").await?;
    p.send("commit t2").await?;

    p.send("begin check").await?;
    let alice = p.send("read check alice").await?;
    let bob = p.send("read check bob").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("two transactions that never touch the same key");
    c.note(
        "Each transaction read the other doctor, saw them on call, and removed itself — \
         correct in isolation, and correct under snapshot isolation, because they write \
         different keys and so never conflict. The constraint spanned both rows and nothing \
         in the protocol was looking at it. This is write skew, it is the defining weakness \
         of snapshot isolation, and it is why serializable snapshot isolation exists.",
    );
    c.eq(
        "t1 saw bob on call",
        1,
        p.expect_i64(&t1_sees_bob, "read", "value")?,
    );
    c.eq(
        "t2 saw alice on call",
        1,
        p.expect_i64(&t2_sees_alice, "read", "value")?,
    );
    c.eq(
        "both prewrites succeeded",
        (true, true),
        (
            p.expect_bool(&p1, "prewrite", "prewritten")?,
            p.expect_bool(&p2, "prewrite", "prewritten")?,
        ),
    );
    c.eq(
        "and nobody is on call",
        (0, 0),
        (
            p.expect_i64(&alice, "read", "value")?,
            p.expect_i64(&bob, "read", "value")?,
        ),
    );
    c.block("transcript", transcript);
    c.finish()
});
