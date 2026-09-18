//! Stage 49 — removing a member.
//!
//! The mirror of stage 48, and the more dangerous direction. An add can only ever make the
//! quorum harder to reach; a remove makes it easier, which is what makes removal the tool for
//! getting a cluster that has lost a member back to a healthy majority — and also what makes
//! a careless one catastrophic. Remove two members of three in a hurry and the survivor is a
//! cluster of one that will happily accept writes the other two never saw.
//!
//! Three things the tests below insist on. The id is what identifies a member, not the name
//! and not the URL, so a remove of an id nobody has must be refused rather than reported as a
//! no-op. The removed member stops counting the moment the change commits, so what was a
//! three-member cluster needing two votes becomes a two-member cluster needing two. And the
//! cluster may not be emptied: there is no such thing as a cluster of nought, so the last
//! member cannot remove itself.

use crate::assert::{Check, Failure, FailureKind};
use crate::cluster::Cluster;
use crate::dist_test;
use crate::etcd::Client;
use crate::examples::{cluster_example, step, ExampleSpec};
use crate::stages::{expect_error, ok, wait_until, Ladder, Stage, Test};
use serde_json::json;
use std::time::{Duration, Instant};

/// Stage 49.
pub fn stage() -> Stage {
    Stage {
        number: 49,
        slug: "member_remove",
        name: "Removing a member",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "POST /v3/cluster/member/remove takes the member id, not its name",
            "The quorum size changes with the membership, so removing a member can restore availability",
            "The removed member must stop being counted, and the rest must keep serving",
            "Removing a member that does not exist is an error, not a silent success",
        ],
        examples,
        tests: vec![
            Test::new("the member list shrinks to two", the_list_shrinks)
                .ext()
                .fresh()
                .min_timeout_ms(60_000),
            Test::new(
                "the cluster keeps serving once a member is removed",
                the_cluster_keeps_serving,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "nothing written before the removal is lost by it",
                nothing_is_lost,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "the removed member is no longer counted anywhere",
                the_removed_member_is_not_counted,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "two of two is the quorum once the third is gone",
                the_quorum_size_changes,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "removing the leader triggers an election and the cluster recovers",
                removing_the_leader,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "removing a member id that does not exist is an error",
                removing_an_id_that_is_not_there,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "you cannot remove your way below one member",
                the_last_member_cannot_be_removed,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![cluster_example("The ids a removal is addressed by", || {
        vec![
            step(
                0,
                "/v3/cluster/member/list",
                json!({}),
                "every member, with the id that identifies it",
            ),
            step(
                2,
                "/v3/maintenance/status",
                json!({}),
                "m3's own id, as it reports it",
            ),
            step(
                0,
                "/v3/kv/put",
                json!({"key": "ZXg0OS9r", "value": "a2VwdA=="}),
                "a write that must outlive any membership change",
            ),
        ]
    })
    .request("the member list, one member's own id, and a write")
    .response("three ids, the same id twice, and a header")
    .note(
        "`POST /v3/cluster/member/remove` takes `{\"ID\":\"<id>\"}` and answers with the list \
         that is left. The id is the decimal form of the 64-bit number in this list — `name` \
         is a label and `peerURLs` can be edited, so neither identifies anything. An example \
         cannot run the removal without shrinking the cluster every later example shares, so \
         what it shows is the lookup: find the id here, send it there. Note that the number is \
         a JSON string, as every 64-bit field on this wire is.",
    )]
}

/// Which members the cluster still lists, by name.
///
/// Read from a member's own `/v3/cluster/member/list` rather than from the harness's idea of
/// the cluster, because the point of every test here is whether the *cluster* agrees that a
/// member has gone.
async fn member_names(client: &Client) -> Result<Vec<String>, crate::assert::Failure> {
    let list = ok(client.member_list().await, "/v3/cluster/member/list")?;
    let mut names: Vec<String> = list.members.iter().map(|m| m.name.clone()).collect();
    names.sort();
    Ok(names)
}

/// Wait until every member that is still running names one leader, and that leader is one of
/// them.
///
/// [`Cluster::wait_for_leader`] is satisfied as soon as the running members agree, and for a
/// second or so after a leader is removed they do agree: they all still name the member that
/// has just gone, because nobody has missed a heartbeat yet. A test that took that answer and
/// sent its next write to the member it named would get a connection refused and blame the
/// removal. Insisting that the leader named is itself still running is what makes the answer
/// usable.
async fn live_leader(cluster: &mut Cluster, within: Duration) -> Result<usize, Failure> {
    let deadline = Instant::now() + within;
    let mut last;
    loop {
        let alive = cluster.running();
        let mut leaders = Vec::new();
        for i in &alive {
            leaders.push(cluster.leader_according_to(*i).await);
        }
        let agreed = leaders
            .first()
            .copied()
            .flatten()
            .filter(|l| alive.contains(l) && leaders.iter().all(|x| *x == Some(*l)));
        if let Some(l) = agreed {
            return Ok(l);
        }
        last = format!(
            "members {:?} name leaders {:?}",
            alive.iter().map(|i| i + 1).collect::<Vec<_>>(),
            leaders.iter().map(|l| l.map(|x| x + 1)).collect::<Vec<_>>()
        );
        if Instant::now() >= deadline {
            return Err(Failure::new(
                FailureKind::Assertion,
                format!(
                    "no member that is still running was named leader by all of them within {} ms",
                    within.as_millis()
                ),
            )
            .note(last));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Remove a member, waiting for the cluster to consider itself well enough connected.
///
/// A leader refuses a configuration change unless it has been in continuous contact with
/// every voting member for the last few seconds, because a membership change made across a
/// link that is about to fail is how a cluster loses its quorum for good. A cluster that has
/// only just been started has not been connected for long enough yet, and the refusal —
/// `unhealthy cluster` — is about the cluster's recent history rather than about the member
/// being removed. The removal is offered again until it is accepted.
async fn remove_settled(
    cluster: &mut Cluster,
    victim: usize,
    through: usize,
) -> Result<(), Failure> {
    let started = Instant::now();
    let mut last;
    loop {
        match cluster.remove_member(victim, through).await {
            Ok(()) => return Ok(()),
            Err(f) => last = f.messages.join("; "),
        }
        if started.elapsed() >= Duration::from_millis(25_000) {
            return Err(Failure::new(
                FailureKind::Assertion,
                format!(
                    "m{} could not be removed within {} ms",
                    victim + 1,
                    started.elapsed().as_millis()
                ),
            )
            .note(last)
            .note(cluster.faults.describe()));
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

dist_test!(the_list_shrinks, |ctx| {
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let victim = (leader + 1) % cluster.initial_size;
    let survivor = (leader + 2) % cluster.initial_size;
    let gone = cluster.members[victim].name.clone();
    remove_settled(cluster, victim, leader).await?;
    live_leader(cluster, Duration::from_millis(30_000)).await?;
    // Both survivors have to have applied the same configuration entry, so both are asked.
    let from_leader = member_names(cluster.client(leader)).await?;
    let from_survivor = member_names(cluster.client(survivor)).await?;
    let described = cluster.describe().await;
    let mut c = Check::new("the member list after one of three was removed");
    c.note(format!("{gone} was removed through m{}", leader + 1));
    c.note(described);
    c.eq("the list according to the leader", 2, from_leader.len());
    c.eq(
        "the list according to the other survivor",
        from_leader.clone(),
        from_survivor,
    );
    c.that(
        "the removed member's name",
        "gone from the list",
        !from_leader.contains(&gone),
        from_leader,
    );
    c.finish()
});

dist_test!(the_cluster_keeps_serving, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let victim = (leader + 1) % cluster.initial_size;
    let survivor = (leader + 2) % cluster.initial_size;
    let before = ok(
        cluster
            .client(leader)
            .put(format!("{prefix}/before").as_bytes(), b"before")
            .await,
        "a write before the removal",
    )?;
    remove_settled(cluster, victim, leader).await?;
    live_leader(cluster, Duration::from_millis(30_000)).await?;
    // Two members is a smaller cluster, not a broken one: it has a leader, it takes writes,
    // and both of its members serve reads.
    let mut refused = Vec::new();
    let mut last = before.header.revision;
    for round in 0..10 {
        let through = if round % 2 == 0 { leader } else { survivor };
        let name = cluster.members[through].name.clone();
        let key = format!("{prefix}/after{round:02}").into_bytes();
        match cluster.client(through).put(&key, b"after").await {
            Ok(p) => last = p.header.revision,
            Err(e) => refused.push(format!("{name} refused round {round}: {e}")),
        }
    }
    cluster
        .wait_for_revision(last, Duration::from_millis(20_000))
        .await?;
    let described = cluster.describe().await;
    let mut c = Check::new("ten writes through a cluster that has just lost a member");
    c.note(described);
    c.that(
        "writes after the removal",
        "all ten acknowledged, through either survivor",
        refused.is_empty(),
        refused,
    );
    c.at_least("the last revision", before.header.revision + 10, last);
    c.finish()
});

dist_test!(nothing_is_lost, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let victim = (leader + 1) % cluster.initial_size;
    let survivor = (leader + 2) % cluster.initial_size;
    let mut last = 0;
    for i in 0..60 {
        let key = format!("{prefix}/{i:02}").into_bytes();
        last = ok(
            cluster
                .client(leader)
                .put(&key, format!("v{i}").as_bytes())
                .await,
            "a write made while the cluster still had three members",
        )?
        .header
        .revision;
    }
    cluster
        .wait_for_revision(last, Duration::from_millis(20_000))
        .await?;
    remove_settled(cluster, victim, leader).await?;
    live_leader(cluster, Duration::from_millis(30_000)).await?;
    // Shrinking the configuration says nothing about the data: every key a quorum accepted is
    // still a key a quorum accepted, and both remaining members must still have it.
    let mut wrong = Vec::new();
    for member in [leader, survivor] {
        let name = cluster.members[member].name.clone();
        for i in 0..60 {
            let key = format!("{prefix}/{i:02}").into_bytes();
            let want = format!("v{i}");
            match cluster.client(member).value_of(&key).await {
                Ok(Some(v)) if v == want.as_bytes() => {}
                other => wrong.push(format!(
                    "{name} {} -> {other:?}",
                    String::from_utf8_lossy(&key)
                )),
            }
        }
    }
    let described = cluster.describe().await;
    let mut c = Check::new("the sixty keys written before a member was removed");
    c.note(described);
    c.that(
        "the keys on both survivors",
        "all sixty present and correct on each",
        wrong.is_empty(),
        wrong.into_iter().take(10).collect::<Vec<_>>(),
    );
    c.finish()
});

dist_test!(the_removed_member_is_not_counted, |ctx| {
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let victim = (leader + 1) % cluster.initial_size;
    let survivor = (leader + 2) % cluster.initial_size;
    let removed_id = cluster.members[victim].member_id;
    remove_settled(cluster, victim, leader).await?;
    live_leader(cluster, Duration::from_millis(30_000)).await?;
    let mut sightings = Vec::new();
    for member in [leader, survivor] {
        let name = cluster.members[member].name.clone();
        let list = ok(
            cluster.client(member).member_list().await,
            "/v3/cluster/member/list after the removal",
        )?;
        if list.members.iter().any(|m| m.id == removed_id) {
            sightings.push(format!("{name} still lists {removed_id:#x}"));
        }
        let status = ok(cluster.client(member).status().await, "a survivor's status")?;
        if status.leader == removed_id {
            sightings.push(format!("{name} still names {removed_id:#x} as leader"));
        }
    }
    let running = cluster.members[victim].running();
    let described = cluster.describe().await;
    let mut c = Check::new("every trace of the member that was removed");
    c.note(format!("m{} was removed ({removed_id:#x})", victim + 1));
    c.note(described);
    c.ne("the removed member's id", 0, removed_id);
    c.that(
        "the removed id in the survivors' lists and status",
        "gone from both",
        sightings.is_empty(),
        sightings,
    );
    c.that(
        "the removed member's process",
        "stopped, because a removed member has nothing left to serve",
        !running,
        running,
    );
    c.finish()
});

dist_test!(the_quorum_size_changes, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let victim = (leader + 1) % cluster.initial_size;
    let survivor = (leader + 2) % cluster.initial_size;
    remove_settled(cluster, victim, leader).await?;
    let two = live_leader(cluster, Duration::from_millis(30_000)).await?;
    ok(
        cluster
            .client(two)
            .put(format!("{prefix}/two").as_bytes(), b"two of two")
            .await,
        "a write with both remaining members alive",
    )?;
    // Two members need two votes. That is the price of the removal, and it has to be real:
    // killing one of the two must stop the writes, exactly as killing two of three would.
    let alone = if two == survivor { leader } else { survivor };
    cluster.kill(two).await;
    let last_one = cluster.client(alone);
    wait_until(
        "the last member to give up on having a leader",
        Duration::from_millis(20_000),
        || {
            let c = last_one;
            async move { matches!(c.status().await, Ok(s) if s.leader == 0) }
        },
    )
    .await?;
    let refused = expect_error(
        cluster
            .client(alone)
            .put(format!("{prefix}/one").as_bytes(), b"one of two")
            .await,
        "a write with one of the two remaining members alive",
    )?;
    let code = refused.code();
    let described = cluster.describe().await;
    let mut c = Check::new("what one member out of two can decide");
    c.note(format!(
        "m{} was removed, then m{} was killed, leaving m{}",
        victim + 1,
        two + 1,
        alone + 1
    ));
    c.note(described);
    c.block("what the last member answered", refused.to_string());
    c.that(
        "error.code",
        "14 (unavailable) or 4 (deadline exceeded), or no code at all when the request simply \
         never came back",
        code.is_none() || code == Some(14) || code == Some(4),
        code,
    );
    c.finish()
});

dist_test!(removing_the_leader, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let through = (leader + 1) % cluster.initial_size;
    let other = (leader + 2) % cluster.initial_size;
    ok(
        cluster
            .client(leader)
            .put(
                format!("{prefix}/before").as_bytes(),
                b"written by the old leader",
            )
            .await,
        "a write by the leader that is about to be removed",
    )?;
    // Removing the leader is a configuration change the leader itself has to commit before it
    // can stand down, and an election has to follow it without anyone asking.
    remove_settled(cluster, leader, through).await?;
    let fresh = live_leader(cluster, Duration::from_millis(30_000)).await?;
    let put = ok(
        cluster
            .client(fresh)
            .put(
                format!("{prefix}/after").as_bytes(),
                b"written by the new leader",
            )
            .await,
        "a write by whoever was elected in its place",
    )?;
    cluster
        .wait_for_revision(put.header.revision, Duration::from_millis(20_000))
        .await?;
    let kept = ok(
        cluster
            .client(other)
            .value_of(format!("{prefix}/before").as_bytes())
            .await,
        "the old leader's last write, read from a survivor",
    )?;
    let names = member_names(cluster.client(fresh)).await?;
    let described = cluster.describe().await;
    let mut c = Check::new("the cluster after its leader was removed");
    c.note(format!("m{} was the leader that was removed", leader + 1));
    c.note(described);
    c.that(
        "the member elected afterwards",
        "one of the two that were left",
        fresh == through || fresh == other,
        fresh + 1,
    );
    c.eq("the member list afterwards", 2, names.len());
    c.eq(
        "the old leader's last write",
        Some(b"written by the old leader".to_vec()),
        kept,
    );
    c.finish()
});

dist_test!(removing_an_id_that_is_not_there, |ctx| {
    let seed = ctx.seed;
    let cluster = ctx.cluster()?;
    live_leader(cluster, Duration::from_millis(20_000)).await?;
    // An id nobody has, derived from the seed so a failing run repeats exactly.
    let mut bogus = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    while cluster.members.iter().any(|m| m.member_id == bogus) {
        bogus = bogus.wrapping_add(1);
    }
    let refused = expect_error(
        cluster.client(0).member_remove(bogus).await,
        "removing an id that belongs to nobody",
    )?;
    let names = member_names(cluster.client(0)).await?;
    let described = cluster.describe().await;
    let mut c = Check::new("a removal addressed to an id the cluster has never had");
    c.note(format!("the id offered was {bogus:#x}"));
    c.note(described);
    c.block("what the cluster answered", refused.to_string());
    c.eq(
        "the member list after the refused removal",
        vec!["m1".to_string(), "m2".into(), "m3".into()],
        names,
    );
    c.finish()
});

dist_test!(the_last_member_cannot_be_removed, |ctx| {
    let alone_key = ctx.key("alone");
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let first = (leader + 1) % cluster.initial_size;
    let second = (leader + 2) % cluster.initial_size;
    remove_settled(cluster, first, leader).await?;
    live_leader(cluster, Duration::from_millis(30_000)).await?;
    remove_settled(cluster, second, leader).await?;
    live_leader(cluster, Duration::from_millis(30_000)).await?;
    let alone = member_names(cluster.client(leader)).await?;
    // A cluster of nought is not a smaller cluster, it is a deleted one, and there would be
    // nobody left to commit the entry that deleted it. The last member has to refuse.
    let last_id = cluster.members[leader].member_id;
    let refused = expect_error(
        cluster.client(leader).member_remove(last_id).await,
        "the only member of a cluster removing itself",
    )?;
    let still_there = member_names(cluster.client(leader)).await?;
    let serving = cluster.client(leader).put(&alone_key, b"still here").await;
    let described = cluster.describe().await;
    let mut c = Check::new("a one-member cluster asked to remove its last member");
    c.note(format!("m{} is the member left", leader + 1));
    c.note(described);
    c.block("what the last member answered", refused.to_string());
    c.eq("the member list after two removals", 1, alone.len());
    c.eq(
        "the member list after the refused removal",
        alone,
        still_there,
    );
    c.that(
        "the last member after refusing",
        "still serving writes",
        serving.is_ok(),
        serving.err().map(|e| e.to_string()),
    );
    c.finish()
});
