//! Stage 39 — Revisions and terms agree across members.
//!
//! Stage 37 asked for the same *value* on every member. This one asks for the same
//! *bookkeeping*: the identifiers, the revision, the term, the applied index and the
//! membership list a member reports are all claims about the cluster, and three members
//! that disagree about any of them have not really formed a cluster at all.
//!
//! The easy mistakes are all local shortcuts. A revision that counts the writes this member
//! handled rather than the entries the log holds; a term that is only bumped when this
//! member campaigns; an applied index copied from the raft index instead of tracking the
//! state machine; a member list rebuilt from `--initial-cluster` rather than from the log.
//! Each of those looks right on a single node and falls apart the moment a second member
//! answers a question.
//!
//! A quiet cluster is the whole setting here: nothing is killed and nothing is cut, so
//! every test shares one cluster and none of them is `.fresh()`. Where a number has to
//! settle before it can be compared, the test polls with [`wait_until`] rather than
//! sleeping and hoping.

use crate::assert::Check;
use crate::dist_test;
use crate::etcd::{b64, RangeRequest};
use crate::examples::{cluster_example, step, ExampleSpec};
use crate::stages::{ok, wait_until, Ladder, Stage, Test};
use serde_json::json;
use std::time::Duration;

/// How long the cluster is given to settle on one leader.
const ELECT_WITHIN: Duration = Duration::from_millis(15_000);
/// How long a number that has to settle is given to settle.
const SETTLE_WITHIN: Duration = Duration::from_millis(10_000);

/// Stage 39.
pub fn stage() -> Stage {
    Stage {
        number: 39,
        slug: "revision_agreement",
        name: "Revisions and terms agree across members",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "cluster_id is the same on every member; member_id is not",
            "Revisions are cluster-wide: two members never answer different revisions for the same state",
            "raft_term is the same on every member once an election has settled",
            "raft_applied_index catches up to raft_index on a quiet cluster",
        ],
        examples,
        tests: vec![
            Test::new(
                "the cluster id is the same on every member",
                one_cluster_id,
            )
            .ext()
            .min_timeout_ms(40_000),
            Test::new("no two members share a member id", distinct_member_ids)
                .ext()
                .min_timeout_ms(40_000),
            Test::new(
                "every member reports the same revision on a quiet cluster",
                one_revision,
            )
            .ext(),
            Test::new(
                "the raft term is the same everywhere once an election has settled",
                one_term,
            )
            .ext(),
            Test::new(
                "the applied index catches up to the raft index",
                applied_catches_up,
            )
            .ext(),
            Test::new(
                "two members never answer different values at the same revision",
                same_value_at_the_same_revision,
            )
            .ext(),
            Test::new(
                "the member list is the same seen from every member",
                one_member_list,
            )
            .ext(),
            Test::new("no member's revision ever goes backwards", no_going_back).ext(),
            Test::new(
                "the member that answered names itself in the header",
                the_header_names_the_answering_member,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        cluster_example("One cluster id, three member ids", || {
            vec![
                step(0, "/v3/maintenance/status", json!({}), "ask m1"),
                step(1, "/v3/maintenance/status", json!({}), "ask m2"),
                step(2, "/v3/maintenance/status", json!({}), "ask m3"),
            ]
        })
        .request("the same status request to each member of a quiet cluster")
        .response("three headers sharing a cluster_id, with three different member_ids, one term and one revision")
        .note(
            "`cluster_id` identifies the cluster and is the same in all three answers; \
             `member_id` identifies whoever answered and is different in all three. \
             `raft_index` is how far the log goes and `raft_applied_index` how far the \
             state machine has got; on a quiet cluster the second catches the first, which \
             is why the suite polls rather than comparing them once.",
        ),
        cluster_example("The same revision seen twice", || {
            vec![
                step(
                    0,
                    "/v3/kv/put",
                    json!({"key": b64(b"agree"), "value": b64(b"one")}),
                    "write through m1",
                ),
                step(
                    1,
                    "/v3/kv/range",
                    json!({"key": b64(b"agree")}),
                    "read on m2 and note the revision",
                ),
                step(
                    2,
                    "/v3/kv/range",
                    json!({"key": b64(b"agree")}),
                    "read the same key on m3",
                ),
            ]
        })
        .request("one write, then the same read on two different members")
        .response("the same value, the same mod_revision, and headers that agree")
        .note(
            "A revision is a position in one shared log, not a per-member counter. Two \
             members answering the same key with different `mod_revision`s have two \
             different histories, which is the bug this stage exists to catch.",
        ),
    ]
}

dist_test!(one_cluster_id, |ctx| {
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT_WITHIN).await?;
    let first = ok(cluster.client(0).status().await, "m1's status")?;
    let mut c = Check::new("the cluster id every member reports");
    c.ne("m1.header.cluster_id", 0, first.header.cluster_id);
    for i in 1..cluster.initial_size {
        let name = cluster.members[i].name.clone();
        let s = ok(
            cluster.client(i).status().await,
            &format!("{name}'s status"),
        )?;
        c.eq(
            &format!("{name}.header.cluster_id"),
            first.header.cluster_id,
            s.header.cluster_id,
        );
    }
    let faults = cluster.faults.describe();
    c.note(faults);
    c.finish()
});

dist_test!(distinct_member_ids, |ctx| {
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT_WITHIN).await?;
    let mut ids = Vec::new();
    for i in 0..cluster.initial_size {
        let name = cluster.members[i].name.clone();
        let s = ok(
            cluster.client(i).status().await,
            &format!("{name}'s status"),
        )?;
        ids.push((name, s.header.member_id));
    }
    let mut unique: Vec<u64> = ids.iter().map(|(_, id)| *id).collect();
    unique.sort_unstable();
    unique.dedup();
    let mut c = Check::new("the member ids of a three-member cluster");
    c.eq("distinct member ids", cluster.initial_size, unique.len());
    for (name, id) in &ids {
        c.ne(&format!("{name}.header.member_id"), 0, *id);
    }
    c.observe("member ids", &ids);
    c.finish()
});

dist_test!(one_revision, |ctx| {
    let key = ctx.key("rev");
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT_WITHIN).await?;
    let put = ok(cluster.client(0).put(&key, b"v").await, "a write on m1")?;
    let target = put.header.revision;
    // Replication is not instantaneous, so the members are given a moment to agree; what
    // the stage forbids is their *disagreeing once things are quiet*, not their lagging by
    // a millisecond.
    let size = cluster.initial_size;
    let clients: Vec<&crate::etcd::Client> = (0..size).map(|i| cluster.client(i)).collect();
    wait_until(
        "every member to report the same revision",
        SETTLE_WITHIN,
        || async {
            let mut seen = Vec::new();
            for client in &clients {
                match client.status().await {
                    Ok(s) => seen.push(s.header.revision),
                    Err(_) => return false,
                }
            }
            seen.iter().all(|r| *r == seen[0]) && seen[0] >= target
        },
    )
    .await?;
    let mut revisions = Vec::new();
    for i in 0..size {
        revisions.push(
            ok(cluster.client(i).status().await, "a status")?
                .header
                .revision,
        );
    }
    let mut c = Check::new("the revision every member reports once the cluster is quiet");
    for i in 1..size {
        let name = cluster.members[i].name.clone();
        c.eq(
            &format!("{name}.status.header.revision"),
            revisions[0],
            revisions[i],
        );
    }
    c.at_least("m1.status.header.revision", target, revisions[0]);
    c.finish()
});

dist_test!(one_term, |ctx| {
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT_WITHIN).await?;
    let mut terms = Vec::new();
    for i in 0..cluster.initial_size {
        let name = cluster.members[i].name.clone();
        let s = ok(
            cluster.client(i).status().await,
            &format!("{name}'s status"),
        )?;
        // The header's term and the top-level term describe the same thing and must not
        // drift apart in one answer, never mind between two members.
        terms.push((name, s.raft_term, s.header.raft_term));
    }
    let mut c = Check::new("the raft term every member reports");
    let first = terms[0].1;
    c.at_least("m1.status.raft_term", 1, first);
    for (name, term, header_term) in &terms {
        c.eq(&format!("{name}.status.raft_term"), first, *term);
        c.eq(
            &format!("{name}.status.header.raft_term"),
            *term,
            *header_term,
        );
    }
    c.finish()
});

dist_test!(applied_catches_up, |ctx| {
    let key = ctx.key("applied");
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT_WITHIN).await?;
    for n in 0..5 {
        ok(
            cluster
                .client(0)
                .put(&key, format!("v{n}").as_bytes())
                .await,
            "a write on m1",
        )?;
    }
    let size = cluster.initial_size;
    let clients: Vec<&crate::etcd::Client> = (0..size).map(|i| cluster.client(i)).collect();
    // The state machine is always a little behind the log while entries are in flight; on
    // a cluster nobody is writing to, it must draw level rather than stay behind for ever.
    wait_until(
        "every member's applied index to reach its raft index",
        SETTLE_WITHIN,
        || async {
            for client in &clients {
                match client.status().await {
                    Ok(s) => {
                        if s.raft_applied_index < s.raft_index || s.raft_index == 0 {
                            return false;
                        }
                    }
                    Err(_) => return false,
                }
            }
            true
        },
    )
    .await?;
    let mut c = Check::new("the raft index and the applied index of a quiet cluster");
    for i in 0..size {
        let name = cluster.members[i].name.clone();
        let s = ok(
            cluster.client(i).status().await,
            &format!("{name}'s status"),
        )?;
        c.at_least(&format!("{name}.status.raft_index"), 1, s.raft_index);
        c.eq(
            &format!("{name}.status.raft_applied_index"),
            s.raft_index,
            s.raft_applied_index,
        );
    }
    c.finish()
});

dist_test!(same_value_at_the_same_revision, |ctx| {
    let key = ctx.key("mvcc");
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT_WITHIN).await?;
    // Four versions of one key, each at a revision the cluster handed back. Reading any of
    // those revisions on any member has exactly one right answer.
    let mut written = Vec::new();
    for n in 0..4 {
        let value = format!("v{n}");
        let put = ok(
            cluster
                .client(n % cluster.initial_size)
                .put(&key, value.as_bytes())
                .await,
            "a write",
        )?;
        written.push((put.header.revision, value));
    }
    let last = written.last().map(|(r, _)| *r).unwrap_or(0);
    cluster.wait_for_revision(last, SETTLE_WITHIN).await?;
    let mut c = Check::new("a historical read taken on every member");
    for (revision, value) in &written {
        for i in 0..cluster.initial_size {
            let name = cluster.members[i].name.clone();
            let read = ok(
                cluster
                    .client(i)
                    .range_req(&RangeRequest::key(&key).at_revision(*revision))
                    .await,
                &format!("a read on {name} at revision {revision}"),
            )?;
            c.eq(
                &format!("{name}.range(revision {revision}).kvs[0].value"),
                value.clone(),
                read.one().map(|kv| kv.value_str()).unwrap_or_default(),
            );
        }
    }
    c.finish()
});

dist_test!(one_member_list, |ctx| {
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT_WITHIN).await?;
    let mut views = Vec::new();
    for i in 0..cluster.initial_size {
        let name = cluster.members[i].name.clone();
        let list = ok(
            cluster.client(i).member_list().await,
            &format!("the member list according to {name}"),
        )?;
        // The list is a set, not a sequence: two members may answer in any order, and
        // nothing in this stage depends on which.
        let mut rows: Vec<(u64, String, Vec<String>, bool)> = list
            .members
            .iter()
            .map(|m| {
                let mut urls = m.peer_urls.clone();
                urls.sort();
                (m.id, m.name.clone(), urls, m.is_learner)
            })
            .collect();
        rows.sort();
        views.push((name, rows));
    }
    let mut c = Check::new("the member list every member answers with");
    let first = views[0].1.clone();
    c.eq(
        "m1.member_list.members.len()",
        cluster.initial_size,
        first.len(),
    );
    for (name, rows) in &views {
        c.eq(&format!("{name}.member_list"), first.clone(), rows.clone());
    }
    c.finish()
});

dist_test!(no_going_back, |ctx| {
    let key = ctx.key("monotonic");
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let mut lowest = vec![0i64; size];
    let mut c = Check::new("the revision each member reports over a run of writes");
    for round in 0..8 {
        ok(
            cluster
                .client(round % size)
                .put(&key, format!("r{round}").as_bytes())
                .await,
            "a write",
        )?;
        for (i, low) in lowest.iter_mut().enumerate() {
            let name = cluster.members[i].name.clone();
            let s = ok(
                cluster.client(i).status().await,
                &format!("{name}'s status"),
            )?;
            // A member may be behind, but the number it reports may never shrink: that
            // would mean the store had forgotten something it had already announced.
            c.at_least(
                &format!("{name}.status.header.revision in round {round}"),
                *low,
                s.header.revision,
            );
            *low = (*low).max(s.header.revision);
        }
    }
    c.finish()
});

dist_test!(the_header_names_the_answering_member, |ctx| {
    let key = ctx.key("who");
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT_WITHIN).await?;
    let mut ids = Vec::new();
    for i in 0..cluster.initial_size {
        ids.push(
            ok(cluster.client(i).status().await, "a status")?
                .header
                .member_id,
        );
    }
    let mut c = Check::new("the member_id in the header of a read and a write");
    for (i, id) in ids.iter().enumerate() {
        let name = cluster.members[i].name.clone();
        // A write is forwarded to the leader, but the answer still comes back through the
        // member the client asked, and that is the member the header names.
        let put = ok(
            cluster.client(i).put(&key, b"v").await,
            &format!("a write through {name}"),
        )?;
        let read = ok(
            cluster.client(i).get_key(&key).await,
            &format!("a read on {name}"),
        )?;
        c.eq(
            &format!("{name}.put.header.member_id"),
            *id,
            put.header.member_id,
        );
        c.eq(
            &format!("{name}.range.header.member_id"),
            *id,
            read.header.member_id,
        );
    }
    c.finish()
});
