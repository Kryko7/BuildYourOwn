//! Stage 43 — A killed leader is replaced.
//!
//! Stages 40 to 42 took the network away; this one takes a process away. The leader is
//! `SIGKILL`ed — no shutdown, no handover, no last message — and the cluster has to notice,
//! elect somebody else and go back to serving, all on its own and within a bound.
//!
//! The bound is the point. "Eventually" is not an availability property: a follower that
//! stops hearing from the leader must start an election after its own election timeout, and
//! a cluster whose recovery takes a minute has failed even though it recovered. The tests
//! measure the gap and put it in the report, so a slow implementation shows up as a number
//! rather than as a feeling.
//!
//! Two invariants sit underneath. The new term must be strictly greater than the old one,
//! because that is what lets everyone tell the new leader's entries from the dead one's.
//! And only a member whose log is at least as up to date as the voter's may win, which is
//! what makes "the new leader still has every acknowledged write" true rather than lucky.
//!
//! Killing a *follower* is the control case: three members minus one follower is still a
//! quorum, so there is no election at all and the leader must stay exactly where it was.
//!
//! Every test kills a member, so every one is `.fresh()` with a generous floor under its
//! timeout. [`Cluster::wait_for_leader`] only asks members that are still running, but a
//! survivor can still name the dead leader for a moment, so these tests ask for a leader
//! that is itself among the survivors.

use crate::assert::{Check, Failure, FailureKind};
use crate::cluster::Cluster;
use crate::dist_test;
use crate::etcd::{b64, RangeRequest};
use crate::examples::{cluster_example, step, ExampleSpec};
use crate::stages::{majority, ok, wait_until, Ladder, Stage, Test};
use serde_json::json;
use std::time::{Duration, Instant};

/// How long the cluster is given to settle on one leader at the start of a test.
const ELECT_WITHIN: Duration = Duration::from_millis(15_000);
/// The bound an election after a kill has to finish inside.
const REELECT_WITHIN: Duration = Duration::from_millis(20_000);
/// The floor under every test in this stage.
const SLOW: u64 = 60_000;

/// Stage 43.
pub fn stage() -> Stage {
    Stage {
        number: 43,
        slug: "leader_election",
        name: "A killed leader is replaced",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "A follower that stops hearing from the leader starts an election",
            "Only a member whose log is at least as up to date may win",
            "The new term is strictly greater than the old one",
            "The cluster must be serving again within a bounded time, not eventually",
        ],
        examples,
        tests: vec![
            Test::new(
                "a killed leader is replaced within a bound",
                a_new_leader_appears,
            )
            .ext()
            .fresh()
            .min_timeout_ms(SLOW),
            Test::new(
                "the term after the election is strictly greater",
                the_term_goes_up,
            )
            .ext()
            .fresh()
            .min_timeout_ms(SLOW),
            Test::new(
                "the survivors agree on who the new leader is",
                the_survivors_agree,
            )
            .ext()
            .fresh()
            .min_timeout_ms(SLOW),
            Test::new(
                "the cluster accepts writes again after the election",
                writes_resume,
            )
            .ext()
            .fresh()
            .min_timeout_ms(SLOW),
            Test::new(
                "the new leader still holds every acknowledged write",
                nothing_acknowledged_is_lost,
            )
            .ext()
            .fresh()
            .min_timeout_ms(SLOW),
            Test::new(
                "the killed member rejoins when it is started again",
                the_dead_member_rejoins,
            )
            .ext()
            .fresh()
            .min_timeout_ms(90_000),
            Test::new(
                "killing a follower changes nothing",
                killing_a_follower_changes_nothing,
            )
            .ext()
            .fresh()
            .min_timeout_ms(SLOW),
            Test::new(
                "a five-member cluster survives two kills",
                five_members_survive_two_kills,
            )
            .ext()
            .cluster(5)
            .min_timeout_ms(90_000),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![cluster_example("Who leads, and in which term", || {
        vec![
            step(0, "/v3/maintenance/status", json!({}), "ask m1"),
            step(1, "/v3/maintenance/status", json!({}), "ask m2"),
            step(
                1,
                "/v3/kv/put",
                json!({"key": b64(b"after-the-election"), "value": b64(b"serving")}),
                "and write something, to prove the cluster is serving",
            ),
        ]
    })
    .request("two status requests and a write on a healthy three-member cluster")
    .response("one leader id, one raft term, and a put that succeeds")
    .note(
        "This is the shape of the answer a test takes before and after killing the leader. \
         The `leader` field is a member id, so it has to be looked up in the member list; a \
         member mid-election answers `leader: 0`, which is why the harness polls until the \
         survivors agree instead of asking once. Across a kill, two things have to change \
         together: `leader` names a different member, and `raft_term` is strictly larger. A \
         new leader in the *same* term would mean two members could both claim that term's \
         entries.",
    )]
}

/// Poll until every member of `group` names one leader that is itself in `group`.
///
/// After a kill the survivors can go on naming the dead member for a moment, so an
/// election is only over when the leader they agree on is one of the members still running.
async fn leader_among(
    cluster: &Cluster,
    group: &[usize],
    within: Duration,
) -> Result<usize, Failure> {
    let start = Instant::now();
    loop {
        let mut leaders = Vec::new();
        for i in group {
            leaders.push(cluster.leader_according_to(*i).await);
        }
        let agreed = leaders
            .first()
            .copied()
            .flatten()
            .filter(|l| group.contains(l) && leaders.iter().all(|x| *x == Some(*l)));
        if let Some(l) = agreed {
            return Ok(l);
        }
        if start.elapsed() >= within {
            return Err(Failure::new(
                FailureKind::Assertion,
                format!(
                    "members {:?} did not agree on a leader among themselves within {} ms",
                    group.iter().map(|i| i + 1).collect::<Vec<_>>(),
                    within.as_millis()
                ),
            )
            .note(format!(
                "they named {:?}",
                leaders.iter().map(|l| l.map(|x| x + 1)).collect::<Vec<_>>()
            ))
            .note(cluster.faults.describe()));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The lowest-numbered member that is not the leader.
fn a_follower(leader: usize, size: usize) -> usize {
    (0..size).find(|i| *i != leader).unwrap_or(0)
}

dist_test!(a_new_leader_appears, |ctx| {
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let name = cluster.members[leader].name.clone();
    let survivors: Vec<usize> = (0..size).filter(|i| *i != leader).collect();
    let started = Instant::now();
    // SIGKILL: no shutdown, no handover, nothing flushed. The survivors find out by not
    // hearing a heartbeat.
    cluster.kill(leader).await;
    let new_leader = leader_among(cluster, &survivors, REELECT_WITHIN).await?;
    let took = started.elapsed();
    let mut c = Check::new("the election that followed the leader being killed");
    c.ne("the new leader", leader, new_leader);
    c.that(
        "the new leader",
        "one of the two members still running",
        survivors.contains(&new_leader),
        new_leader + 1,
    );
    c.at_most(
        "how long the cluster went without a leader, in ms",
        REELECT_WITHIN.as_millis(),
        took.as_millis(),
    );
    c.eq("the members still running", 2, cluster.running().len());
    c.eq("the quorum two survivors make up", 2, majority(3));
    let described = cluster.describe().await;
    c.note(described);
    c.finish()?;
    ctx.note(format!(
        "{name} was killed; m{} took over after {} ms",
        new_leader + 1,
        took.as_millis()
    ));
    Ok(())
});

dist_test!(the_term_goes_up, |ctx| {
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let old_term = ok(
        cluster.client(leader).status().await,
        "the old leader's status",
    )?
    .raft_term;
    let survivors: Vec<usize> = (0..size).filter(|i| *i != leader).collect();
    cluster.kill(leader).await;
    let new_leader = leader_among(cluster, &survivors, REELECT_WITHIN).await?;
    let mut c = Check::new("the raft term on both sides of an election");
    let mut terms = Vec::new();
    for i in &survivors {
        let member = cluster.members[*i].name.clone();
        let s = ok(
            cluster.client(*i).status().await,
            &format!("{member}'s status"),
        )?;
        terms.push((member, s.raft_term));
    }
    // Strictly greater, not merely different: the term is what lets a member tell the dead
    // leader's entries from the new one's, and it only ever goes up.
    for (member, term) in &terms {
        c.that(
            &format!("{member}.status.raft_term"),
            "a strictly larger term than the killed leader's",
            *term > old_term,
            (old_term, *term),
        );
    }
    c.eq("the survivors' terms", terms[0].1, terms[1].1);
    let described = cluster.describe().await;
    c.note(format!(
        "m{} led in term {old_term}; m{} leads in term {}",
        leader + 1,
        new_leader + 1,
        terms[0].1
    ))
    .note(described);
    c.finish()
});

dist_test!(the_survivors_agree, |ctx| {
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let survivors: Vec<usize> = (0..size).filter(|i| *i != leader).collect();
    cluster.kill(leader).await;
    let new_leader = leader_among(cluster, &survivors, REELECT_WITHIN).await?;
    let new_id = cluster.members[new_leader].member_id;
    // Agreeing once could be luck; a settled cluster goes on agreeing.
    let mut polls = Vec::new();
    for _ in 0..5 {
        let mut round = Vec::new();
        for i in &survivors {
            round.push(ok(cluster.client(*i).status().await, "a status")?.leader);
        }
        polls.push(round);
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    let mut c = Check::new("who the survivors name as leader, five times over");
    for (n, round) in polls.iter().enumerate() {
        c.that(
            &format!("poll {n}: the leader every survivor names"),
            "the same member id on both survivors, and the one that won",
            round.iter().all(|id| *id == new_id),
            round.clone(),
        );
    }
    // The member list still has three entries: killing a process is not a membership
    // change, and the cluster has not forgotten the member it lost.
    let list = ok(
        cluster.client(new_leader).member_list().await,
        "the member list after the kill",
    )?;
    c.eq("member_list.members.len()", size, list.members.len());
    let described = cluster.describe().await;
    c.note(described);
    c.finish()
});

dist_test!(writes_resume, |ctx| {
    let key = ctx.key("resumed");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let survivors: Vec<usize> = (0..size).filter(|i| *i != leader).collect();
    let before = ok(
        cluster.client(leader).put(&key, b"before").await,
        "a write before the kill",
    )?;
    cluster.kill(leader).await;
    let new_leader = leader_among(cluster, &survivors, REELECT_WITHIN).await?;
    let mut revisions = Vec::new();
    for (n, i) in survivors.iter().enumerate() {
        let member = cluster.members[*i].name.clone();
        let put = ok(
            cluster
                .client(*i)
                .put(&key, format!("after-{n}").as_bytes())
                .await,
            &format!("a write through {member} after the election"),
        )?;
        revisions.push(put.header.revision);
    }
    let mut c = Check::new("writes taken by a cluster that has just elected");
    c.at_least(
        "the first write after the election",
        before.header.revision + 1,
        revisions[0],
    );
    c.that(
        "the revision across the two writes after the election",
        "to keep advancing",
        revisions[1] > revisions[0],
        (revisions[0], revisions[1]),
    );
    // And both survivors can read the last of them at once, election or no election.
    for i in &survivors {
        let member = cluster.members[*i].name.clone();
        let read = ok(
            cluster.client(*i).get_key(&key).await,
            &format!("a read on {member}"),
        )?;
        c.eq(
            &format!("{member}.range.kvs[0].value"),
            "after-1".to_string(),
            read.one().map(|kv| kv.value_str()).unwrap_or_default(),
        );
    }
    let described = cluster.describe().await;
    c.note(format!("m{} leads now", new_leader + 1))
        .note(described);
    c.finish()
});

dist_test!(nothing_acknowledged_is_lost, |ctx| {
    let prefix = ctx.key("acked");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let survivors: Vec<usize> = (0..size).filter(|i| *i != leader).collect();
    // Twelve writes the client was told had committed. Only a member holding all of them
    // may win the election that follows, so all twelve have to be there afterwards.
    let mut last = 0;
    for n in 0..12 {
        let key = [prefix.as_slice(), format!("/{n:02}").as_bytes()].concat();
        last = ok(
            cluster
                .client(leader)
                .put(&key, format!("v{n}").as_bytes())
                .await,
            "an acknowledged write",
        )?
        .header
        .revision;
    }
    cluster
        .wait_for_revision(last, Duration::from_millis(10_000))
        .await?;
    cluster.kill(leader).await;
    let new_leader = leader_among(cluster, &survivors, REELECT_WITHIN).await?;
    let mut c = Check::new("the acknowledged writes, after the leader that took them died");
    let read = ok(
        cluster
            .client(new_leader)
            .range_req(&RangeRequest::prefix(&prefix))
            .await,
        "a prefix read on the new leader",
    )?;
    c.eq("range.count", 12, read.count);
    c.eq(
        "range.values",
        (0..12).map(|n| format!("v{n}")).collect::<Vec<_>>(),
        read.values(),
    );
    c.at_least("range.header.revision", last, read.header.revision);
    let described = cluster.describe().await;
    c.note(format!(
        "m{} took the twelve writes and was killed; m{} leads now",
        leader + 1,
        new_leader + 1
    ))
    .note(described);
    c.finish()
});

dist_test!(the_dead_member_rejoins, |ctx| {
    let key = ctx.key("rejoin");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let name = cluster.members[leader].name.clone();
    let survivors: Vec<usize> = (0..size).filter(|i| *i != leader).collect();
    cluster.kill(leader).await;
    let new_leader = leader_among(cluster, &survivors, REELECT_WITHIN).await?;
    let put = ok(
        cluster
            .client(new_leader)
            .put(&key, b"while-you-were-dead")
            .await,
        "a write the dead member missed",
    )?;
    // Started from its own data directory, as a member of a cluster that already exists.
    cluster.start_member(leader).await?;
    let settled = cluster.wait_for_leader(ELECT_WITHIN).await?;
    cluster
        .wait_for_revision(put.header.revision, Duration::from_millis(30_000))
        .await?;
    let client = cluster.client(leader);
    let req = RangeRequest::key(&key).with("serializable", json!(true));
    let took = wait_until(
        &format!("{name}'s own store to hold the write it missed"),
        Duration::from_millis(30_000),
        || async {
            matches!(
                client.range_req(&req).await,
                Ok(r) if r.one().map(|kv| kv.value_str()).as_deref() == Some("while-you-were-dead")
            )
        },
    )
    .await?;
    let status = ok(
        cluster.client(leader).status().await,
        &format!("{name}'s status after it rejoined"),
    )?;
    let mut c = Check::new("a member that was killed and started again");
    c.eq("the members running again", size, cluster.running().len());
    // It comes back as a follower, not as the leader it used to be: the term moved on
    // while it was gone, and its old claim went with it.
    c.eq(
        &format!("{name}.status.leader"),
        cluster.members[settled].member_id,
        status.leader,
    );
    c.at_least(
        &format!("{name}.status.header.revision"),
        put.header.revision,
        status.header.revision,
    );
    let described = cluster.describe().await;
    c.note(format!(
        "{name} was killed, m{} took over, and {name} caught up {} ms after it was started again",
        new_leader + 1,
        took.as_millis()
    ))
    .note(described);
    c.finish()
});

dist_test!(killing_a_follower_changes_nothing, |ctx| {
    let key = ctx.key("follower-kill");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let victim = a_follower(leader, size);
    let old_term = ok(cluster.client(leader).status().await, "the leader's status")?.raft_term;
    cluster.kill(victim).await;
    // Two of three is still a quorum, so there is nothing to elect. A cluster that holds an
    // election here is reacting to the wrong thing.
    let mut leaders = Vec::new();
    let mut revisions = Vec::new();
    for round in 0..5 {
        leaders.push(cluster.leader_according_to(leader).await);
        let put = ok(
            cluster
                .client(leader)
                .put(&key, format!("r{round}").as_bytes())
                .await,
            "a write with one follower dead",
        )?;
        revisions.push(put.header.revision);
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let after = ok(cluster.client(leader).status().await, "the leader's status")?;
    let mut c = Check::new("a cluster that lost a follower rather than a leader");
    c.that(
        "the leader over five rounds",
        "the same leader throughout, with no election",
        leaders.iter().all(|l| *l == Some(leader)),
        leaders.iter().map(|l| l.map(|x| x + 1)).collect::<Vec<_>>(),
    );
    c.eq("the leader's raft_term", old_term, after.raft_term);
    for w in revisions.windows(2) {
        c.that(
            "the revision with a follower dead",
            "to keep advancing",
            w[1] > w[0],
            (w[0], w[1]),
        );
    }
    c.eq("the members still running", 2, cluster.running().len());
    let described = cluster.describe().await;
    c.note(format!(
        "m{} was killed; m{} led before and after",
        victim + 1,
        leader + 1
    ))
    .note(described);
    c.finish()
});

dist_test!(five_members_survive_two_kills, |ctx| {
    let key = ctx.key("two-kills");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let second = a_follower(leader, size);
    let mut survivors: Vec<usize> = (0..size).filter(|i| *i != leader && *i != second).collect();
    survivors.sort_unstable();
    // Five members tolerate two failures, and this is the worst pair to lose: the leader,
    // and then one of the four left over.
    cluster.kill(leader).await;
    let first_new = leader_among(
        cluster,
        &(0..size).filter(|i| *i != leader).collect::<Vec<_>>(),
        REELECT_WITHIN,
    )
    .await?;
    let started = Instant::now();
    cluster.kill(second).await;
    let final_leader = leader_among(cluster, &survivors, REELECT_WITHIN).await?;
    let took = started.elapsed();
    let mut c = Check::new("a five-member cluster that has lost two members");
    c.eq("the quorum of five", 3, majority(5));
    c.eq("the members still running", 3, cluster.running().len());
    c.that(
        "the final leader",
        "one of the three members still running",
        survivors.contains(&final_leader),
        final_leader + 1,
    );
    let put = ok(
        cluster.client(final_leader).put(&key, b"three-left").await,
        "a write with two of five dead",
    )?;
    for i in &survivors {
        let member = cluster.members[*i].name.clone();
        let read = ok(
            cluster.client(*i).get_key(&key).await,
            &format!("a read on {member}"),
        )?;
        c.eq(
            &format!("{member}.range.kvs[0].value"),
            "three-left".to_string(),
            read.one().map(|kv| kv.value_str()).unwrap_or_default(),
        );
        c.at_least(
            &format!("{member}.range.header.revision"),
            put.header.revision,
            read.header.revision,
        );
    }
    let described = cluster.describe().await;
    c.note(format!(
        "m{} was killed, then m{}; the second election took {} ms and left m{} leading \
         (m{} had taken over in between)",
        leader + 1,
        second + 1,
        took.as_millis(),
        final_leader + 1,
        first_new + 1
    ))
    .note(described);
    c.finish()
});
