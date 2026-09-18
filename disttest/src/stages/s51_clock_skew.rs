//! Stage 51 — Clock skew changes nothing about safety.
//!
//! Raft never compares wall-clock times to decide anything. A log entry is ordered by its
//! term and its index; a vote is granted on the strength of a log, not on the strength of a
//! clock; a commit is a counting argument over members, not over seconds. Wall time appears
//! in exactly one place — the timers that decide when to campaign and when to beat — and
//! those are durations measured locally, not instants compared across machines. That is the
//! property this stage is about: skewing time must not change a single value.
//!
//! **Why this stage skews arrival instead of clocks.** The harness starts your program as an
//! ordinary process and cannot move its clock: there is no privileged call it is willing to
//! make, `libfaketime` is not something a conformance suite should require, and a container
//! per member would change what is being tested. So the stage tests the honest consequence
//! instead. A member whose clock runs behind, or that is paused, or that is scheduled late,
//! is indistinguishable *from the outside* from a member whose peer traffic arrives tens or
//! hundreds of milliseconds after it was sent — heartbeats look old, votes look late,
//! appends look stale. `every_link(LinkFault { delay })` produces exactly that, on every
//! link at once, and the stage demands what a clock-independent design promises: elections
//! may happen, terms may climb, throughput may collapse, and not one value may change.
//!
//! The trap is a design that reaches for a timestamp when it wants an order: an entry
//! tagged with the writer's clock and resolved last-write-wins, a lease checked against a
//! remote member's idea of now, a read served locally because "the lease has not expired
//! yet by my clock". Each of those turns a clock difference into a lost write, and each of
//! them survives every earlier stage in this ladder.

use crate::assert::{Check, Failure, FailureKind};
use crate::cluster::proxy::LinkFault;
use crate::cluster::workload::verify_convergence;
use crate::cluster::Cluster;
use crate::dist_test;
use crate::examples::{cluster_example, step, ExampleSpec};
use crate::stages::{check_non_decreasing, check_strictly_increasing, ok, Ladder, Stage, Test};
use serde_json::json;
use std::time::{Duration, Instant};

/// How long a skewed cluster is given to name a leader.
const ELECT: Duration = Duration::from_millis(30_000);
/// How long one write is given to find a member willing to take it.
const PATIENCE: Duration = Duration::from_millis(25_000);

/// Stage 51.
pub fn stage() -> Stage {
    Stage {
        number: 51,
        slug: "clock_skew",
        name: "Clock skew changes nothing about safety",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "Raft safety rests on terms and indexes, never on wall-clock time",
            "A member whose clock is hours off must still agree on every value",
            "Lease expiry may be affected; linearizability may not",
            "Never compare timestamps from two different machines to order writes",
        ],
        examples,
        tests: vec![
            Test::new("a 50 ms delay on every link changes no value", delay_50)
                .ext()
                .fresh()
                .min_timeout_ms(60_000),
            Test::new("a 200 ms delay on every link changes no value", delay_200)
                .ext()
                .fresh()
                .min_timeout_ms(60_000),
            Test::new(
                "the revision sequence stays strictly increasing under a delay",
                revisions_increase,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "every member converges once the delay is removed",
                converges_after_heal,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "the leader may change under a large delay, the values may not",
                leader_may_change,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new("a delay on one link alone changes nothing", one_slow_link)
                .ext()
                .fresh()
                .min_timeout_ms(60_000),
            Test::new(
                "no member's revision goes backwards while peer traffic is skewed",
                revision_never_retreats,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "writes made under skew are all readable once it is gone",
                readable_after_the_skew,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        cluster_example("What a skewed member may and may not change", || {
            vec![
                step(
                    0,
                    "/v3/kv/put",
                    json!({"key": "czUxL2sw", "value": "c2tld2Vk"}),
                    "write s51/k0 while peer traffic is arriving late",
                ),
                step(
                    1,
                    "/v3/maintenance/status",
                    json!({}),
                    "m2's term: it may have climbed, which is allowed",
                ),
                step(
                    2,
                    "/v3/kv/range",
                    json!({"key": "czUxL2sw"}),
                    "m3's copy of the value: this is what may not change",
                ),
            ]
        })
        .request(
            "a write under delayed peer traffic, then a term and a value from two other members",
        )
        .response("a possibly higher term everywhere, and the same value on every member")
        .note(
            "The harness cannot move a member's clock, so it delays the arrival of that member's \
         peer traffic instead, which is what a skewed or descheduled member looks like from \
         outside. Read the two answers in the right order: the term is allowed to differ from \
         the one before the delay, the value is not.",
        ),
    ]
}

/// One acknowledged write.
#[derive(Debug, Clone)]
struct Written {
    key: String,
    value: String,
    revision: i64,
}

/// Put one key, trying every running member in turn until one takes it.
///
/// Under a large delay the leader can be mid-election when a client arrives, and refusing
/// the write is the correct answer at that instant. Retrying is not papering over a bug: a
/// cluster that never accepts the write inside `within` fails the test.
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

/// Write `n` keys while whatever fault is in place stays in place.
async fn write_keys(
    cluster: &mut Cluster,
    prefix: &str,
    n: usize,
) -> Result<Vec<Written>, Failure> {
    let mut out = Vec::new();
    for i in 0..n {
        let key = format!("{prefix}/k{i}");
        let value = format!("v{i}");
        let revision = put_patiently(cluster, &key, &value, PATIENCE).await?;
        out.push(Written {
            key,
            value,
            revision,
        });
    }
    Ok(out)
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

/// Skew the arrival of every member's peer traffic by `ms` milliseconds.
async fn skew_every_link(cluster: &mut Cluster, ms: u64) {
    cluster
        .every_link(LinkFault {
            delay: Duration::from_millis(ms),
            ..Default::default()
        })
        .await;
}

/// The body shared by the 50 ms and the 200 ms tests.
async fn values_survive_a_delay(
    cluster: &mut Cluster,
    prefix: &str,
    ms: u64,
) -> Result<Check, Failure> {
    cluster.wait_for_leader(ELECT).await?;
    skew_every_link(cluster, ms).await;
    let written = write_keys(cluster, prefix, 5).await?;
    cluster.heal().await;
    let last = written.last().map(|w| w.revision).unwrap_or(0);
    cluster
        .wait_for_revision(last, Duration::from_millis(30_000))
        .await?;
    let mut c = Check::new(format!(
        "five writes made while every link delayed peer traffic by {ms} ms"
    ));
    check_every_member_holds(cluster, &written, &mut c).await;
    attach_state(cluster, &mut c).await;
    Ok(c)
}

dist_test!(delay_50, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let mut c = values_survive_a_delay(cluster, &prefix, 50).await?;
    c.finish()
});

dist_test!(delay_200, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let mut c = values_survive_a_delay(cluster, &prefix, 200).await?;
    c.finish()
});

dist_test!(revisions_increase, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    skew_every_link(cluster, 120).await;
    let written = write_keys(cluster, &prefix, 8).await?;
    cluster.heal().await;
    let revisions: Vec<i64> = written.iter().map(|w| w.revision).collect();
    let mut c = Check::new("the revisions eight delayed writes were acknowledged with");
    check_strictly_increasing(&mut c, "revisions", &revisions);
    c.observe("revisions", &revisions);
    attach_state(cluster, &mut c).await;
    c.finish()
});

dist_test!(converges_after_heal, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    skew_every_link(cluster, 200).await;
    let written = write_keys(cluster, &prefix, 5).await?;
    cluster.heal().await;
    let keys: Vec<String> = written.iter().map(|w| w.key.clone()).collect();
    let seen = verify_convergence(cluster, &keys, Duration::from_millis(30_000)).await?;
    let mut c = Check::new("what every member sees once the skew is gone");
    c.eq("keys agreed on by every member", written.len(), seen.len());
    for (w, (key, value)) in written.iter().zip(seen.iter()) {
        c.eq(
            &format!("converged value of {key}"),
            Some(w.value.clone()),
            value.clone(),
        );
    }
    attach_state(cluster, &mut c).await;
    c.finish()
});

dist_test!(leader_may_change, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let before = cluster.wait_for_leader(ELECT).await?;
    let term_before = ok(cluster.client(before).status().await, "the leader's status")?.raft_term;
    // Large enough that a heartbeat can miss its window: an election here is correct
    // behaviour, not a failure, and the values are what the test is actually about.
    skew_every_link(cluster, 250).await;
    let written = write_keys(cluster, &prefix, 4).await?;
    cluster.heal().await;
    let after = cluster.wait_for_leader(ELECT).await?;
    let term_after = ok(cluster.client(after).status().await, "the leader's status")?.raft_term;
    let last = written.last().map(|w| w.revision).unwrap_or(0);
    cluster
        .wait_for_revision(last, Duration::from_millis(30_000))
        .await?;
    let mut c = Check::new("four writes made across a delay big enough to cost an election");
    // Deliberately no assertion on who leads: a changed leader is one of the correct
    // outcomes here. The term may only ever move forwards.
    c.at_least("raft_term after the delay", term_before, term_after);
    check_every_member_holds(cluster, &written, &mut c).await;
    attach_state(cluster, &mut c).await;
    ctx.note(format!(
        "leader m{} in term {term_before} before the delay, m{} in term {term_after} after it",
        before + 1,
        after + 1
    ));
    c.finish()
});

dist_test!(one_slow_link, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT).await?;
    // Only the two followers' link is slow, so the leader can still reach a quorum
    // promptly: this is the shape where a timestamp-ordered store quietly picks a winner.
    let a = (leader + 1) % 3;
    let b = (leader + 2) % 3;
    cluster
        .link_fault(
            a,
            b,
            LinkFault {
                delay: Duration::from_millis(200),
                ..Default::default()
            },
        )
        .await;
    let written = write_keys(cluster, &prefix, 5).await?;
    cluster.heal().await;
    let last = written.last().map(|w| w.revision).unwrap_or(0);
    cluster
        .wait_for_revision(last, Duration::from_millis(30_000))
        .await?;
    let mut c = Check::new("five writes made while one link ran 200 ms late");
    check_every_member_holds(cluster, &written, &mut c).await;
    attach_state(cluster, &mut c).await;
    ctx.note(format!("the slow link was m{}–m{}", a + 1, b + 1));
    c.finish()
});

dist_test!(revision_never_retreats, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    skew_every_link(cluster, 150).await;
    // Watch one member's idea of the revision while the writes go in: it may stall while
    // it waits for late traffic, but it may never hand back a smaller number than it
    // already handed back.
    let mut per_member: Vec<Vec<i64>> = vec![Vec::new(); cluster.members.len()];
    for i in 0..6 {
        let key = format!("{prefix}/k{i}");
        put_patiently(cluster, &key, "v", PATIENCE).await?;
        for m in cluster.running() {
            if let Ok(s) = cluster.members[m].client.status().await {
                per_member[m].push(s.header.revision);
            }
        }
    }
    cluster.heal().await;
    let mut c = Check::new("the revisions members reported while peer traffic was late");
    let mut high = 0;
    for (m, seq) in per_member.iter().enumerate() {
        if seq.is_empty() {
            continue;
        }
        check_non_decreasing(&mut c, &format!("m{}.status.header.revision", m + 1), seq);
        c.observe(&format!("m{}.revisions", m + 1), seq);
        high = high.max(seq.iter().copied().max().unwrap_or(0));
    }
    c.at_least("the highest revision any member reported", 6, high);
    attach_state(cluster, &mut c).await;
    c.finish()
});

dist_test!(readable_after_the_skew, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    skew_every_link(cluster, 200).await;
    let written = write_keys(cluster, &prefix, 6).await?;
    cluster.heal().await;
    cluster.wait_for_leader(ELECT).await?;
    let last = written.last().map(|w| w.revision).unwrap_or(0);
    cluster
        .wait_for_revision(last, Duration::from_millis(30_000))
        .await?;
    let mut c = Check::new("six writes made under skew, read back once the skew is gone");
    check_every_member_holds(cluster, &written, &mut c).await;
    let revisions: Vec<i64> = written.iter().map(|w| w.revision).collect();
    check_strictly_increasing(&mut c, "revisions", &revisions);
    attach_state(cluster, &mut c).await;
    c.finish()
});
