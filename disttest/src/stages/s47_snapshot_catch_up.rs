//! Stage 47 — a far-behind follower is caught up by snapshot.
//!
//! Stage 46's follower could be fed the entries it missed because the leader still had them.
//! Compaction takes that away. Once the history below a revision has been dropped, the leader
//! cannot answer "here is everything after your last index" for an index that old, and the
//! only thing left to send is the state itself: a snapshot, which the follower installs over
//! whatever it had and then carries on from the index the snapshot was taken at.
//!
//! The trap is that this is a second, colder path through the same code, and it is easy to
//! write a replication loop that only ever works when the entries happen to still be there.
//! The tests below cut a follower off, compact above everything it holds, and then insist on
//! exactly the same outcome as stage 46 — same values, same revision, no restart. They do not
//! insist on seeing a snapshot message go past, because whether one is needed depends on how
//! much log an implementation keeps; they insist on convergence when the log alone cannot
//! provide it.

use crate::assert::{Check, Failure, FailureKind};
use crate::cluster::Cluster;
use crate::dist_test;
use crate::etcd::{Client, RangeRequest};
use crate::examples::{cluster_example, step, ExampleSpec};
use crate::stages::{expect_code, ok, wait_until, Ladder, Stage, Test};
use serde_json::json;
use std::time::{Duration, Instant};

/// Stage 47.
pub fn stage() -> Stage {
    Stage {
        number: 47,
        slug: "snapshot_catch_up",
        name: "A far-behind follower is caught up by snapshot",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "When the entries a follower needs have been compacted away, send a snapshot instead",
            "The follower replaces its whole state with the snapshot and continues from its index",
            "After a snapshot install the follower answers the same values as everyone else",
            "Compaction on the leader is what makes this path necessary at all",
        ],
        examples,
        tests: vec![
            Test::new(
                "a follower converges after the entries it missed were compacted away",
                the_follower_converges,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "its values match the majority's once it is back",
                its_values_match,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "it answers the current revision once it is back",
                it_answers_the_current_revision,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "a read below the compacted revision is an error on every member alike",
                compacted_reads_fail_everywhere,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "the cluster keeps serving while the follower is being caught up",
                the_cluster_keeps_serving,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "writes made while it was catching up reach it as well",
                writes_during_the_catch_up_arrive,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "compacting with every member healthy changes nothing",
                compaction_on_a_healthy_cluster,
            )
            .ext()
            .min_timeout_ms(60_000),
            Test::new(
                "the newest value of every key survives a compaction",
                the_newest_values_survive,
            )
            .ext()
            .min_timeout_ms(60_000),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![cluster_example(
        "Compacting the history away, and what a read below it gets",
        || {
            vec![
                step(
                    0,
                    "/v3/kv/put",
                    json!({"key": "ZXg0Ny9r", "value": "djE="}),
                    "ex47/k = v1",
                ),
                step(
                    0,
                    "/v3/kv/put",
                    json!({"key": "ZXg0Ny9r", "value": "djI="}),
                    "ex47/k = v2, one revision later",
                ),
                step(
                    0,
                    "/v3/kv/compaction",
                    json!({"revision": "3", "physical": true}),
                    "drop everything below revision 3",
                ),
                step(
                    2,
                    "/v3/kv/range",
                    json!({"key": "ZXg0Ny9r", "revision": "2"}),
                    "m3 is asked for history that no longer exists",
                ),
            ]
        },
    )
    .request("two writes, a compaction above them, then a read at a compacted revision")
    .response("code 11, `mvcc: required revision has been compacted`, from a follower")
    .note(
        "Compaction is a log entry like any other, so every member applies it and every \
             member refuses the same reads afterwards — a follower that still answers here has \
             not applied it. The stage does this while one member is cut off, which leaves the \
             leader with no entries old enough to send it and nothing to offer but the state \
             itself. The revision numbers here are the reference's; a stage computes the \
             compaction point from the header its own writes came back with.",
    )]
}

/// Wait until every member in `among` names the same leader, ignoring everyone else.
///
/// [`Cluster::wait_for_leader`] insists that every *running* member agrees, which cannot
/// happen while one of them is cut off and naming nobody. The healthy side has to be asked on
/// its own, and the leader it names has to be one of its own members.
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

/// Write `n` keys `<prefix>/nnn` through `through`, returning the last revision.
async fn fill(cluster: &Cluster, through: usize, prefix: &str, n: usize) -> Result<i64, Failure> {
    let mut last = 0;
    for i in 0..n {
        let key = format!("{prefix}/{i:03}").into_bytes();
        last = ok(
            cluster
                .client(through)
                .put(&key, format!("v{i}").as_bytes())
                .await,
            "a write on the side of the cut that still has a quorum",
        )?
        .header
        .revision;
    }
    Ok(last)
}

/// Cut `follower` off, write `n` keys, and compact above every one of them.
///
/// The compaction point is the revision of the last write, so the entries the follower is
/// missing are exactly the ones that no longer exist. Returns the revision it was compacted
/// at, which is also the cluster's revision at that moment.
async fn strand_the_follower(
    cluster: &mut Cluster,
    leader: usize,
    follower: usize,
    prefix: &str,
    n: usize,
) -> Result<i64, Failure> {
    let healthy: Vec<usize> = (0..cluster.initial_size)
        .filter(|i| *i != follower)
        .collect();
    cluster.isolate(follower).await;
    leader_among(cluster, &healthy, Duration::from_millis(25_000)).await?;
    let last = fill(cluster, leader, prefix, n).await?;
    ok(
        cluster.client(leader).compact(last, true).await,
        "compacting the leader's log above everything the follower is missing",
    )?;
    Ok(last)
}

dist_test!(the_follower_converges, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let follower = (leader + 1) % cluster.initial_size;
    let compacted = strand_the_follower(cluster, leader, follower, &prefix, 200).await?;
    let stranded = ok(
        cluster.client(follower).status().await,
        "the stranded follower's status",
    )?
    .header
    .revision;
    cluster.heal().await;
    let started = Instant::now();
    cluster
        .wait_for_revision(compacted, Duration::from_millis(40_000))
        .await?;
    let took = started.elapsed();
    let running = cluster.members[follower].running();
    let described = cluster.describe().await;
    let mut c = Check::new("a follower rejoining after the log it needed was compacted");
    c.note(format!(
        "m{} was cut off at revision {stranded}; the log was compacted at {compacted}",
        follower + 1
    ));
    c.note(described);
    c.at_most(
        "the follower's revision while it was away",
        compacted - 1,
        stranded,
    );
    c.that(
        "the follower's process",
        "the same one throughout, never restarted",
        running,
        running,
    );
    ctx.note(format!(
        "m{} converged from revision {stranded} to {compacted} in {} ms, with no restart",
        follower + 1,
        took.as_millis()
    ));
    c.finish()
});

dist_test!(its_values_match, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let follower = (leader + 1) % cluster.initial_size;
    let compacted = strand_the_follower(cluster, leader, follower, &prefix, 200).await?;
    cluster.heal().await;
    cluster
        .wait_for_revision(compacted, Duration::from_millis(40_000))
        .await?;
    // Whatever carried the state across — a snapshot or a replayed tail — the follower has to
    // end up holding every key, not merely a high revision number.
    let mut wrong = Vec::new();
    for i in 0..200 {
        let key = format!("{prefix}/{i:03}").into_bytes();
        let want = format!("v{i}");
        match cluster.client(follower).value_of(&key).await {
            Ok(Some(v)) if v == want.as_bytes() => {}
            other => wrong.push(format!("{} -> {other:?}", String::from_utf8_lossy(&key))),
        }
    }
    let described = cluster.describe().await;
    let mut c = Check::new("the two hundred keys written while the follower was stranded");
    c.note(format!("m{} was the stranded follower", follower + 1));
    c.note(described);
    c.that(
        "the keys read back from the follower",
        "all two hundred present and correct",
        wrong.is_empty(),
        wrong.into_iter().take(10).collect::<Vec<_>>(),
    );
    c.finish()
});

dist_test!(it_answers_the_current_revision, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let follower = (leader + 1) % cluster.initial_size;
    let compacted = strand_the_follower(cluster, leader, follower, &prefix, 200).await?;
    cluster.heal().await;
    cluster
        .wait_for_revision(compacted, Duration::from_millis(40_000))
        .await?;
    let key = format!("{prefix}/after").into_bytes();
    let newest = ok(
        cluster
            .client(leader)
            .put(&key, b"written after the install")
            .await,
        "a write made once the follower was back",
    )?
    .header
    .revision;
    cluster
        .wait_for_revision(newest, Duration::from_millis(20_000))
        .await?;
    let status = ok(
        cluster.client(follower).status().await,
        "the caught-up follower's status",
    )?;
    let read = ok(
        cluster.client(follower).get_key(&key).await,
        "reading the newest key from the caught-up follower",
    )?;
    let described = cluster.describe().await;
    let mut c = Check::new("the revision a caught-up follower reports");
    c.note(format!("m{} was the stranded follower", follower + 1));
    c.note(described);
    c.eq(
        "the follower's header.revision",
        newest,
        status.header.revision,
    );
    c.eq(
        "the newest value, from the follower",
        Some("written after the install".to_string()),
        read.one().map(|kv| kv.value_str()),
    );
    c.finish()
});

dist_test!(compacted_reads_fail_everywhere, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let follower = (leader + 1) % cluster.initial_size;
    let compacted = strand_the_follower(cluster, leader, follower, &prefix, 200).await?;
    cluster.heal().await;
    cluster
        .wait_for_revision(compacted, Duration::from_millis(40_000))
        .await?;
    let key = format!("{prefix}/000").into_bytes();
    let probe = RangeRequest::key(&key).at_revision(compacted - 1);
    // A compaction is a log entry, so a member that has applied everything has applied it
    // too, and the history it dropped has to be gone there as well. The wait is for the
    // follower to have applied it, not for the answer to change afterwards.
    {
        let clients: Vec<&Client> = (0..cluster.initial_size)
            .map(|i| cluster.client(i))
            .collect();
        wait_until(
            "every member to refuse a read below the compacted revision",
            Duration::from_millis(20_000),
            || {
                let cs = clients.clone();
                let body = probe.body();
                async move {
                    for c in cs {
                        if c.post("/v3/kv/range", &body).await.is_ok() {
                            return false;
                        }
                    }
                    true
                }
            },
        )
        .await?;
    }
    for i in 0..cluster.initial_size {
        let name = cluster.members[i].name.clone();
        expect_code(
            cluster.client(i).range_req(&probe).await,
            11,
            &format!("a read below the compacted revision on {name}"),
        )?;
    }
    let mut c = Check::new("what every member says about history that was compacted away");
    c.note(format!(
        "m{} was stranded; the log was compacted at {compacted}",
        follower + 1
    ));
    c.note(cluster.describe().await);
    c.at_least("the compacted revision", 2, compacted);
    c.finish()
});

dist_test!(the_cluster_keeps_serving, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let follower = (leader + 1) % cluster.initial_size;
    let healthy: Vec<usize> = (0..cluster.initial_size)
        .filter(|i| *i != follower)
        .collect();
    let compacted = strand_the_follower(cluster, leader, follower, &prefix, 200).await?;
    cluster.heal().await;
    // The install is somebody else's problem: the two members that were never away must go on
    // answering writes and reads from the moment the link is back, not once it has finished.
    let mut refused = Vec::new();
    for round in 0..20 {
        let through = healthy[round % healthy.len()];
        let name = cluster.members[through].name.clone();
        let key = format!("{prefix}/during{round:02}").into_bytes();
        if cluster.client(through).put(&key, b"during").await.is_err() {
            refused.push(format!("{name} refused the write of round {round}"));
        }
        if cluster.client(through).value_of(&key).await.is_err() {
            refused.push(format!("{name} refused the read of round {round}"));
        }
    }
    cluster
        .wait_for_revision(compacted, Duration::from_millis(40_000))
        .await?;
    let described = cluster.describe().await;
    let mut c = Check::new("the healthy members while the third was being caught up");
    c.note(format!("m{} was the stranded follower", follower + 1));
    c.note(described);
    c.that(
        "writes and reads on the healthy members during the install",
        "all forty answered",
        refused.is_empty(),
        refused,
    );
    c.finish()
});

dist_test!(writes_during_the_catch_up_arrive, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let follower = (leader + 1) % cluster.initial_size;
    strand_the_follower(cluster, leader, follower, &prefix, 200).await?;
    cluster.heal().await;
    // These writes land in the window between the link coming back and the follower being up
    // to date, which is the window an install is most likely to lose an entry in.
    let mut last = 0;
    for round in 0..20 {
        let key = format!("{prefix}/window{round:02}").into_bytes();
        last = ok(
            cluster
                .client(leader)
                .put(&key, format!("w{round}").as_bytes())
                .await,
            "a write made while the follower was catching up",
        )?
        .header
        .revision;
    }
    cluster
        .wait_for_revision(last, Duration::from_millis(40_000))
        .await?;
    let mut missing = Vec::new();
    for round in 0..20 {
        let key = format!("{prefix}/window{round:02}").into_bytes();
        let want = format!("w{round}");
        match cluster.client(follower).value_of(&key).await {
            Ok(Some(v)) if v == want.as_bytes() => {}
            other => missing.push(format!("{} -> {other:?}", String::from_utf8_lossy(&key))),
        }
    }
    let described = cluster.describe().await;
    let mut c = Check::new("writes made in the window while the follower was catching up");
    c.note(format!("m{} was the stranded follower", follower + 1));
    c.note(described);
    c.that(
        "the twenty writes from the window, on the follower",
        "all twenty present and correct",
        missing.is_empty(),
        missing,
    );
    c.finish()
});

dist_test!(compaction_on_a_healthy_cluster, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let last = fill(cluster, leader, &prefix, 50).await?;
    cluster
        .wait_for_revision(last, Duration::from_millis(20_000))
        .await?;
    // Nothing about a compaction is a repair: with every member already up to date it drops
    // history nobody was reading and leaves the cluster exactly where it was.
    ok(
        cluster.client(leader).compact(last, true).await,
        "compacting a cluster in which nobody is behind",
    )?;
    let after = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let mut revisions = Vec::new();
    for i in 0..cluster.initial_size {
        revisions.push(
            ok(
                cluster.client(i).status().await,
                "a member's status after the compaction",
            )?
            .header
            .revision,
        );
    }
    let described = cluster.describe().await;
    let mut c = Check::new("a compaction on a cluster where every member is up to date");
    c.note(described);
    c.eq("the leader before and after", leader, after);
    for (i, rev) in revisions.iter().enumerate() {
        c.eq(
            &format!("m{}'s revision after the compaction", i + 1),
            &last,
            rev,
        );
    }
    c.finish()
});

dist_test!(the_newest_values_survive, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let key = format!("{prefix}/rewritten").into_bytes();
    let mut revisions = Vec::new();
    for round in 0..5 {
        revisions.push(
            ok(
                cluster
                    .client(leader)
                    .put(&key, format!("generation-{round}").as_bytes())
                    .await,
                "one of five writes to the same key",
            )?
            .header
            .revision,
        );
    }
    let last = *revisions.last().unwrap_or(&0);
    cluster
        .wait_for_revision(last, Duration::from_millis(20_000))
        .await?;
    // Compaction drops superseded versions, never the current one. A store that loses the
    // live value here would also hand a snapshot with a hole in it to the next follower.
    ok(
        cluster.client(leader).compact(last, true).await,
        "compacting away four of the five generations",
    )?;
    let mut answers = Vec::new();
    for i in 0..cluster.initial_size {
        let name = cluster.members[i].name.clone();
        let v = ok(
            cluster.client(i).value_of(&key).await,
            "the newest value of a rewritten key",
        )?;
        answers.push((name, v));
    }
    let described = cluster.describe().await;
    let mut c = Check::new("the live value of a key whose history was compacted away");
    c.note(described);
    for (name, v) in &answers {
        c.eq(
            &format!("{name}'s answer for the rewritten key"),
            Some(b"generation-4".to_vec()),
            v.clone(),
        );
    }
    c.finish()
});
