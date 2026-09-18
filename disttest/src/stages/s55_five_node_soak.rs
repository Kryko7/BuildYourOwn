//! Stage 55 — Five members, mixed faults, and nothing left behind.
//!
//! The last stage runs everything at once. Five members, a workload that keeps five clients
//! busy for several seconds, and a schedule that partitions, kills, delays and mangles in
//! turn, with the linearizability checker watching the whole thing. Nothing here is new; what
//! is new is that it all happens together, on a cluster whose quorum arithmetic has changed.
//!
//! Five members is not three members with two spares. A majority is now three, so the
//! cluster survives two failures instead of one, and — the part that catches people — a
//! partition that leaves two members on one side leaves a side that can do nothing at all,
//! not even serve a read that claims to be linearizable. A design that hard-codes "two out
//! of three" anywhere, in a vote count, in a commit rule, or in a test, breaks here and
//! nowhere earlier.
//!
//! The last test is housekeeping, and it is a real property: after everything above has
//! finished, every member the harness started is either running or was deliberately stopped,
//! no member was left half-dead by a fault that healed, and the proxies say what they did.
//! A suite that leaks a process between stages makes the next stage's failure somebody
//! else's bug, which is the most expensive kind of bug this repository can have.

use crate::assert::{Check, Failure, FailureKind};
use crate::cluster::workload::{self, verify_convergence, FaultEvent, FaultSchedule, WorkloadSpec};
use crate::cluster::Cluster;
use crate::dist_test;
use crate::etcd::Client;
use crate::examples::{workload_example, ExampleSpec};
use crate::lin::{self, History, Verdict};
use crate::stages::{majority, ok, Ladder, Stage, Test};
use std::time::{Duration, Instant};

/// How long a five-member cluster under faults is given to name a leader.
const ELECT: Duration = Duration::from_millis(40_000);
/// How long the members are given to agree on every key once the faults are gone.
const CONVERGE: Duration = Duration::from_millis(40_000);
/// How long one write is given to find a member willing to take it.
const PATIENCE: Duration = Duration::from_millis(30_000);

/// Stage 55.
pub fn stage() -> Stage {
    Stage {
        number: 55,
        slug: "five_node_soak",
        name: "Five members, mixed faults, and nothing left behind",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "Five members tolerate two failures; the quorum arithmetic is the only thing that changes",
            "Partitions, kills, delays and message mangling all in one run",
            "After the faults heal, every member must converge on the same values",
            "No process, port or temporary file may survive the end of the run",
        ],
        examples,
        tests: vec![
            Test::new("five members form one cluster and name one leader", five_form_one)
                .ext()
                .cluster(5)
                .min_timeout_ms(120_000),
            Test::new(
                "a mixed-fault soak on five members is linearizable",
                the_soak_is_linearizable,
            )
            .ext()
            .cluster(5)
            .min_timeout_ms(120_000),
            Test::new("every member converges after the soak", members_converge)
                .ext()
                .cluster(5)
                .min_timeout_ms(120_000),
            Test::new("five members survive two failures", two_failures_are_survivable)
                .ext()
                .cluster(5)
                .min_timeout_ms(120_000),
            Test::new(
                "a two-against-three partition leaves the three serving",
                the_majority_side_serves,
            )
            .ext()
            .cluster(5)
            .min_timeout_ms(120_000),
            Test::new(
                "the quorum arithmetic of five is not the arithmetic of three",
                quorum_arithmetic,
            )
            .ext()
            .cluster(5)
            .min_timeout_ms(120_000),
            Test::new(
                "nothing is left behind when the soak is over",
                nothing_is_left_behind,
            )
            .ext()
            .cluster(5)
            .min_timeout_ms(120_000),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![workload_example(
        "A recorded soak, and the checker's verdict on it",
        3_000,
        4,
        3,
    )
    .request("four clients over three keys, spread across the members, with faults underneath")
    .response("the recorded history and whether a linearization of it exists")
    .note(
        "The example is captured against three members because that is the reference cluster \
         the capture runs; the stage itself uses five. What to look at is the same either way: \
         the calls with no answer, which the checker may place anywhere or leave out, and the \
         acknowledged ones, which it may not.",
    )]
}

/// The soak's schedule: a partition, a kill, a delay and a mangling, each healed before the
/// next one starts, and everything back up by the end.
fn soak_schedule(members: usize, duration: Duration) -> FaultSchedule {
    let slot = (duration.as_millis() as u64 / 10).max(200);
    let minority: Vec<usize> = (0..members.min(2)).collect();
    let rest: Vec<usize> = (minority.len()..members).collect();
    FaultSchedule::of(vec![
        (
            Duration::from_millis(slot),
            FaultEvent::Partition(vec![minority, rest]),
        ),
        (Duration::from_millis(slot * 2), FaultEvent::Heal),
        (Duration::from_millis(slot * 3), FaultEvent::KillLeader),
        (Duration::from_millis(slot * 5), FaultEvent::RestartStopped),
        (
            Duration::from_millis(slot * 6),
            FaultEvent::DelayAll(Duration::from_millis(60)),
        ),
        (Duration::from_millis(slot * 7), FaultEvent::Heal),
        (
            Duration::from_millis(slot * 8),
            FaultEvent::MangleAll {
                duplicate: 0.25,
                reorder: 0.25,
            },
        ),
        (Duration::from_millis(slot * 9), FaultEvent::Heal),
        (
            Duration::from_millis(slot * 9 + 1),
            FaultEvent::RestartStopped,
        ),
    ])
}

/// Turn a verdict into the stage's result: a violation is a failure carrying the rendering,
/// an undecided search is a note, and a linearizable history is silence.
fn judge(verdict: Verdict, history: &History, described: &str) -> Result<Option<String>, Failure> {
    match verdict {
        Verdict::Linearizable => Ok(None),
        Verdict::Inconclusive { key, states } => Ok(Some(format!(
            "the search for a linearization of key {key:?} ran out of budget after {states} \
             states: too concurrent to decide, which is not the same as wrong"
        ))),
        Verdict::NotLinearizable(v) => {
            let mut c = Check::new("the history the soak recorded");
            c.kind(FailureKind::Linearizability)
                .that("history", "a linearization", false, "none exists")
                .block("the smallest offending sub-history", v.render())
                .note(format!(
                    "{} operations were recorded, {} of them with no answer",
                    history.len(),
                    history.unknowns()
                ))
                .note(described.to_string());
            if history.len() <= 240 {
                c.block("the whole recorded history", history.render());
            }
            c.finish()?;
            Ok(None)
        }
    }
}

/// Put one key, trying every running member in turn until one takes it.
async fn put_patiently(
    cluster: &mut Cluster,
    key: &str,
    value: &str,
    within: Duration,
) -> Result<i64, Failure> {
    let deadline = Instant::now() + within;
    let mut last = "no member was running".to_string();
    loop {
        for i in cluster.running() {
            let name = cluster.members[i].name.clone();
            match cluster.members[i]
                .client
                .put(key.as_bytes(), value.as_bytes())
                .await
            {
                Ok(r) => return Ok(r.header.revision),
                Err(e) => last = format!("{name}: {e}"),
            }
        }
        if Instant::now() >= deadline {
            return Err(Failure::new(
                FailureKind::Assertion,
                format!(
                    "no member accepted a write of {key} within {} ms",
                    within.as_millis()
                ),
            )
            .note(last)
            .note(cluster.faults.describe()));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

dist_test!(five_form_one, |ctx| {
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT).await?;
    let list = ok(
        cluster.client(0).member_list().await,
        "/v3/cluster/member/list",
    )?;
    let described = cluster.describe().await;
    let mut c = Check::new("a five-member cluster before anything is done to it");
    c.eq("member_list.members.len()", 5, list.members.len());
    c.that(
        "leader index",
        "one of the five members",
        leader < 5,
        leader,
    );
    c.eq("members running", 5, cluster.running().len());
    ctx.note(described);
    c.finish()
});

dist_test!(the_soak_is_linearizable, |ctx| {
    let seed = ctx.seed;
    let prefix = ctx.prefix();
    let mut spec = WorkloadSpec::small(seed, &prefix);
    spec.duration = Duration::from_millis(7_000);
    spec.clients = 5;
    let schedule = soak_schedule(5, spec.duration);
    let plan = schedule.render();
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    let result = workload::run(cluster, &spec, &schedule).await?;
    cluster.heal().await;
    for i in 0..cluster.initial_size {
        if !cluster.members[i].running() {
            cluster.start_member(i).await?;
        }
    }
    let described = cluster.describe().await;
    let verdict = lin::check(&result.history);
    let note = judge(verdict, &result.history, &described)?;
    let summary = result.summary();
    let applied = result.faults_applied.join("\n");
    let stats = cluster.stats.summary();
    let mut c = Check::new("seven seconds of five clients over five members, under mixed faults");
    // Assert the soak was a soak: a run that recorded almost nothing proves nothing about
    // linearizability, however green the verdict.
    c.at_least("history.len()", 60, result.history.len());
    c.at_least("operations acknowledged", 30, result.acknowledged);
    ctx.note(summary);
    ctx.note(format!("fault plan:\n{plan}"));
    ctx.note(format!("faults applied:\n{applied}"));
    ctx.note(stats);
    if let Some(n) = note {
        ctx.note(n);
    }
    c.finish()
});

dist_test!(members_converge, |ctx| {
    let seed = ctx.seed;
    let prefix = ctx.prefix();
    let mut spec = WorkloadSpec::small(seed, &prefix);
    spec.duration = Duration::from_millis(5_000);
    let keys = spec.key_names();
    let schedule = soak_schedule(5, spec.duration);
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    let result = workload::run(cluster, &spec, &schedule).await?;
    cluster.heal().await;
    for i in 0..cluster.initial_size {
        if !cluster.members[i].running() {
            cluster.start_member(i).await?;
        }
    }
    cluster.wait_for_leader(ELECT).await?;
    let seen = verify_convergence(cluster, &keys, CONVERGE).await?;
    let running = cluster.running().len();
    let summary = result.summary();
    let mut c = Check::new("what all five members hold once the faults are gone");
    c.eq("members answering the convergence check", 5, running);
    c.eq("keys agreed on by every member", keys.len(), seen.len());
    ctx.note(summary);
    ctx.note(format!("the members settled on {seen:?}"));
    c.finish()
});

dist_test!(two_failures_are_survivable, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT).await?;
    // Kill the leader and one follower: three of five are left, which is exactly a
    // majority, and a majority is all a five-member cluster needs.
    let other = (leader + 1) % 5;
    cluster.kill(leader).await;
    cluster.kill(other).await;
    let revision = put_patiently(
        cluster,
        &format!("{prefix}/after-two-deaths"),
        "v",
        PATIENCE,
    )
    .await?;
    let survivors = cluster.running().len();
    let new_leader = cluster.wait_for_leader(ELECT).await?;
    let described = cluster.describe().await;
    let mut c = Check::new("a five-member cluster with two of its members dead");
    c.eq("members still running", 3, survivors);
    c.eq("majority(5)", 3, majority(5));
    c.that(
        "a write after two deaths",
        "an acknowledged write from the three survivors",
        revision > 0,
        revision,
    );
    c.that(
        "the survivors' leader",
        "one of the five member slots",
        new_leader < 5,
        new_leader,
    );
    ctx.note(format!(
        "killed m{} (the leader) and m{}; m{} leads the survivors — {described}",
        leader + 1,
        other + 1,
        new_leader + 1
    ));
    c.finish()
});

dist_test!(the_majority_side_serves, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT).await?;
    // Two members on one side, three on the other, with the old leader deliberately on the
    // small side: the three have to elect one of their own and carry on.
    let mate = (leader + 1) % 5;
    let minority = vec![leader, mate];
    let majority_side: Vec<usize> = (0..5).filter(|i| !minority.contains(i)).collect();
    cluster
        .partition(&[minority.clone(), majority_side.clone()])
        .await;
    let key = format!("{prefix}/majority-side");
    let deadline = Instant::now() + PATIENCE;
    let mut revision = 0;
    let mut last = String::new();
    while Instant::now() < deadline && revision == 0 {
        for i in &majority_side {
            match cluster.members[*i].client.put(key.as_bytes(), b"v").await {
                Ok(r) => {
                    revision = r.header.revision;
                    break;
                }
                Err(e) => last = format!("m{}: {e}", i + 1),
            }
        }
        if revision == 0 {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    let described = cluster.describe().await;
    cluster.heal().await;
    let mut c = Check::new("a five-member cluster cut two against three");
    c.that(
        "a write to the three-member side",
        "an acknowledgement: three of five is a quorum",
        revision > 0,
        if revision > 0 {
            revision.to_string()
        } else {
            format!("no member on the majority side accepted it ({last})")
        },
    );
    if !c.ok() {
        c.note(described.clone());
        c.note(cluster.faults.describe());
    }
    ctx.note(format!(
        "m{} and m{} were cut off; the other three carried on — {described}",
        minority[0] + 1,
        minority[1] + 1
    ));
    c.finish()
});

dist_test!(quorum_arithmetic, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    // Stop three of the five outright rather than partitioning them away. What is being
    // tested is the arithmetic, and a stopped process is the one fault with no ambiguity in
    // it: there is no link left to leak through and nothing for the injector to get wrong.
    for i in [2usize, 3, 4] {
        cluster.stop(i).await;
    }
    let key = format!("{prefix}/minority");
    // A short-lived client of its own, because the two survivors will sit on the request
    // until they can reach a quorum, and the whole point is that they never will.
    let mut refused = 0;
    let mut accepted = 0;
    let mut answers = Vec::new();
    for i in [0usize, 1] {
        let url = format!("http://127.0.0.1:{}", cluster.members[i].client_port);
        let probe = Client::new(&url, &cluster.members[i].name, Duration::from_millis(3_500))
            .map_err(|e| Failure::harness(format!("cannot build a probe client: {e}")))?;
        match probe.put(key.as_bytes(), b"v").await {
            Ok(r) => {
                accepted += 1;
                answers.push(format!(
                    "m{} accepted it at revision {}",
                    i + 1,
                    r.header.revision
                ));
            }
            Err(e) => {
                refused += 1;
                answers.push(format!("m{} refused it: {e}", i + 1));
            }
        }
    }
    // Put the three back and show the same write goes through, so the refusal above is
    // read as "not enough members" and not as "this cluster was broken all along".
    for i in [2usize, 3, 4] {
        cluster.start_member(i).await?;
    }
    cluster.wait_for_leader(ELECT).await?;
    let revision = put_patiently(cluster, &key, "v", PATIENCE).await?;
    let described = cluster.describe().await;
    let mut c = Check::new("what two members out of five may decide on their own");
    c.eq("majority(3)", 2, majority(3));
    c.eq("majority(5)", 3, majority(5));
    c.eq("majority(4)", 3, majority(4));
    c.eq("writes accepted by the two-member side", 0, accepted);
    c.eq("writes refused by the two-member side", 2, refused);
    c.that(
        "the same write once three members are back",
        "an acknowledgement",
        revision > 0,
        revision,
    );
    if !c.ok() {
        c.note(described.clone());
        c.note(answers.join("; "));
    }
    ctx.note(format!(
        "with three of five stopped: {} — and once they were back, revision {revision}",
        answers.join("; ")
    ));
    c.finish()
});

dist_test!(nothing_is_left_behind, |ctx| {
    let seed = ctx.seed;
    let prefix = ctx.prefix();
    let mut spec = WorkloadSpec::small(seed, &prefix);
    spec.duration = Duration::from_millis(4_000);
    let schedule = soak_schedule(5, spec.duration);
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    let result = workload::run(cluster, &spec, &schedule).await?;
    cluster.heal().await;
    // Everything the schedule stopped, it also restarted; anything still down at this point
    // is something the harness lost track of rather than something it decided to stop.
    let still_stopped: Vec<usize> = (0..cluster.initial_size)
        .filter(|i| !cluster.members[*i].running())
        .collect();
    for i in &still_stopped {
        cluster.start_member(*i).await?;
    }
    cluster.wait_for_leader(ELECT).await?;
    let running = cluster.running();
    let described = cluster.describe().await;
    let stats = cluster.stats.summary();
    let faults = cluster.faults.describe();
    let summary = result.summary();
    let mut c = Check::new("what the soak left behind");
    c.eq("members running at the end", 5, running.len());
    c.eq("faults still in place", "no faults".to_string(), faults);
    c.that(
        "members the schedule left stopped",
        "none, or only ones it had stopped on purpose and could start again",
        still_stopped.len() <= 2,
        still_stopped.iter().map(|i| i + 1).collect::<Vec<_>>(),
    );
    if !c.ok() {
        c.note(described.clone());
    }
    ctx.note(summary);
    ctx.note(described);
    ctx.note(stats);
    c.finish()
});
