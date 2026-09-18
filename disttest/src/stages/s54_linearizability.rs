//! Stage 54 — Linearizability under fault injection.
//!
//! The centrepiece of the cluster ladder. Several client tasks put, read, compare-and-swap
//! and delete a handful of keys through *different* members while a seeded schedule cuts the
//! network, isolates a member and kills the leader underneath them. Every call is recorded
//! with the instant it was made and the instant the answer came back, and the resulting
//! history is handed to the checker in `src/lin/`, which looks for one total order — each
//! operation placed at a single instant inside its own call/return window — that a single
//! sequential key/value store could have produced.
//!
//! Two things make this a fair test rather than a random-failure generator.
//!
//! The first is that an operation whose answer never arrived is *optional*. A timeout, a
//! killed member or a partitioned client leaves the harness genuinely unable to say whether
//! the write took effect, so such an operation may be linearized anywhere after it was
//! called, or left out altogether. Anything stricter would fail a correct implementation the
//! moment a packet went missing.
//!
//! The second is that an undecided search is not a failure. The checker is given a budget,
//! and a history too concurrent to decide inside it comes back `Inconclusive`. That is
//! reported as a note. Turning "I could not tell" into "you are wrong" is how a self-check
//! becomes noise.
//!
//! When a history really does admit no linearization, the failure carries the *smallest
//! offending sub-history*: the few operations that already make it impossible, each with its
//! window, the one that made it impossible marked `>>`, the furthest the search ever got,
//! and the state that left the rest with nowhere to go. That rendering is the point of the
//! stage — a bare "not linearizable" tells nobody anything — so the first test in the file
//! feeds the checker a hand-built impossible history and prints exactly what a real failure
//! would print.

use crate::assert::{Check, Failure, FailureKind};
use crate::cluster::workload::{self, verify_convergence, FaultEvent, FaultSchedule, WorkloadSpec};
use crate::cluster::Cluster;
use crate::dist_test;
use crate::examples::{workload_example, ExampleSpec};
use crate::lin::{self, Entry, History, Op, Outcome, Verdict};
use crate::stages::{Ladder, Stage, Test};
use std::time::Duration;

/// How long a faulted cluster is given to name a leader.
const ELECT: Duration = Duration::from_millis(30_000);
/// How long the members are given to agree on every key once the faults are gone.
const CONVERGE: Duration = Duration::from_millis(40_000);

/// Stage 54.
pub fn stage() -> Stage {
    Stage {
        number: 54,
        slug: "linearizability",
        name: "Linearizability under fault injection",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "A randomized concurrent workload runs while the seeded schedule cuts and heals the network",
            "Every call is recorded with the instant it was made and the instant it answered",
            "An operation whose answer was lost may have happened or not; everything else is pinned",
            "The checker looks for one total order that a single key/value store could have produced",
        ],
        examples,
        tests: vec![
            Test::new(
                "an impossible history is reported with the operations that make it impossible",
                a_broken_history_is_rendered,
            )
            .ext()
            .min_timeout_ms(60_000),
            Test::new(
                "an undecided search is reported as a note, not as a failure",
                inconclusive_is_not_a_failure,
            )
            .ext()
            .min_timeout_ms(60_000),
            Test::new("a workload with no faults is linearizable", no_faults)
                .ext()
                .min_timeout_ms(120_000),
            Test::new(
                "the recorded history holds a real workload",
                the_history_is_substantial,
            )
            .ext()
            .fresh()
            .min_timeout_ms(120_000),
            Test::new(
                "every member converges once the workload stops",
                members_converge,
            )
            .ext()
            .min_timeout_ms(120_000),
            Test::new(
                "a workload under the seeded schedule is linearizable",
                under_the_seeded_schedule,
            )
            .ext()
            .fresh()
            .min_timeout_ms(120_000),
            Test::new(
                "a workload with one member isolated throughout is linearizable",
                with_a_member_isolated,
            )
            .ext()
            .fresh()
            .min_timeout_ms(120_000),
            Test::new(
                "a workload across a leader kill is linearizable",
                across_a_leader_kill,
            )
            .ext()
            .fresh()
            .min_timeout_ms(120_000),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![workload_example(
        "A small history, and the linearization the checker found",
        2_500,
        3,
        2,
    )
    .request("three clients, two keys, a couple of seconds of puts, reads, swaps and deletes")
    .response("one call per line with its window, and the checker's verdict on the whole history")
    .note(
        "Read the windows, not the line order. An operation may be placed at any single \
         instant between its call and its return, so two lines that overlap in time may be \
         ordered either way; two that do not overlap may not. An operation whose answer never \
         arrived carries `NO ANSWER` and a return of `never`, and the checker is free to place \
         it late or leave it out.",
    )]
}

/// Turn a verdict into the stage's result.
///
/// Linearizable passes. Inconclusive comes back as a line for the report, because a search
/// that ran out of budget has found nothing wrong. A violation is the failure this whole
/// stage exists to produce, and it carries the rendering rather than a bare verdict.
fn judge(
    verdict: Verdict,
    history: &History,
    described: &str,
    faults: &str,
) -> Result<Option<String>, Failure> {
    match verdict {
        Verdict::Linearizable => Ok(None),
        Verdict::Inconclusive { key, states } => Ok(Some(format!(
            "the search for a linearization of key {key:?} ran out of budget after {states} \
             states: too concurrent to decide, which is not the same as wrong"
        ))),
        Verdict::NotLinearizable(v) => {
            let mut c = Check::new("the recorded history");
            c.kind(FailureKind::Linearizability)
                .that("history", "a linearization", false, "none exists")
                .block("the smallest offending sub-history", v.render())
                .note(format!(
                    "{} operations were recorded, {} of them with no answer",
                    history.len(),
                    history.unknowns()
                ))
                .note(described.to_string())
                .note(faults.to_string());
            if history.len() <= 240 {
                c.block("the whole recorded history", history.render());
            }
            c.finish()?;
            // `finish` always fails here: the check above was recorded as failed.
            Ok(None)
        }
    }
}

/// A history that no sequential key/value store could have produced: a write, a read that
/// sees it, and a later read that finds the key absent with nothing having deleted it.
fn an_impossible_history() -> History {
    let mut h = History::new();
    h.push(Entry {
        id: 0,
        client: 1,
        key: "k1".into(),
        op: Op::Write(b"a".to_vec()),
        outcome: Outcome::Ok,
        call_ns: 12_004_000,
        ret_ns: 12_310_000,
        note: "m1".into(),
    });
    h.push(Entry {
        id: 0,
        client: 2,
        key: "k1".into(),
        op: Op::Read,
        outcome: Outcome::Value(Some(b"a".to_vec())),
        call_ns: 12_415_000,
        ret_ns: 12_902_000,
        note: "m2".into(),
    });
    h.push(Entry {
        id: 0,
        client: 0,
        key: "k1".into(),
        op: Op::Read,
        outcome: Outcome::Value(None),
        call_ns: 13_001_000,
        ret_ns: 13_204_000,
        note: "m3".into(),
    });
    h
}

/// Run a workload under a schedule and check the history it produced.
///
/// Everything the report wants is pulled out of the cluster before the borrow ends, so the
/// caller can hand it all to `ctx.note`.
async fn run_and_check(
    cluster: &mut Cluster,
    spec: &WorkloadSpec,
    schedule: &FaultSchedule,
) -> Result<String, Failure> {
    cluster.wait_for_leader(ELECT).await?;
    let result = workload::run(cluster, spec, schedule).await?;
    cluster.heal().await;
    for i in 0..cluster.initial_size {
        if !cluster.members[i].running() {
            cluster.start_member(i).await?;
        }
    }
    let described = cluster.describe().await;
    let faults = cluster.faults.describe();
    let verdict = lin::check(&result.history);
    let note = judge(verdict, &result.history, &described, &faults)?;
    let mut lines = vec![result.summary()];
    if !result.faults_applied.is_empty() {
        lines.push(format!(
            "faults applied:\n{}",
            result.faults_applied.join("\n")
        ));
    }
    if let Some(n) = note {
        lines.push(n);
    }
    Ok(lines.join("\n"))
}

dist_test!(a_broken_history_is_rendered, |ctx| {
    let history = an_impossible_history();
    let verdict = lin::check(&history);
    let mut c = Check::new("the checker's verdict on a history that admits no linearization");
    let rendered = match &verdict {
        Verdict::NotLinearizable(v) => v.render(),
        other => {
            c.that(
                "check(an impossible history)",
                "NotLinearizable",
                false,
                format!("{other:?}"),
            );
            String::new()
        }
    };
    // The rendering is the whole point of the stage; a bare verdict tells nobody anything.
    c.that(
        "violation.render() names the key",
        "the key whose sub-history failed",
        rendered.contains("no linearization exists"),
        rendered.lines().next().unwrap_or("").to_string(),
    );
    c.that(
        "violation.render() marks the culprit",
        "the offending operation marked with >>",
        rendered.contains(">>"),
        rendered.contains(">>"),
    );
    c.that(
        "violation.render() explains what the search did",
        "the furthest the search got, and how many states it explored",
        rendered.contains("linearization states explored"),
        rendered.contains("linearization states explored"),
    );
    // And this is what a real failure would print.
    ctx.note(format!(
        "a deliberately impossible history renders as:\n{rendered}"
    ));
    c.finish()
});

dist_test!(inconclusive_is_not_a_failure, |ctx| {
    let seed = ctx.seed;
    let prefix = ctx.prefix();
    let mut spec = WorkloadSpec::small(seed, &prefix);
    spec.duration = Duration::from_millis(2_500);
    spec.keys = 2;
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    let result = workload::run(cluster, &spec, &FaultSchedule::none()).await?;
    let described = cluster.describe().await;
    let faults = cluster.faults.describe();
    // A budget of one state cannot decide any history with two operations on a key, so this
    // is the undecided verdict, reproducibly, without waiting for a pathological workload.
    let verdict = lin::check_with_budget(&result.history, 1);
    let starved = matches!(verdict, Verdict::Inconclusive { .. });
    let note = judge(verdict, &result.history, &described, &faults)?;
    let full = lin::check(&result.history);
    let full_note = judge(full, &result.history, &described, &faults)?;
    let summary = result.summary();
    let mut c = Check::new("a search that ran out of budget");
    c.that(
        "check_with_budget(history, 1)",
        "Inconclusive: undecided, not wrong",
        starved,
        starved,
    );
    c.that(
        "the stage's own handling of Inconclusive",
        "a note rather than a failure",
        note.is_some(),
        note.clone(),
    );
    ctx.note(summary);
    if let Some(n) = note {
        ctx.note(n);
    }
    if let Some(n) = full_note {
        ctx.note(format!("with the full budget: {n}"));
    }
    c.finish()
});

dist_test!(no_faults, |ctx| {
    let seed = ctx.seed;
    let prefix = ctx.prefix();
    let mut spec = WorkloadSpec::small(seed, &prefix);
    spec.duration = Duration::from_millis(5_000);
    let cluster = ctx.cluster()?;
    let notes = run_and_check(cluster, &spec, &FaultSchedule::none()).await?;
    ctx.note("no faults were injected: a history that is not linearizable here is a plain bug");
    ctx.note(notes);
    Ok(())
});

dist_test!(the_history_is_substantial, |ctx| {
    let seed = ctx.seed;
    let prefix = ctx.prefix();
    let mut spec = WorkloadSpec::small(seed, &prefix);
    spec.duration = Duration::from_millis(5_000);
    let expected_keys = spec.keys;
    let schedule = FaultSchedule::random(seed, 3, spec.duration);
    let plan = schedule.render();
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    let result = workload::run(cluster, &spec, &schedule).await?;
    cluster.heal().await;
    let counts = lin::per_key_counts(&result.history);
    let described = cluster.describe().await;
    let summary = result.summary();
    let applied = result.faults_applied.join("\n");
    let mut c = Check::new("the history a five-second workload recorded");
    // A stage that checks an empty history passes for the wrong reason, so the size of the
    // history is asserted before anything is concluded from it.
    c.at_least("history.len()", 60, result.history.len());
    c.at_least("operations acknowledged", 30, result.acknowledged);
    c.eq("keys touched", expected_keys, result.history.keys().len());
    c.that(
        "per_member",
        "every member served at least one call",
        result.per_member.iter().all(|n| *n > 0),
        result.per_member.clone(),
    );
    for (key, n) in &counts {
        c.at_least(&format!("operations on {key}"), 5, *n);
    }
    if !c.ok() {
        c.note(described);
        c.block("the recorded history", result.history.render());
    }
    ctx.note(summary);
    ctx.note(format!("fault plan:\n{plan}"));
    ctx.note(format!("faults applied:\n{applied}"));
    c.finish()
});

dist_test!(members_converge, |ctx| {
    let seed = ctx.seed;
    let prefix = ctx.prefix();
    let mut spec = WorkloadSpec::small(seed, &prefix);
    spec.duration = Duration::from_millis(4_000);
    let keys = spec.key_names();
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    let result = workload::run(cluster, &spec, &FaultSchedule::none()).await?;
    cluster.heal().await;
    let seen = verify_convergence(cluster, &keys, CONVERGE).await?;
    let summary = result.summary();
    let mut c = Check::new("what every member holds once the workload has stopped");
    c.eq("keys agreed on by every member", keys.len(), seen.len());
    ctx.note(summary);
    ctx.note(format!("the members settled on {seen:?}"));
    c.finish()
});

dist_test!(under_the_seeded_schedule, |ctx| {
    let seed = ctx.seed;
    let prefix = ctx.prefix();
    let mut spec = WorkloadSpec::small(seed, &prefix);
    spec.duration = Duration::from_millis(6_000);
    let schedule = FaultSchedule::random(seed, 3, spec.duration);
    let plan = schedule.render();
    let cluster = ctx.cluster()?;
    let notes = run_and_check(cluster, &spec, &schedule).await?;
    ctx.note(format!("fault plan:\n{plan}"));
    ctx.note(notes);
    Ok(())
});

dist_test!(with_a_member_isolated, |ctx| {
    let seed = ctx.seed;
    let prefix = ctx.prefix();
    let mut rng_pick = seed % 3;
    let mut spec = WorkloadSpec::small(seed, &prefix);
    spec.duration = Duration::from_millis(6_000);
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT).await?;
    // Isolating the leader would be a different test; this one keeps one follower away for
    // the whole run, so a client that talks to it has to be refused rather than served a
    // stale value.
    if rng_pick == leader as u64 {
        rng_pick = (rng_pick + 1) % 3;
    }
    let victim = rng_pick as usize;
    let schedule = FaultSchedule::of(vec![(
        Duration::from_millis(0),
        FaultEvent::Isolate(victim),
    )]);
    let plan = schedule.render();
    let notes = run_and_check(cluster, &spec, &schedule).await?;
    ctx.note(format!(
        "m{} was cut off from both its peers for the whole run",
        victim + 1
    ));
    ctx.note(format!("fault plan:\n{plan}"));
    ctx.note(notes);
    Ok(())
});

dist_test!(across_a_leader_kill, |ctx| {
    let seed = ctx.seed;
    let prefix = ctx.prefix();
    let mut spec = WorkloadSpec::small(seed, &prefix);
    spec.duration = Duration::from_millis(7_000);
    // Kill the leader a third of the way in, and let the survivors run on for the rest:
    // every write acknowledged before the kill has to survive it, and the checker is what
    // notices when one does not.
    let schedule = FaultSchedule::of(vec![
        (Duration::from_millis(2_500), FaultEvent::KillLeader),
        (Duration::from_millis(5_000), FaultEvent::RestartStopped),
    ]);
    let plan = schedule.render();
    let cluster = ctx.cluster()?;
    let notes = run_and_check(cluster, &spec, &schedule).await?;
    ctx.note(format!("fault plan:\n{plan}"));
    ctx.note(notes);
    Ok(())
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stage_turns_an_impossible_history_into_a_rendered_failure() {
        let history = an_impossible_history();
        let verdict = lin::check(&history);
        assert!(
            matches!(verdict, Verdict::NotLinearizable(_)),
            "a write, a read of it and a later read of nothing cannot be linearized, got {verdict:?}"
        );
        let failure = judge(verdict, &history, "m1 | m2 | m3", "no faults")
            .expect_err("a violation must become a failure");
        assert_eq!(failure.kind, FailureKind::Linearizability);
        let (_, block) = failure
            .blocks
            .iter()
            .find(|(t, _)| t == "the smallest offending sub-history")
            .expect("the rendering must be attached");
        assert!(block.contains("no linearization exists"), "{block}");
        assert!(block.contains(">>"), "{block}");
        assert!(block.contains("linearization states explored"), "{block}");
    }

    #[test]
    fn a_linearizable_history_passes_and_a_starved_search_is_only_a_note() {
        let mut h = History::new();
        h.push(Entry {
            id: 0,
            client: 0,
            key: "k".into(),
            op: Op::Write(b"a".to_vec()),
            outcome: Outcome::Ok,
            call_ns: 0,
            ret_ns: 1_000,
            note: String::new(),
        });
        h.push(Entry {
            id: 0,
            client: 1,
            key: "k".into(),
            op: Op::Read,
            outcome: Outcome::Value(Some(b"a".to_vec())),
            call_ns: 2_000,
            ret_ns: 3_000,
            note: String::new(),
        });
        assert_eq!(
            judge(lin::check(&h), &h, "", "").expect("linearizable"),
            None
        );
        let starved = judge(lin::check_with_budget(&h, 1), &h, "", "")
            .expect("an undecided search is not a failure");
        assert!(
            starved.is_some_and(|n| n.contains("ran out of budget")),
            "an inconclusive verdict must come back as a note"
        );
    }
}
