//! Stage 64 — Three-phase commit.
//!
//! Stage 63 left every prepared participant holding locks it could not release. Three-phase
//! commit buys its way out of that with one more round trip: before it commits, the
//! coordinator tells everybody that everybody voted yes. A participant that has heard the
//! pre-commit can therefore decide commit on its own, and a participant that has not can
//! decide abort, so nobody is ever blocked.
//!
//! The oracle is the termination rule itself, and the stage is built around what that rule
//! costs. It works because a participant treats "no pre-commit arrived" as proof that none
//! was sent — which is true only on a synchronous network with honest failure detection. The
//! partition tests break exactly that assumption, and the two halves of the cluster walk away
//! with different answers that healing never repairs. That is why real systems replicate the
//! decision with consensus instead of adding rounds.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;

/// Stage 64.
pub fn stage() -> Stage {
    Stage {
        number: 64,
        slug: "three_phase_commit",
        name: "Three-phase commit",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `three-phase-commit`: vote, then pre-commit, then commit",
            "Pre-commit is only legal once every participant has voted yes",
            "On a timeout a pre-committed participant commits and a merely prepared one aborts",
            "That rule reads 'no pre-commit arrived' as 'none was sent', which a partition breaks",
        ],
        examples,
        tests: vec![
            Test::new(
                "a fresh transaction has every participant working",
                a_fresh_transaction,
            ),
            Test::new(
                "a pre-commit before every yes vote is refused",
                pre_commit_needs_every_yes,
            ),
            Test::new(
                "one no aborts before the pre-commit round begins",
                one_no_aborts_early,
            ),
            Test::new("the happy path commits every participant", the_happy_path),
            Test::new(
                "a commit without a majority of acknowledgements is refused",
                commit_needs_a_majority_of_acks,
            ),
            Test::new(
                "a pre-committed participant commits on its own when the coordinator dies",
                a_pre_committed_participant_commits,
            ),
            Test::new(
                "a participant that never saw the pre-commit aborts on its own",
                a_prepared_participant_aborts,
            ),
            Test::new(
                "no participant is ever blocked, which is what the extra round buys",
                nobody_is_ever_blocked,
            ),
            Test::new(
                "a partition makes two groups decide differently",
                a_partition_splits_the_decision,
            ),
            Test::new(
                "the split decision survives the heal",
                the_split_is_permanent,
            )
            .ext(),
            Test::new(
                "a partition stops the coordinator's broadcast reaching the far side",
                a_partition_swallows_the_broadcast,
            )
            .ext(),
            Test::new(
                "an acknowledgement from a participant that never saw the pre-commit does not count",
                an_ack_without_a_pre_commit_does_not_count,
            )
            .ext(),
            Test::new(
                "seeded runs with a complete or a lost pre-commit round never diverge",
                seeded_runs_never_diverge,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("What the extra round buys", "three-phase-commit", || {
            lines(&[
                "init [\"p1\",\"p2\"]",
                "vote p1 yes",
                "vote p2 yes",
                "pre-commit",
                "p-precommit p1",
                "p-precommit p2",
                "crash coordinator",
                "p-timeout p1",
            ])
        })
        .request("both participants pre-committed, and then a coordinator crash")
        .response("commit, decided by the participant itself, with blocked false")
        .note(
            "Compare stage 63, where the same crash left everyone stuck. The pre-commit is \
             the participant's evidence that every vote was yes, which is exactly the fact \
             the coordinator was about to act on, so it can act on it too.",
        ),
        prim_example(
            "What it still does not survive",
            "three-phase-commit",
            || {
                lines(&[
                    "init [\"p1\",\"p2\",\"p3\"]",
                    "vote p1 yes",
                    "vote p2 yes",
                    "vote p3 yes",
                    "pre-commit",
                    "p-precommit p1",
                    "crash coordinator",
                    "partition [\"p1\"]",
                    "p-timeout p1",
                    "p-timeout p2",
                    "decisions",
                ])
            },
        )
        .request("a pre-commit that reached one participant, then a crash and a partition")
        .response("p1 commits, p2 aborts, and `inconsistent` is true")
        .note(
            "The termination rule reads silence as proof that no pre-commit was sent. A \
             partition makes silence mean nothing at all, and the two sides walk away with \
             different answers. No extra round fixes this; replicating the decision does.",
        ),
    ]
}

/// One seeded trial: who voted yes, and whether the pre-commit round reached everybody.
struct Trial {
    /// True where that participant voted yes.
    votes: [bool; 3],
    /// True when the pre-commit reached every participant rather than none of them.
    complete: bool,
}

impl Trial {
    /// The decision every participant must reach, from the termination rule alone.
    ///
    /// Commit needs every vote and a pre-commit round that everybody saw; anything else is
    /// an abort. The rule is only safe because these two are the only possibilities here —
    /// a half-delivered pre-commit is what the partition tests are about.
    fn expected(&self) -> &'static str {
        if self.votes.iter().all(|yes| *yes) && self.complete {
            "commit"
        } else {
            "abort"
        }
    }
}

dist_test!(a_fresh_transaction, |ctx| {
    let p = ctx.prim("three-phase-commit").await?;
    let init = p.send("init [\"p1\",\"p2\",\"p3\"]").await?;
    let names = p.expect_strs(&init, "init", "participants")?;
    let p1 = p.send("p-state p1").await?;
    let d = p.send("decisions").await?;
    let mut c = Check::new("a transaction nobody has voted on");
    c.eq(
        "init.participants",
        vec!["p1".to_string(), "p2".into(), "p3".into()],
        names,
    );
    c.eq(
        "init.state",
        "init".to_string(),
        p.expect_str(&init, "init", "state")?,
    );
    c.eq(
        "p-state(p1).state",
        "working".to_string(),
        p.expect_str(&p1, "p-state p1", "state")?,
    );
    c.eq(
        "decisions.decisions.p1",
        &serde_json::Value::Null,
        &d["decisions"]["p1"],
    );
    c.eq(
        "decisions.inconsistent",
        false,
        p.expect_bool(&d, "decisions", "inconsistent")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(pre_commit_needs_every_yes, |ctx| {
    let p = ctx.prim("three-phase-commit").await?;
    p.send("init [\"p1\",\"p2\",\"p3\"]").await?;
    p.send("vote p1 yes").await?;
    // Two of three have agreed; the pre-commit round says "everybody said yes", and that is
    // not yet true, so sending it would be a lie the participants are entitled to act on.
    p.send("vote p2 yes").await?;
    let early = p.send("pre-commit").await?;
    p.send("vote p3 no").await?;
    let refused = p.send("pre-commit").await?;
    let mut c = Check::new("a pre-commit sent before the vote was unanimous");
    c.eq(
        "the early pre-commit's ok",
        false,
        p.expect_bool(&early, "pre-commit", "ok")?,
    );
    c.eq(
        "the early pre-commit's sent",
        0,
        p.expect_i64(&early, "pre-commit", "sent")?,
    );
    c.eq(
        "the pre-commit after a no",
        false,
        p.expect_bool(&refused, "pre-commit", "ok")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(one_no_aborts_early, |ctx| {
    let p = ctx.prim("three-phase-commit").await?;
    p.send("init [\"p1\",\"p2\",\"p3\"]").await?;
    p.send("vote p1 yes").await?;
    let refusal = p.send("vote p2 no").await?;
    let d1 = p.send("deliver p1").await?;
    let d3 = p.send("deliver p3").await?;
    let decisions = p.send("decisions").await?;
    let mut c = Check::new("a refusal that arrives before the pre-commit round");
    // Abort needs no extra round: the coordinator already knows the answer and nothing
    // later can change it, so the third participant never even has to be asked.
    c.eq(
        "vote(p2, no).decision",
        "abort".to_string(),
        p.expect_str(&refusal, "vote p2 no", "decision")?,
    );
    c.eq(
        "deliver(p1).state",
        "aborted".to_string(),
        p.expect_str(&d1, "deliver p1", "state")?,
    );
    c.eq(
        "deliver(p3).state",
        "aborted".to_string(),
        p.expect_str(&d3, "deliver p3", "state")?,
    );
    c.eq(
        "decisions.inconsistent",
        false,
        p.expect_bool(&decisions, "decisions", "inconsistent")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_happy_path, |ctx| {
    let p = ctx.prim("three-phase-commit").await?;
    p.send("init [\"p1\",\"p2\",\"p3\"]").await?;
    for name in ["p1", "p2", "p3"] {
        p.send(&format!("vote {name} yes")).await?;
    }
    let pre = p.send("pre-commit").await?;
    for name in ["p1", "p2", "p3"] {
        p.send(&format!("p-precommit {name}")).await?;
    }
    p.send("ack p1").await?;
    let acks = p.send("ack p2").await?;
    let commit = p.send("commit").await?;
    for name in ["p1", "p2", "p3"] {
        p.send(&format!("deliver {name}")).await?;
    }
    let decisions = p.send("decisions").await?;
    let mut c = Check::new("a unanimous transaction taken all the way through");
    c.eq(
        "pre-commit.ok",
        true,
        p.expect_bool(&pre, "pre-commit", "ok")?,
    );
    c.eq(
        "pre-commit.sent",
        3,
        p.expect_i64(&pre, "pre-commit", "sent")?,
    );
    c.eq("ack(p2).acks", 2, p.expect_i64(&acks, "ack p2", "acks")?);
    c.eq("commit.ok", true, p.expect_bool(&commit, "commit", "ok")?);
    for name in ["p1", "p2", "p3"] {
        c.eq(
            &format!("decisions.decisions.{name}"),
            "commit".to_string(),
            p.expect_str(&decisions["decisions"], "decisions", name)?,
        );
    }
    c.eq(
        "decisions.inconsistent",
        false,
        p.expect_bool(&decisions, "decisions", "inconsistent")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(commit_needs_a_majority_of_acks, |ctx| {
    let p = ctx.prim("three-phase-commit").await?;
    p.send("init [\"p1\",\"p2\",\"p3\"]").await?;
    for name in ["p1", "p2", "p3"] {
        p.send(&format!("vote {name} yes")).await?;
    }
    p.send("pre-commit").await?;
    p.send("p-precommit p1").await?;
    p.send("ack p1").await?;
    // One acknowledgement out of three: the coordinator cannot yet be sure the pre-commit
    // survived anywhere it would be found again after a crash.
    let early = p.send("commit").await?;
    p.send("p-precommit p2").await?;
    p.send("ack p2").await?;
    let later = p.send("commit").await?;
    let mut c = Check::new("a commit attempted on one acknowledgement, and then on two");
    c.eq(
        "the early commit's ok",
        false,
        p.expect_bool(&early, "commit", "ok")?,
    );
    c.eq(
        "the early commit's decision",
        &serde_json::Value::Null,
        &early["decision"],
    );
    c.eq(
        "the later commit's ok",
        true,
        p.expect_bool(&later, "commit", "ok")?,
    );
    c.eq(
        "the later commit's decision",
        "commit".to_string(),
        p.expect_str(&later, "commit", "decision")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_pre_committed_participant_commits, |ctx| {
    let p = ctx.prim("three-phase-commit").await?;
    p.send("init [\"p1\",\"p2\",\"p3\"]").await?;
    for name in ["p1", "p2", "p3"] {
        p.send(&format!("vote {name} yes")).await?;
    }
    p.send("pre-commit").await?;
    for name in ["p1", "p2", "p3"] {
        p.send(&format!("p-precommit {name}")).await?;
    }
    p.send("crash coordinator").await?;
    let t = p.send("p-timeout p1").await?;
    let state = p.send("p-state p1").await?;
    let decisions = p.send("decisions").await?;
    let mut c = Check::new("a pre-committed participant whose coordinator went away");
    // The pre-commit is the participant's own evidence that every vote was yes. It is
    // therefore acting on the same fact the coordinator was about to act on, not guessing.
    c.eq(
        "p-timeout(p1).decision",
        "commit".to_string(),
        p.expect_str(&t, "p-timeout p1", "decision")?,
    );
    c.eq(
        "p-timeout(p1).blocked",
        false,
        p.expect_bool(&t, "p-timeout p1", "blocked")?,
    );
    c.eq(
        "p-timeout(p1).reason",
        "pre-commit received, every participant voted yes".to_string(),
        p.expect_str(&t, "p-timeout p1", "reason")?,
    );
    c.eq(
        "p-state(p1).state",
        "committed".to_string(),
        p.expect_str(&state, "p-state p1", "state")?,
    );
    c.eq(
        "decisions.inconsistent",
        false,
        p.expect_bool(&decisions, "decisions", "inconsistent")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_prepared_participant_aborts, |ctx| {
    let p = ctx.prim("three-phase-commit").await?;
    p.send("init [\"p1\",\"p2\",\"p3\"]").await?;
    for name in ["p1", "p2", "p3"] {
        p.send(&format!("vote {name} yes")).await?;
    }
    // The coordinator dies before the pre-commit round, so no participant ever saw one.
    p.send("crash coordinator").await?;
    let mut answers = Vec::new();
    for name in ["p1", "p2", "p3"] {
        let t = p.send(&format!("p-timeout {name}")).await?;
        answers.push((
            p.expect_str(&t, "p-timeout", "decision")?,
            p.expect_bool(&t, "p-timeout", "blocked")?,
            p.expect_str(&t, "p-timeout", "reason")?,
        ));
    }
    let decisions = p.send("decisions").await?;
    let mut c = Check::new("three prepared participants with no pre-commit between them");
    for (i, (decision, blocked, reason)) in answers.iter().enumerate() {
        c.eq(
            &format!("p-timeout(p{}).decision", i + 1),
            "abort".to_string(),
            decision.clone(),
        );
        c.eq(&format!("p-timeout(p{}).blocked", i + 1), false, *blocked);
        c.eq(
            &format!("p-timeout(p{}).reason", i + 1),
            "no pre-commit received".to_string(),
            reason.clone(),
        );
    }
    c.eq(
        "decisions.inconsistent",
        false,
        p.expect_bool(&decisions, "decisions", "inconsistent")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(nobody_is_ever_blocked, |ctx| {
    let p = ctx.prim("three-phase-commit").await?;
    // A participant that never voted, and one that voted yes and heard nothing since.
    p.send("init [\"p1\",\"p2\",\"p3\"]").await?;
    p.send("vote p1 yes").await?;
    p.send("crash coordinator").await?;
    let working = p.send("p-timeout p2").await?;
    let prepared = p.send("p-timeout p1").await?;
    // And one that reached the pre-commit round before the coordinator went away.
    p.send("init [\"p1\",\"p2\",\"p3\"]").await?;
    for name in ["p1", "p2", "p3"] {
        p.send(&format!("vote {name} yes")).await?;
    }
    p.send("pre-commit").await?;
    p.send("p-precommit p1").await?;
    p.send("crash coordinator").await?;
    let pre_committed = p.send("p-timeout p1").await?;
    let mut c = Check::new("a timeout in each of the three states a participant can be in");
    // This is the whole difference from stage 63, where the prepared case answered blocked.
    // Three-phase commit turns the blocking window into a rule every participant can apply.
    c.eq(
        "a working participant is blocked",
        false,
        p.expect_bool(&working, "p-timeout p2", "blocked")?,
    );
    c.eq(
        "a prepared participant is blocked",
        false,
        p.expect_bool(&prepared, "p-timeout p1", "blocked")?,
    );
    c.eq(
        "a pre-committed participant is blocked",
        false,
        p.expect_bool(&pre_committed, "p-timeout p1", "blocked")?,
    );
    c.eq(
        "the pre-committed participant's decision",
        "commit".to_string(),
        p.expect_str(&pre_committed, "p-timeout p1", "decision")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_partition_splits_the_decision, |ctx| {
    let p = ctx.prim("three-phase-commit").await?;
    p.send("init [\"p1\",\"p2\",\"p3\"]").await?;
    for name in ["p1", "p2", "p3"] {
        p.send(&format!("vote {name} yes")).await?;
    }
    p.send("pre-commit").await?;
    // The pre-commit reached p1 and nobody else before the coordinator died.
    p.send("p-precommit p1").await?;
    p.send("crash coordinator").await?;
    let split = p.send("partition [\"p1\"]").await?;
    let one = p.send("p-timeout p1").await?;
    let two = p.send("p-timeout p2").await?;
    let three = p.send("p-timeout p3").await?;
    let decisions = p.send("decisions").await?;
    let mut c = Check::new("a half-delivered pre-commit round, and then a partition");
    c.eq(
        "partition.split",
        true,
        p.expect_bool(&split, "partition", "split")?,
    );
    c.eq(
        "p-timeout(p1).decision",
        "commit".to_string(),
        p.expect_str(&one, "p-timeout p1", "decision")?,
    );
    c.eq(
        "p-timeout(p2).decision",
        "abort".to_string(),
        p.expect_str(&two, "p-timeout p2", "decision")?,
    );
    c.eq(
        "p-timeout(p3).decision",
        "abort".to_string(),
        p.expect_str(&three, "p-timeout p3", "decision")?,
    );
    // Both sides applied the termination rule correctly and reached opposite answers. The
    // rule assumes a participant that heard nothing can conclude nothing was sent, and a
    // partition is precisely the case where that inference is wrong.
    c.eq(
        "decisions.inconsistent",
        true,
        p.expect_bool(&decisions, "decisions", "inconsistent")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_split_is_permanent, |ctx| {
    let p = ctx.prim("three-phase-commit").await?;
    p.send("init [\"p1\",\"p2\",\"p3\"]").await?;
    for name in ["p1", "p2", "p3"] {
        p.send(&format!("vote {name} yes")).await?;
    }
    p.send("pre-commit").await?;
    p.send("p-precommit p1").await?;
    p.send("crash coordinator").await?;
    p.send("partition [\"p1\"]").await?;
    for name in ["p1", "p2", "p3"] {
        p.send(&format!("p-timeout {name}")).await?;
    }
    let healed = p.send("heal").await?;
    let after = p.send("decisions").await?;
    let p1 = p.send("p-state p1").await?;
    let p2 = p.send("p-state p2").await?;
    let mut c = Check::new("a healed network after both sides had already decided");
    c.eq(
        "heal.split",
        false,
        p.expect_bool(&healed, "heal", "split")?,
    );
    // A commit is not a cache entry. Once a participant has told its clients the
    // transaction happened, reconnecting to a peer that says otherwise repairs nothing.
    c.eq(
        "p-state(p1).state after the heal",
        "committed".to_string(),
        p.expect_str(&p1, "p-state p1", "state")?,
    );
    c.eq(
        "p-state(p2).state after the heal",
        "aborted".to_string(),
        p.expect_str(&p2, "p-state p2", "state")?,
    );
    c.eq(
        "decisions.inconsistent after the heal",
        true,
        p.expect_bool(&after, "decisions", "inconsistent")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_partition_swallows_the_broadcast, |ctx| {
    let p = ctx.prim("three-phase-commit").await?;
    p.send("init [\"p1\",\"p2\",\"p3\"]").await?;
    for name in ["p1", "p2", "p3"] {
        p.send(&format!("vote {name} yes")).await?;
    }
    p.send("pre-commit").await?;
    for name in ["p1", "p2", "p3"] {
        p.send(&format!("p-precommit {name}")).await?;
    }
    p.send("ack p1").await?;
    p.send("ack p2").await?;
    p.send("partition [\"p3\"]").await?;
    let pre_again = p.send("pre-commit").await?;
    p.send("commit").await?;
    let near = p.send("deliver p1").await?;
    let far = p.send("deliver p3").await?;
    let mut c = Check::new("a commit broadcast sent across a partition");
    // The coordinator sits with the side the partition did not cut off, so its messages
    // reach two participants and never reach the third.
    c.eq(
        "pre-commit.sent while p3 is cut off",
        2,
        p.expect_i64(&pre_again, "pre-commit", "sent")?,
    );
    c.eq(
        "deliver(p1).state",
        "committed".to_string(),
        p.expect_str(&near, "deliver p1", "state")?,
    );
    c.eq(
        "deliver(p3).state",
        "pre-committed".to_string(),
        p.expect_str(&far, "deliver p3", "state")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(an_ack_without_a_pre_commit_does_not_count, |ctx| {
    let p = ctx.prim("three-phase-commit").await?;
    p.send("init [\"p1\",\"p2\",\"p3\"]").await?;
    for name in ["p1", "p2", "p3"] {
        p.send(&format!("vote {name} yes")).await?;
    }
    p.send("pre-commit").await?;
    p.send("p-precommit p1").await?;
    let one = p.send("ack p1").await?;
    // p2 is still merely prepared; an acknowledgement from it would be an acknowledgement
    // of a message it never received, and counting it would let commit run on one replica.
    let two = p.send("ack p2").await?;
    let commit = p.send("commit").await?;
    let mut c = Check::new("an acknowledgement from a participant that never saw the pre-commit");
    c.eq("ack(p1).acks", 1, p.expect_i64(&one, "ack p1", "acks")?);
    c.eq("ack(p2).acks", 1, p.expect_i64(&two, "ack p2", "acks")?);
    c.eq("commit.ok", false, p.expect_bool(&commit, "commit", "ok")?);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(seeded_runs_never_diverge, |ctx| {
    // Every trial is planned here first. The pre-commit round either reaches everybody or
    // nobody, which is the case the termination rule was designed for; the partition tests
    // above cover the half-delivered case it cannot handle.
    let mut trials: Vec<Trial> = Vec::new();
    for _ in 0..20 {
        trials.push(Trial {
            votes: [
                ctx.rng.random_bool(0.85),
                ctx.rng.random_bool(0.85),
                ctx.rng.random_bool(0.85),
            ],
            complete: ctx.rng.random_bool(0.5),
        });
    }
    let seed = ctx.seed;
    let p = ctx.prim("three-phase-commit").await?;
    let mut observed: Vec<(Vec<String>, bool)> = Vec::with_capacity(trials.len());
    for trial in &trials {
        p.send("init [\"p1\",\"p2\",\"p3\"]").await?;
        for (i, yes) in trial.votes.iter().enumerate() {
            let vote = if *yes { "yes" } else { "no" };
            p.send(&format!("vote p{} {vote}", i + 1)).await?;
        }
        if trial.votes.iter().all(|yes| *yes) {
            p.send("pre-commit").await?;
            if trial.complete {
                for i in 0..3 {
                    p.send(&format!("p-precommit p{}", i + 1)).await?;
                }
            }
        }
        p.send("crash coordinator").await?;
        let mut decisions = Vec::new();
        for i in 0..3 {
            let t = p.send(&format!("p-timeout p{}", i + 1)).await?;
            decisions.push(p.expect_str(&t, "p-timeout", "decision")?);
        }
        let d = p.send("decisions").await?;
        let inconsistent = p.expect_bool(&d, "decisions", "inconsistent")?;
        observed.push((decisions, inconsistent));
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("twenty seeded runs with an all-or-nothing pre-commit round");
    c.note(format!("seed {seed}, {} trials", trials.len()));
    for (t, (trial, (decisions, inconsistent))) in trials.iter().zip(&observed).enumerate() {
        let want = trial.expected().to_string();
        for (i, got) in decisions.iter().enumerate() {
            c.eq(
                &format!("trial[{t}] p{} decision", i + 1),
                want.clone(),
                got.clone(),
            );
        }
        c.eq(&format!("trial[{t}] inconsistent"), false, *inconsistent);
        if !c.ok() {
            break;
        }
    }
    c.block("transcript", transcript);
    c.finish()
});
