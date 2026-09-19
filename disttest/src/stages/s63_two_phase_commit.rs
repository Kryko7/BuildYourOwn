//! Stage 63 — Two-phase commit.
//!
//! The oldest atomic commit protocol there is, and the one everybody reaches for before
//! learning why it is not enough. A coordinator asks every participant whether it can
//! commit; a participant that answers yes has given up its right to abort and must wait.
//! Both halves are state machines, so the whole protocol fits in one process and the
//! harness drives it by handing it votes, timeouts and a coordinator crash.
//!
//! The oracle is the protocol's own rules. The tester decides for itself what the
//! coordinator may know (one `no` is enough to abort; commit needs every vote), what a
//! participant is allowed to do on its own (abort while working, nothing once prepared),
//! and what asking the peers can and cannot settle. The point of the stage is the window
//! the last test sweeps: a coordinator that dies between the last vote and the broadcast
//! leaves every prepared participant holding locks it cannot release, and no amount of
//! asking around helps.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;

/// Stage 63.
pub fn stage() -> Stage {
    Stage {
        number: 63,
        slug: "two_phase_commit",
        name: "Two-phase commit",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `two-phase-commit`: a coordinator state machine and one per participant",
            "One no is enough to abort; commit needs every vote, so a missing vote decides nothing",
            "A yes vote is a promise: from then on the participant may not decide for itself",
            "Cooperative termination settles some cases and not the one that matters",
        ],
        examples,
        tests: vec![
            Test::new(
                "a fresh transaction has every participant working",
                a_fresh_transaction,
            ),
            Test::new(
                "the coordinator does not decide until every vote is in",
                no_decision_until_every_vote,
            ),
            Test::new("every yes decides commit", every_yes_decides_commit),
            Test::new(
                "one no decides abort whatever the rest say",
                one_no_decides_abort,
            ),
            Test::new(
                "a yes vote leaves the participant prepared",
                a_yes_vote_prepares,
            ),
            Test::new(
                "a participant that has not voted may abort on its own",
                an_unvoted_participant_may_abort,
            ),
            Test::new(
                "a prepared participant is blocked when the coordinator dies",
                the_blocking_window,
            ),
            Test::new(
                "asking the peers settles nothing when every peer is prepared",
                consulting_peers_does_not_help,
            ),
            Test::new(
                "a peer that already decided settles the question",
                a_decided_peer_settles_it,
            ),
            Test::new(
                "a peer that never voted makes abort safe",
                an_unvoted_peer_makes_abort_safe,
            ),
            Test::new(
                "delivering the decision twice changes nothing",
                delivery_is_idempotent,
            )
            .ext(),
            Test::new(
                "a unilateral abort makes commit impossible for ever",
                a_unilateral_abort_rules_out_commit,
            )
            .ext(),
            Test::new(
                "seeded vote orders and crash points never split the participants",
                seeded_runs_never_split_the_participants,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("The blocking window", "two-phase-commit", || {
            lines(&[
                "init [\"p1\",\"p2\"]",
                "prepare",
                "vote p1 yes",
                "vote p2 yes",
                "crash coordinator",
                "p-timeout p1",
                "p-consult p1",
            ])
        })
        .request(
            "two participants that both voted yes, and a coordinator that dies before it speaks",
        )
        .response("blocked twice: once on its own timer, and again after asking its peer")
        .note(
            "The decision exists — every vote was yes — and it is unreachable. Both \
             participants are holding their locks, neither may abort, and the only cure is \
             the coordinator coming back. This is the window that makes two-phase commit a \
             blocking protocol, and it is why stage 64 adds a round.",
        ),
        prim_example("One no aborts the lot", "two-phase-commit", || {
            lines(&[
                "init [\"p1\",\"p2\",\"p3\"]",
                "prepare",
                "vote p2 no",
                "vote p1 yes",
                "vote p3 yes",
                "decide",
                "deliver p1",
            ])
        })
        .request("one refusal, then two agreements, then the broadcast")
        .response("abort as soon as the no lands, and the later yes votes change nothing")
        .note(
            "Abort needs one vote; commit needs all of them. An implementation that waits \
             for every vote before deciding abort is not wrong, but one that lets a later \
             yes overturn an abort has lost atomicity outright.",
        ),
    ]
}

/// How one seeded trial is driven: who voted what, in which order, and what the
/// coordinator managed to do before it died.
struct Trial {
    /// True where that participant voted yes.
    votes: [bool; 3],
    /// The order the votes arrive in.
    order: Vec<usize>,
    /// The coordinator dies before it can broadcast anything.
    crash: bool,
    /// Which participants the broadcast reached, when there was one.
    delivered: Vec<usize>,
}

impl Trial {
    /// The state every participant must end in, worked out from the rules alone.
    ///
    /// Abort needs one no; commit needs every vote and a coordinator alive long enough to
    /// say so. Once any participant has settled, cooperative termination carries that
    /// decision to the rest — and when none has, they are all stuck.
    fn expected_state(&self) -> &'static str {
        let any_no = self.votes.iter().any(|yes| !yes);
        let anyone_settled = any_no || (!self.crash && !self.delivered.is_empty());
        if !anyone_settled {
            return "prepared";
        }
        if any_no {
            "aborted"
        } else {
            "committed"
        }
    }
}

dist_test!(a_fresh_transaction, |ctx| {
    let p = ctx.prim("two-phase-commit").await?;
    let init = p.send("init [\"p1\",\"p2\",\"p3\"]").await?;
    let names = p.expect_strs(&init, "init", "participants")?;
    let s = p.send("state").await?;
    let p1 = p.send("p-state p1").await?;
    let mut c = Check::new("a transaction nobody has voted on");
    c.eq(
        "init.participants",
        vec!["p1".to_string(), "p2".into(), "p3".into()],
        names,
    );
    c.eq(
        "state.state",
        "init".to_string(),
        p.expect_str(&s, "state", "state")?,
    );
    c.eq("state.decision", &serde_json::Value::Null, &s["decision"]);
    c.eq(
        "state.coordinator",
        "up".to_string(),
        p.expect_str(&s, "state", "coordinator")?,
    );
    c.eq(
        "p-state(p1).state",
        "working".to_string(),
        p.expect_str(&p1, "p-state p1", "state")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(no_decision_until_every_vote, |ctx| {
    let p = ctx.prim("two-phase-commit").await?;
    p.send("init [\"p1\",\"p2\",\"p3\"]").await?;
    p.send("prepare").await?;
    p.send("vote p1 yes").await?;
    let two = p.send("vote p2 yes").await?;
    // Two of three have agreed, which tells the coordinator nothing: the third may still
    // refuse, and a commit broadcast now could never be taken back.
    let early = p.send("decide").await?;
    let three = p.send("vote p3 yes").await?;
    let late = p.send("decide").await?;
    let mut c = Check::new("a coordinator asked to decide before the last vote arrived");
    c.eq(
        "vote(p2).decision",
        &serde_json::Value::Null,
        &two["decision"],
    );
    c.eq(
        "the early decide's decision",
        &serde_json::Value::Null,
        &early["decision"],
    );
    c.eq(
        "the early decide's sent",
        0,
        p.expect_i64(&early, "decide", "sent")?,
    );
    c.eq(
        "vote(p3).decision",
        "commit".to_string(),
        p.expect_str(&three, "vote p3 yes", "decision")?,
    );
    c.eq(
        "the later decide's decision",
        "commit".to_string(),
        p.expect_str(&late, "decide", "decision")?,
    );
    c.eq(
        "the later decide's sent",
        3,
        p.expect_i64(&late, "decide", "sent")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(every_yes_decides_commit, |ctx| {
    let p = ctx.prim("two-phase-commit").await?;
    p.send("init [\"p1\",\"p2\"]").await?;
    p.send("prepare").await?;
    p.send("vote p1 yes").await?;
    p.send("vote p2 yes").await?;
    p.send("decide").await?;
    let d1 = p.send("deliver p1").await?;
    let d2 = p.send("deliver p2").await?;
    let s = p.send("state").await?;
    let mut c = Check::new("a transaction every participant agreed to");
    c.eq(
        "state.state",
        "committed".to_string(),
        p.expect_str(&s, "state", "state")?,
    );
    c.eq(
        "state.decision",
        "commit".to_string(),
        p.expect_str(&s, "state", "decision")?,
    );
    c.eq(
        "deliver(p1).state",
        "committed".to_string(),
        p.expect_str(&d1, "deliver p1", "state")?,
    );
    c.eq(
        "deliver(p2).state",
        "committed".to_string(),
        p.expect_str(&d2, "deliver p2", "state")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(one_no_decides_abort, |ctx| {
    let p = ctx.prim("two-phase-commit").await?;
    p.send("init [\"p1\",\"p2\",\"p3\"]").await?;
    p.send("prepare").await?;
    let refused = p.send("vote p2 no").await?;
    // The two yes votes arrive after the abort is already settled. Letting them overturn it
    // would commit a transaction one participant has already rolled back.
    p.send("vote p1 yes").await?;
    let last = p.send("vote p3 yes").await?;
    let decide = p.send("decide").await?;
    let d1 = p.send("deliver p1").await?;
    let mut c = Check::new("a transaction one participant refused");
    c.eq(
        "vote(p2, no).decision",
        "abort".to_string(),
        p.expect_str(&refused, "vote p2 no", "decision")?,
    );
    c.eq(
        "vote(p2, no).state",
        "aborted".to_string(),
        p.expect_str(&refused, "vote p2 no", "state")?,
    );
    c.eq(
        "the decision after the later yes votes",
        "abort".to_string(),
        p.expect_str(&last, "vote p3 yes", "decision")?,
    );
    c.eq(
        "decide.decision",
        "abort".to_string(),
        p.expect_str(&decide, "decide", "decision")?,
    );
    c.eq(
        "deliver(p1).state",
        "aborted".to_string(),
        p.expect_str(&d1, "deliver p1", "state")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_yes_vote_prepares, |ctx| {
    let p = ctx.prim("two-phase-commit").await?;
    p.send("init [\"p1\",\"p2\"]").await?;
    p.send("prepare").await?;
    p.send("vote p1 yes").await?;
    let yes = p.send("p-state p1").await?;
    p.send("vote p2 no").await?;
    let no = p.send("p-state p2").await?;
    let mut c = Check::new("what a vote does to the participant that cast it");
    // Voting no costs nothing: the participant may forget the transaction at once. Voting
    // yes is the promise that the rest of the protocol rests on.
    c.eq(
        "p-state(p1).state after a yes",
        "prepared".to_string(),
        p.expect_str(&yes, "p-state p1", "state")?,
    );
    c.eq(
        "p-state(p2).state after a no",
        "aborted".to_string(),
        p.expect_str(&no, "p-state p2", "state")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(an_unvoted_participant_may_abort, |ctx| {
    let p = ctx.prim("two-phase-commit").await?;
    p.send("init [\"p1\",\"p2\"]").await?;
    p.send("prepare").await?;
    p.send("vote p1 yes").await?;
    let timeout = p.send("p-timeout p2").await?;
    let state = p.send("p-state p2").await?;
    let mut c = Check::new("a participant whose prepare never arrived");
    c.eq(
        "p-timeout(p2).decision",
        "abort".to_string(),
        p.expect_str(&timeout, "p-timeout p2", "decision")?,
    );
    c.eq(
        "p-timeout(p2).blocked",
        false,
        p.expect_bool(&timeout, "p-timeout p2", "blocked")?,
    );
    c.eq(
        "p-timeout(p2).reason",
        "has not voted".to_string(),
        p.expect_str(&timeout, "p-timeout p2", "reason")?,
    );
    c.eq(
        "p-state(p2).state",
        "aborted".to_string(),
        p.expect_str(&state, "p-state p2", "state")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_blocking_window, |ctx| {
    let p = ctx.prim("two-phase-commit").await?;
    p.send("init [\"p1\",\"p2\",\"p3\"]").await?;
    p.send("prepare").await?;
    for name in ["p1", "p2", "p3"] {
        p.send(&format!("vote {name} yes")).await?;
    }
    // Every vote is in and the decision is commit, so the coordinator knows the answer —
    // and then it dies before putting it on the wire. Nobody else can ever learn it.
    p.send("crash coordinator").await?;
    let decide = p.send("decide").await?;
    let mut timeouts = Vec::new();
    for name in ["p1", "p2", "p3"] {
        let t = p.send(&format!("p-timeout {name}")).await?;
        timeouts.push((
            p.expect_bool(&t, "p-timeout", "blocked")?,
            p.expect_str(&t, "p-timeout", "reason")?,
        ));
    }
    let s = p.send("p-state p1").await?;
    let mut c = Check::new("a coordinator that died between the last vote and the broadcast");
    c.eq(
        "decide.sent after the crash",
        0,
        p.expect_i64(&decide, "decide", "sent")?,
    );
    c.eq(
        "decide.decision after the crash",
        &serde_json::Value::Null,
        &decide["decision"],
    );
    for (i, (blocked, reason)) in timeouts.iter().enumerate() {
        c.eq(&format!("p-timeout(p{}).blocked", i + 1), true, *blocked);
        c.eq(
            &format!("p-timeout(p{}).reason", i + 1),
            "prepared, waiting for the coordinator".to_string(),
            reason.clone(),
        );
    }
    // The locks stay held. That is the cost, and it is unbounded.
    c.eq(
        "p-state(p1).state",
        "prepared".to_string(),
        p.expect_str(&s, "p-state p1", "state")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(consulting_peers_does_not_help, |ctx| {
    let p = ctx.prim("two-phase-commit").await?;
    p.send("init [\"p1\",\"p2\",\"p3\"]").await?;
    p.send("prepare").await?;
    for name in ["p1", "p2", "p3"] {
        p.send(&format!("vote {name} yes")).await?;
    }
    p.send("crash coordinator").await?;
    let mut answers = Vec::new();
    for name in ["p1", "p2", "p3"] {
        let r = p.send(&format!("p-consult {name}")).await?;
        answers.push((
            p.expect_bool(&r, "p-consult", "blocked")?,
            p.expect_str(&r, "p-consult", "reason")?,
        ));
    }
    let mut c = Check::new("cooperative termination with every peer prepared");
    // Every peer is in exactly the same position, so the round of questions adds no
    // information at all. Two-phase commit blocks, and asking nicely does not change that.
    for (i, (blocked, reason)) in answers.iter().enumerate() {
        c.eq(&format!("p-consult(p{}).blocked", i + 1), true, *blocked);
        c.eq(
            &format!("p-consult(p{}).reason", i + 1),
            "every peer is prepared".to_string(),
            reason.clone(),
        );
    }
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_decided_peer_settles_it, |ctx| {
    let p = ctx.prim("two-phase-commit").await?;
    p.send("init [\"p1\",\"p2\",\"p3\"]").await?;
    p.send("prepare").await?;
    for name in ["p1", "p2", "p3"] {
        p.send(&format!("vote {name} yes")).await?;
    }
    p.send("decide").await?;
    // Exactly one participant heard the broadcast before the coordinator went away.
    p.send("deliver p1").await?;
    p.send("crash coordinator").await?;
    let consult = p.send("p-consult p2").await?;
    let state = p.send("p-state p2").await?;
    let mut c = Check::new("one participant that heard the decision, and one that did not");
    c.eq(
        "p-consult(p2).decision",
        "commit".to_string(),
        p.expect_str(&consult, "p-consult p2", "decision")?,
    );
    c.eq(
        "p-consult(p2).blocked",
        false,
        p.expect_bool(&consult, "p-consult p2", "blocked")?,
    );
    c.eq(
        "p-consult(p2).reason",
        "a peer already decided".to_string(),
        p.expect_str(&consult, "p-consult p2", "reason")?,
    );
    c.eq(
        "p-state(p2).state",
        "committed".to_string(),
        p.expect_str(&state, "p-state p2", "state")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(an_unvoted_peer_makes_abort_safe, |ctx| {
    let p = ctx.prim("two-phase-commit").await?;
    p.send("init [\"p1\",\"p2\",\"p3\"]").await?;
    p.send("prepare").await?;
    p.send("vote p1 yes").await?;
    p.send("vote p2 yes").await?;
    p.send("crash coordinator").await?;
    // p3 never voted, so the coordinator cannot have seen every yes, so it cannot have
    // decided commit. That is enough for p1 to abort without hearing from anyone.
    let consult = p.send("p-consult p1").await?;
    let state = p.send("p-state p1").await?;
    let mut c = Check::new("cooperative termination with one peer still working");
    c.eq(
        "p-consult(p1).decision",
        "abort".to_string(),
        p.expect_str(&consult, "p-consult p1", "decision")?,
    );
    c.eq(
        "p-consult(p1).blocked",
        false,
        p.expect_bool(&consult, "p-consult p1", "blocked")?,
    );
    c.eq(
        "p-consult(p1).reason",
        "a peer never voted".to_string(),
        p.expect_str(&consult, "p-consult p1", "reason")?,
    );
    c.eq(
        "p-state(p1).state",
        "aborted".to_string(),
        p.expect_str(&state, "p-state p1", "state")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(delivery_is_idempotent, |ctx| {
    let p = ctx.prim("two-phase-commit").await?;
    p.send("init [\"p1\",\"p2\"]").await?;
    p.send("prepare").await?;
    p.send("vote p1 yes").await?;
    p.send("vote p2 yes").await?;
    p.send("decide").await?;
    let mut states = Vec::new();
    for _ in 0..3 {
        let d = p.send("deliver p1").await?;
        states.push(p.expect_str(&d, "deliver p1", "state")?);
    }
    // A decision broadcast is retried until it is acknowledged, so a participant sees it
    // more than once as a matter of course.
    let timeout = p.send("p-timeout p1").await?;
    let mut c = Check::new("the same decision delivered three times");
    c.eq(
        "the three deliveries",
        vec![
            "committed".to_string(),
            "committed".into(),
            "committed".into(),
        ],
        states,
    );
    c.eq(
        "p-timeout(p1).decision once it has committed",
        "commit".to_string(),
        p.expect_str(&timeout, "p-timeout p1", "decision")?,
    );
    c.eq(
        "p-timeout(p1).blocked",
        false,
        p.expect_bool(&timeout, "p-timeout p1", "blocked")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_unilateral_abort_rules_out_commit, |ctx| {
    let p = ctx.prim("two-phase-commit").await?;
    p.send("init [\"p1\",\"p2\"]").await?;
    p.send("prepare").await?;
    // p2 gives up before the prepare reaches it, which is the same as answering no: from
    // here the coordinator can never assemble a full set of yes votes.
    p.send("p-timeout p2").await?;
    let vote = p.send("vote p1 yes").await?;
    let decide = p.send("decide").await?;
    let s = p.send("state").await?;
    let mut c = Check::new("a participant that aborted before it was ever asked");
    c.eq(
        "the decision once p1 has agreed",
        "abort".to_string(),
        p.expect_str(&vote, "vote p1 yes", "decision")?,
    );
    c.eq(
        "decide.decision",
        "abort".to_string(),
        p.expect_str(&decide, "decide", "decision")?,
    );
    c.eq(
        "state.votes.p2",
        "no".to_string(),
        p.expect_str(&s["votes"], "state", "p2")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(seeded_runs_never_split_the_participants, |ctx| {
    // Every trial is planned here first — the votes, the order they arrive in, whether the
    // coordinator survives to broadcast, and who the broadcast reached — and the state each
    // participant must end in is worked out from the protocol's rules, not from the program.
    let mut trials: Vec<Trial> = Vec::new();
    for _ in 0..24 {
        let votes = [
            ctx.rng.random_bool(0.75),
            ctx.rng.random_bool(0.75),
            ctx.rng.random_bool(0.75),
        ];
        let mut order: Vec<usize> = (0..3).collect();
        for i in (1..order.len()).rev() {
            let j = ctx.rng.random_range(0..=i);
            order.swap(i, j);
        }
        let crash = ctx.rng.random_bool(0.4);
        let delivered: Vec<usize> = (0..3).filter(|_| ctx.rng.random_bool(0.6)).collect();
        trials.push(Trial {
            votes,
            order,
            crash,
            delivered,
        });
    }
    let seed = ctx.seed;
    let p = ctx.prim("two-phase-commit").await?;
    let mut observed: Vec<Vec<String>> = Vec::with_capacity(trials.len());
    for trial in &trials {
        p.send("init [\"p1\",\"p2\",\"p3\"]").await?;
        p.send("prepare").await?;
        for i in &trial.order {
            let vote = if trial.votes[*i] { "yes" } else { "no" };
            p.send(&format!("vote p{} {vote}", i + 1)).await?;
        }
        if trial.crash {
            p.send("crash coordinator").await?;
        } else {
            p.send("decide").await?;
            for i in &trial.delivered {
                p.send(&format!("deliver p{}", i + 1)).await?;
            }
            p.send("crash coordinator").await?;
        }
        for i in 0..3 {
            p.send(&format!("p-consult p{}", i + 1)).await?;
        }
        let mut states = Vec::new();
        for i in 0..3 {
            let s = p.send(&format!("p-state p{}", i + 1)).await?;
            states.push(p.expect_str(&s, "p-state", "state")?);
        }
        observed.push(states);
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("twenty-four seeded runs of the whole protocol");
    c.note(format!("seed {seed}, {} trials", trials.len()));
    for (t, (trial, states)) in trials.iter().zip(&observed).enumerate() {
        let want = trial.expected_state().to_string();
        for (i, got) in states.iter().enumerate() {
            c.eq(
                &format!("trial[{t}] p{} final state", i + 1),
                want.clone(),
                got.clone(),
            );
        }
        // Atomicity, stated directly: whatever happened, no two participants may disagree.
        c.that(
            &format!("trial[{t}] agreement"),
            "every participant in the same state",
            states.iter().all(|s| *s == states[0]),
            states.clone(),
        );
        if !c.ok() {
            break;
        }
    }
    c.block("transcript", transcript);
    c.finish()
});
