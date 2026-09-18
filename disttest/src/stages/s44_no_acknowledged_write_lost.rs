//! Stage 44 — no acknowledged write is lost.
//!
//! This is the property the whole design exists to provide, and the only one a user of the
//! store can actually lean on. A write that was answered has been accepted by a quorum, so
//! every majority that could elect the next leader holds at least one copy of it, and a
//! candidate missing it cannot win. Killing the leader is therefore allowed to cost a second
//! or two of availability. It is never allowed to cost data.
//!
//! The mirror image matters just as much, and it is where a test of this property usually
//! goes wrong. A write whose answer never arrived — the leader died between accepting it and
//! replying — may be present afterwards or absent, and **both are correct**: the client was
//! told nothing, so nothing was promised. The tests below therefore only ever demand the
//! keys whose put returned `ok`. What they demand of the doubtful ones instead is that every
//! surviving member says the same thing about them, because a write appearing on one member
//! and not another is a split brain whatever the client was told.

use crate::assert::{Check, Failure, FailureKind};
use crate::cluster::Cluster;
use crate::dist_test;
use crate::etcd::{Client, EtcdError, RangeRequest, RangeResponse};
use crate::examples::{cluster_example, step, ExampleSpec};
use crate::stages::{ok, Ladder, Stage, Test};
use serde_json::json;
use std::time::{Duration, Instant};

/// Stage 44.
pub fn stage() -> Stage {
    Stage {
        number: 44,
        slug: "no_acknowledged_write_lost",
        name: "No acknowledged write is lost",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "A write is only acknowledged once a quorum has it on disk",
            "After the leader is killed, every acknowledged write must still be readable",
            "A write that was never acknowledged may be there or not: both are correct",
            "This is the property the whole design exists to provide",
        ],
        examples,
        tests: vec![
            Test::new(
                "an acknowledged write survives the leader being killed",
                one_acknowledged_write,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "a hundred acknowledged writes survive a leader kill",
                a_hundred_acknowledged_writes,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "writes acknowledged through a follower survive too",
                acknowledged_through_a_follower,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "the revision an acknowledged write was given is still readable",
                the_revision_survives,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "a kill in the middle of a burst loses nothing that was acknowledged",
                a_kill_during_a_burst,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "the survivors agree about every key, acknowledged or not",
                survivors_agree,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "an acknowledged delete is as durable as an acknowledged write",
                a_delete_is_durable,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "five members and two leader kills lose nothing",
                five_members_two_kills,
            )
            .ext()
            .cluster(5)
            .min_timeout_ms(60_000),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![cluster_example(
        "An acknowledged write, then every member's copy of it",
        || {
            vec![
                step(
                    0,
                    "/v3/kv/put",
                    json!({"key": "ZXg0NC9r", "value": "YWNrZWQ="}),
                    "write ex44/k through m1 and wait for the answer",
                ),
                step(
                    1,
                    "/v3/kv/range",
                    json!({"key": "ZXg0NC9r"}),
                    "m2 already has it",
                ),
                step(
                    2,
                    "/v3/kv/range",
                    json!({"key": "ZXg0NC9r"}),
                    "and so does m3",
                ),
                step(
                    2,
                    "/v3/maintenance/status",
                    json!({}),
                    "the revision the write was given, as m3 sees it",
                ),
            ]
        },
    )
    .request("a put that returned, then the same key read from each member")
    .response("one header revision, and three identical copies of the value")
    .note(
        "The put's answer is the promise: by the time that header came back a quorum had the \
         entry, which is why m2 and m3 can serve it. The stage kills the leader at exactly \
         this point and reads the key from whoever is elected next. A put that never answered \
         carries no such promise and may be missing afterwards, so the tests record which \
         puts returned and demand only those.",
    )]
}

/// Wait until every member that is still running names one leader, and that leader is one of
/// them.
///
/// [`Cluster::wait_for_leader`] is satisfied as soon as the running members agree, and for a
/// second or so after a leader is killed they do agree: they all still name the member that
/// has just died, because nobody has missed a heartbeat yet. A test that took that answer and
/// sent its reads to the named member would get a connection refused and call it a lost
/// write. Insisting the leader is itself still running is what makes the answer usable.
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
            .note(last)
            .note(cluster.faults.describe()));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Read a range, retrying for as long as the cluster is still handing over.
///
/// A linearizable read made in the moment a new leader takes over comes back as
/// `etcdserver: leader changed`: the request was never applied, so it says nothing at all
/// about the data. Every real client retries that, and a durability stage that counted it as
/// a vanished write would fail for a reason with nothing to do with durability. A read that
/// keeps failing past the last attempt is still reported, error and all.
async fn range_settled(client: &Client, req: &RangeRequest) -> Result<RangeResponse, EtcdError> {
    let mut attempt = 0;
    loop {
        match client.range_req(req).await {
            Ok(r) => return Ok(r),
            Err(e) if attempt >= 5 => return Err(e),
            Err(_) => attempt += 1,
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// The value of one key, read with [`range_settled`].
async fn value_settled(client: &Client, key: &[u8]) -> Result<Option<Vec<u8>>, EtcdError> {
    Ok(range_settled(client, &RangeRequest::key(key))
        .await?
        .one()
        .map(|kv| kv.value.clone()))
}

dist_test!(one_acknowledged_write, |ctx| {
    let key = ctx.key("only");
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let put = ok(
        cluster.client(leader).put(&key, b"acknowledged").await,
        "the one write this test makes",
    )?;
    cluster.kill(leader).await;
    let fresh = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let read = ok(
        value_settled(cluster.client(fresh), &key).await,
        "reading the key back from the new leader",
    )?;
    let described = cluster.describe().await;
    let mut c = Check::new("the single acknowledged write after the leader was killed");
    c.note(format!(
        "m{} was killed, m{} leads now",
        leader + 1,
        fresh + 1
    ));
    c.note(described);
    c.eq(
        "the value after the kill",
        Some(b"acknowledged".to_vec()),
        read,
    );
    c.at_least("put.header.revision", 1, put.header.revision);
    c.finish()
});

dist_test!(a_hundred_acknowledged_writes, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    // Only the puts that answered go into `acknowledged`. Nothing else is ever demanded.
    let mut acknowledged = Vec::new();
    for i in 0..100 {
        let key = format!("{prefix}/k{i:03}").into_bytes();
        let value = format!("v{i}");
        if cluster
            .client(leader)
            .put(&key, value.as_bytes())
            .await
            .is_ok()
        {
            acknowledged.push((key, value));
        }
    }
    cluster.kill(leader).await;
    let fresh = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let mut lost = Vec::new();
    for (key, want) in &acknowledged {
        match value_settled(cluster.client(fresh), key).await {
            Ok(Some(v)) if v == want.as_bytes() => {}
            other => lost.push(format!("{} -> {other:?}", String::from_utf8_lossy(key))),
        }
    }
    let described = cluster.describe().await;
    let faults = cluster.faults.describe();
    let mut c = Check::new("a hundred acknowledged writes after the leader was killed");
    c.note(format!(
        "m{} was killed, m{} leads now",
        leader + 1,
        fresh + 1
    ));
    c.note(described).note(faults);
    c.eq("puts that were acknowledged", 100, acknowledged.len());
    c.that(
        "acknowledged writes still readable",
        "every one of them",
        lost.is_empty(),
        lost,
    );
    c.finish()
});

dist_test!(acknowledged_through_a_follower, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    // A write sent to a follower is forwarded to the leader and answered from there, so the
    // promise it carries is exactly the same one. Killing the leader must not undo it.
    let follower = (leader + 1) % cluster.initial_size;
    let mut acknowledged = Vec::new();
    for i in 0..30 {
        let key = format!("{prefix}/f{i:03}").into_bytes();
        let value = format!("through-a-follower-{i}");
        if cluster
            .client(follower)
            .put(&key, value.as_bytes())
            .await
            .is_ok()
        {
            acknowledged.push((key, value));
        }
    }
    cluster.kill(leader).await;
    let fresh = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let mut lost = Vec::new();
    for (key, want) in &acknowledged {
        match value_settled(cluster.client(fresh), key).await {
            Ok(Some(v)) if v == want.as_bytes() => {}
            other => lost.push(format!("{} -> {other:?}", String::from_utf8_lossy(key))),
        }
    }
    let described = cluster.describe().await;
    let mut c = Check::new("writes acknowledged by a follower, after the leader was killed");
    c.note(format!(
        "written through m{}, m{} killed, m{} leads now",
        follower + 1,
        leader + 1,
        fresh + 1
    ));
    c.note(described);
    c.eq("puts that were acknowledged", 30, acknowledged.len());
    c.that(
        "acknowledged writes still readable",
        "every one of them",
        lost.is_empty(),
        lost,
    );
    c.finish()
});

dist_test!(the_revision_survives, |ctx| {
    let key = ctx.key("versioned");
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let first = ok(
        cluster.client(leader).put(&key, b"one").await,
        "the first write",
    )?;
    let second = ok(
        cluster.client(leader).put(&key, b"two").await,
        "the second write",
    )?;
    cluster.kill(leader).await;
    let fresh = live_leader(cluster, Duration::from_millis(20_000)).await?;
    // The revision a write was acknowledged with is part of the acknowledgement: history
    // that was promised to a client cannot be rewritten by an election either.
    let old = ok(
        range_settled(
            cluster.client(fresh),
            &RangeRequest::key(&key).at_revision(first.header.revision),
        )
        .await,
        "reading the key at the revision the first write was given",
    )?;
    let now = ok(
        range_settled(cluster.client(fresh), &RangeRequest::key(&key)).await,
        "reading the key at head",
    )?;
    let described = cluster.describe().await;
    let mut c = Check::new("the revisions two acknowledged writes were given");
    c.note(format!(
        "m{} was killed, m{} leads now",
        leader + 1,
        fresh + 1
    ));
    c.note(described);
    c.at_least(
        "the second revision is above the first",
        first.header.revision + 1,
        second.header.revision,
    );
    c.eq(
        "the value at the first revision",
        Some("one".to_string()),
        old.one().map(|kv| kv.value_str()),
    );
    c.eq(
        "the value at head",
        Some("two".to_string()),
        now.one().map(|kv| kv.value_str()),
    );
    c.eq(
        "kvs[0].mod_revision at head",
        Some(second.header.revision),
        now.one().map(|kv| kv.mod_revision),
    );
    c.finish()
});

dist_test!(a_kill_during_a_burst, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    // Every put in the burst goes to a member that is not about to die, so the burst can
    // carry on straight across the kill. The puts that fall in the election window fail and
    // are simply not recorded: an unanswered write promised nothing.
    let writer = (leader + 1) % cluster.initial_size;
    let mut acknowledged = Vec::new();
    let mut refused = 0usize;
    for i in 0..40 {
        if i == 20 {
            cluster.kill(leader).await;
        }
        let key = format!("{prefix}/b{i:03}").into_bytes();
        let value = format!("burst-{i}");
        if cluster
            .client(writer)
            .put(&key, value.as_bytes())
            .await
            .is_ok()
        {
            acknowledged.push((key, value));
        } else {
            refused += 1;
        }
    }
    let fresh = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let mut lost = Vec::new();
    for (key, want) in &acknowledged {
        match value_settled(cluster.client(fresh), key).await {
            Ok(Some(v)) if v == want.as_bytes() => {}
            other => lost.push(format!("{} -> {other:?}", String::from_utf8_lossy(key))),
        }
    }
    let described = cluster.describe().await;
    let acked = acknowledged.len();
    let mut c = Check::new("a burst of writes with the leader killed halfway through");
    c.note(format!(
        "written through m{}, m{} killed after 20 puts, m{} leads now",
        writer + 1,
        leader + 1,
        fresh + 1
    ));
    c.note(described);
    c.observe("puts refused during the election", refused);
    c.at_least("puts that were acknowledged", 20, acked);
    c.that(
        "acknowledged writes still readable",
        "every one of them",
        lost.is_empty(),
        lost,
    );
    c.finish()
});

dist_test!(survivors_agree, |ctx| {
    let prefix = ctx.prefix();
    let range = RangeRequest::prefix(format!("{prefix}/").as_bytes());
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    for i in 0..25 {
        let key = format!("{prefix}/a{i:03}").into_bytes();
        let _ = cluster.client(leader).put(&key, b"mixed").await;
    }
    cluster.kill(leader).await;
    let fresh = live_leader(cluster, Duration::from_millis(20_000)).await?;
    // Writes that were still in flight when the leader died may be present or absent, and
    // this test deliberately does not say which. What it does say is that the survivors
    // cannot disagree: a key that exists on one of them and not on the other is a split
    // brain regardless of what any client was told.
    let mut views = Vec::new();
    for i in cluster.running() {
        let name = cluster.members[i].name.clone();
        let r = ok(
            range_settled(cluster.client(i), &range).await,
            &format!("reading the whole prefix from {name}"),
        )?;
        let mut pairs: Vec<String> = r
            .kvs
            .iter()
            .map(|kv| format!("{}={}", kv.key_str(), kv.value_str()))
            .collect();
        pairs.sort();
        views.push((name, pairs));
    }
    let described = cluster.describe().await;
    let mut c = Check::new("what each survivor holds after the leader was killed");
    c.note(format!(
        "m{} was killed, m{} leads now",
        leader + 1,
        fresh + 1
    ));
    c.note(described);
    c.at_least("surviving members", 2, views.len());
    let (first_name, first) = views[0].clone();
    for (name, pairs) in views.iter().skip(1) {
        c.that(
            &format!("{name} holds what {first_name} holds"),
            "the same keys and values on every survivor",
            *pairs == first,
            (pairs.len(), first.len()),
        );
    }
    c.finish()
});

dist_test!(a_delete_is_durable, |ctx| {
    let gone = ctx.key("gone");
    let kept = ctx.key("kept");
    let cluster = ctx.cluster()?;
    let leader = live_leader(cluster, Duration::from_millis(20_000)).await?;
    ok(
        cluster.client(leader).put(&gone, b"about to go").await,
        "writing the key that will be deleted",
    )?;
    ok(
        cluster.client(leader).put(&kept, b"stays").await,
        "writing the key that stays",
    )?;
    let deleted = ok(
        cluster.client(leader).delete(&gone).await,
        "deleting the first key",
    )?;
    cluster.kill(leader).await;
    let fresh = live_leader(cluster, Duration::from_millis(20_000)).await?;
    // A delete is an entry like any other. An acknowledged one coming back after an election
    // would be a resurrected value, which is the same bug as a lost write wearing a mask.
    let after_gone = ok(
        value_settled(cluster.client(fresh), &gone).await,
        "reading the deleted key after the kill",
    )?;
    let after_kept = ok(
        value_settled(cluster.client(fresh), &kept).await,
        "reading the surviving key after the kill",
    )?;
    let described = cluster.describe().await;
    let mut c = Check::new("an acknowledged delete after the leader was killed");
    c.note(format!(
        "m{} was killed, m{} leads now",
        leader + 1,
        fresh + 1
    ));
    c.note(described);
    c.eq("deleterange.deleted", 1, deleted.deleted);
    c.eq("the deleted key after the kill", None, after_gone);
    c.eq(
        "the untouched key after the kill",
        Some(b"stays".to_vec()),
        after_kept,
    );
    c.finish()
});

dist_test!(five_members_two_kills, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let mut acknowledged = Vec::new();
    let mut killed = Vec::new();
    // Five members tolerate two failures, so the same promise has to hold across two
    // elections in a row rather than one.
    for round in 0..2 {
        let leader = live_leader(cluster, Duration::from_millis(25_000)).await?;
        for i in 0..20 {
            let key = format!("{prefix}/r{round}k{i:02}").into_bytes();
            let value = format!("round-{round}-{i}");
            if cluster
                .client(leader)
                .put(&key, value.as_bytes())
                .await
                .is_ok()
            {
                acknowledged.push((key, value));
            }
        }
        cluster.kill(leader).await;
        killed.push(leader + 1);
    }
    let fresh = live_leader(cluster, Duration::from_millis(25_000)).await?;
    let mut lost = Vec::new();
    for (key, want) in &acknowledged {
        match value_settled(cluster.client(fresh), key).await {
            Ok(Some(v)) if v == want.as_bytes() => {}
            other => lost.push(format!("{} -> {other:?}", String::from_utf8_lossy(key))),
        }
    }
    let described = cluster.describe().await;
    let mut c = Check::new("forty acknowledged writes across two leader kills of five members");
    c.note(format!("killed m{killed:?}, m{} leads now", fresh + 1));
    c.note(described);
    c.eq("puts that were acknowledged", 40, acknowledged.len());
    c.eq("members still running", 3, cluster.running().len());
    c.that(
        "acknowledged writes still readable",
        "every one of them",
        lost.is_empty(),
        lost,
    );
    c.finish()
});
