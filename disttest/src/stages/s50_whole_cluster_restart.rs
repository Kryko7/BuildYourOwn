//! Stage 50 — Restarting the whole cluster loses nothing.
//!
//! Every member is stopped and started again from its own data directory. Nothing is
//! reformatted, no member is given `--initial-cluster-state new`, and no data directory is
//! thrown away. What comes back has to be the same cluster: the same cluster id, the same
//! member ids, the same values at the same revisions, and a revision counter that carries
//! on from where it stopped rather than starting again at one.
//!
//! The trap this stage catches is a store that keeps its keys but forgets its bookkeeping.
//! A member that replays its log but restarts the revision counter, or that rejoins under a
//! fresh member id, will answer every read correctly and still be broken: a client holding
//! a revision from before the restart now reads a completely different point in history,
//! and a watcher resumes from a revision that has been handed out twice. Keeping the values
//! is the easy half. Keeping the numbers that name them is the half that fails.

use crate::assert::{Check, Failure};
use crate::cluster::Cluster;
use crate::dist_test;
use crate::etcd::Client;
use crate::examples::{cluster_example, step, ExampleSpec};
use crate::stages::{ok, Ladder, Stage, Test};
use serde_json::json;
use std::time::Duration;

/// How long a restarted cluster is given to name a leader again.
const ELECT: Duration = Duration::from_millis(25_000);

/// Stage 50.
pub fn stage() -> Stage {
    Stage {
        number: 50,
        slug: "whole_cluster_restart",
        name: "Restarting the whole cluster loses nothing",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "Every member restarts from its own data directory with no reformatting",
            "The cluster re-elects a leader and continues from the revision it had",
            "Every acknowledged write is still there, with the same revisions",
            "A member that restarts must not rejoin as a brand new one",
        ],
        examples,
        tests: vec![
            Test::new(
                "every acknowledged write survives a restart of every member",
                writes_survive,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new("the cluster elects a leader again", leader_again)
                .ext()
                .fresh()
                .min_timeout_ms(60_000),
            Test::new(
                "the revision continues rather than restarting",
                revision_continues,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "the member list and the cluster id are unchanged",
                identity_is_unchanged,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "the cluster serves writes again after a restart",
                serves_again,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new("two restarts in a row lose nothing", two_restarts)
                .ext()
                .fresh()
                .min_timeout_ms(90_000),
            Test::new(
                "a restart after a partition has healed loses nothing",
                restart_after_a_partition,
            )
            .ext()
            .fresh()
            .min_timeout_ms(90_000),
            Test::new(
                "every member still holds the same data",
                members_agree_after_restart,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new("five members restart together", five_members_restart)
                .ext()
                .cluster(5)
                .min_timeout_ms(90_000),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![cluster_example("What a restart has to preserve", || {
        vec![
            step(
                0,
                "/v3/kv/put",
                json!({"key": "czUwL2sw", "value": "djA="}),
                "write s50/k0 through m1 and note the revision it was given",
            ),
            step(
                0,
                "/v3/kv/range",
                json!({"key": "czUwL2sw"}),
                "read it back: mod_revision is the number a restart must preserve",
            ),
            step(
                1,
                "/v3/maintenance/status",
                json!({}),
                "m2's revision and cluster id, the two numbers a restart must not change",
            ),
            step(
                2,
                "/v3/cluster/member/list",
                json!({}),
                "and the member ids, which a restarted member must keep",
            ),
        ]
    })
    .request("a write, a read of its revision, and the cluster's own identity")
    .response("a value at a revision, and a cluster id and member ids that outlive a restart")
    .note(
        "The harness stops every member and starts it again from the same data directory, \
         then asks these same questions. A store that keeps the values but hands out a fresh \
         revision counter, or a member that rejoins under a new id, passes a read-back test \
         and still breaks every client holding a revision from before the restart.",
    )]
}

/// One acknowledged write: the key, the value, and the revision it was given.
#[derive(Debug, Clone)]
struct Written {
    key: String,
    value: String,
    revision: i64,
}

/// Write `n` keys through one member, remembering the revision of each acknowledgement.
async fn write_keys(client: &Client, prefix: &str, n: usize) -> Result<Vec<Written>, Failure> {
    let mut out = Vec::new();
    for i in 0..n {
        let key = format!("{prefix}/k{i}");
        let value = format!("v{i}");
        let put = ok(
            client.put(key.as_bytes(), value.as_bytes()).await,
            &format!("put {key}"),
        )?;
        out.push(Written {
            key,
            value,
            revision: put.header.revision,
        });
    }
    Ok(out)
}

/// Demand that every running member holds every acknowledged write, at the same revision.
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

/// Attach everything a reader of a failed restart wants: who is running, who leads, and
/// what the fault table holds.
async fn attach_state(cluster: &mut Cluster, c: &mut Check) {
    if c.ok() {
        return;
    }
    let described = cluster.describe().await;
    c.note(described);
    c.note(cluster.faults.describe());
    c.block("what the members printed", cluster.output());
}

dist_test!(writes_survive, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT).await?;
    let written = write_keys(cluster.client(leader), &prefix, 8).await?;
    let last = written.last().map(|w| w.revision).unwrap_or(0);
    cluster
        .wait_for_revision(last, Duration::from_millis(15_000))
        .await?;
    cluster.restart_all().await?;
    cluster.wait_for_leader(ELECT).await?;
    let mut c = Check::new("eight acknowledged writes, after every member was restarted");
    check_every_member_holds(cluster, &written, &mut c).await;
    attach_state(cluster, &mut c).await;
    c.finish()
});

dist_test!(leader_again, |ctx| {
    let cluster = ctx.cluster()?;
    let before = cluster.wait_for_leader(ELECT).await?;
    let term_before = ok(cluster.client(before).status().await, "the leader's status")?.raft_term;
    cluster.restart_all().await?;
    let after = cluster.wait_for_leader(ELECT).await?;
    let status = ok(cluster.client(after).status().await, "the leader's status")?;
    let described = cluster.describe().await;
    let mut c = Check::new("the leader a restarted cluster elects");
    c.that(
        "leader index after the restart",
        "one of the three members",
        after < 3,
        after,
    );
    c.eq(
        "leader.status.leader",
        status.header.member_id,
        status.leader,
    );
    // A term is a count of elections and nothing resets it, so whatever the restarted
    // cluster settles on may not be smaller than what it had before it stopped.
    c.at_least("raft_term after the restart", term_before, status.raft_term);
    c.observe("leader before and after", (before + 1, after + 1));
    ctx.note(format!("after the restart: {described}"));
    c.finish()
});

dist_test!(revision_continues, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT).await?;
    let written = write_keys(cluster.client(leader), &prefix, 5).await?;
    let before = written.last().map(|w| w.revision).unwrap_or(0);
    cluster
        .wait_for_revision(before, Duration::from_millis(15_000))
        .await?;
    cluster.restart_all().await?;
    let leader = cluster.wait_for_leader(ELECT).await?;
    let after = ok(cluster.client(leader).status().await, "the leader's status")?
        .header
        .revision;
    let next = ok(
        cluster
            .client(leader)
            .put(format!("{prefix}/after").as_bytes(), b"again")
            .await,
        "a put after the restart",
    )?
    .header
    .revision;
    let mut c = Check::new("the revision counter across a restart");
    c.at_least("status.header.revision after the restart", before, after);
    c.that(
        "put.header.revision after the restart",
        "a revision past the last one acknowledged before the restart",
        next > before,
        (before, next),
    );
    attach_state(cluster, &mut c).await;
    c.finish()
});

dist_test!(identity_is_unchanged, |ctx| {
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    let before = ok(cluster.client(0).member_list().await, "the member list")?;
    let cluster_id = before.header.cluster_id;
    let mut ids_before: Vec<(String, u64)> = before
        .members
        .iter()
        .map(|m| (m.name.clone(), m.id))
        .collect();
    ids_before.sort();
    cluster.restart_all().await?;
    cluster.wait_for_leader(ELECT).await?;
    let after = ok(
        cluster.client(0).member_list().await,
        "the member list after the restart",
    )?;
    let mut ids_after: Vec<(String, u64)> = after
        .members
        .iter()
        .map(|m| (m.name.clone(), m.id))
        .collect();
    ids_after.sort();
    let mut c = Check::new("the identity of a restarted cluster");
    c.eq("member_list.members.len()", 3, after.members.len());
    c.eq("member_list names and ids", ids_before, ids_after);
    for i in 0..3 {
        let name = cluster.members[i].name.clone();
        let s = ok(
            cluster.client(i).status().await,
            "a member's status after the restart",
        )?;
        c.eq(
            &format!("{name}.header.cluster_id"),
            cluster_id,
            s.header.cluster_id,
        );
    }
    attach_state(cluster, &mut c).await;
    c.finish()
});

dist_test!(serves_again, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT).await?;
    cluster.restart_all().await?;
    let leader = cluster.wait_for_leader(ELECT).await?;
    let written = write_keys(cluster.client(leader), &prefix, 4).await?;
    let last = written.last().map(|w| w.revision).unwrap_or(0);
    cluster
        .wait_for_revision(last, Duration::from_millis(15_000))
        .await?;
    let mut c = Check::new("four writes made after the whole cluster was restarted");
    check_every_member_holds(cluster, &written, &mut c).await;
    attach_state(cluster, &mut c).await;
    c.finish()
});

dist_test!(two_restarts, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT).await?;
    let mut written = write_keys(cluster.client(leader), &prefix, 4).await?;
    cluster
        .wait_for_revision(
            written.last().map(|w| w.revision).unwrap_or(0),
            Duration::from_millis(15_000),
        )
        .await?;
    cluster.restart_all().await?;
    let leader = cluster.wait_for_leader(ELECT).await?;
    // Write again between the two restarts, so the second one has to preserve entries that
    // were only ever appended by a restarted leader.
    written.extend(write_keys(cluster.client(leader), &format!("{prefix}/round2"), 4).await?);
    cluster
        .wait_for_revision(
            written.last().map(|w| w.revision).unwrap_or(0),
            Duration::from_millis(15_000),
        )
        .await?;
    cluster.restart_all().await?;
    cluster.wait_for_leader(ELECT).await?;
    let mut c = Check::new("eight writes across two whole-cluster restarts");
    check_every_member_holds(cluster, &written, &mut c).await;
    attach_state(cluster, &mut c).await;
    c.finish()
});

dist_test!(restart_after_a_partition, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT).await?;
    // Cut the leader's own follower away, write through the majority, then heal and let the
    // straggler catch up before everything is stopped.
    let minority = (leader + 1) % 3;
    let majority: Vec<usize> = (0..3).filter(|i| *i != minority).collect();
    cluster.partition(&[vec![minority], majority.clone()]).await;
    let written = write_keys(cluster.client(leader), &prefix, 5).await?;
    let last = written.last().map(|w| w.revision).unwrap_or(0);
    cluster.heal().await;
    cluster
        .wait_for_revision(last, Duration::from_millis(20_000))
        .await?;
    cluster.restart_all().await?;
    cluster.wait_for_leader(ELECT).await?;
    let mut c = Check::new("writes made during a partition, after the heal and a restart");
    check_every_member_holds(cluster, &written, &mut c).await;
    attach_state(cluster, &mut c).await;
    c.finish()
});

dist_test!(members_agree_after_restart, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT).await?;
    let written = write_keys(cluster.client(leader), &prefix, 6).await?;
    let last = written.last().map(|w| w.revision).unwrap_or(0);
    cluster
        .wait_for_revision(last, Duration::from_millis(15_000))
        .await?;
    cluster.restart_all().await?;
    cluster.wait_for_leader(ELECT).await?;
    let keys: Vec<String> = written.iter().map(|w| w.key.clone()).collect();
    let seen =
        crate::cluster::workload::verify_convergence(cluster, &keys, Duration::from_millis(20_000))
            .await?;
    let mut c = Check::new("what every member sees once it has restarted");
    c.eq("keys agreed on by every member", written.len(), seen.len());
    for (w, (key, value)) in written.iter().zip(seen.iter()) {
        c.eq("converged key", &w.key, key);
        c.eq(
            &format!("converged value of {key}"),
            Some(w.value.clone()),
            value.clone(),
        );
    }
    attach_state(cluster, &mut c).await;
    c.finish()
});

dist_test!(five_members_restart, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT).await?;
    let written = write_keys(cluster.client(leader), &prefix, 6).await?;
    let last = written.last().map(|w| w.revision).unwrap_or(0);
    cluster
        .wait_for_revision(last, Duration::from_millis(20_000))
        .await?;
    cluster.restart_all().await?;
    let leader = cluster.wait_for_leader(ELECT).await?;
    let described = cluster.describe().await;
    let mut c = Check::new("a five-member cluster, restarted whole");
    c.that(
        "leader index",
        "one of the five members",
        leader < 5,
        leader,
    );
    c.eq("members running", 5, cluster.running().len());
    check_every_member_holds(cluster, &written, &mut c).await;
    attach_state(cluster, &mut c).await;
    ctx.note(described);
    c.finish()
});
