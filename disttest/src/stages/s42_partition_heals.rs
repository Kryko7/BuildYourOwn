//! Stage 42 — A healed partition catches the minority up.
//!
//! The third act of the partition story. Stage 40 made the cut-off member refuse, stage 41
//! made the majority carry on, and this one puts the link back and watches the member that
//! missed everything rejoin. Nothing restarts it, nobody tells it what it missed, and no
//! operator intervenes: the leader notices its follower is behind, walks its log index back
//! until the two agree, and ships the entries from there.
//!
//! Three things have to be true afterwards, and they are easy to get two out of three on.
//! The stale member has to end up holding the values the majority committed; it has to
//! adopt the term the majority moved to, so a member that still thought it was leader steps
//! down rather than arguing; and whatever it accepted on its own side while it was cut off
//! has to be *gone*, not merged in. A catch-up that keeps the minority's own entries is a
//! split-brain with good manners.
//!
//! The catch-up is checked with `serializable: true` reads on purpose. A linearizable read
//! on the healed member would be answered via the leader and would prove nothing about what
//! that member actually holds; a serializable read is the only way to ask a member what is
//! in its own store.
//!
//! Every test cuts the network, so every one is `.fresh()` with a generous floor under its
//! timeout.

use crate::assert::{Check, Failure, FailureKind};
use crate::cluster::proxy::LinkFault;
use crate::cluster::Cluster;
use crate::dist_test;
use crate::etcd::{b64, Client, RangeRequest};
use crate::examples::{cluster_example, step, ExampleSpec};
use crate::stages::{expect_error, ok, wait_until, Ladder, Stage, Test};
use serde_json::json;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// How long the cluster is given to settle on one leader.
const ELECT_WITHIN: Duration = Duration::from_millis(15_000);
/// How long a member that missed some entries is given to catch up once the link is back.
const CATCH_UP_WITHIN: Duration = Duration::from_millis(30_000);
/// The floor under every test in this stage.
const SLOW: u64 = 60_000;

/// Stage 42.
pub fn stage() -> Stage {
    Stage {
        number: 42,
        slug: "partition_heals",
        name: "A healed partition catches the minority up",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "When the link comes back, the stale member learns the entries it missed",
            "It adopts the higher term and steps down if it still thought it was leader",
            "Nothing the minority side accepted may appear in the log afterwards",
            "The catch-up must finish without a restart or an operator",
        ],
        examples,
        tests: vec![
            Test::new(
                "a cut-off member catches up on its own once the link is back",
                catches_up_on_its_own,
            )
            .ext()
            .fresh()
            .min_timeout_ms(SLOW),
            Test::new(
                "the healed member adopts the term and the leader the majority moved to",
                adopts_the_higher_term,
            )
            .ext()
            .fresh()
            .min_timeout_ms(SLOW),
            Test::new(
                "nothing the cut-off member tried to write survives the heal",
                the_minoritys_write_never_appears,
            )
            .ext()
            .fresh()
            .min_timeout_ms(SLOW),
            Test::new(
                "the whole cluster agrees on one revision again",
                one_revision_again,
            )
            .ext()
            .fresh()
            .min_timeout_ms(SLOW),
            Test::new(
                "a delete made during the partition reaches the healed member too",
                a_delete_catches_up,
            )
            .ext()
            .fresh()
            .min_timeout_ms(SLOW),
            Test::new(
                "a second partition and heal works exactly the same way",
                a_second_cycle,
            )
            .ext()
            .fresh()
            .min_timeout_ms(90_000),
            Test::new(
                "a member that missed a few hundred writes still catches up",
                a_long_absence,
            )
            .ext()
            .fresh()
            .min_timeout_ms(90_000),
            Test::new(
                "a five-member cluster heals a two-against-three split",
                five_members_heal,
            )
            .ext()
            .cluster(5)
            .min_timeout_ms(90_000),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![cluster_example("Asking a member what it holds itself", || {
        vec![
            step(
                0,
                "/v3/kv/put",
                json!({"key": b64(b"missed"), "value": b64(b"while-you-were-out")}),
                "write through m1",
            ),
            step(
                2,
                "/v3/kv/range",
                json!({"key": b64(b"missed"), "serializable": true}),
                "ask m3 what is in its own store, with no round trip to the leader",
            ),
            step(
                2,
                "/v3/maintenance/status",
                json!({}),
                "and how far m3's log and term have got",
            ),
        ]
    })
    .request("a write, then a serializable read and a status on a different member")
    .response("m3's own copy of the value, and the term and revision it is on")
    .note(
        "These are the two requests a catch-up is measured with. Imagine m3 cut off for the \
         write: the serializable read would answer nothing, because that read is the only \
         one that tells you what a member really holds rather than what the leader would \
         say on its behalf. Put the link back and the same read starts answering \
         `while-you-were-out` a moment later, with no restart — and `raft_term` in the \
         status has moved up to whatever the majority elected in the meantime.",
    )]
}

/// Poll until every member of `group` names one leader that is itself in `group`.
///
/// [`Cluster::wait_for_leader`] asks every *running* member, which never agrees while a
/// partition stands, so a partitioned test has to ask one side of the split.
async fn leader_of(cluster: &Cluster, group: &[usize], within: Duration) -> Result<usize, Failure> {
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
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Treat a dialer the harness cannot attribute as being on the wrong side of the split.
///
/// A proxy only sees the destination of a connection, so it asks the kernel which process
/// opened it; when that lookup cannot be made it falls back to the destination's own link,
/// which is clean by default. An unattributable connection would then cross the partition
/// the test just asked for. Cutting every member's self-link closes that door: identified
/// peer traffic is untouched, and anything the injector cannot account for is refused
/// rather than waved through. [`Cluster::heal`] clears these along with everything else.
async fn cut_unattributable_links(cluster: &mut Cluster) {
    for j in 0..cluster.members.len() {
        cluster
            .link_fault(
                j,
                j,
                LinkFault {
                    cut: true,
                    ..Default::default()
                },
            )
            .await;
    }
}

/// Wait until the proxies have actually refused something, so a test that is about to rely
/// on a member being cut off is not racing the fault it just injected.
async fn wait_for_the_cut_to_bite(cluster: &Cluster, before: u64) -> Result<(), Failure> {
    wait_until(
        "the proxies to start refusing the cut-off member's peer traffic",
        Duration::from_millis(15_000),
        || async { cluster.stats.blocked.load(Ordering::Relaxed) > before },
    )
    .await
    .map(|_| ())
}

/// Poll one member's *own* store until it holds `want` for `key`.
///
/// `serializable` is the point: it is the only read that cannot be answered on the
/// member's behalf by the leader, so it is the only one that proves a catch-up happened.
async fn wait_for_local_value(
    client: &Client,
    member: &str,
    key: &[u8],
    want: &str,
) -> Result<Duration, Failure> {
    let req = RangeRequest::key(key).with("serializable", json!(true));
    wait_until(
        &format!("{member}'s own store to hold {want:?}"),
        CATCH_UP_WITHIN,
        || async {
            matches!(
                client.range_req(&req).await,
                Ok(r) if r.one().map(|kv| kv.value_str()).as_deref() == Some(want)
            )
        },
    )
    .await
}

/// The lowest-numbered member that is not the leader.
fn a_follower(leader: usize, size: usize) -> usize {
    (0..size).find(|i| *i != leader).unwrap_or(0)
}

dist_test!(catches_up_on_its_own, |ctx| {
    let prefix = ctx.key("missed");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let cut = a_follower(leader, size);
    let name = cluster.members[cut].name.clone();
    let before_blocked = cluster.stats.blocked.load(Ordering::Relaxed);
    cluster.isolate(cut).await;
    cut_unattributable_links(cluster).await;
    wait_for_the_cut_to_bite(cluster, before_blocked).await?;
    let rest: Vec<usize> = (0..size).filter(|i| *i != cut).collect();
    let serving = leader_of(cluster, &rest, ELECT_WITHIN).await?;
    for n in 0..5 {
        let key = [prefix.as_slice(), format!("/{n}").as_bytes()].concat();
        ok(
            cluster
                .client(serving)
                .put(&key, format!("v{n}").as_bytes())
                .await,
            "a write the cut-off member is missing",
        )?;
    }
    let last = [prefix.as_slice(), b"/4"].concat();
    // Nothing else happens here: no restart, no command, no operator. The link comes back
    // and the leader works out how far behind its follower is.
    cluster.heal().await;
    let took = wait_for_local_value(cluster.client(cut), &name, &last, "v4").await?;
    let mut c = Check::new("the store of a member that has just rejoined");
    for n in 0..5 {
        let key = [prefix.as_slice(), format!("/{n}").as_bytes()].concat();
        let read = ok(
            cluster
                .client(cut)
                .range_req(&RangeRequest::key(&key).with("serializable", json!(true)))
                .await,
            &format!("a serializable read on {name}"),
        )?;
        c.eq(
            &format!("{name}.range({n}).kvs[0].value (serializable)"),
            format!("v{n}"),
            read.one().map(|kv| kv.value_str()).unwrap_or_default(),
        );
    }
    // And it serves a linearizable read again, which it could not do while cut off.
    let fresh = ok(
        cluster.client(cut).get_key(&last).await,
        &format!("a linearizable read on {name} after the heal"),
    )?;
    c.eq(
        &format!("{name}.range.kvs[0].value"),
        "v4".to_string(),
        fresh.one().map(|kv| kv.value_str()).unwrap_or_default(),
    );
    c.eq("the members still running", size, cluster.running().len());
    let described = cluster.describe().await;
    c.note(format!(
        "{name} missed five writes and caught up {} ms after the heal",
        took.as_millis()
    ))
    .note(described);
    c.finish()?;
    ctx.note(format!(
        "{name} caught up on its own {} ms after the link came back",
        took.as_millis()
    ));
    Ok(())
});

dist_test!(adopts_the_higher_term, |ctx| {
    let key = ctx.key("term");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let name = cluster.members[leader].name.clone();
    let old_term = ok(
        cluster.client(leader).status().await,
        "the old leader's status",
    )?
    .raft_term;
    let size = cluster.initial_size;
    let rest: Vec<usize> = (0..size).filter(|i| *i != leader).collect();
    // Cutting the leader off is what makes the term move: the other two elect one of their
    // own, and the old leader comes back into a cluster that has moved on without it.
    cluster.isolate(leader).await;
    cut_unattributable_links(cluster).await;
    let new_leader = leader_of(cluster, &rest, ELECT_WITHIN).await?;
    let new_term = ok(
        cluster.client(new_leader).status().await,
        "the new leader's status",
    )?
    .raft_term;
    ok(
        cluster
            .client(new_leader)
            .put(&key, b"after-the-election")
            .await,
        "a write under the new leadership",
    )?;
    cluster.heal().await;
    let client = cluster.client(leader);
    let took = wait_until(
        &format!("{name} to adopt the majority's term"),
        CATCH_UP_WITHIN,
        || async { matches!(client.status().await, Ok(s) if s.raft_term >= new_term) },
    )
    .await?;
    let settled = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let status = ok(
        cluster.client(leader).status().await,
        &format!("{name}'s status after the heal"),
    )?;
    let mut c = Check::new("the term and the leader a rejoining member ends up on");
    c.that(
        "the majority's term against the old leader's",
        "a strictly larger term after the election",
        new_term > old_term,
        (old_term, new_term),
    );
    c.at_least(
        &format!("{name}.status.raft_term"),
        new_term,
        status.raft_term,
    );
    // It must have stepped down as well as caught up: naming somebody else as leader is
    // how a member says it is no longer arguing.
    c.eq(
        &format!("{name}.status.leader"),
        cluster.members[settled].member_id,
        status.leader,
    );
    let described = cluster.describe().await;
    c.note(format!(
        "{name} was the leader in term {old_term}; the majority elected m{} in term {new_term}, \
         and {name} adopted it {} ms after the heal",
        new_leader + 1,
        took.as_millis()
    ))
    .note(described);
    c.finish()
});

dist_test!(the_minoritys_write_never_appears, |ctx| {
    let key = ctx.key("never");
    let marker_key = ctx.key("marker");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let name = cluster.members[leader].name.clone();
    let rest: Vec<usize> = (0..size).filter(|i| *i != leader).collect();
    // The old leader is the member most likely to have written something locally: it is the
    // one that was allowed to append to its own log a moment ago.
    let before_blocked = cluster.stats.blocked.load(Ordering::Relaxed);
    cluster.isolate(leader).await;
    cut_unattributable_links(cluster).await;
    wait_for_the_cut_to_bite(cluster, before_blocked).await?;
    let e = expect_error(
        cluster.client(leader).put(&key, b"from-the-minority").await,
        &format!("a write through {name} while it had no quorum"),
    )?;
    let new_leader = leader_of(cluster, &rest, ELECT_WITHIN).await?;
    let marker = ok(
        cluster
            .client(new_leader)
            .put(&marker_key, b"majority")
            .await,
        "a write on the majority side",
    )?;
    cluster.heal().await;
    cluster
        .wait_for_revision(marker.header.revision, CATCH_UP_WITHIN)
        .await?;
    let mut c = Check::new("what happened to the write the minority refused");
    // Refused means refused, on every member and in every store, for ever. A catch-up that
    // merged the old leader's uncommitted entry back in would show up right here.
    for i in 0..size {
        let member = cluster.members[i].name.clone();
        let read = ok(
            cluster
                .client(i)
                .range_req(&RangeRequest::key(&key).with("serializable", json!(true)))
                .await,
            &format!("a serializable read on {member}"),
        )?;
        c.eq(
            &format!("{member}.range.count (serializable)"),
            0,
            read.count,
        );
    }
    let linearizable = ok(
        cluster.client(new_leader).get_key(&key).await,
        "a linearizable read of the refused key",
    )?;
    c.eq("range.count", 0, linearizable.count);
    let described = cluster.describe().await;
    c.note(format!("{name} refused the write with: {e}"))
        .note(described);
    c.finish()
});

dist_test!(one_revision_again, |ctx| {
    let prefix = ctx.key("converge");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let cut = a_follower(leader, size);
    let before_blocked = cluster.stats.blocked.load(Ordering::Relaxed);
    cluster.isolate(cut).await;
    cut_unattributable_links(cluster).await;
    wait_for_the_cut_to_bite(cluster, before_blocked).await?;
    let rest: Vec<usize> = (0..size).filter(|i| *i != cut).collect();
    let serving = leader_of(cluster, &rest, ELECT_WITHIN).await?;
    let mut last = 0;
    for n in 0..8 {
        let key = [prefix.as_slice(), format!("/{n}").as_bytes()].concat();
        last = ok(
            cluster.client(serving).put(&key, b"x").await,
            "a write during the partition",
        )?
        .header
        .revision;
    }
    let behind = ok(
        cluster.client(cut).status().await,
        "the cut-off member's revision",
    )?
    .header
    .revision;
    cluster.heal().await;
    cluster.wait_for_revision(last, CATCH_UP_WITHIN).await?;
    let mut revisions = Vec::new();
    for i in 0..size {
        let name = cluster.members[i].name.clone();
        let s = ok(
            cluster.client(i).status().await,
            &format!("{name}'s status after the heal"),
        )?;
        revisions.push((name, s.header.revision));
    }
    let mut c = Check::new("the revision every member reports after the heal");
    c.that(
        "the cut-off member's revision during the partition",
        "strictly behind the majority's",
        behind < last,
        (behind, last),
    );
    for (name, revision) in &revisions {
        c.eq(&format!("{name}.status.header.revision"), last, *revision);
    }
    let described = cluster.describe().await;
    c.note(described);
    c.finish()
});

dist_test!(a_delete_catches_up, |ctx| {
    let key = ctx.key("deleted");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let cut = a_follower(leader, size);
    let name = cluster.members[cut].name.clone();
    let put = ok(
        cluster.client(leader).put(&key, b"doomed").await,
        "the write everybody sees first",
    )?;
    cluster
        .wait_for_revision(put.header.revision, Duration::from_millis(10_000))
        .await?;
    let before_blocked = cluster.stats.blocked.load(Ordering::Relaxed);
    cluster.isolate(cut).await;
    cut_unattributable_links(cluster).await;
    wait_for_the_cut_to_bite(cluster, before_blocked).await?;
    let rest: Vec<usize> = (0..size).filter(|i| *i != cut).collect();
    let serving = leader_of(cluster, &rest, ELECT_WITHIN).await?;
    let del = ok(
        cluster.client(serving).delete(&key).await,
        "a delete the cut-off member is missing",
    )?;
    // While it is cut off it still holds the value, which is what makes the catch-up
    // visible: the entry it has to learn is a removal, not an addition.
    let during = ok(
        cluster
            .client(cut)
            .range_req(&RangeRequest::key(&key).with("serializable", json!(true)))
            .await,
        &format!("a serializable read on {name} during the partition"),
    )?;
    cluster.heal().await;
    let client = cluster.client(cut);
    let req = RangeRequest::key(&key).with("serializable", json!(true));
    let took = wait_until(
        &format!("{name}'s own store to lose the deleted key"),
        CATCH_UP_WITHIN,
        || async { matches!(client.range_req(&req).await, Ok(r) if r.count == 0) },
    )
    .await?;
    let mut c = Check::new("a delete that happened while a member was cut off");
    c.eq("deleterange.deleted", 1, del.deleted);
    c.eq(
        &format!("{name}.range.count during the partition"),
        1,
        during.count,
    );
    let described = cluster.describe().await;
    c.note(format!(
        "{name} caught the delete up {} ms after the heal",
        took.as_millis()
    ))
    .note(described);
    c.finish()
});

dist_test!(a_second_cycle, |ctx| {
    let key = ctx.key("cycles");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let cut = a_follower(leader, size);
    let name = cluster.members[cut].name.clone();
    let rest: Vec<usize> = (0..size).filter(|i| *i != cut).collect();
    let mut times = Vec::new();
    // Healing must leave the cluster in the state it started in, not in a slightly worse
    // one; the second cycle is the cheapest way to say so.
    for round in 0..2 {
        let before_blocked = cluster.stats.blocked.load(Ordering::Relaxed);
        cluster.isolate(cut).await;
        cut_unattributable_links(cluster).await;
        wait_for_the_cut_to_bite(cluster, before_blocked).await?;
        let serving = leader_of(cluster, &rest, ELECT_WITHIN).await?;
        let value = format!("round-{round}");
        ok(
            cluster.client(serving).put(&key, value.as_bytes()).await,
            "a write during the partition",
        )?;
        cluster.heal().await;
        times.push(wait_for_local_value(cluster.client(cut), &name, &key, &value).await?);
    }
    let mut c = Check::new("two partition-and-heal cycles in a row");
    let read = ok(
        cluster.client(cut).get_key(&key).await,
        &format!("a linearizable read on {name} after the second heal"),
    )?;
    c.eq(
        &format!("{name}.range.kvs[0].value"),
        "round-1".to_string(),
        read.one().map(|kv| kv.value_str()).unwrap_or_default(),
    );
    c.eq("the members still running", size, cluster.running().len());
    let described = cluster.describe().await;
    c.note(format!(
        "{name} caught up after {} ms and then after {} ms",
        times[0].as_millis(),
        times[1].as_millis()
    ))
    .note(described);
    c.finish()
});

dist_test!(a_long_absence, |ctx| {
    let prefix = ctx.key("long");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let cut = a_follower(leader, size);
    let name = cluster.members[cut].name.clone();
    let before_blocked = cluster.stats.blocked.load(Ordering::Relaxed);
    cluster.isolate(cut).await;
    cut_unattributable_links(cluster).await;
    wait_for_the_cut_to_bite(cluster, before_blocked).await?;
    let rest: Vec<usize> = (0..size).filter(|i| *i != cut).collect();
    let serving = leader_of(cluster, &rest, ELECT_WITHIN).await?;
    // Three hundred entries is far more than a heartbeat carries, so the leader has to
    // walk its follower's index back and ship a run of entries rather than one.
    let writes = 300;
    let mut last = 0;
    for n in 0..writes {
        let key = [prefix.as_slice(), format!("/{n:03}").as_bytes()].concat();
        last = ok(
            cluster
                .client(serving)
                .put(&key, format!("v{n}").as_bytes())
                .await,
            "one of three hundred writes during the partition",
        )?
        .header
        .revision;
    }
    cluster.heal().await;
    let took = wait_for_local_value(
        cluster.client(cut),
        &name,
        &[prefix.as_slice(), b"/299"].concat(),
        "v299",
    )
    .await?;
    cluster.wait_for_revision(last, CATCH_UP_WITHIN).await?;
    let read = ok(
        cluster
            .client(cut)
            .range_req(&RangeRequest::prefix(&prefix).with("serializable", json!(true)))
            .await,
        &format!("a serializable prefix read on {name}"),
    )?;
    let mut c = Check::new("a member that missed three hundred writes");
    c.eq(
        &format!("{name}.range.count (serializable)"),
        writes as i64,
        read.count,
    );
    c.eq("the members still running", size, cluster.running().len());
    let described = cluster.describe().await;
    c.note(format!(
        "{name} caught up on {writes} entries {} ms after the heal",
        took.as_millis()
    ))
    .note(described);
    c.finish()?;
    ctx.note(format!(
        "{name} replayed {writes} missed entries in {} ms with no restart",
        took.as_millis()
    ));
    Ok(())
});

dist_test!(five_members_heal, |ctx| {
    let key = ctx.key("five");
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT_WITHIN).await?;
    let minority = vec![0usize, 1];
    let majority_side = vec![2usize, 3, 4];
    let before_blocked = cluster.stats.blocked.load(Ordering::Relaxed);
    cluster
        .partition(&[minority.clone(), majority_side.clone()])
        .await;
    cut_unattributable_links(cluster).await;
    wait_for_the_cut_to_bite(cluster, before_blocked).await?;
    let serving = leader_of(cluster, &majority_side, ELECT_WITHIN).await?;
    let put = ok(
        cluster.client(serving).put(&key, b"three-of-five").await,
        "a write on the majority side of five",
    )?;
    cluster.heal().await;
    let mut times = Vec::new();
    for i in &minority {
        let name = cluster.members[*i].name.clone();
        times.push(wait_for_local_value(cluster.client(*i), &name, &key, "three-of-five").await?);
    }
    cluster
        .wait_for_revision(put.header.revision, CATCH_UP_WITHIN)
        .await?;
    let settled = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let mut c = Check::new("both members of a five-member minority, after the heal");
    for i in 0..cluster.initial_size {
        let name = cluster.members[i].name.clone();
        let s = ok(
            cluster.client(i).status().await,
            &format!("{name}'s status after the heal"),
        )?;
        c.eq(
            &format!("{name}.status.leader"),
            cluster.members[settled].member_id,
            s.leader,
        );
        c.at_least(
            &format!("{name}.status.header.revision"),
            put.header.revision,
            s.header.revision,
        );
    }
    let described = cluster.describe().await;
    c.note(format!(
        "the two cut-off members caught up after {} ms and {} ms",
        times[0].as_millis(),
        times[1].as_millis()
    ))
    .note(described);
    c.finish()
});
