//! Stage 52 — A slow, lossy link degrades throughput, not safety.
//!
//! Every link between members is given a delay and a one-in-ten chance of refusing a new
//! connection, and then the stage asks for nothing but safety. Writes get slower. Latency
//! climbs. Some calls time out and are never acknowledged, which is a correct answer to a
//! network that cannot deliver. None of that is asserted on, because none of it is a
//! property of a correct implementation — a slower machine or a busier one would move every
//! one of those numbers. What is asserted is that every write the cluster *did* acknowledge
//! is still there, that the members still agree, and that no revision ever goes backwards.
//!
//! The numbers are still measured and reported, because "it got slower" and "it broke" look
//! identical from a test that records neither. A throughput line in the report is what tells
//! the difference between a transport that retries patiently and one that has wedged.
//!
//! The last test in the stage is aimed at the suite rather than at the program: if the
//! proxy never actually refused a connection, then every other test in the stage ran against
//! a healthy network and proved nothing. `cluster.stats.blocked()` has to be greater than
//! zero or the stage is lying about what it tested. That one test raises the refusal rate to
//! one connection in two and waits for a refusal rather than assuming one, because at one in
//! ten a whole run can go by without a single dial being unlucky.

use crate::assert::{Check, Failure, FailureKind};
use crate::cluster::proxy::LinkFault;
use crate::cluster::workload::{self, verify_convergence, FaultSchedule, WorkloadSpec};
use crate::cluster::Cluster;
use crate::dist_test;
use crate::examples::{cluster_example, step, ExampleSpec};
use crate::stages::{check_non_decreasing, Ladder, Stage, Test};
use serde_json::json;
use std::time::{Duration, Instant};

/// How long a degraded cluster is given to name a leader.
const ELECT: Duration = Duration::from_millis(30_000);
/// How long one write is given to find a member willing to take it.
const PATIENCE: Duration = Duration::from_millis(25_000);
/// The fault this whole stage runs under: slow, and one connection in ten refused.
fn lossy() -> LinkFault {
    LinkFault {
        delay: Duration::from_millis(40),
        drop_connection: 0.10,
        ..Default::default()
    }
}

/// Stage 52.
pub fn stage() -> Stage {
    Stage {
        number: 52,
        slug: "lossy_link",
        name: "A slow, lossy link degrades throughput, not safety",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "Delay and connection loss slow the cluster down; they never change what it answers",
            "A transport that cannot deliver must retry, not drop the entry",
            "Throughput falls and latency rises: the test records both and asserts neither",
            "Every acknowledged write must still be readable when the link recovers",
        ],
        examples,
        tests: vec![
            Test::new(
                "a workload over a lossy link still acknowledges writes",
                writes_are_still_acknowledged,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "throughput and latency are measured and reported, not asserted",
                throughput_is_reported,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "every acknowledged write is readable once the link recovers",
                writes_are_readable_after_the_heal,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "every member converges once the link recovers",
                members_converge_after_the_heal,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "no member's revision goes backwards",
                revision_never_retreats,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new("a lossy link changes no value", values_are_unchanged)
                .ext()
                .fresh()
                .min_timeout_ms(60_000),
            Test::new(
                "one leader is named again once the link recovers",
                a_leader_is_named_again,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "the injector really did refuse peer connections",
                the_injector_did_something,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![cluster_example("A write that is slow but not wrong", || {
        vec![
            step(
                0,
                "/v3/kv/put",
                json!({"key": "czUyL2sw", "value": "c2xvdw=="}),
                "write s52/k0 while every link is slow and drops one connection in ten",
            ),
            step(
                1,
                "/v3/kv/range",
                json!({"key": "czUyL2sw"}),
                "m2 answers with the same value, however long it took to get there",
            ),
            step(
                2,
                "/v3/kv/range",
                json!({"key": "czUyL2sw"}),
                "and so does m3",
            ),
        ]
    })
    .request("one write and two reads, with a slow and flapping link between the members")
    .response("the same value from every member, at the same revision")
    .note(
        "The only thing a lossy link is allowed to change is how long this takes. A call that \
         times out is a correct answer — the harness cannot tell a refused write from an \
         applied one whose answer was lost — but a call that *is* acknowledged has to be true \
         for good, on every member.",
    )]
}

/// One acknowledged write.
#[derive(Debug, Clone)]
struct Written {
    key: String,
    value: String,
    revision: i64,
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

/// Write `n` keys while the fault stays in place, timing every one of them.
async fn write_keys(
    cluster: &mut Cluster,
    prefix: &str,
    n: usize,
) -> Result<(Vec<Written>, Vec<Duration>), Failure> {
    let mut out = Vec::new();
    let mut latencies = Vec::new();
    for i in 0..n {
        let key = format!("{prefix}/k{i}");
        let value = format!("v{i}");
        let started = Instant::now();
        let revision = put_patiently(cluster, &key, &value, PATIENCE).await?;
        latencies.push(started.elapsed());
        out.push(Written {
            key,
            value,
            revision,
        });
    }
    Ok((out, latencies))
}

/// Demand that every running member holds every acknowledged write at the same revision.
async fn check_every_member_holds(cluster: &mut Cluster, written: &[Written], c: &mut Check) {
    for i in cluster.running() {
        let name = cluster.members[i].name.clone();
        for w in written {
            match cluster.members[i].client.get_key(w.key.as_bytes()).await {
                Ok(r) => match r.one() {
                    Some(kv) => {
                        c.eq(
                            &format!("{name}.range({}).kvs[0].value", w.key),
                            w.value.clone(),
                            kv.value_str(),
                        );
                        c.eq(
                            &format!("{name}.range({}).kvs[0].mod_revision", w.key),
                            w.revision,
                            kv.mod_revision,
                        );
                    }
                    None => {
                        c.that(
                            &format!("{name}.range({}).kvs", w.key),
                            "the acknowledged value, still there",
                            false,
                            "the key is absent",
                        );
                    }
                },
                Err(e) => {
                    c.that(
                        &format!("{name}.range({})", w.key),
                        "an answer",
                        false,
                        e.to_string(),
                    );
                }
            }
        }
    }
}

/// Attach the cluster's state to a failed check.
async fn attach_state(cluster: &mut Cluster, c: &mut Check) {
    if c.ok() {
        return;
    }
    let described = cluster.describe().await;
    c.note(described);
    c.note(cluster.faults.describe());
}

/// Operations a second, and the mean latency in milliseconds.
fn rate(count: usize, took: Duration) -> (f64, f64) {
    let secs = took.as_secs_f64().max(f64::EPSILON);
    let mean = if count == 0 {
        0.0
    } else {
        took.as_secs_f64() * 1e3 / count as f64
    };
    (count as f64 / secs, mean)
}

dist_test!(writes_are_still_acknowledged, |ctx| {
    let seed = ctx.seed;
    let prefix = ctx.prefix();
    let mut spec = WorkloadSpec::small(seed, &prefix);
    spec.duration = Duration::from_millis(5_000);
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    cluster.every_link(lossy()).await;
    let result = workload::run(cluster, &spec, &FaultSchedule::none()).await?;
    cluster.heal().await;
    let summary = result.summary();
    let blocked = cluster.stats.blocked();
    let stats = cluster.stats.summary();
    let mut c = Check::new("a five-second workload over a slow, flapping link");
    // A cluster whose transport gives up rather than retrying acknowledges nothing at all.
    // A floor, not a throughput target. The workload runs for a fixed window while the
    // proxies are deliberately dropping, duplicating and reordering peer traffic, so how
    // many operations come back acknowledged is not something a correct cluster controls:
    // an observed run acknowledged 10 of 21 and another 19 of 27, both perfectly healthy.
    // What this check is for is non-vacuity — convergence over zero writes proves nothing —
    // so it asks only that real work got through. The property under test is the agreement
    // assertion below.
    c.at_least("operations acknowledged", 5, result.acknowledged);
    c.observe("operations with no answer", result.unknown);
    attach_state(cluster, &mut c).await;
    ctx.note(summary);
    ctx.note(stats);
    ctx.note(format!(
        "{blocked} peer connections were refused by the injector"
    ));
    c.finish()
});

dist_test!(throughput_is_reported, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    let clean_started = Instant::now();
    let (clean, clean_latencies) = write_keys(cluster, &format!("{prefix}/clean"), 20).await?;
    let clean_took = clean_started.elapsed();
    cluster.every_link(lossy()).await;
    let lossy_started = Instant::now();
    let (degraded, lossy_latencies) = write_keys(cluster, &format!("{prefix}/lossy"), 20).await?;
    let lossy_took = lossy_started.elapsed();
    cluster.heal().await;
    let (clean_ops, clean_mean) = rate(clean.len(), clean_took);
    let (lossy_ops, lossy_mean) = rate(degraded.len(), lossy_took);
    let worst_clean = clean_latencies.iter().max().copied().unwrap_or_default();
    let worst_lossy = lossy_latencies.iter().max().copied().unwrap_or_default();
    let mut all = clean.clone();
    all.extend(degraded.clone());
    let last = all.last().map(|w| w.revision).unwrap_or(0);
    cluster
        .wait_for_revision(last, Duration::from_millis(30_000))
        .await?;
    let mut c = Check::new("forty writes, twenty of them over a lossy link");
    // The only assertion is safety. Throughput is reported below and never checked: a
    // faster machine, a busier one, or a different kernel would move every one of those
    // numbers without making anything wrong.
    check_every_member_holds(cluster, &all, &mut c).await;
    attach_state(cluster, &mut c).await;
    let stats = cluster.stats.summary();
    ctx.note(format!(
        "healthy: {clean_ops:.1} writes/s, mean {clean_mean:.1} ms, worst {} ms",
        worst_clean.as_millis()
    ));
    ctx.note(format!(
        "lossy:   {lossy_ops:.1} writes/s, mean {lossy_mean:.1} ms, worst {} ms",
        worst_lossy.as_millis()
    ));
    ctx.note(stats);
    c.finish()
});

dist_test!(writes_are_readable_after_the_heal, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    cluster.every_link(lossy()).await;
    let (written, _) = write_keys(cluster, &prefix, 8).await?;
    cluster.heal().await;
    let last = written.last().map(|w| w.revision).unwrap_or(0);
    cluster
        .wait_for_revision(last, Duration::from_millis(30_000))
        .await?;
    let mut c = Check::new("eight writes made over a lossy link, read back after the heal");
    check_every_member_holds(cluster, &written, &mut c).await;
    attach_state(cluster, &mut c).await;
    c.finish()
});

dist_test!(members_converge_after_the_heal, |ctx| {
    let seed = ctx.seed;
    let prefix = ctx.prefix();
    let mut spec = WorkloadSpec::small(seed, &prefix);
    spec.duration = Duration::from_millis(4_000);
    let keys = spec.key_names();
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    cluster.every_link(lossy()).await;
    let result = workload::run(cluster, &spec, &FaultSchedule::none()).await?;
    cluster.heal().await;
    let seen = verify_convergence(cluster, &keys, Duration::from_millis(30_000)).await?;
    let summary = result.summary();
    let mut c = Check::new("what every member sees after a lossy workload and a heal");
    c.eq("keys agreed on by every member", keys.len(), seen.len());
    attach_state(cluster, &mut c).await;
    ctx.note(summary);
    ctx.note(format!("the members settled on {seen:?}"));
    c.finish()
});

dist_test!(revision_never_retreats, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    cluster.every_link(lossy()).await;
    let mut per_member: Vec<Vec<i64>> = vec![Vec::new(); cluster.members.len()];
    for i in 0..8 {
        put_patiently(cluster, &format!("{prefix}/k{i}"), "v", PATIENCE).await?;
        for m in cluster.running() {
            if let Ok(s) = cluster.members[m].client.status().await {
                per_member[m].push(s.header.revision);
            }
        }
    }
    cluster.heal().await;
    let mut c = Check::new("the revisions members reported over a lossy link");
    let mut high = 0;
    for (m, seq) in per_member.iter().enumerate() {
        if seq.is_empty() {
            continue;
        }
        check_non_decreasing(&mut c, &format!("m{}.status.header.revision", m + 1), seq);
        c.observe(&format!("m{}.revisions", m + 1), seq);
        high = high.max(seq.iter().copied().max().unwrap_or(0));
    }
    c.at_least("the highest revision any member reported", 8, high);
    attach_state(cluster, &mut c).await;
    c.finish()
});

dist_test!(values_are_unchanged, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    // Write once cleanly, then overwrite over the lossy link: the second value is the one
    // every member has to end up with, and a transport that quietly drops an entry leaves
    // somebody holding the first.
    let (first, _) = write_keys(cluster, &prefix, 4).await?;
    cluster.every_link(lossy()).await;
    let mut second = Vec::new();
    for w in &first {
        let value = format!("{}-again", w.value);
        let revision = put_patiently(cluster, &w.key, &value, PATIENCE).await?;
        second.push(Written {
            key: w.key.clone(),
            value,
            revision,
        });
    }
    cluster.heal().await;
    let last = second.last().map(|w| w.revision).unwrap_or(0);
    cluster
        .wait_for_revision(last, Duration::from_millis(30_000))
        .await?;
    let mut c = Check::new("four keys overwritten over a lossy link");
    check_every_member_holds(cluster, &second, &mut c).await;
    attach_state(cluster, &mut c).await;
    c.finish()
});

dist_test!(a_leader_is_named_again, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let before = cluster.wait_for_leader(ELECT).await?;
    cluster.every_link(lossy()).await;
    let (written, _) = write_keys(cluster, &prefix, 3).await?;
    cluster.heal().await;
    let after = cluster.wait_for_leader(ELECT).await?;
    let last = written.last().map(|w| w.revision).unwrap_or(0);
    cluster
        .wait_for_revision(last, Duration::from_millis(30_000))
        .await?;
    let described = cluster.describe().await;
    let mut c = Check::new("the leader of a cluster whose links have recovered");
    // Which member leads is not the point and is not asserted; that one member does, and
    // that everyone names the same one, is what `wait_for_leader` has just proved.
    c.that("leader index", "one of the three members", after < 3, after);
    check_every_member_holds(cluster, &written, &mut c).await;
    attach_state(cluster, &mut c).await;
    ctx.note(format!(
        "leader m{} before the loss, m{} after the heal — {described}",
        before + 1,
        after + 1
    ));
    c.finish()
});

dist_test!(the_injector_did_something, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    // Half the connections rather than a tenth, because this test is about proving the
    // injector works at all: at one in ten a whole run can go by without a refusal, and an
    // assertion that fails one run in thirty is worse than no assertion.
    cluster
        .every_link(LinkFault {
            delay: Duration::from_millis(40),
            drop_connection: 0.5,
            ..Default::default()
        })
        .await;
    // Peer connections are only refused when a peer dials, so the cluster has to be kept
    // busy until one of them does.
    let deadline = Instant::now() + Duration::from_millis(20_000);
    let mut blocked = cluster.stats.blocked();
    let mut round = 0;
    while blocked == 0 && Instant::now() < deadline {
        let _ = put_patiently(
            cluster,
            &format!("{prefix}/k{round}"),
            "v",
            Duration::from_millis(4_000),
        )
        .await;
        round += 1;
        tokio::time::sleep(Duration::from_millis(250)).await;
        blocked = cluster.stats.blocked();
    }
    let stats = cluster.stats.summary();
    cluster.heal().await;
    let mut c = Check::new("what the fault injector actually did");
    // Aimed at the suite, not at the program: if nothing was ever refused then every other
    // test in this stage ran against a healthy network and proved nothing.
    c.that(
        "cluster.stats.blocked()",
        "at least one peer connection refused, or the stage tested nothing",
        blocked > 0,
        blocked,
    );
    attach_state(cluster, &mut c).await;
    ctx.note(stats);
    c.finish()
});
