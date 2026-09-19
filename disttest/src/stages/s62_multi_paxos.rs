//! Stage 62 — Multi-Paxos and a stable leader.
//!
//! Single-decree Paxos agrees one value in two round trips and can be starved for ever by a
//! rival proposer. Multi-Paxos fixes both with one idea: run phase one **once**, for every
//! slot at the same time. A proposer that has won a majority of promises at ballot `n` may
//! then skip straight to phase two for slot after slot, at one round trip each, until
//! somebody preempts it — and a proposer that stays up is a leader, whatever the paper
//! calls it. Raft starts from the other end and makes leadership the primitive, which is
//! why the two protocols end up so close together.
//!
//! The oracle is the tester's own encoding of those rules. It keeps each acceptor's
//! promised ballot, the leader's ballot and every slot's acceptance set itself, and asks
//! the program only the questions it has already worked out the answers to. The two traps
//! the stage exists for are both here: preemption, which silently doubles the cost of every
//! proposal from then on, and a **gap** in the slot sequence, which stops the state machine
//! dead however many later slots have been chosen.

use crate::assert::{Check, Failure};
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::prim::PrimProc;
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Stage 62.
pub fn stage() -> Stage {
    Stage {
        number: 62,
        slug: "multi_paxos",
        name: "Multi-Paxos and a stable leader",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `multi-paxos`: one ballot, every slot — phase one is run once, not per slot",
            "While a proposer holds a majority of promises, every proposal costs one round \
             trip; without them it costs two",
            "A higher ballot strips leadership but never un-chooses a slot that was settled",
            "The state machine applies a contiguous prefix: a gap blocks every slot above it",
        ],
        examples,
        tests: vec![
            Test::new(
                "a fresh instance has no leader and every slot needs phase one",
                a_fresh_instance_has_no_leader,
            ),
            Test::new(
                "a proposal without leadership costs two round trips",
                a_proposal_without_leadership_costs_two,
            ),
            Test::new(
                "one prepare turns every slot into a single accept round",
                one_prepare_covers_every_slot,
            ),
            Test::new(
                "a stable leader commits in one round trip",
                a_stable_leader_commits_in_one,
            ),
            Test::new(
                "a prepare that wins no majority grants no leadership",
                a_losing_prepare_grants_nothing,
            ),
            Test::new(
                "a slot is chosen once a majority has accepted it",
                a_majority_chooses_a_slot,
            ),
            Test::new(
                "an acceptor accepting the same slot twice counts once",
                a_duplicate_acceptance_counts_once,
            ),
            Test::new(
                "a higher ballot sends the leader back to phase one",
                preemption_restores_phase_one,
            ),
            Test::new(
                "a lower ballot does not preempt the leader",
                a_lower_ballot_does_not_preempt,
            ),
            Test::new(
                "preemption does not un-choose what was already chosen",
                preemption_does_not_unchoose,
            ),
            Test::new(
                "the state machine cannot apply past a gap",
                a_gap_blocks_the_state_machine,
            ),
            Test::new(
                "re-proposing a chosen slot cannot change its value",
                a_chosen_slot_keeps_its_value,
            ),
            Test::new(
                "ten slots under one leader cost ten round trips, not twenty",
                leadership_halves_the_cost,
            )
            .ext(),
            Test::new(
                "seventy seeded ballots and acceptances match the tester's own replay",
                a_long_seeded_run,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("One prepare, then slot after slot", "multi-paxos", || {
            lines(&[
                "init 3",
                "phase 1",
                "propose 1 alpha",
                "prepare 4",
                "phase 1",
                "phase 900",
                "propose 1 alpha",
            ])
        })
        .request("a proposal before phase one, then a prepare, then the same proposal again")
        .response("two round trips and a refusal, then `accept` for every slot and one round trip")
        .note(
            "`phase 900` is the point of the whole stage: a single prepare at ballot 4 \
             covers slots nobody has thought of yet, because the promise an acceptor makes \
             is about the ballot, not about any one decree. That is what turns Paxos from \
             two round trips per value into one.",
        ),
        prim_example(
            "A gap the state machine cannot cross",
            "multi-paxos",
            || {
                lines(&[
                    "init 3",
                    "prepare 1",
                    "propose 1 alpha",
                    "accepted 1 a1",
                    "accepted 1 a2",
                    "propose 3 gamma",
                    "accepted 3 a1",
                    "accepted 3 a2",
                    "applied",
                ])
            },
        )
        .request("slots 1 and 3 chosen, with slot 2 left open")
        .response("`applied` is 1, with slot 2 named as the gap")
        .note(
            "Multi-Paxos chooses slots independently, so holes are normal and a leader has \
             to go back and fill them before anything above can be applied. Raft's \
             AppendEntries consistency check removes the problem by making a hole \
             impossible in the first place, which is most of why Raft is easier to hold in \
             your head.",
        ),
        prim_example("Preemption doubles the cost again", "multi-paxos", || {
            lines(&[
                "init 3",
                "prepare 5",
                "propose 1 alpha",
                "preempt 9",
                "phase 2",
                "propose 2 beta",
            ])
        })
        .request("a leader at ballot 5, then a rival's ballot 9, then another proposal")
        .response("leadership lost, the slot back in `prepare`, and two round trips again")
        .note(
            "Nothing was corrupted and nothing was un-chosen — the only casualty is \
             latency. A system whose leader is preempted repeatedly is correct and slow, \
             which is exactly the failure mode that makes leader stability a production \
             concern rather than a theoretical one.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// The tester's own model of Multi-Paxos. Nothing below consults the program.
// ---------------------------------------------------------------------------------------

/// One slot of the replicated log, as the tester tracks it.
#[derive(Clone, Default)]
struct Slot {
    value: Option<String>,
    ballot: i64,
    accepted_by: BTreeSet<String>,
    chosen: bool,
}

/// The acceptors, the leader's ballot and the slots.
struct Model {
    promised: Vec<i64>,
    ballot: i64,
    leader: bool,
    ever_leader: bool,
    slots: BTreeMap<i64, Slot>,
}

impl Model {
    fn new(count: usize) -> Model {
        Model {
            promised: vec![0; count],
            ballot: 0,
            leader: false,
            ever_leader: false,
            slots: BTreeMap::new(),
        }
    }

    fn majority(&self) -> usize {
        self.promised.len() / 2 + 1
    }

    /// Returns what a prepare must answer: `(promised, leader)`.
    fn prepare(&mut self, n: i64) -> (usize, bool) {
        let majority = self.majority();
        let mut promised = 0usize;
        for p in &mut self.promised {
            if n > *p {
                *p = n;
                promised += 1;
            }
        }
        if promised >= majority {
            self.leader = true;
            self.ever_leader = true;
            self.ballot = n;
        }
        (promised, self.leader)
    }

    /// Returns what a preempt must answer: `(leader, promised_n)`.
    fn preempt(&mut self, n: i64) -> (bool, i64) {
        for p in &mut self.promised {
            if n > *p {
                *p = n;
            }
        }
        if n > self.ballot {
            self.leader = false;
        }
        (
            self.leader,
            self.promised.iter().copied().max().unwrap_or(0),
        )
    }

    /// Returns what a propose must answer: `(ok, round_trips)`.
    fn propose(&mut self, slot: i64, value: &str) -> (bool, i64) {
        if !self.leader {
            return (false, 2);
        }
        let ballot = self.ballot;
        let entry = self.slots.entry(slot).or_default();
        if !entry.chosen {
            if entry.ballot != ballot {
                entry.accepted_by.clear();
                entry.ballot = ballot;
            }
            entry.value = Some(value.to_string());
        }
        (true, 1)
    }

    /// Returns what an acceptance must answer: `(chosen, count)`.
    fn accepted(&mut self, slot: i64, who: &str) -> (bool, usize) {
        let majority = self.majority();
        let Some(s) = self.slots.get_mut(&slot) else {
            return (false, 0);
        };
        s.accepted_by.insert(who.to_string());
        if s.accepted_by.len() >= majority {
            s.chosen = true;
        }
        (s.chosen, s.accepted_by.len())
    }

    /// True when this slot has a proposal an acceptance could refer to.
    fn is_open(&self, slot: i64) -> bool {
        self.slots.get(&slot).is_some_and(|s| s.value.is_some())
    }

    /// The contiguous prefix the state machine may apply, and the holes below the highest
    /// chosen slot.
    fn progress(&self) -> (i64, Vec<i64>) {
        let is_chosen = |i: i64| self.slots.get(&i).is_some_and(|s| s.chosen);
        let highest = self
            .slots
            .iter()
            .filter(|(_, s)| s.chosen)
            .map(|(k, _)| *k)
            .max()
            .unwrap_or(0);
        let mut index = 0;
        while is_chosen(index + 1) {
            index += 1;
        }
        (index, (1..=highest).filter(|i| !is_chosen(*i)).collect())
    }
}

/// What one scripted command must answer.
enum Expect {
    Prepare { promised: usize, leader: bool },
    Preempt { leader: bool, promised_n: i64 },
    Propose { ok: bool, round_trips: i64 },
    Accepted { chosen: bool, count: usize },
    Applied { index: i64, gaps: Vec<i64> },
}

/// Read a named array of whole numbers out of an answer, tolerating either JSON shape.
fn int_list(p: &PrimProc, v: &Value, command: &str, field: &str) -> Result<Vec<i64>, Failure> {
    match v.get(field) {
        Some(Value::Array(items)) => items
            .iter()
            .map(|x| match x {
                Value::Number(n) => n.as_i64().ok_or_else(|| {
                    p.shape(command, &format!("{field} holds {n}, not a whole number"))
                }),
                Value::String(s) => s.parse().map_err(|_| {
                    p.shape(command, &format!("{field} holds {s:?}, not a whole number"))
                }),
                other => Err(p.shape(
                    command,
                    &format!("{field} holds {other}, not a whole number"),
                )),
            })
            .collect(),
        Some(other) => Err(p.shape(command, &format!("{field} should be an array, got {other}"))),
        None => Err(p.shape(command, &format!("the answer has no {field:?} field"))),
    }
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(a_fresh_instance_has_no_leader, |ctx| {
    let p = ctx.prim("multi-paxos").await?;
    let init = p.send("init 3").await?;
    let phase = p.send("phase 1").await?;
    let far = p.send("phase 4242").await?;
    let state = p.send("state").await?;
    let mut c = Check::new("an instance where phase one has never run");
    c.eq(
        "init.acceptors",
        3,
        p.expect_i64(&init, "init 3", "acceptors")?,
    );
    c.eq(
        "init.majority",
        2,
        p.expect_i64(&init, "init 3", "majority")?,
    );
    c.eq(
        "phase(1).phase",
        "prepare".to_string(),
        p.expect_str(&phase, "phase 1", "phase")?,
    );
    c.eq(
        "phase(1).reason",
        "no leader".to_string(),
        p.expect_str(&phase, "phase 1", "reason")?,
    );
    // A slot nobody has touched is in exactly the same position as slot 1: the answer is
    // about the ballot, not about the decree.
    c.eq(
        "phase(4242).phase",
        "prepare".to_string(),
        p.expect_str(&far, "phase 4242", "phase")?,
    );
    c.eq(
        "state.leader",
        false,
        p.expect_bool(&state, "state", "leader")?,
    );
    c.eq("state.n", 0, p.expect_i64(&state, "state", "n")?);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_proposal_without_leadership_costs_two, |ctx| {
    let p = ctx.prim("multi-paxos").await?;
    p.send("init 3").await?;
    let r = p.send("propose 1 alpha").await?;
    let chosen = p.send("chosen 1").await?;
    let mut c = Check::new("a proposal made without a majority of promises");
    c.eq(
        "propose.ok",
        false,
        p.expect_bool(&r, "propose 1 alpha", "ok")?,
    );
    // Two round trips is the honest accounting: this proposer still has phase one to run.
    c.eq(
        "propose.round_trips",
        2,
        p.expect_i64(&r, "propose 1 alpha", "round_trips")?,
    );
    c.eq(
        "propose.phase",
        "prepare".to_string(),
        p.expect_str(&r, "propose 1 alpha", "phase")?,
    );
    c.eq(
        "chosen(1).chosen",
        false,
        p.expect_bool(&chosen, "chosen 1", "chosen")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(one_prepare_covers_every_slot, |ctx| {
    let p = ctx.prim("multi-paxos").await?;
    p.send("init 5").await?;
    let prepared = p.send("prepare 4").await?;
    let mut phases = Vec::new();
    for slot in [1, 2, 7, 99, 100_000] {
        let r = p.send(&format!("phase {slot}")).await?;
        phases.push((
            p.expect_str(&r, "phase", "phase")?,
            p.expect_str(&r, "phase", "reason")?,
        ));
    }
    let mut c = Check::new("every slot's phase after a single prepare");
    c.eq(
        "prepare(4).promised",
        5,
        p.expect_i64(&prepared, "prepare 4", "promised")?,
    );
    c.eq(
        "prepare(4).leader",
        true,
        p.expect_bool(&prepared, "prepare 4", "leader")?,
    );
    // One prepare, and slot 100000 is already in phase two. An implementation that runs
    // phase one per slot is not wrong, only twice as slow for ever.
    for (slot, (phase, reason)) in [1, 2, 7, 99, 100_000].iter().zip(&phases) {
        c.eq(
            &format!("phase({slot}).phase"),
            "accept".to_string(),
            phase.clone(),
        );
        c.eq(
            &format!("phase({slot}).reason"),
            "stable leader".to_string(),
            reason.clone(),
        );
    }
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_stable_leader_commits_in_one, |ctx| {
    let p = ctx.prim("multi-paxos").await?;
    p.send("init 3").await?;
    p.send("prepare 2").await?;
    let r = p.send("propose 1 alpha").await?;
    let again = p.send("propose 2 beta").await?;
    let mut c = Check::new("two proposals made by an established leader");
    c.eq(
        "propose(1).ok",
        true,
        p.expect_bool(&r, "propose 1 alpha", "ok")?,
    );
    c.eq(
        "propose(1).round_trips",
        1,
        p.expect_i64(&r, "propose 1 alpha", "round_trips")?,
    );
    c.eq(
        "propose(1).phase",
        "accept".to_string(),
        p.expect_str(&r, "propose 1 alpha", "phase")?,
    );
    c.eq(
        "propose(2).round_trips",
        1,
        p.expect_i64(&again, "propose 2 beta", "round_trips")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_losing_prepare_grants_nothing, |ctx| {
    let p = ctx.prim("multi-paxos").await?;
    p.send("init 5").await?;
    // A rival has already taken ballot 9 everywhere.
    p.send("preempt 9").await?;
    let lost = p.send("prepare 4").await?;
    let phase = p.send("phase 1").await?;
    let won = p.send("prepare 11").await?;
    let mut c = Check::new("a prepare at a ballot the acceptors have moved past");
    c.eq(
        "prepare(4).promised",
        0,
        p.expect_i64(&lost, "prepare 4", "promised")?,
    );
    c.eq(
        "prepare(4).leader",
        false,
        p.expect_bool(&lost, "prepare 4", "leader")?,
    );
    c.eq(
        "phase(1).phase",
        "prepare".to_string(),
        p.expect_str(&phase, "phase 1", "phase")?,
    );
    // Going above the rival is the only way back in, which is the same escalation that
    // makes two duelling proposers livelock in stage 61.
    c.eq(
        "prepare(11).promised",
        5,
        p.expect_i64(&won, "prepare 11", "promised")?,
    );
    c.eq(
        "prepare(11).leader",
        true,
        p.expect_bool(&won, "prepare 11", "leader")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_majority_chooses_a_slot, |ctx| {
    let p = ctx.prim("multi-paxos").await?;
    p.send("init 5").await?;
    p.send("prepare 1").await?;
    p.send("propose 7 gamma").await?;
    let one = p.send("accepted 7 a1").await?;
    let two = p.send("accepted 7 a2").await?;
    let three = p.send("accepted 7 a3").await?;
    let chosen = p.send("chosen 7").await?;
    let mut c = Check::new("acceptances arriving for one slot of five acceptors");
    c.eq(
        "after one, chosen",
        false,
        p.expect_bool(&one, "accepted 7 a1", "chosen")?,
    );
    c.eq(
        "after two, chosen",
        false,
        p.expect_bool(&two, "accepted 7 a2", "chosen")?,
    );
    c.eq(
        "after three, chosen",
        true,
        p.expect_bool(&three, "accepted 7 a3", "chosen")?,
    );
    c.eq(
        "after three, count",
        3,
        p.expect_i64(&three, "accepted 7 a3", "count")?,
    );
    c.eq(
        "chosen(7).value",
        "gamma".to_string(),
        p.expect_str(&chosen, "chosen 7", "value")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_duplicate_acceptance_counts_once, |ctx| {
    let p = ctx.prim("multi-paxos").await?;
    p.send("init 5").await?;
    p.send("prepare 1").await?;
    p.send("propose 1 alpha").await?;
    let mut counts = Vec::new();
    for _ in 0..3 {
        let r = p.send("accepted 1 a1").await?;
        counts.push(p.expect_i64(&r, "accepted 1 a1", "count")?);
    }
    let chosen = p.send("chosen 1").await?;
    let mut c = Check::new("one acceptor's acceptance retransmitted three times");
    // Counting messages rather than acceptors is how one acceptor becomes a majority of
    // five, and the failure is silent: the slot simply looks chosen when it is not.
    c.eq("the three counts", vec![1, 1, 1], counts);
    c.eq(
        "chosen(1).chosen",
        false,
        p.expect_bool(&chosen, "chosen 1", "chosen")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(preemption_restores_phase_one, |ctx| {
    let p = ctx.prim("multi-paxos").await?;
    p.send("init 3").await?;
    p.send("prepare 5").await?;
    let taken = p.send("preempt 7").await?;
    let phase = p.send("phase 1").await?;
    let refused = p.send("propose 2 beta").await?;
    let mut c = Check::new("a rival ballot arriving at the acceptors");
    c.eq(
        "preempt(7).leader",
        false,
        p.expect_bool(&taken, "preempt 7", "leader")?,
    );
    c.eq(
        "preempt(7).promised_n",
        7,
        p.expect_i64(&taken, "preempt 7", "promised_n")?,
    );
    c.eq(
        "phase(1).phase",
        "prepare".to_string(),
        p.expect_str(&phase, "phase 1", "phase")?,
    );
    // "preempted" rather than "no leader": the proposer knows it had leadership and lost
    // it, which is the difference between starting up and being pushed out.
    c.eq(
        "phase(1).reason",
        "preempted".to_string(),
        p.expect_str(&phase, "phase 1", "reason")?,
    );
    c.eq(
        "propose.round_trips",
        2,
        p.expect_i64(&refused, "propose 2 beta", "round_trips")?,
    );
    c.eq(
        "propose.ok",
        false,
        p.expect_bool(&refused, "propose 2 beta", "ok")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_lower_ballot_does_not_preempt, |ctx| {
    let p = ctx.prim("multi-paxos").await?;
    p.send("init 3").await?;
    p.send("prepare 5").await?;
    let stale = p.send("preempt 3").await?;
    let phase = p.send("phase 1").await?;
    let still = p.send("propose 1 alpha").await?;
    let mut c = Check::new("a rival whose ballot is behind the leader's");
    c.eq(
        "preempt(3).leader",
        true,
        p.expect_bool(&stale, "preempt 3", "leader")?,
    );
    c.eq(
        "preempt(3).promised_n",
        5,
        p.expect_i64(&stale, "preempt 3", "promised_n")?,
    );
    c.eq(
        "phase(1).phase",
        "accept".to_string(),
        p.expect_str(&phase, "phase 1", "phase")?,
    );
    c.eq(
        "propose.round_trips",
        1,
        p.expect_i64(&still, "propose 1 alpha", "round_trips")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(preemption_does_not_unchoose, |ctx| {
    let p = ctx.prim("multi-paxos").await?;
    p.send("init 3").await?;
    p.send("prepare 5").await?;
    p.send("propose 1 alpha").await?;
    p.send("accepted 1 a1").await?;
    p.send("preempt 9").await?;
    // a2 had already accepted at ballot 5; its answer was simply still on the wire when the
    // rival's prepare landed, and it counts exactly as much as it did before.
    let late = p.send("accepted 1 a2").await?;
    let chosen = p.send("chosen 1").await?;
    let mut c = Check::new("an acceptance that was in flight when leadership changed");
    c.eq(
        "the late acceptance's chosen",
        true,
        p.expect_bool(&late, "accepted 1 a2", "chosen")?,
    );
    c.eq(
        "the late acceptance's count",
        2,
        p.expect_i64(&late, "accepted 1 a2", "count")?,
    );
    c.eq(
        "chosen(1).chosen",
        true,
        p.expect_bool(&chosen, "chosen 1", "chosen")?,
    );
    c.eq(
        "chosen(1).value",
        "alpha".to_string(),
        p.expect_str(&chosen, "chosen 1", "value")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_gap_blocks_the_state_machine, |ctx| {
    let p = ctx.prim("multi-paxos").await?;
    p.send("init 3").await?;
    p.send("prepare 1").await?;
    for (slot, value) in [(1, "alpha"), (2, "beta"), (4, "delta")] {
        p.send(&format!("propose {slot} {value}")).await?;
        p.send(&format!("accepted {slot} a1")).await?;
        p.send(&format!("accepted {slot} a2")).await?;
    }
    let blocked = p.send("applied").await?;
    let blocked_gaps = int_list(p, &blocked, "applied", "gaps")?;
    p.send("propose 3 gamma").await?;
    p.send("accepted 3 a1").await?;
    p.send("accepted 3 a2").await?;
    let filled = p.send("applied").await?;
    let filled_gaps = int_list(p, &filled, "applied", "gaps")?;
    let mut c = Check::new("slots 1, 2 and 4 chosen, and then the hole filled");
    // Slot 4 is chosen and agreed by everyone, and still cannot be applied: a state machine
    // that skipped the hole would diverge from one that waited.
    c.eq(
        "applied.index with a gap",
        2,
        p.expect_i64(&blocked, "applied", "index")?,
    );
    c.eq("applied.gaps", vec![3], blocked_gaps);
    c.eq(
        "applied.index once filled",
        4,
        p.expect_i64(&filled, "applied", "index")?,
    );
    c.eq("applied.gaps once filled", Vec::<i64>::new(), filled_gaps);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_chosen_slot_keeps_its_value, |ctx| {
    let p = ctx.prim("multi-paxos").await?;
    p.send("init 3").await?;
    p.send("prepare 2").await?;
    p.send("propose 1 alpha").await?;
    p.send("accepted 1 a1").await?;
    p.send("accepted 1 a2").await?;
    let reproposed = p.send("propose 1 omega").await?;
    let chosen = p.send("chosen 1").await?;
    let mut c = Check::new("a leader proposing a second value into a settled slot");
    // The command is accepted — the leader is entitled to send it — but the decree is
    // already made, and re-deciding it would break the one guarantee Paxos offers.
    c.eq(
        "propose.ok",
        true,
        p.expect_bool(&reproposed, "propose 1 omega", "ok")?,
    );
    c.eq(
        "chosen(1).value",
        "alpha".to_string(),
        p.expect_str(&chosen, "chosen 1", "value")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(leadership_halves_the_cost, |ctx| {
    let p = ctx.prim("multi-paxos").await?;
    p.send("init 5").await?;
    // Ten proposals with no leader: every one of them has phase one still to do.
    let mut without = 0i64;
    for slot in 1..=10 {
        let r = p.send(&format!("propose {slot} v{slot}")).await?;
        without += p.expect_i64(&r, "propose", "round_trips")?;
    }
    p.send("prepare 3").await?;
    let mut with = 0i64;
    let mut all_ok = true;
    for slot in 1..=10 {
        let r = p.send(&format!("propose {slot} v{slot}")).await?;
        with += p.expect_i64(&r, "propose", "round_trips")?;
        all_ok &= p.expect_bool(&r, "propose", "ok")?;
    }
    let mut c = Check::new("the cost of ten proposals with and without leadership");
    c.eq("round trips without a leader", 20, without);
    // One prepare, amortised over every proposal that follows it: the saving is not a
    // constant factor on one call, it is a constant factor on all of them.
    c.eq("round trips with a leader", 10, with);
    c.eq("every proposal under the leader succeeded", true, all_ok);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_long_seeded_run, |ctx| {
    // The whole conversation is worked out here first — every command and the answer it
    // must produce — by replaying the rules over the seeded plan. The program is then asked
    // the same questions and never consulted about the answers.
    let names = ["a1", "a2", "a3", "a4", "a5"];
    let mut model = Model::new(names.len());
    let mut script: Vec<(String, Expect)> = Vec::new();
    let mut ballot = 1i64;
    for _ in 0..70 {
        match ctx.rng.random_range(0..5) {
            0 => {
                ballot += ctx.rng.random_range(1..3);
                let (promised, leader) = model.prepare(ballot);
                script.push((
                    format!("prepare {ballot}"),
                    Expect::Prepare { promised, leader },
                ));
            }
            1 => {
                let n = ballot + ctx.rng.random_range(-2..3);
                let (leader, promised_n) = model.preempt(n);
                script.push((
                    format!("preempt {n}"),
                    Expect::Preempt { leader, promised_n },
                ));
            }
            2 => {
                let slot = ctx.rng.random_range(1..7);
                let value = format!("v{}", ctx.rng.random_range(0..4));
                let (ok, round_trips) = model.propose(slot, &value);
                script.push((
                    format!("propose {slot} {value}"),
                    Expect::Propose { ok, round_trips },
                ));
            }
            3 => {
                let slot = ctx.rng.random_range(1..7);
                let who = names[ctx.rng.random_range(0..names.len())];
                // An acceptance for a slot nobody has proposed into is not a legal event;
                // the grammar answers an error there, and the replay stays on the path the
                // rules actually define.
                if model.is_open(slot) {
                    let (chosen, count) = model.accepted(slot, who);
                    script.push((
                        format!("accepted {slot} {who}"),
                        Expect::Accepted { chosen, count },
                    ));
                }
            }
            _ => {
                let (index, gaps) = model.progress();
                script.push(("applied".to_string(), Expect::Applied { index, gaps }));
            }
        }
    }
    let (index, gaps) = model.progress();
    script.push(("applied".to_string(), Expect::Applied { index, gaps }));

    let seed = ctx.seed;
    let p = ctx.prim("multi-paxos").await?;
    p.send("init 5").await?;
    let mut c = Check::new("seventy seeded events replayed against the tester's own rules");
    c.note(format!("seed {seed}, {} commands", script.len()));
    for (i, (command, expect)) in script.iter().enumerate() {
        let answer = p.send(command).await?;
        let at = |field: &str| format!("step[{i}] {command} → {field}");
        match expect {
            Expect::Prepare { promised, leader } => {
                c.eq(
                    &at("promised"),
                    *promised as i64,
                    p.expect_i64(&answer, command, "promised")?,
                );
                c.eq(
                    &at("leader"),
                    *leader,
                    p.expect_bool(&answer, command, "leader")?,
                );
            }
            Expect::Preempt { leader, promised_n } => {
                c.eq(
                    &at("leader"),
                    *leader,
                    p.expect_bool(&answer, command, "leader")?,
                );
                c.eq(
                    &at("promised_n"),
                    *promised_n,
                    p.expect_i64(&answer, command, "promised_n")?,
                );
            }
            Expect::Propose { ok, round_trips } => {
                c.eq(&at("ok"), *ok, p.expect_bool(&answer, command, "ok")?);
                c.eq(
                    &at("round_trips"),
                    *round_trips,
                    p.expect_i64(&answer, command, "round_trips")?,
                );
            }
            Expect::Accepted { chosen, count } => {
                c.eq(
                    &at("chosen"),
                    *chosen,
                    p.expect_bool(&answer, command, "chosen")?,
                );
                c.eq(
                    &at("count"),
                    *count as i64,
                    p.expect_i64(&answer, command, "count")?,
                );
            }
            Expect::Applied { index, gaps } => {
                c.eq(
                    &at("index"),
                    *index,
                    p.expect_i64(&answer, command, "index")?,
                );
                c.eq(
                    &at("gaps"),
                    gaps.clone(),
                    int_list(p, &answer, command, "gaps")?,
                );
            }
        }
        if !c.ok() {
            break;
        }
    }
    c.block("transcript", p.transcript_block());
    c.finish()
});
