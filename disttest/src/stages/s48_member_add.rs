//! Stage 48 — adding a member.
//!
//! Membership is not configuration that sits beside the log; it is in the log. The add is an
//! entry, it commits like any other entry, and from the moment it commits the quorum size has
//! changed for everybody — which is why the cluster can go on serving across it and why the
//! new member starts counting the instant it has caught up.
//!
//! Two traps, and both bite before the new member ever answers a request. The first is the
//! order: the cluster has to be told about the member before the member is started, because a
//! process that boots claiming to be part of a cluster nobody has heard of is a second
//! cluster. The second is `--initial-cluster-state existing`: the new member must be told
//! that the cluster already exists and must be given the full list including itself, or it
//! will bootstrap a fresh one and quietly diverge.
//!
//! Every test here asks for `.spare(1)`, which configures a fourth member with its own data
//! directory, ports and proxy but leaves it stopped until the test adds it.

use crate::assert::{Check, Failure, FailureKind};
use crate::cluster::Cluster;
use crate::dist_test;
use crate::examples::{cluster_example, step, ExampleSpec};
use crate::stages::{expect_error, ok, Ladder, Stage, Test};
use serde_json::json;
use std::time::{Duration, Instant};

/// The index of the spare member `.spare(1)` configures.
const SPARE: usize = 3;

/// Stage 48.
pub fn stage() -> Stage {
    Stage {
        number: 48,
        slug: "member_add",
        name: "Adding a member",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "POST /v3/cluster/member/add takes the new member's peer URLs and changes the configuration",
            "The new member starts with --initial-cluster-state existing and the full member list",
            "The cluster keeps serving throughout: a configuration change is one more log entry",
            "The new member catches up from the leader and then counts towards the quorum",
        ],
        examples,
        tests: vec![
            Test::new("the member list grows to four", the_list_grows)
                .ext()
                .spare(1)
                .min_timeout_ms(60_000),
            Test::new(
                "the new member catches up and answers every value",
                the_new_member_catches_up,
            )
            .ext()
            .spare(1)
            .min_timeout_ms(60_000),
            Test::new(
                "values written after the add reach the new member too",
                writes_after_the_add_arrive,
            )
            .ext()
            .spare(1)
            .min_timeout_ms(60_000),
            Test::new(
                "the cluster keeps serving before, during and after the add",
                the_cluster_keeps_serving,
            )
            .ext()
            .spare(1)
            .min_timeout_ms(60_000),
            Test::new(
                "the new member counts towards the quorum afterwards",
                the_new_member_counts,
            )
            .ext()
            .spare(1)
            .min_timeout_ms(60_000),
            Test::new(
                "adding a member that is already there is an error",
                adding_a_member_twice,
            )
            .ext()
            .spare(1)
            .min_timeout_ms(60_000),
            Test::new(
                "the new member reports the same cluster id, leader and term",
                the_same_cluster_and_term,
            )
            .ext()
            .spare(1)
            .min_timeout_ms(60_000),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        cluster_example("The member list a new member has to join", || {
            vec![
                step(
                    0,
                    "/v3/cluster/member/list",
                    json!({}),
                    "who is in the cluster before anything is added",
                ),
                step(
                    0,
                    "/v3/kv/put",
                    json!({"key": "ZXg0OC9r", "value": "YmVmb3Jl"}),
                    "a write the new member will have to catch up on",
                ),
                step(
                    1,
                    "/v3/maintenance/status",
                    json!({}),
                    "the revision it will have to reach",
                ),
            ]
        })
        .request("the member list and the revision of a cluster about to gain a member")
        .response("three members with their ids and peer URLs, and the revision to catch up to")
        .note(
            "An example cannot add a member without leaving a fourth one behind for every later \
         example, so this shows the two things the add is built out of. `POST \
         /v3/cluster/member/add` with `{\"peerURLs\":[\"http://...\"]}` answers with the new \
         member's id and the list it now belongs to, and only then is the process started, \
         with `--initial-cluster-state existing` and an `--initial-cluster` naming all four. \
         Starting it first, or leaving that flag off, bootstraps a second cluster that will \
         never merge with this one.",
        ),
    ]
}

/// Wait until every member in `among` names the same leader, ignoring everyone else.
///
/// [`Cluster::wait_for_leader`] insists that every *running* member agrees, which cannot
/// happen once one of them has been cut off. The side that still holds a quorum has to be
/// asked on its own, and the leader it names has to be one of its own members.
async fn leader_among(
    cluster: &mut Cluster,
    among: &[usize],
    within: Duration,
) -> Result<usize, Failure> {
    let deadline = Instant::now() + within;
    let mut last;
    loop {
        let mut leaders = Vec::new();
        for i in among {
            leaders.push(cluster.leader_according_to(*i).await);
        }
        let agreed = leaders
            .first()
            .copied()
            .flatten()
            .filter(|l| among.contains(l) && leaders.iter().all(|x| *x == Some(*l)));
        if let Some(l) = agreed {
            return Ok(l);
        }
        last = format!(
            "members {:?} name leaders {:?}",
            among.iter().map(|i| i + 1).collect::<Vec<_>>(),
            leaders.iter().map(|l| l.map(|x| x + 1)).collect::<Vec<_>>()
        );
        if Instant::now() >= deadline {
            return Err(Failure::new(
                FailureKind::Assertion,
                format!(
                    "members {:?} did not agree on a leader of their own within {} ms",
                    among.iter().map(|i| i + 1).collect::<Vec<_>>(),
                    within.as_millis()
                ),
            )
            .note(last)
            .note(cluster.faults.describe()));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Wait until every member that is still running names one leader, and that leader is one of
/// them.
///
/// [`Cluster::wait_for_leader`] is satisfied as soon as the running members agree, and for a
/// second or so after a leader stops running they do agree: they all still name the member
/// that has just gone, because nobody has missed a heartbeat yet. A test that took that
/// answer and sent its next request to the member it named would get a connection refused and
/// blame the wrong thing. Insisting that the leader named is itself still running is what
/// makes the answer usable.
async fn live_leader(cluster: &mut Cluster, within: Duration) -> Result<usize, Failure> {
    let alive = cluster.running();
    leader_among(cluster, &alive, within).await
}

/// Add the spare member, waiting for the cluster to consider itself well enough connected.
///
/// A configuration change made across a link that is about to fail is how a cluster loses its
/// quorum for good, so a leader refuses one unless it has been in continuous contact with
/// every peer for the last few seconds. A cluster that has only just been started has not
/// been connected for long enough yet, and what comes back is `unhealthy cluster` — a
/// statement about the cluster's recent history, not about the member being added. The add is
/// simply offered again until it is accepted, and the time that took is reported.
async fn add_spare(cluster: &mut Cluster, through: usize) -> Result<Duration, Failure> {
    let started = Instant::now();
    let mut last;
    loop {
        match cluster.add_member(SPARE, through).await {
            Ok(()) => return Ok(started.elapsed()),
            Err(f) => last = f.messages.join("; "),
        }
        if started.elapsed() >= Duration::from_millis(25_000) {
            return Err(Failure::new(
                FailureKind::Assertion,
                format!(
                    "the spare member could not be added within {} ms",
                    started.elapsed().as_millis()
                ),
            )
            .note(last)
            .note(cluster.faults.describe()));
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

dist_test!(the_list_grows, |ctx| {
    let cluster = ctx.cluster()?;
    live_leader(cluster, Duration::from_millis(20_000)).await?;
    let before = ok(
        cluster.client(0).member_list().await,
        "the member list before the add",
    )?;
    add_spare(cluster, 0).await?;
    live_leader(cluster, Duration::from_millis(30_000)).await?;
    // Every member has to have applied the same configuration entry, so the list is asked of
    // all four rather than of the one the add went through.
    let mut lists = Vec::new();
    for i in 0..=SPARE {
        let name = cluster.members[i].name.clone();
        let list = ok(
            cluster.client(i).member_list().await,
            &format!("the member list according to {name}"),
        )?;
        let mut names: Vec<String> = list.members.iter().map(|m| m.name.clone()).collect();
        names.sort();
        lists.push((name, names));
    }
    let described = cluster.describe().await;
    let mut c = Check::new("the member list after a fourth member was added");
    c.note(described);
    c.eq("the member list before the add", 3, before.members.len());
    for (name, names) in &lists {
        c.eq(
            &format!("the member list according to {name}"),
            vec!["m1".to_string(), "m2".into(), "m3".into(), "m4".into()],
            names.clone(),
        );
    }
    c.finish()
});

dist_test!(the_new_member_catches_up, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let mut last = 0;
    for i in 0..100 {
        let key = format!("{prefix}/{i:03}").into_bytes();
        last = ok(
            cluster
                .client(leader)
                .put(&key, format!("v{i}").as_bytes())
                .await,
            "a write made before the fourth member existed",
        )?
        .header
        .revision;
    }
    let started = Instant::now();
    add_spare(cluster, 0).await?;
    cluster
        .wait_for_revision(last, Duration::from_millis(40_000))
        .await?;
    let took = started.elapsed();
    // A member that has joined but not caught up is worse than no member at all: it has a
    // vote and nothing behind it. Every key written before it existed has to be there.
    let mut wrong = Vec::new();
    for i in 0..100 {
        let key = format!("{prefix}/{i:03}").into_bytes();
        let want = format!("v{i}");
        match cluster.client(SPARE).value_of(&key).await {
            Ok(Some(v)) if v == want.as_bytes() => {}
            other => wrong.push(format!("{} -> {other:?}", String::from_utf8_lossy(&key))),
        }
    }
    let described = cluster.describe().await;
    let mut c = Check::new("what the newly added member holds");
    c.note(described);
    c.that(
        "the hundred keys written before the add, on the new member",
        "all one hundred present and correct",
        wrong.is_empty(),
        wrong.into_iter().take(10).collect::<Vec<_>>(),
    );
    ctx.note(format!(
        "m4 was added and had caught up to revision {last} {} ms later",
        took.as_millis()
    ));
    c.finish()
});

dist_test!(writes_after_the_add_arrive, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    add_spare(cluster, 0).await?;
    live_leader(cluster, Duration::from_millis(30_000)).await?;
    let mut last = 0;
    for i in 0..40 {
        let key = format!("{prefix}/after{i:02}").into_bytes();
        last = ok(
            cluster
                .client(leader)
                .put(&key, format!("after-{i}").as_bytes())
                .await,
            "a write made once the fourth member had joined",
        )?
        .header
        .revision;
    }
    cluster
        .wait_for_revision(last, Duration::from_millis(30_000))
        .await?;
    let mut wrong = Vec::new();
    for i in 0..40 {
        let key = format!("{prefix}/after{i:02}").into_bytes();
        let want = format!("after-{i}");
        match cluster.client(SPARE).value_of(&key).await {
            Ok(Some(v)) if v == want.as_bytes() => {}
            other => wrong.push(format!("{} -> {other:?}", String::from_utf8_lossy(&key))),
        }
    }
    // The new member also has to be able to take a write itself, which means forwarding it to
    // the leader like any other follower does.
    let through_the_new_member = cluster
        .client(SPARE)
        .put(format!("{prefix}/its-own").as_bytes(), b"forwarded")
        .await;
    let described = cluster.describe().await;
    let mut c = Check::new("writes made after the fourth member joined");
    c.note(described);
    c.that(
        "the forty keys written after the add, on the new member",
        "all forty present and correct",
        wrong.is_empty(),
        wrong,
    );
    c.that(
        "a write sent to the new member",
        "forwarded to the leader and acknowledged",
        through_the_new_member.is_ok(),
        through_the_new_member.err().map(|e| e.to_string()),
    );
    c.finish()
});

dist_test!(the_cluster_keeps_serving, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    // A configuration change is one more entry in the same log, so there is no window in
    // which the cluster is allowed to stop answering. The three writes below straddle it.
    let before = cluster
        .client(leader)
        .put(format!("{prefix}/before").as_bytes(), b"before")
        .await;
    add_spare(cluster, 0).await?;
    let during = cluster
        .client(leader)
        .put(format!("{prefix}/during").as_bytes(), b"during")
        .await;
    live_leader(cluster, Duration::from_millis(30_000)).await?;
    let after = cluster
        .client(leader)
        .put(format!("{prefix}/after").as_bytes(), b"after")
        .await;
    let revisions: Vec<i64> = [&before, &during, &after]
        .iter()
        .map(|r| r.as_ref().map(|p| p.header.revision).unwrap_or(-1))
        .collect();
    let last = revisions.iter().copied().max().unwrap_or(-1);
    cluster
        .wait_for_revision(last, Duration::from_millis(30_000))
        .await?;
    let described = cluster.describe().await;
    let mut c = Check::new("writes made before, during and after a member was added");
    c.note(described);
    for (what, r) in [("before", &before), ("during", &during), ("after", &after)] {
        c.that(
            &format!("the write made {what} the add"),
            "acknowledged",
            r.is_ok(),
            r.as_ref().err().map(|e| e.to_string()),
        );
    }
    crate::stages::check_strictly_increasing(&mut c, "revisions", &revisions);
    c.finish()
});

dist_test!(the_new_member_counts, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    add_spare(cluster, 0).await?;
    live_leader(cluster, Duration::from_millis(30_000)).await?;
    // Four members need three to agree. Cutting off one of the three that were here before
    // leaves the leader with one old member and the new one, so a write can only commit if
    // the new member is really voting.
    let victim = (0..cluster.initial_size)
        .find(|i| *i != leader)
        .unwrap_or(0);
    let remaining: Vec<usize> = (0..=SPARE).filter(|i| *i != victim).collect();
    cluster.isolate(victim).await;
    let still_leads = leader_among(cluster, &remaining, Duration::from_millis(30_000)).await?;
    let key = format!("{prefix}/quorum").into_bytes();
    let put = cluster
        .client(still_leads)
        .put(&key, b"three of four")
        .await;
    let described = cluster.describe().await;
    let faults = cluster.faults.describe();
    let mut c = Check::new("a write needing three of four members, with one cut off");
    c.note(format!(
        "m{} was cut off after m4 joined; m{} leads",
        victim + 1,
        still_leads + 1
    ));
    c.note(described).note(faults);
    c.that(
        "a write with three of four members reachable",
        "acknowledged",
        put.is_ok(),
        put.as_ref().err().map(|e| e.to_string()),
    );
    if let Ok(p) = &put {
        let seen = ok(
            cluster.client(SPARE).value_of(&key).await,
            "the quorum write, read from the new member",
        )?;
        c.eq(
            "the new member's copy",
            Some(b"three of four".to_vec()),
            seen,
        );
        c.at_least("the write's revision", 1, p.header.revision);
    }
    c.finish()
});

dist_test!(adding_a_member_twice, |ctx| {
    let cluster = ctx.cluster()?;
    live_leader(cluster, Duration::from_millis(20_000)).await?;
    // m1 is in the cluster already, so its peer URL cannot be added again: a configuration
    // change that silently succeeded here would leave two entries claiming the same address.
    let existing = cluster.members[0].advertised_peer_url();
    let refused = expect_error(
        cluster
            .client(0)
            .member_add(std::slice::from_ref(&existing))
            .await,
        "adding a peer URL that is already a member",
    )?;
    let list = ok(
        cluster.client(0).member_list().await,
        "the member list after the refused add",
    )?;
    let described = cluster.describe().await;
    let mut c = Check::new("an add of a peer URL the cluster already knows");
    c.note(format!("the URL offered again was {existing}"));
    c.note(described);
    c.block("what the cluster answered", refused.to_string());
    c.eq(
        "the member list after the refused add",
        3,
        list.members.len(),
    );
    c.finish()
});

dist_test!(the_same_cluster_and_term, |ctx| {
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let first = ok(cluster.client(0).status().await, "m1's status")?;
    let before = ok(cluster.client(leader).status().await, "the leader's status")?;
    add_spare(cluster, 0).await?;
    let settled = live_leader(cluster, Duration::from_millis(30_000)).await?;
    let joined = ok(
        cluster.client(SPARE).status().await,
        "the new member's status",
    )?;
    let now = ok(
        cluster.client(settled).status().await,
        "the leader's status after the add",
    )?;
    // A new member with a cluster id of its own has bootstrapped a cluster of one and is
    // answering out of an empty store. From the outside it looks perfectly healthy, and it
    // agrees with nobody.
    let ids = ok(
        cluster.client(SPARE).member_list().await,
        "the member list according to the new member",
    )?;
    let names = cluster.leader_according_to(SPARE).await;
    let described = cluster.describe().await;
    let mut c = Check::new("the cluster, the leader and the term the new member reports");
    c.note(format!(
        "m{} led in term {} before the add",
        leader + 1,
        before.raft_term
    ));
    c.note(described);
    c.eq(
        "the new member's header.cluster_id",
        first.header.cluster_id,
        joined.header.cluster_id,
    );
    c.ne(
        "the new member's header.member_id",
        0,
        joined.header.member_id,
    );
    c.ne(
        "the new member's id against m1's",
        first.header.member_id,
        joined.header.member_id,
    );
    c.eq("the member list it reports", 4, ids.members.len());
    c.eq("the leader the new member names", Some(settled), names);
    c.eq(
        "the new member's raft term",
        now.raft_term,
        joined.raft_term,
    );
    c.at_least("the term after the add", before.raft_term, now.raft_term);
    c.finish()
});
