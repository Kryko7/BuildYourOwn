//! Stage 65 — Commit recovery from the log.
//!
//! Stages 63 and 64 asked what a live participant may decide. This one asks what a *dead*
//! one may decide when it comes back: the process has lost everything it believed, and all
//! it has left is a write-ahead log and a store. A participant that voted yes and then
//! crashed holds the only piece of state that matters, and if it guesses at the outcome it
//! can commit a transaction the coordinator aborted, or roll back one the coordinator has
//! already told a client about.
//!
//! The oracle is the log, read the way the recovery rules say to read it. The tester works
//! out from the records alone what each transaction's state must be — no prepare record
//! means presumed abort, a prepare record with no decision means ask and do not guess, a
//! commit record whose effect never landed means redo — and then asks the program. The
//! seeded sweep at the end crashes at every point of a whole transaction and checks that
//! the recovered state is exactly what the durable records imply, every time.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;

/// Stage 65.
pub fn stage() -> Stage {
    Stage {
        number: 65,
        slug: "commit_recovery",
        name: "Commit recovery from the log",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `commit-recovery`: a durable log, a durable store, and volatile state a crash eats",
            "The prepare record is forced out before the vote is sent, never after",
            "No prepare record means presumed abort; a prepare record with no decision means ask",
            "A commit record whose effect never landed is redone, and redo is idempotent",
        ],
        examples,
        tests: vec![
            Test::new(
                "a fresh participant has an empty log",
                a_fresh_participant,
            ),
            Test::new(
                "the prepare record is forced out before the vote is sent",
                the_prepare_record_is_forced_out,
            ),
            Test::new(
                "a crash takes the volatile state and leaves the log",
                a_crash_takes_only_volatile_state,
            ),
            Test::new(
                "a transaction that was never prepared is aborted on recovery",
                an_unprepared_transaction_is_presumed_aborted,
            ),
            Test::new(
                "a no vote is logged as an abort and needs nobody's permission",
                a_no_vote_is_logged_as_an_abort,
            ),
            Test::new(
                "a prepared transaction asks rather than guessing",
                a_prepared_transaction_asks,
            ),
            Test::new(
                "the coordinator's answer resolves a prepared transaction",
                the_coordinator_resolves_it,
            ),
            Test::new(
                "a commit record whose effect never landed is redone",
                a_commit_that_never_landed_is_redone,
            ),
            Test::new("redo is idempotent", redo_is_idempotent),
            Test::new(
                "an aborted transaction needs nothing doing",
                an_aborted_transaction_needs_nothing,
            ),
            Test::new("recovering twice says the same thing", recovery_is_idempotent).ext(),
            Test::new(
                "the log survives any number of crashes",
                the_log_survives_repeated_crashes,
            )
            .ext(),
            Test::new(
                "several transactions recover independently",
                several_transactions_recover_independently,
            )
            .ext(),
            Test::new(
                "a seeded sweep of crash points recovers exactly what the log implies",
                a_seeded_sweep_of_crash_points,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example(
            "A participant that crashed while prepared",
            "commit-recovery",
            || {
                lines(&[
                    "init p1",
                    "begin tx1",
                    "prepare tx1 yes",
                    "crash",
                    "outcome tx1",
                    "recover",
                    "outcome tx1",
                ])
            },
        )
        .request("a yes vote, a crash, and then a restart")
        .response("unknown while it is down, and prepared once it has read its own log back")
        .note(
            "The crash took everything the process believed and left the log untouched. \
             `prepared` is the honest answer and the only safe one: the coordinator may have \
             committed, and it may have aborted, and nothing on this machine can tell which.",
        ),
        prim_example(
            "A commit whose effect never landed",
            "commit-recovery",
            || {
                lines(&[
                    "init p1",
                    "begin tx1",
                    "prepare tx1 yes",
                    "decide tx1 commit",
                    "crash",
                    "recover",
                    "outcome tx1",
                ])
            },
        )
        .request("a decision written to the log, and a crash before the store was touched")
        .response("committed with applied false, and an action of redo")
        .note(
            "Logging the decision before applying it is what makes this recoverable at all. \
             Recovery redoes the effect from the record; because redo is idempotent, doing \
             it again after a second crash costs nothing.",
        ),
    ]
}

/// What a crash after `steps` steps of a whole transaction must leave behind.
///
/// This is the recovery rule stated directly, so that the seeded sweep never has to ask the
/// program what it thinks: `(state, action, applied)`, where an empty action means the
/// transaction is not in the log at all.
fn expected_after(steps: usize, commit: bool) -> (&'static str, &'static str, bool) {
    match steps {
        // Nothing was written, so the transaction does not exist as far as this machine is
        // concerned, and it can never have voted yes for it.
        0 => ("unknown", "", false),
        // Only a begin record: presumed abort is safe, and recovery performs it.
        1 => ("aborted", "abort", false),
        // Prepared, with no decision in sight. `vote-sent` writes nothing, so crashing
        // before or after the vote left the machine looks exactly the same from the log.
        2 | 3 => ("prepared", "ask-coordinator", false),
        4 => {
            if commit {
                ("committed", "redo", false)
            } else {
                ("aborted", "none", false)
            }
        }
        _ => ("committed", "none", true),
    }
}

/// The commands of one whole transaction, up to and including the decision's effect.
fn script(commit: bool) -> Vec<String> {
    let mut steps = vec![
        "begin tx1".to_string(),
        "prepare tx1 yes".to_string(),
        "vote-sent tx1".to_string(),
    ];
    if commit {
        steps.push("decide tx1 commit".to_string());
        steps.push("apply tx1".to_string());
    } else {
        steps.push("decide tx1 abort".to_string());
    }
    steps
}

dist_test!(a_fresh_participant, |ctx| {
    let p = ctx.prim("commit-recovery").await?;
    let init = p.send("init p1").await?;
    let log = p.send("log").await?;
    let outcome = p.send("outcome tx1").await?;
    let recover = p.send("recover").await?;
    let mut c = Check::new("a participant that has never seen a transaction");
    c.eq("init.ok", true, p.expect_bool(&init, "init p1", "ok")?);
    c.eq(
        "log.records",
        Vec::<String>::new(),
        p.expect_strs(&log, "log", "records")?,
    );
    c.eq(
        "outcome(tx1).state",
        "unknown".to_string(),
        p.expect_str(&outcome, "outcome tx1", "state")?,
    );
    c.eq(
        "recover.pending",
        Vec::<String>::new(),
        p.expect_strs(&recover, "recover", "pending")?,
    );
    c.eq(
        "recover.decided",
        Vec::<String>::new(),
        p.expect_strs(&recover, "recover", "decided")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_prepare_record_is_forced_out, |ctx| {
    let p = ctx.prim("commit-recovery").await?;
    p.send("init p1").await?;
    p.send("begin tx1").await?;
    let prepared = p.send("prepare tx1 yes").await?;
    // The crash lands between the vote being decided and the vote leaving the machine. A
    // participant that answered first and logged afterwards has nothing here, and on
    // recovery would presume abort for a transaction the coordinator may already have
    // committed on the strength of its yes.
    p.send("crash").await?;
    let recover = p.send("recover").await?;
    let log = p.send("log").await?;
    let mut c = Check::new("a crash between the vote being written and the vote being sent");
    c.eq(
        "prepare.logged",
        "prepared".to_string(),
        p.expect_str(&prepared, "prepare tx1 yes", "logged")?,
    );
    c.eq(
        "prepare.vote",
        "yes".to_string(),
        p.expect_str(&prepared, "prepare tx1 yes", "vote")?,
    );
    c.eq(
        "log.records",
        vec!["begin:tx1".to_string(), "prepared:tx1".into()],
        p.expect_strs(&log, "log", "records")?,
    );
    c.eq(
        "recover.pending",
        vec!["tx1".to_string()],
        p.expect_strs(&recover, "recover", "pending")?,
    );
    c.eq(
        "recover.actions.tx1",
        "ask-coordinator".to_string(),
        p.expect_str(&recover["actions"], "recover", "tx1")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_crash_takes_only_volatile_state, |ctx| {
    let p = ctx.prim("commit-recovery").await?;
    p.send("init p1").await?;
    p.send("begin tx1").await?;
    p.send("prepare tx1 yes").await?;
    let before = p.send("outcome tx1").await?;
    p.send("crash").await?;
    let during = p.send("outcome tx1").await?;
    let log = p.send("log").await?;
    p.send("recover").await?;
    let after = p.send("outcome tx1").await?;
    let mut c = Check::new("what a crash takes and what it leaves");
    c.eq(
        "outcome before the crash",
        "prepared".to_string(),
        p.expect_str(&before, "outcome tx1", "state")?,
    );
    // Nothing is known while the process is down: the belief lived in memory.
    c.eq(
        "outcome while it is down",
        "unknown".to_string(),
        p.expect_str(&during, "outcome tx1", "state")?,
    );
    c.eq(
        "log.records across the crash",
        vec!["begin:tx1".to_string(), "prepared:tx1".into()],
        p.expect_strs(&log, "log", "records")?,
    );
    c.eq(
        "outcome after the restart",
        "prepared".to_string(),
        p.expect_str(&after, "outcome tx1", "state")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(an_unprepared_transaction_is_presumed_aborted, |ctx| {
    let p = ctx.prim("commit-recovery").await?;
    p.send("init p1").await?;
    p.send("begin tx1").await?;
    p.send("crash").await?;
    let recover = p.send("recover").await?;
    let outcome = p.send("outcome tx1").await?;
    let mut c = Check::new("a transaction that got as far as begin and no further");
    // With no prepare record the coordinator can never have seen a yes from this
    // participant, so it can never have committed. Abort needs nobody's permission.
    c.eq(
        "recover.pending",
        Vec::<String>::new(),
        p.expect_strs(&recover, "recover", "pending")?,
    );
    c.eq(
        "recover.decided",
        vec!["tx1".to_string()],
        p.expect_strs(&recover, "recover", "decided")?,
    );
    c.eq(
        "recover.actions.tx1",
        "abort".to_string(),
        p.expect_str(&recover["actions"], "recover", "tx1")?,
    );
    c.eq(
        "outcome(tx1).state",
        "aborted".to_string(),
        p.expect_str(&outcome, "outcome tx1", "state")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_no_vote_is_logged_as_an_abort, |ctx| {
    let p = ctx.prim("commit-recovery").await?;
    p.send("init p1").await?;
    p.send("begin tx1").await?;
    let refusal = p.send("prepare tx1 no").await?;
    p.send("crash").await?;
    let recover = p.send("recover").await?;
    let outcome = p.send("outcome tx1").await?;
    let mut c = Check::new("a participant that refused and then crashed");
    c.eq(
        "prepare.vote",
        "no".to_string(),
        p.expect_str(&refusal, "prepare tx1 no", "vote")?,
    );
    c.eq(
        "prepare.logged",
        "abort".to_string(),
        p.expect_str(&refusal, "prepare tx1 no", "logged")?,
    );
    // Refusing costs the participant nothing and binds it to nothing, so recovery has no
    // question to ask: the answer was settled the moment the record was written.
    c.eq(
        "recover.actions.tx1",
        "none".to_string(),
        p.expect_str(&recover["actions"], "recover", "tx1")?,
    );
    c.eq(
        "outcome(tx1).state",
        "aborted".to_string(),
        p.expect_str(&outcome, "outcome tx1", "state")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_prepared_transaction_asks, |ctx| {
    let p = ctx.prim("commit-recovery").await?;
    p.send("init p1").await?;
    p.send("begin tx1").await?;
    p.send("prepare tx1 yes").await?;
    p.send("vote-sent tx1").await?;
    p.send("crash").await?;
    let recover = p.send("recover").await?;
    let outcome = p.send("outcome tx1").await?;
    let mut c = Check::new("a transaction whose yes was sent and whose answer never came");
    // Either guess is wrong half the time, and both halves are unrecoverable: guessing
    // commit can apply an effect the coordinator rolled back, guessing abort can discard
    // one it has already reported as committed to a client.
    c.eq(
        "outcome(tx1).state",
        "prepared".to_string(),
        p.expect_str(&outcome, "outcome tx1", "state")?,
    );
    c.ne(
        "outcome(tx1).state is not a guess at commit",
        "committed".to_string(),
        p.expect_str(&outcome, "outcome tx1", "state")?,
    );
    c.ne(
        "outcome(tx1).state is not a guess at abort",
        "aborted".to_string(),
        p.expect_str(&outcome, "outcome tx1", "state")?,
    );
    c.eq(
        "recover.pending",
        vec!["tx1".to_string()],
        p.expect_strs(&recover, "recover", "pending")?,
    );
    c.eq(
        "recover.actions.tx1",
        "ask-coordinator".to_string(),
        p.expect_str(&recover["actions"], "recover", "tx1")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_coordinator_resolves_it, |ctx| {
    let p = ctx.prim("commit-recovery").await?;
    p.send("init p1").await?;
    p.send("begin tx1").await?;
    p.send("prepare tx1 yes").await?;
    p.send("crash").await?;
    p.send("recover").await?;
    let answered = p.send("coordinator-says tx1 commit").await?;
    let log = p.send("log").await?;
    // The same participant, a second transaction, and the opposite answer.
    p.send("begin tx2").await?;
    p.send("prepare tx2 yes").await?;
    p.send("crash").await?;
    p.send("recover").await?;
    let rolled_back = p.send("coordinator-says tx2 abort").await?;
    let mut c = Check::new("a prepared transaction resolved by the coordinator");
    c.eq(
        "coordinator-says(tx1, commit).state",
        "committed".to_string(),
        p.expect_str(&answered, "coordinator-says tx1 commit", "state")?,
    );
    c.eq(
        "coordinator-says(tx1, commit).applied",
        true,
        p.expect_bool(&answered, "coordinator-says tx1 commit", "applied")?,
    );
    c.that(
        "log.records",
        "a commit record for tx1",
        p.expect_strs(&log, "log", "records")?
            .iter()
            .any(|r| r == "commit:tx1"),
        p.expect_strs(&log, "log", "records")?,
    );
    c.eq(
        "coordinator-says(tx2, abort).state",
        "aborted".to_string(),
        p.expect_str(&rolled_back, "coordinator-says tx2 abort", "state")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_commit_that_never_landed_is_redone, |ctx| {
    let p = ctx.prim("commit-recovery").await?;
    p.send("init p1").await?;
    p.send("begin tx1").await?;
    p.send("prepare tx1 yes").await?;
    p.send("decide tx1 commit").await?;
    // The decision is durable and the effect is not, which is the whole reason the decision
    // is written first: the other order loses transactions that were reported as committed.
    p.send("crash").await?;
    let recover = p.send("recover").await?;
    let before = p.send("outcome tx1").await?;
    p.send("apply tx1").await?;
    let after = p.send("outcome tx1").await?;
    let mut c = Check::new("a commit record with no effect behind it");
    c.eq(
        "recover.decided",
        vec!["tx1".to_string()],
        p.expect_strs(&recover, "recover", "decided")?,
    );
    c.eq(
        "recover.actions.tx1",
        "redo".to_string(),
        p.expect_str(&recover["actions"], "recover", "tx1")?,
    );
    c.eq(
        "outcome(tx1).state",
        "committed".to_string(),
        p.expect_str(&before, "outcome tx1", "state")?,
    );
    c.eq(
        "outcome(tx1).applied before the redo",
        false,
        p.expect_bool(&before, "outcome tx1", "applied")?,
    );
    c.eq(
        "outcome(tx1).applied after the redo",
        true,
        p.expect_bool(&after, "outcome tx1", "applied")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(redo_is_idempotent, |ctx| {
    let p = ctx.prim("commit-recovery").await?;
    p.send("init p1").await?;
    p.send("begin tx1").await?;
    p.send("prepare tx1 yes").await?;
    p.send("decide tx1 commit").await?;
    p.send("crash").await?;
    p.send("recover").await?;
    // Recovery may itself be interrupted, so the redo has to survive being run twice — and
    // a third time, and a fourth.
    for _ in 0..3 {
        p.send("apply tx1").await?;
    }
    let outcome = p.send("outcome tx1").await?;
    p.send("crash").await?;
    let again = p.send("recover").await?;
    let mut c = Check::new("an effect redone three times");
    c.eq(
        "outcome(tx1).applied",
        true,
        p.expect_bool(&outcome, "outcome tx1", "applied")?,
    );
    c.eq(
        "outcome(tx1).state",
        "committed".to_string(),
        p.expect_str(&outcome, "outcome tx1", "state")?,
    );
    // Once the effect is there, recovery has nothing left to do for this transaction.
    c.eq(
        "recover.actions.tx1 after the redo",
        "none".to_string(),
        p.expect_str(&again["actions"], "recover", "tx1")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(an_aborted_transaction_needs_nothing, |ctx| {
    let p = ctx.prim("commit-recovery").await?;
    p.send("init p1").await?;
    p.send("begin tx1").await?;
    p.send("prepare tx1 yes").await?;
    p.send("decide tx1 abort").await?;
    p.send("crash").await?;
    let recover = p.send("recover").await?;
    let outcome = p.send("outcome tx1").await?;
    let mut c = Check::new("a transaction the coordinator rolled back");
    c.eq(
        "recover.pending",
        Vec::<String>::new(),
        p.expect_strs(&recover, "recover", "pending")?,
    );
    c.eq(
        "recover.actions.tx1",
        "none".to_string(),
        p.expect_str(&recover["actions"], "recover", "tx1")?,
    );
    c.eq(
        "outcome(tx1).state",
        "aborted".to_string(),
        p.expect_str(&outcome, "outcome tx1", "state")?,
    );
    c.eq(
        "outcome(tx1).applied",
        false,
        p.expect_bool(&outcome, "outcome tx1", "applied")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(recovery_is_idempotent, |ctx| {
    let p = ctx.prim("commit-recovery").await?;
    p.send("init p1").await?;
    p.send("begin tx1").await?;
    p.send("prepare tx1 yes").await?;
    p.send("crash").await?;
    let mut pendings = Vec::new();
    let mut actions = Vec::new();
    // Recovery can be interrupted by another crash at any point, so running it again must
    // reach the same conclusion rather than drifting a step further each time.
    for _ in 0..3 {
        let r = p.send("recover").await?;
        pendings.push(p.expect_strs(&r, "recover", "pending")?);
        actions.push(p.expect_str(&r["actions"], "recover", "tx1")?);
    }
    let log = p.send("log").await?;
    let mut c = Check::new("three recoveries in a row");
    c.eq(
        "the three pending lists",
        vec![
            vec!["tx1".to_string()],
            vec!["tx1".into()],
            vec!["tx1".into()],
        ],
        pendings,
    );
    c.eq(
        "the three actions",
        vec![
            "ask-coordinator".to_string(),
            "ask-coordinator".into(),
            "ask-coordinator".into(),
        ],
        actions,
    );
    // Reading the log must not write to it, or a crash loop would grow it without bound.
    c.eq(
        "log.records",
        vec!["begin:tx1".to_string(), "prepared:tx1".into()],
        p.expect_strs(&log, "log", "records")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_log_survives_repeated_crashes, |ctx| {
    let p = ctx.prim("commit-recovery").await?;
    p.send("init p1").await?;
    p.send("begin tx1").await?;
    p.send("prepare tx1 yes").await?;
    p.send("decide tx1 commit").await?;
    p.send("apply tx1").await?;
    let expected = vec![
        "begin:tx1".to_string(),
        "prepared:tx1".into(),
        "commit:tx1".into(),
    ];
    let mut logs = Vec::new();
    let mut outcomes = Vec::new();
    for _ in 0..4 {
        p.send("crash").await?;
        p.send("recover").await?;
        let log = p.send("log").await?;
        logs.push(p.expect_strs(&log, "log", "records")?);
        let o = p.send("outcome tx1").await?;
        outcomes.push((
            p.expect_str(&o, "outcome tx1", "state")?,
            p.expect_bool(&o, "outcome tx1", "applied")?,
        ));
    }
    let mut c = Check::new("four crashes and four restarts over one committed transaction");
    for (i, log) in logs.iter().enumerate() {
        c.eq(
            &format!("log.records after crash {i}"),
            expected.clone(),
            log.clone(),
        );
    }
    for (i, (state, applied)) in outcomes.iter().enumerate() {
        c.eq(
            &format!("outcome after crash {i}"),
            "committed".to_string(),
            state.clone(),
        );
        // The store is durable too, so the effect is still there and no redo is owed.
        c.eq(&format!("applied after crash {i}"), true, *applied);
    }
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(several_transactions_recover_independently, |ctx| {
    let p = ctx.prim("commit-recovery").await?;
    p.send("init p1").await?;
    // One prepared, one committed but never applied, one that never got past begin, and one
    // the coordinator rolled back.
    p.send("begin tx1").await?;
    p.send("prepare tx1 yes").await?;
    p.send("begin tx2").await?;
    p.send("prepare tx2 yes").await?;
    p.send("decide tx2 commit").await?;
    p.send("begin tx3").await?;
    p.send("begin tx4").await?;
    p.send("prepare tx4 yes").await?;
    p.send("decide tx4 abort").await?;
    p.send("crash").await?;
    let r = p.send("recover").await?;
    let mut states = Vec::new();
    for tx in ["tx1", "tx2", "tx3", "tx4"] {
        let o = p.send(&format!("outcome {tx}")).await?;
        states.push(p.expect_str(&o, "outcome", "state")?);
    }
    let mut c = Check::new("four transactions in four different states across one crash");
    c.eq(
        "recover.pending",
        vec!["tx1".to_string()],
        p.expect_strs(&r, "recover", "pending")?,
    );
    c.eq(
        "recover.decided",
        vec!["tx2".to_string(), "tx3".into(), "tx4".into()],
        p.expect_strs(&r, "recover", "decided")?,
    );
    for (tx, action) in [
        ("tx1", "ask-coordinator"),
        ("tx2", "redo"),
        ("tx3", "abort"),
        ("tx4", "none"),
    ] {
        c.eq(
            &format!("recover.actions.{tx}"),
            action.to_string(),
            p.expect_str(&r["actions"], "recover", tx)?,
        );
    }
    c.eq(
        "the four outcomes",
        vec![
            "prepared".to_string(),
            "committed".into(),
            "aborted".into(),
            "aborted".into(),
        ],
        states,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_seeded_sweep_of_crash_points, |ctx| {
    // Plan every trial first: which way the coordinator decided, and how far the
    // transaction got before the machine died. What each one must recover to comes from
    // the records alone, worked out here, never from the program.
    let mut trials: Vec<(bool, usize)> = Vec::new();
    for _ in 0..16 {
        let commit = ctx.rng.random_bool(0.6);
        let steps = script(commit).len();
        trials.push((commit, ctx.rng.random_range(0..=steps)));
    }
    let seed = ctx.seed;
    let p = ctx.prim("commit-recovery").await?;
    let mut observed = Vec::with_capacity(trials.len());
    for (commit, crash_after) in &trials {
        p.send("init p1").await?;
        for command in script(*commit).iter().take(*crash_after) {
            p.send(command).await?;
        }
        p.send("crash").await?;
        let r = p.send("recover").await?;
        let o = p.send("outcome tx1").await?;
        let action = match r["actions"].get("tx1") {
            Some(v) => v.as_str().unwrap_or_default().to_string(),
            None => String::new(),
        };
        observed.push((
            p.expect_str(&o, "outcome tx1", "state")?,
            action,
            p.expect_bool(&o, "outcome tx1", "applied")?,
        ));
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("sixteen seeded crash points through a whole transaction");
    c.note(format!("seed {seed}, {} trials", trials.len()));
    for (t, ((commit, crash_after), (state, action, applied))) in
        trials.iter().zip(&observed).enumerate()
    {
        let (want_state, want_action, want_applied) = expected_after(*crash_after, *commit);
        let where_ = format!(
            "trial[{t}] crash after {crash_after} step(s) of a {}",
            if *commit { "commit" } else { "abort" }
        );
        c.eq(
            &format!("{where_} → state"),
            want_state.to_string(),
            state.clone(),
        );
        c.eq(
            &format!("{where_} → action"),
            want_action.to_string(),
            action.clone(),
        );
        c.eq(&format!("{where_} → applied"), want_applied, *applied);
        if !c.ok() {
            break;
        }
    }
    c.block("transcript", transcript);
    c.finish()
});
