//! Stage 60 — Raft membership change.
//!
//! Changing who is in the cluster is the one operation that cannot be done with an ordinary
//! log entry, because the log entry that changes the configuration has to be agreed by the
//! configuration it is changing. Switch straight from C_old to C_new and there is a window
//! in which two disjoint majorities exist at once — one of the old set, one of the new —
//! and two leaders can be elected in the same term. Joint consensus closes that window by
//! requiring, for a while, a majority of *both*.
//!
//! The oracle is §6 transcribed. The tester holds both configurations, works out for itself
//! whether a given set of voters carries a decision in each of them, and compares. The
//! disjoint-majorities scenario has a test of its own, written out with the two sets named,
//! because it is the whole reason the joint phase exists. The single-server alternative gets
//! the same treatment from the other side: one change at a time, proved over an exhaustive
//! sweep of cluster sizes, because it is only safe while the old and new majorities overlap.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;

/// Stage 60.
pub fn stage() -> Stage {
    Stage {
        number: 60,
        slug: "raft_membership",
        name: "Raft membership change",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `raft-membership`: a configuration is a set of voters, and its quorum is \
             len/2 + 1",
            "In the joint phase a decision needs a majority of C_old *and* a majority of C_new",
            "Leaving the joint phase takes two committed steps: C_old,new, then C_new",
            "One change at a time: refuse a second while the first is still uncommitted",
        ],
        examples,
        tests: vec![
            Test::new(
                "a fresh cluster reports its members and its quorum",
                a_fresh_cluster_reports_itself,
            ),
            Test::new(
                "entering the joint configuration names both sides",
                entering_the_joint_phase,
            ),
            Test::new(
                "a majority of the old configuration alone does not decide",
                the_old_side_alone_does_not_decide,
            ),
            Test::new(
                "a majority of the new configuration alone does not decide",
                the_new_side_alone_does_not_decide,
            ),
            Test::new(
                "a majority of both configurations decides",
                both_sides_together_decide,
            ),
            Test::new(
                "leaving the joint configuration takes two committed steps",
                leaving_the_joint_phase,
            ),
            Test::new(
                "a second change while one is in flight is refused",
                one_change_at_a_time,
            ),
            Test::new(
                "adding a member that is already there is an error",
                adding_a_member_twice_is_an_error,
            ),
            Test::new(
                "removing a member that is not there is an error",
                removing_a_stranger_is_an_error,
            ),
            Test::new(
                "a single-server change moves the quorum by at most one",
                a_single_server_change_keeps_the_majorities_overlapping,
            )
            .ext(),
            Test::new(
                "sixty seeded changes never leave two configurations in flight",
                a_long_seeded_reconfiguration,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("Two majorities that never meet", "raft-membership", || {
            lines(&[
                "init [\"s1\",\"s2\",\"s3\"]",
                "joint [\"s3\",\"s4\",\"s5\"]",
                "agree [\"s1\",\"s2\"]",
                "agree [\"s4\",\"s5\"]",
                "agree [\"s1\",\"s2\",\"s3\",\"s4\"]",
            ])
        })
        .request("a cluster moving from {s1,s2,s3} to {s3,s4,s5}, and three sets of voters")
        .response("old only, new only, and finally old and new")
        .note(
            "{s1,s2} is a majority of the old configuration and {s4,s5} of the new, and the \
             two sets are disjoint. Switch directly from C_old to C_new and both could elect \
             a leader in the same term, which is the one thing Raft promises cannot happen. \
             In the joint phase neither carries the decision, so neither can.",
        ),
        prim_example("One change at a time", "raft-membership", || {
            lines(&[
                "init [\"s1\",\"s2\",\"s3\"]",
                "add s4",
                "state",
                "add s5",
                "commit-change",
                "add s5",
                "state",
            ])
        })
        .request("two single-server additions, the second attempted before the first commits")
        .response("the second is refused; after commit-change it is accepted")
        .note(
            "A single-server change is safe on its own because the old and new majorities \
             always overlap — three of four still contains two of three. Two overlapping \
             changes give that up: {s1,s2,s3} to {s1,s2,s3,s4} to {s1,s2,s3,s4,s5} has \
             {s1,s2} deciding for the first and {s3,s4,s5} for the last, and they are \
             disjoint again.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// The tester's own model of §6. Nothing below consults the program.
// ---------------------------------------------------------------------------------------

/// A majority of a configuration.
fn quorum_of(members: &[String]) -> i64 {
    members.len() as i64 / 2 + 1
}

/// A configuration and whatever change is in flight over it.
struct Model {
    phase: &'static str,
    members: Vec<String>,
}

impl Model {
    fn new(members: &[&str]) -> Model {
        Model {
            phase: "stable",
            members: members.iter().map(|m| (*m).to_string()).collect(),
        }
    }

    /// Returns whether the change was accepted.
    fn change(&mut self, add: bool, who: &str) -> bool {
        if self.phase != "stable" {
            return false;
        }
        let present = self.members.iter().any(|m| m == who);
        if add {
            if present {
                return false;
            }
            self.members.push(who.to_string());
            self.members.sort();
        } else {
            if !present || self.members.len() == 1 {
                return false;
            }
            self.members.retain(|m| m != who);
        }
        self.phase = "pending";
        true
    }

    fn commit(&mut self) -> bool {
        if self.phase != "pending" {
            return false;
        }
        self.phase = "stable";
        true
    }
}

/// A member list as the command line wants it: compact JSON, no spaces.
fn members_arg(members: &[&str]) -> String {
    let body: Vec<String> = members.iter().map(|m| format!("\"{m}\"")).collect();
    format!("[{}]", body.join(","))
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(a_fresh_cluster_reports_itself, |ctx| {
    let p = ctx.prim("raft-membership").await?;
    let three = p.send("init [\"s1\",\"s2\",\"s3\"]").await?;
    let state = p.send("state").await?;
    let five = p.send("init [\"a\",\"b\",\"c\",\"d\",\"e\"]").await?;
    let four = p.send("init [\"a\",\"b\",\"c\",\"d\"]").await?;
    let mut c = Check::new("the quorum of a cluster that is not changing");
    c.eq(
        "init.phase",
        "stable".to_string(),
        p.expect_str(&three, "init", "phase")?,
    );
    c.eq("init(3).quorum", 2, p.expect_i64(&three, "init", "quorum")?);
    c.json_eq(
        "state.members",
        &serde_json::json!(["s1", "s2", "s3"]),
        &state["members"],
    );
    c.eq("init(5).quorum", 3, p.expect_i64(&five, "init", "quorum")?);
    // An even cluster buys nothing: four members tolerate one failure, exactly as three do.
    c.eq("init(4).quorum", 3, p.expect_i64(&four, "init", "quorum")?);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(entering_the_joint_phase, |ctx| {
    let p = ctx.prim("raft-membership").await?;
    p.send("init [\"s1\",\"s2\",\"s3\"]").await?;
    let joint = p.send("joint [\"s3\",\"s4\",\"s5\",\"s6\",\"s7\"]").await?;
    let state = p.send("state").await?;
    let mut c = Check::new("a cluster that has proposed C_old,new");
    c.eq(
        "joint.phase",
        "joint".to_string(),
        p.expect_str(&joint, "joint", "phase")?,
    );
    c.json_eq(
        "joint.old",
        &serde_json::json!(["s1", "s2", "s3"]),
        &joint["old"],
    );
    c.json_eq(
        "joint.new",
        &serde_json::json!(["s3", "s4", "s5", "s6", "s7"]),
        &joint["new"],
    );
    // There is no single quorum in the joint phase: each configuration keeps its own, and
    // both have to be satisfied.
    c.eq(
        "joint.quorum_old",
        2,
        p.expect_i64(&joint, "joint", "quorum_old")?,
    );
    c.eq(
        "joint.quorum_new",
        3,
        p.expect_i64(&joint, "joint", "quorum_new")?,
    );
    c.eq(
        "state.phase",
        "joint".to_string(),
        p.expect_str(&state, "state", "phase")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_old_side_alone_does_not_decide, |ctx| {
    let p = ctx.prim("raft-membership").await?;
    p.send("init [\"s1\",\"s2\",\"s3\"]").await?;
    p.send("joint [\"s3\",\"s4\",\"s5\"]").await?;
    let two_old = p.send("agree [\"s1\",\"s2\"]").await?;
    let all_old = p.send("agree [\"s1\",\"s2\",\"s3\"]").await?;
    let mut c = Check::new("voters drawn only from the configuration being left");
    c.eq(
        "agree([s1,s2]).ok",
        false,
        p.expect_bool(&two_old, "agree [s1,s2]", "ok")?,
    );
    c.eq(
        "agree([s1,s2]).reason",
        "old only".to_string(),
        p.expect_str(&two_old, "agree [s1,s2]", "reason")?,
    );
    // Even the whole old configuration is not enough: it holds only one member of the new
    // one, and one of three is not a majority there.
    c.eq(
        "agree([s1,s2,s3]).ok",
        false,
        p.expect_bool(&all_old, "agree [s1,s2,s3]", "ok")?,
    );
    c.eq(
        "agree([s1,s2,s3]).reason",
        "old only".to_string(),
        p.expect_str(&all_old, "agree [s1,s2,s3]", "reason")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_new_side_alone_does_not_decide, |ctx| {
    let p = ctx.prim("raft-membership").await?;
    p.send("init [\"s1\",\"s2\",\"s3\"]").await?;
    p.send("joint [\"s3\",\"s4\",\"s5\"]").await?;
    let two_new = p.send("agree [\"s4\",\"s5\"]").await?;
    let nobody = p.send("agree [\"s5\"]").await?;
    let mut c = Check::new("voters drawn only from the configuration being joined");
    // {s1,s2} and {s4,s5} are disjoint, and each is a majority of one side. This is the
    // pair that could elect two leaders in one term if the switch were direct.
    c.eq(
        "agree([s4,s5]).ok",
        false,
        p.expect_bool(&two_new, "agree [s4,s5]", "ok")?,
    );
    c.eq(
        "agree([s4,s5]).reason",
        "new only".to_string(),
        p.expect_str(&two_new, "agree [s4,s5]", "reason")?,
    );
    c.eq(
        "agree([s5]).ok",
        false,
        p.expect_bool(&nobody, "agree [s5]", "ok")?,
    );
    c.eq(
        "agree([s5]).reason",
        "neither".to_string(),
        p.expect_str(&nobody, "agree [s5]", "reason")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(both_sides_together_decide, |ctx| {
    let p = ctx.prim("raft-membership").await?;
    p.send("init [\"s1\",\"s2\",\"s3\"]").await?;
    p.send("joint [\"s3\",\"s4\",\"s5\"]").await?;
    let four = p.send("agree [\"s1\",\"s2\",\"s3\",\"s4\"]").await?;
    // s3 is in both configurations, so it can be counted on both sides at once.
    let overlap = p.send("agree [\"s1\",\"s3\",\"s4\"]").await?;
    let everyone = p.send("agree [\"s1\",\"s2\",\"s3\",\"s4\",\"s5\"]").await?;
    let mut c = Check::new("voter sets that carry a joint decision");
    c.eq(
        "agree([s1,s2,s3,s4]).ok",
        true,
        p.expect_bool(&four, "agree", "ok")?,
    );
    c.eq(
        "agree([s1,s2,s3,s4]).reason",
        "old and new".to_string(),
        p.expect_str(&four, "agree", "reason")?,
    );
    c.eq(
        "agree([s1,s3,s4]).ok",
        true,
        p.expect_bool(&overlap, "agree", "ok")?,
    );
    c.eq(
        "agree(everyone).ok",
        true,
        p.expect_bool(&everyone, "agree", "ok")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(leaving_the_joint_phase, |ctx| {
    let p = ctx.prim("raft-membership").await?;
    p.send("init [\"s1\",\"s2\",\"s3\"]").await?;
    p.send("joint [\"s3\",\"s4\",\"s5\"]").await?;
    let early = p.send("commit-new").await?;
    let joint_committed = p.send("commit-joint").await?;
    let now = p.send("agree [\"s4\",\"s5\"]").await?;
    let new_committed = p.send("commit-new").await?;
    let state = p.send("state").await?;
    let mut c = Check::new("the two steps out of the joint configuration");
    // C_new cannot be committed before C_old,new is: the whole point of the joint phase is
    // that it is entered and left in order.
    c.eq(
        "commit-new before commit-joint",
        false,
        p.expect_bool(&early, "commit-new", "ok")?,
    );
    c.eq(
        "commit-joint.phase",
        "new".to_string(),
        p.expect_str(&joint_committed, "commit-joint", "phase")?,
    );
    c.json_eq(
        "commit-joint.members",
        &serde_json::json!(["s3", "s4", "s5"]),
        &joint_committed["members"],
    );
    // Once C_old,new is committed the old configuration no longer holds a veto, so the very
    // voters that could not decide a moment ago now can.
    c.eq(
        "agree([s4,s5]).ok after commit-joint",
        true,
        p.expect_bool(&now, "agree", "ok")?,
    );
    c.eq(
        "commit-new.phase",
        "stable".to_string(),
        p.expect_str(&new_committed, "commit-new", "phase")?,
    );
    c.eq("state.quorum", 2, p.expect_i64(&state, "state", "quorum")?);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(one_change_at_a_time, |ctx| {
    let p = ctx.prim("raft-membership").await?;
    p.send("init [\"s1\",\"s2\",\"s3\"]").await?;
    let first = p.send("add s4").await?;
    // A second change while the first is still uncommitted would give up the overlap that
    // makes a single-server change safe at all.
    let second = p.send("add s5").await?;
    let removal = p.send("remove s1").await?;
    let joint_too = p.send("joint [\"s9\"]").await?;
    let committed = p.send("commit-change").await?;
    let allowed = p.send("add s5").await?;
    let mut c = Check::new("changes attempted while another is in flight");
    c.eq(
        "add(s4).phase",
        "pending".to_string(),
        p.expect_str(&first, "add s4", "phase")?,
    );
    c.eq(
        "add(s4).quorum",
        3,
        p.expect_i64(&first, "add s4", "quorum")?,
    );
    c.eq(
        "add(s5) while pending",
        false,
        p.expect_bool(&second, "add s5", "ok")?,
    );
    c.eq(
        "remove(s1) while pending",
        false,
        p.expect_bool(&removal, "remove s1", "ok")?,
    );
    c.eq(
        "joint while pending",
        false,
        p.expect_bool(&joint_too, "joint", "ok")?,
    );
    // Nothing moved while the change was refused.
    c.json_eq(
        "members after the refusals",
        &serde_json::json!(["s1", "s2", "s3", "s4"]),
        &removal["members"],
    );
    c.eq(
        "commit-change.phase",
        "stable".to_string(),
        p.expect_str(&committed, "commit-change", "phase")?,
    );
    c.eq(
        "add(s5) after the commit",
        true,
        p.expect_bool(&allowed, "add s5", "ok")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(adding_a_member_twice_is_an_error, |ctx| {
    let p = ctx.prim("raft-membership").await?;
    p.send("init [\"s1\",\"s2\",\"s3\"]").await?;
    let again = p.send("add s2").await?;
    let state = p.send("state").await?;
    let mut c = Check::new("adding a member the cluster already has");
    // Silently succeeding would leave the caller believing the quorum had moved when it had
    // not, and a retried request must not look like a second member.
    c.eq("add(s2).ok", false, p.expect_bool(&again, "add s2", "ok")?);
    c.eq(
        "add(s2).phase",
        "stable".to_string(),
        p.expect_str(&again, "add s2", "phase")?,
    );
    c.json_eq(
        "state.members",
        &serde_json::json!(["s1", "s2", "s3"]),
        &state["members"],
    );
    c.eq("state.quorum", 2, p.expect_i64(&state, "state", "quorum")?);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(removing_a_stranger_is_an_error, |ctx| {
    let p = ctx.prim("raft-membership").await?;
    p.send("init [\"s1\",\"s2\",\"s3\"]").await?;
    let stranger = p.send("remove s9").await?;
    let state = p.send("state").await?;
    let real = p.send("remove s3").await?;
    let mut c = Check::new("removing a member the cluster does not have");
    c.eq(
        "remove(s9).ok",
        false,
        p.expect_bool(&stranger, "remove s9", "ok")?,
    );
    c.json_eq(
        "state.members",
        &serde_json::json!(["s1", "s2", "s3"]),
        &state["members"],
    );
    c.eq(
        "remove(s3).ok",
        true,
        p.expect_bool(&real, "remove s3", "ok")?,
    );
    // Two members still need two to decide, so removing one from three buys no availability
    // at all — the reason to shrink a cluster is never the quorum.
    c.eq(
        "remove(s3).quorum",
        2,
        p.expect_i64(&real, "remove s3", "quorum")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(
    a_single_server_change_keeps_the_majorities_overlapping,
    |ctx| {
        // A single-server change needs no joint phase because any majority of the old
        // configuration and any majority of the new one must share a member. Sweep every
        // cluster size the exercise cares about and prove it from the quorums the program
        // itself reports.
        let p = ctx.prim("raft-membership").await?;
        let mut rows: Vec<(usize, i64, i64, i64)> = Vec::new();
        for n in 1..=9usize {
            let names: Vec<String> = (1..=n).map(|i| format!("s{i}")).collect();
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            let before = p.send(&format!("init {}", members_arg(&refs))).await?;
            let added = p.send(&format!("add x{n}")).await?;
            p.send("commit-change").await?;
            let removed = p.send(&format!("remove x{n}")).await?;
            rows.push((
                n,
                p.expect_i64(&before, "init", "quorum")?,
                p.expect_i64(&added, "add", "quorum")?,
                p.expect_i64(&removed, "remove", "quorum")?,
            ));
        }
        let transcript = p.transcript_block();
        let mut c = Check::new("the quorum of every cluster size either side of one change");
        for (n, before, after_add, after_remove) in &rows {
            let expected_before = *n as i64 / 2 + 1;
            let expected_add = (*n as i64 + 1) / 2 + 1;
            c.eq(&format!("quorum of {n} members"), expected_before, *before);
            c.eq(
                &format!("quorum of {n} + 1 members"),
                expected_add,
                *after_add,
            );
            c.eq(
                "the quorum after removing the added member",
                expected_before,
                *after_remove,
            );
            // The union of the two configurations has n + 1 members, so two majorities totalling
            // more than n + 1 cannot be disjoint. That inequality is the safety argument.
            c.that(
                &format!("majorities of {n} and {} overlap", n + 1),
                "quorum(old) + quorum(new) > |old union new|",
                expected_before + expected_add > *n as i64 + 1,
                (expected_before, expected_add, *n as i64 + 1),
            );
        }
        c.block("transcript", transcript);
        c.finish()
    }
);

dist_test!(a_long_seeded_reconfiguration, |ctx| {
    // The whole conversation is worked out here first — every command, whether it is
    // accepted and the configuration it leaves behind — by replaying §6 over the seeded
    // plan. The program is then asked the same questions and never consulted about the
    // answers.
    let start = ["s1", "s2", "s3"];
    let pool = ["s1", "s2", "s3", "s4", "s5", "s6"];
    let mut model = Model::new(&start);
    let mut script: Vec<(String, bool, &'static str, Vec<String>)> = Vec::new();
    for _ in 0..60 {
        let who = pool[ctx.rng.random_range(0..pool.len())];
        let (command, ok) = match ctx.rng.random_range(0..5) {
            0 | 1 => (format!("add {who}"), model.change(true, who)),
            2 => (format!("remove {who}"), model.change(false, who)),
            _ => ("commit-change".to_string(), model.commit()),
        };
        script.push((command, ok, model.phase, model.members.clone()));
    }
    let seed = ctx.seed;
    let p = ctx.prim("raft-membership").await?;
    p.send(&format!("init {}", members_arg(&start))).await?;
    let mut actual = Vec::with_capacity(script.len());
    for (command, ..) in &script {
        let r = p.send(command).await?;
        actual.push((
            p.expect_bool(&r, command, "ok")?,
            p.expect_str(&r, command, "phase")?,
            r["members"].clone(),
            p.expect_i64(&r, command, "quorum")?,
        ));
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("sixty seeded membership changes replayed against §6");
    c.note(format!("seed {seed}, {} changes", script.len()));
    for (i, ((command, ok, phase, members), (got_ok, got_phase, got_members, got_quorum))) in
        script.iter().zip(&actual).enumerate()
    {
        c.eq(&format!("step[{i}] {command} → ok"), *ok, *got_ok);
        c.eq(
            &format!("step[{i}] {command} → phase"),
            (*phase).to_string(),
            got_phase.clone(),
        );
        c.json_eq(
            &format!("step[{i}] {command} → members"),
            &serde_json::json!(members),
            got_members,
        );
        c.eq(
            &format!("step[{i}] {command} → quorum"),
            quorum_of(members),
            *got_quorum,
        );
        // Whatever the schedule asked for, there is never more than one change in flight.
        c.that(
            &format!("step[{i}] {command} → one change at a time"),
            "a phase of stable or pending",
            got_phase == "stable" || got_phase == "pending",
            got_phase.clone(),
        );
        if !c.ok() {
            break;
        }
    }
    c.block("transcript", transcript);
    c.finish()
});
