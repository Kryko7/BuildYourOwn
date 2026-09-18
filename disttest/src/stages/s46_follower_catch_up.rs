//! Stage 46 — a long-partitioned follower catches up.
//!
//! One member misses hundreds of entries and then the link comes back. Nobody restarts it,
//! nobody copies a data directory, nobody runs a repair tool: the leader notices the follower
//! is behind, walks back to the last index they agree on, and ships everything after it. The
//! follower is a full voter again when that finishes and not a moment before.
//!
//! Two traps live here. The first is that catching up is not allowed to be a special mode:
//! the other two members were serving the whole time and must go on serving at the same
//! revision, with the same leader, while the third is being fed. The second is the stale read
//! — a follower that answers a linearizable read out of its own store before it has caught up
//! hands the client a value the cluster moved past a minute ago, and nothing else in the
//! system will ever contradict it.

use crate::assert::{Check, Failure, FailureKind};
use crate::cluster::Cluster;
use crate::dist_test;
use crate::examples::{cluster_example, step, ExampleSpec};
use crate::stages::{ok, Ladder, Stage, Test};
use serde_json::json;
use std::time::{Duration, Instant};

/// Stage 46.
pub fn stage() -> Stage {
    Stage {
        number: 46,
        slug: "follower_catch_up",
        name: "A long-partitioned follower catches up",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "The leader keeps sending from the follower's next index until it catches up",
            "Hundreds of missed writes must arrive without a restart",
            "Until it has caught up, the follower must not answer linearizable reads from stale state",
            "Catching up must not disturb the members that were healthy",
        ],
        examples,
        tests: vec![
            Test::new(
                "a follower that missed three hundred writes catches up on its own",
                the_follower_catches_up,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "every key it missed is there and correct afterwards",
                every_missed_key_arrives,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "its revision lags behind the majority's while it is cut off",
                it_lags_while_it_is_away,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "it answers the newest revision once it has caught up",
                it_answers_the_newest_revision,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "the members that stayed healthy were undisturbed",
                the_healthy_members_were_undisturbed,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "the cluster kept serving while the follower was away",
                the_cluster_kept_serving,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "catching up does not change the leader",
                catching_up_does_not_change_the_leader,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "a follower cut off twice in a row catches up twice",
                twice_in_a_row,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        cluster_example("A follower's revision, next to the leader's", || {
            vec![
                step(
                    0,
                    "/v3/kv/put",
                    json!({"key": "ZXg0Ni9r", "value": "djE="}),
                    "one write through m1",
                ),
                step(
                    0,
                    "/v3/maintenance/status",
                    json!({}),
                    "m1's revision and raft index",
                ),
                step(
                    2,
                    "/v3/maintenance/status",
                    json!({}),
                    "m3's, which is the same",
                ),
                step(
                    2,
                    "/v3/kv/range",
                    json!({"key": "ZXg0Ni9r"}),
                    "and the value m3 serves",
                ),
            ]
        })
        .request("a write, then the status of the leader and of one follower")
        .response("the same revision on both, and the written value from the follower")
        .note(
            "`header.revision` is what the stage polls: it is the one number that says whether a \
         member is up to date. The stage cuts m3 off, writes three hundred keys, and watches \
         this number stand still on m3 while it climbs on m1; after the heal it must climb \
         back to m1's with nobody restarting anything. Note that the follower's answer here \
         is a linearizable read, which reaches the leader — a member that serves such a read \
         out of its own store while it is behind is exactly the bug this stage looks for.",
        ),
    ]
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
///
/// Every put is demanded to succeed: these writes happen on the side of the cut that still
/// holds a quorum, so a refusal there is a failure of the stage, not a fault being injected.
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

dist_test!(the_follower_catches_up, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let follower = (leader + 1) % cluster.initial_size;
    let healthy: Vec<usize> = (0..cluster.initial_size)
        .filter(|i| *i != follower)
        .collect();
    cluster.isolate(follower).await;
    leader_among(cluster, &healthy, Duration::from_millis(25_000)).await?;
    let last = fill(cluster, leader, &prefix, 300).await?;
    cluster.heal().await;
    // No restart, no repair: the only thing that happens between here and the follower being
    // up to date is the leader noticing and shipping what it missed.
    let started = Instant::now();
    cluster
        .wait_for_revision(last, Duration::from_millis(40_000))
        .await?;
    let took = started.elapsed();
    let reached = ok(
        cluster.client(follower).status().await,
        "the caught-up follower's status",
    )?;
    let described = cluster.describe().await;
    let mut c = Check::new("a follower catching up on three hundred missed writes");
    c.note(format!("m{} was the follower cut off", follower + 1));
    c.note(described);
    c.at_least("the follower's revision", last, reached.header.revision);
    ctx.note(format!(
        "m{} caught up on 300 writes (to revision {last}) in {} ms, with no restart",
        follower + 1,
        took.as_millis()
    ));
    c.finish()
});

dist_test!(every_missed_key_arrives, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let follower = (leader + 1) % cluster.initial_size;
    let healthy: Vec<usize> = (0..cluster.initial_size)
        .filter(|i| *i != follower)
        .collect();
    cluster.isolate(follower).await;
    leader_among(cluster, &healthy, Duration::from_millis(25_000)).await?;
    let last = fill(cluster, leader, &prefix, 300).await?;
    cluster.heal().await;
    cluster
        .wait_for_revision(last, Duration::from_millis(40_000))
        .await?;
    // Reaching the revision is necessary but not sufficient: the entries have to have been
    // applied, not merely accepted, so every key is read back off the follower itself.
    let mut wrong = Vec::new();
    for i in 0..300 {
        let key = format!("{prefix}/{i:03}").into_bytes();
        let want = format!("v{i}");
        match cluster.client(follower).value_of(&key).await {
            Ok(Some(v)) if v == want.as_bytes() => {}
            other => wrong.push(format!("{} -> {other:?}", String::from_utf8_lossy(&key))),
        }
    }
    let described = cluster.describe().await;
    let mut c = Check::new("the three hundred keys the follower missed");
    c.note(format!("m{} was the follower cut off", follower + 1));
    c.note(described);
    c.that(
        "the missed keys, read back from the follower",
        "all three hundred present and correct",
        wrong.is_empty(),
        wrong.into_iter().take(10).collect::<Vec<_>>(),
    );
    c.finish()
});

dist_test!(it_lags_while_it_is_away, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let follower = (leader + 1) % cluster.initial_size;
    let healthy: Vec<usize> = (0..cluster.initial_size)
        .filter(|i| *i != follower)
        .collect();
    let before = ok(
        cluster.client(follower).status().await,
        "the follower's status before the cut",
    )?
    .header
    .revision;
    cluster.isolate(follower).await;
    leader_among(cluster, &healthy, Duration::from_millis(25_000)).await?;
    let last = fill(cluster, leader, &prefix, 120).await?;
    // The cut has to be real. A follower whose revision keeps climbing while it is isolated
    // is being fed by something the fault injector cannot see, and every other test in this
    // stage would then be proving nothing at all.
    let away = ok(
        cluster.client(follower).status().await,
        "the isolated follower's status",
    )?
    .header
    .revision;
    let described = cluster.describe().await;
    let faults = cluster.faults.describe();
    let mut c = Check::new("the isolated follower's revision while the others write");
    c.note(format!("m{} was the follower cut off", follower + 1));
    c.note(described).note(faults);
    c.at_least("the majority's revision", before + 120, last);
    c.at_most("the isolated follower's revision", last - 1, away);
    c.finish()
});

dist_test!(it_answers_the_newest_revision, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let follower = (leader + 1) % cluster.initial_size;
    let healthy: Vec<usize> = (0..cluster.initial_size)
        .filter(|i| *i != follower)
        .collect();
    cluster.isolate(follower).await;
    leader_among(cluster, &healthy, Duration::from_millis(25_000)).await?;
    let last = fill(cluster, leader, &prefix, 120).await?;
    cluster.heal().await;
    cluster
        .wait_for_revision(last, Duration::from_millis(40_000))
        .await?;
    let newest = ok(
        cluster
            .client(leader)
            .put(&newest_key(&prefix), b"after the heal")
            .await,
        "a write made after the follower rejoined",
    )?
    .header
    .revision;
    cluster
        .wait_for_revision(newest, Duration::from_millis(20_000))
        .await?;
    let status = ok(
        cluster.client(follower).status().await,
        "the rejoined follower's status",
    )?;
    let read = ok(
        cluster.client(follower).get_key(&newest_key(&prefix)).await,
        "reading the newest key from the rejoined follower",
    )?;
    let described = cluster.describe().await;
    let mut c = Check::new("the revision a rejoined follower reports");
    c.note(format!("m{} was the follower cut off", follower + 1));
    c.note(described);
    c.eq(
        "the follower's header.revision",
        newest,
        status.header.revision,
    );
    c.eq(
        "the follower's answer for the newest key",
        newest,
        read.header.revision,
    );
    c.eq(
        "the newest value, from the follower",
        Some("after the heal".to_string()),
        read.one().map(|kv| kv.value_str()),
    );
    c.finish()
});

dist_test!(the_healthy_members_were_undisturbed, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let follower = (leader + 1) % cluster.initial_size;
    let healthy: Vec<usize> = (0..cluster.initial_size)
        .filter(|i| *i != follower)
        .collect();
    cluster.isolate(follower).await;
    leader_among(cluster, &healthy, Duration::from_millis(25_000)).await?;
    let last = fill(cluster, leader, &prefix, 120).await?;
    // Two of three is a quorum, so losing the third costs the cluster nothing at all: both
    // survivors must be at the newest revision and serving it while the third is away.
    let mut lagging = Vec::new();
    for i in &healthy {
        let name = cluster.members[*i].name.clone();
        let s = ok(
            cluster.client(*i).status().await,
            "a healthy member's status during the cut",
        )?;
        if s.header.revision < last {
            lagging.push(format!("{name} at revision {}", s.header.revision));
        }
        let v = ok(
            cluster
                .client(*i)
                .value_of(format!("{prefix}/119").as_bytes())
                .await,
            "the newest key, read from a healthy member",
        )?;
        if v.as_deref() != Some(b"v119".as_slice()) {
            lagging.push(format!("{name} answered {v:?} for the newest key"));
        }
    }
    let described = cluster.describe().await;
    let mut c = Check::new("the two members that never lost contact");
    c.note(format!("m{} was the follower cut off", follower + 1));
    c.note(described);
    c.that(
        "the healthy members during the cut",
        "both at the newest revision and serving it",
        lagging.is_empty(),
        lagging,
    );
    c.finish()
});

dist_test!(the_cluster_kept_serving, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let follower = (leader + 1) % cluster.initial_size;
    let healthy: Vec<usize> = (0..cluster.initial_size)
        .filter(|i| *i != follower)
        .collect();
    let mut revisions = Vec::new();
    cluster.isolate(follower).await;
    leader_among(cluster, &healthy, Duration::from_millis(25_000)).await?;
    // Writes go through both healthy members in turn, so this also says that the one that is
    // not the leader still forwards while a third of the cluster is missing.
    for round in 0..20 {
        let through = healthy[round % healthy.len()];
        let key = format!("{prefix}/serving{round:02}").into_bytes();
        revisions.push(
            ok(
                cluster.client(through).put(&key, b"served").await,
                "a write while one member was cut off",
            )?
            .header
            .revision,
        );
    }
    cluster.heal().await;
    let last = *revisions.last().unwrap_or(&0);
    cluster
        .wait_for_revision(last, Duration::from_millis(40_000))
        .await?;
    let described = cluster.describe().await;
    let mut c = Check::new("twenty writes made while one member was cut off");
    c.note(format!("m{} was the follower cut off", follower + 1));
    c.note(described);
    c.eq("writes that were acknowledged", 20, revisions.len());
    crate::stages::check_strictly_increasing(&mut c, "revisions", &revisions);
    c.finish()
});

dist_test!(catching_up_does_not_change_the_leader, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let follower = (leader + 1) % cluster.initial_size;
    let healthy: Vec<usize> = (0..cluster.initial_size)
        .filter(|i| *i != follower)
        .collect();
    cluster.isolate(follower).await;
    let during = leader_among(cluster, &healthy, Duration::from_millis(25_000)).await?;
    let last = fill(cluster, leader, &prefix, 120).await?;
    cluster.heal().await;
    cluster
        .wait_for_revision(last, Duration::from_millis(40_000))
        .await?;
    // A follower rejoining has no business unseating anybody: it is behind, so it cannot win
    // an election, and a candidate that cannot win should not be starting one either.
    let after = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let described = cluster.describe().await;
    let mut c = Check::new("who leads before, during and after one member's absence");
    c.note(format!("m{} was the follower cut off", follower + 1));
    c.note(described);
    c.eq("the leader while the follower was away", leader, during);
    c.eq("the leader after the follower came back", leader, after);
    c.finish()
});

dist_test!(twice_in_a_row, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let follower = (leader + 1) % cluster.initial_size;
    let healthy: Vec<usize> = (0..cluster.initial_size)
        .filter(|i| *i != follower)
        .collect();
    let mut rounds = Vec::new();
    for round in 0..2 {
        cluster.isolate(follower).await;
        leader_among(cluster, &healthy, Duration::from_millis(25_000)).await?;
        let last = fill(cluster, leader, &format!("{prefix}/r{round}"), 100).await?;
        cluster.heal().await;
        let started = Instant::now();
        cluster
            .wait_for_revision(last, Duration::from_millis(40_000))
            .await?;
        let took = started.elapsed();
        let key = format!("{prefix}/r{round}/099").into_bytes();
        let seen = ok(
            cluster.client(follower).value_of(&key).await,
            "the last key of this round, read from the follower",
        )?;
        rounds.push((round, took, seen));
    }
    let described = cluster.describe().await;
    let mut c = Check::new("two absences and two catch-ups of the same follower");
    c.note(format!("m{} was the follower cut off twice", follower + 1));
    c.note(described);
    for (round, _, seen) in &rounds {
        c.eq(
            &format!("the last key of round {round}, on the follower"),
            Some(b"v99".to_vec()),
            seen.clone(),
        );
    }
    let timings: Vec<u128> = rounds.iter().map(|(_, t, _)| t.as_millis()).collect();
    ctx.note(format!(
        "two catch-ups of 100 writes each took {timings:?} ms"
    ));
    c.finish()
});

/// The one key `it_answers_the_newest_revision` writes after the heal.
fn newest_key(prefix: &str) -> Vec<u8> {
    format!("{prefix}/newest").into_bytes()
}
