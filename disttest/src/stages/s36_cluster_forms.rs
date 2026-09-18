//! Stage 36 — a cluster forms and elects one leader.
//!
//! The first cluster stage, and the one that proves the harness's own plumbing as much as
//! the program's: three members, each advertising a peer URL that points at a proxy, all
//! given the same `--initial-cluster`, all agreeing on one leader and one term.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{cluster_example, step, ExampleSpec};
use crate::stages::{majority, ok, Ladder, Stage, Test};
use serde_json::json;
use std::time::Duration;

/// Stage 36.
pub fn stage() -> Stage {
    Stage {
        number: 36,
        slug: "cluster_forms",
        name: "A cluster forms and elects one leader",
        ext: false,
        ladder: Ladder::Cluster,
        hints: &[
            "--initial-cluster lists every member as name=peer-url, and every member gets the same list",
            "Dial the peer URL you were told, not the address the member listens on",
            "/v3/maintenance/status reports the leader's member id and the current raft term",
            "Every member must name the same leader, and the term must be the same on all of them",
        ],
        examples,
        tests: vec![
            Test::new("every member answers", every_member_answers).min_timeout_ms(40_000),
            Test::new("one leader, named by everyone", one_leader).min_timeout_ms(40_000),
            Test::new("the leader is a member of the cluster", leader_is_a_member),
            Test::new("every member reports the same cluster id", same_cluster_id),
            Test::new("member ids are distinct", distinct_member_ids),
            Test::new("the member list names every member", member_list),
            Test::new("the term is the same everywhere", same_term),
            Test::new("a quorum is a majority of three", quorum_is_two),
            Test::new("the leader survives being asked repeatedly", leader_is_stable).ext(),
            Test::new("five members also form one cluster", five_members)
                .cluster(5)
                .ext()
                .min_timeout_ms(60_000),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![cluster_example("Three members, one leader", || {
        vec![
            step(0, "/v3/maintenance/status", json!({}), "ask m1 who leads"),
            step(1, "/v3/maintenance/status", json!({}), "ask m2 who leads"),
            step(2, "/v3/maintenance/status", json!({}), "ask m3 who leads"),
            step(
                0,
                "/v3/cluster/member/list",
                json!({}),
                "and who is in the cluster at all",
            ),
        ]
    })
    .request("the same status request to each of the three members")
    .response("three answers naming one leader id and one raft term")
    .note(
        "`leader` is a member id, not a name or an index: look it up in the member list. A \
         member that is mid-election answers leader 0, which is why the harness polls until \
         every member agrees rather than asking once.",
    )]
}

dist_test!(every_member_answers, |ctx| {
    let cluster = ctx.cluster()?;
    let mut c = Check::new("three members answering /v3/maintenance/status");
    for i in 0..3 {
        let name = cluster.members[i].name.clone();
        match cluster.client(i).status().await {
            Ok(s) => {
                c.ne(&format!("{name}.header.member_id"), 0, s.header.member_id);
                c.at_least(&format!("{name}.header.revision"), 1, s.header.revision);
            }
            Err(e) => {
                c.that(&format!("{name}.status"), "an answer", false, e.to_string());
            }
        }
    }
    c.finish()
});

dist_test!(one_leader, |ctx| {
    let cluster = ctx.cluster()?;
    let leader = cluster
        .wait_for_leader(Duration::from_millis(15_000))
        .await?;
    let described = cluster.describe().await;
    ctx.note(format!("leader m{} — {described}", leader + 1));
    Ok(())
});

dist_test!(leader_is_a_member, |ctx| {
    let cluster = ctx.cluster()?;
    let leader = cluster
        .wait_for_leader(Duration::from_millis(15_000))
        .await?;
    let status = ok(cluster.client(leader).status().await, "the leader's status")?;
    let mut c = Check::new("the member the cluster named as leader");
    c.eq(
        "leader.status.leader",
        status.header.member_id,
        status.leader,
    );
    c.that(
        "leader index",
        "one of the three members",
        leader < 3,
        leader,
    );
    c.finish()
});

dist_test!(same_cluster_id, |ctx| {
    let cluster = ctx.cluster()?;
    let first = ok(cluster.client(0).status().await, "m1's status")?;
    let mut c = Check::new("the cluster id on every member");
    for i in 1..3 {
        let name = cluster.members[i].name.clone();
        let s = ok(cluster.client(i).status().await, "a member's status")?;
        c.eq(
            &format!("{name}.header.cluster_id"),
            first.header.cluster_id,
            s.header.cluster_id,
        );
    }
    c.finish()
});

dist_test!(distinct_member_ids, |ctx| {
    let cluster = ctx.cluster()?;
    let mut ids = Vec::new();
    for i in 0..3 {
        ids.push(
            ok(cluster.client(i).status().await, "a member's status")?
                .header
                .member_id,
        );
    }
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    let mut c = Check::new("three member ids");
    c.eq("distinct member ids", 3, sorted.len());
    c.observe("member ids", &ids);
    c.finish()
});

dist_test!(member_list, |ctx| {
    let cluster = ctx.cluster()?;
    let list = ok(
        cluster.client(0).member_list().await,
        "/v3/cluster/member/list",
    )?;
    let mut names: Vec<String> = list.members.iter().map(|m| m.name.clone()).collect();
    names.sort();
    let mut c = Check::new("the member list");
    c.eq("member_list.members.len()", 3, list.members.len());
    c.eq(
        "member_list names",
        vec!["m1".to_string(), "m2".into(), "m3".into()],
        names,
    );
    for m in &list.members {
        c.that(
            &format!("member_list[{}].peerURLs", m.name),
            "at least one peer URL",
            !m.peer_urls.is_empty(),
            m.peer_urls.clone(),
        );
        c.eq(
            &format!("member_list[{}].isLearner", m.name),
            false,
            m.is_learner,
        );
    }
    c.finish()
});

dist_test!(same_term, |ctx| {
    let cluster = ctx.cluster()?;
    cluster
        .wait_for_leader(Duration::from_millis(15_000))
        .await?;
    let mut terms = Vec::new();
    for i in 0..3 {
        terms.push(ok(cluster.client(i).status().await, "a member's status")?.raft_term);
    }
    let mut c = Check::new("the raft term on every member");
    c.at_least("m1.raft_term", 1, terms[0]);
    c.eq("m2.raft_term", terms[0], terms[1]);
    c.eq("m3.raft_term", terms[0], terms[2]);
    c.finish()
});

dist_test!(quorum_is_two, |_ctx| {
    // Not a property of the program: a check that the suite and the learner agree on what a
    // quorum of three is before stage 40 starts relying on it.
    let mut c = Check::new("the arithmetic the partition stages rest on");
    c.eq("majority(3)", 2, majority(3));
    c.eq("majority(5)", 3, majority(5));
    c.finish()
});

dist_test!(leader_is_stable, |ctx| {
    let cluster = ctx.cluster()?;
    let first = cluster
        .wait_for_leader(Duration::from_millis(15_000))
        .await?;
    let mut seen = Vec::new();
    for _ in 0..5 {
        seen.push(cluster.leader_according_to(0).await);
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    let mut c = Check::new("the leader of a quiet cluster");
    c.that(
        "leader over five polls",
        "the same leader every time, with no election in a quiet cluster",
        seen.iter().all(|l| *l == Some(first)),
        seen.iter().map(|l| l.map(|x| x + 1)).collect::<Vec<_>>(),
    );
    c.finish()
});

dist_test!(five_members, |ctx| {
    let cluster = ctx.cluster()?;
    let leader = cluster
        .wait_for_leader(Duration::from_millis(25_000))
        .await?;
    let list = ok(
        cluster.client(0).member_list().await,
        "/v3/cluster/member/list",
    )?;
    let mut c = Check::new("a five-member cluster");
    c.eq("member_list.members.len()", 5, list.members.len());
    c.that(
        "leader index",
        "one of the five members",
        leader < 5,
        leader,
    );
    let described = cluster.describe().await;
    ctx.note(described);
    c.finish()
});
