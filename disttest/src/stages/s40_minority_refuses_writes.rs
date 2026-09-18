//! Stage 40 — The minority of a partition refuses writes.
//!
//! The first stage where the network stops working, and the first where the correct
//! behaviour is to *fail*. A member cut off from the rest of the cluster can still be
//! reached by clients — client traffic goes straight to the member, only peer traffic goes
//! through the harness's proxies — so it is perfectly able to accept a write, apply it to
//! its own store and answer `200`. That is the bug. Without a quorum it cannot know the
//! write will survive, and it cannot know the value it holds is still current.
//!
//! The same argument covers reads, and that is the part people get wrong. A linearizable
//! read answered out of a partitioned member's own store is a stale read dressed up as a
//! fresh one, which is the lost write's quieter sibling. The one read that may be answered
//! is the one that asked to be: `serializable: true` means "local state is fine", and then
//! a stale answer is documented behaviour rather than a lie.
//!
//! How the refusal arrives is deliberately not pinned down. A member that has already
//! noticed it has no leader can say so at once — etcd answers `etcdserver: no leader`,
//! code 14. A member that has not noticed yet accepts the request, waits for a quorum that
//! never answers, and gives up on its own deadline, which reaches the client as a timeout
//! instead. The reference does both, and which one arrives depends on how far through its
//! election timeout the member was when the link went down, so every test here demands only
//! that the request *failed* and records what the failure was. None of them tolerate a 200.
//!
//! Every test cuts the network, so every one is `.fresh()` with a generous floor under its
//! timeout: starting a cluster takes a couple of seconds, an election another, and a
//! refused write can sit on the server's own request timeout for several more.

use crate::assert::{Check, Failure, FailureKind};
use crate::cluster::proxy::LinkFault;
use crate::cluster::Cluster;
use crate::dist_test;
use crate::etcd::{b64, RangeRequest};
use crate::examples::{cluster_example, step, ExampleSpec};
use crate::stages::{expect_error, majority, ok, wait_until, Ladder, Stage, Test};
use serde_json::json;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// How long the cluster is given to settle on one leader.
const ELECT_WITHIN: Duration = Duration::from_millis(15_000);
/// How long a cut-off member is given to admit, of its own accord, that it has no leader.
const GIVE_UP_WITHIN: Duration = Duration::from_millis(8_000);
/// The floor under every test in this stage: start a cluster, cut it, wait for a refusal.
const SLOW: u64 = 60_000;

/// Stage 40.
pub fn stage() -> Stage {
    Stage {
        number: 40,
        slug: "minority_refuses_writes",
        name: "The minority of a partition refuses writes",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "A member that cannot reach a quorum must refuse a write rather than apply it locally",
            "It must also refuse a linearizable read: answering from stale state is the same bug",
            "Refuse with an error the client can see, and do not hang forever",
            "A serializable read may still be answered, and may be stale — that is what it means",
        ],
        examples,
        tests: vec![
            Test::new("the isolated member refuses a write", refuses_a_write)
                .ext()
                .fresh()
                .min_timeout_ms(SLOW),
            Test::new(
                "the isolated member refuses a linearizable read",
                refuses_a_linearizable_read,
            )
            .ext()
            .fresh()
            .min_timeout_ms(SLOW),
            Test::new(
                "a serializable read on the isolated member is still answered",
                serializable_still_answers,
            )
            .ext()
            .fresh()
            .min_timeout_ms(SLOW),
            Test::new(
                "the isolated leader does not carry on as if nothing had happened",
                the_isolated_leader_gives_up,
            )
            .ext()
            .fresh()
            .min_timeout_ms(SLOW),
            Test::new(
                "the majority keeps accepting writes while one member is cut off",
                the_majority_keeps_writing,
            )
            .ext()
            .fresh()
            .min_timeout_ms(SLOW),
            Test::new(
                "the isolated member still answers for itself",
                still_answers_for_itself,
            )
            .ext()
            .fresh()
            .min_timeout_ms(SLOW),
            Test::new(
                "the isolated member writes again once the partition heals",
                healing_restores_writes,
            )
            .ext()
            .fresh()
            .min_timeout_ms(SLOW),
            Test::new(
                "two members of five, cut off together, still refuse writes",
                a_minority_of_two_in_five,
            )
            .ext()
            .cluster(5)
            .min_timeout_ms(90_000),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![cluster_example("A write, and the quorum behind it", || {
        vec![
            step(
                0,
                "/v3/kv/put",
                json!({"key": b64(b"quorum"), "value": b64(b"accepted")}),
                "write through m1 while all three members can see each other",
            ),
            step(
                1,
                "/v3/maintenance/status",
                json!({}),
                "m2 names a leader, which is what makes that write safe",
            ),
        ]
    })
    .request("a put on a healthy three-member cluster, and the status behind it")
    .response("the put succeeds, and every member names the same leader")
    .note(
        "The transcript shows the good path, because a recorded example cannot cut a \
         network. Now picture m1 alone on one side of a partition. The client can still \
         reach it, so nothing stops it answering this same put with a 200 — and the value \
         would be gone the moment the partition healed. What it must answer instead is an \
         error the client can see: either `etcdserver: no leader` (code 14) if it has \
         already noticed, or a deadline if it is still waiting for a quorum that will never \
         answer. Both are correct; a 200 is not.",
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
/// on a member being cut off is not racing the fault it just asked for.
///
/// Injecting a fault sets a flag; it does not reach into an open TCP connection. A peer
/// stream that was live a millisecond ago stays live until the proxy next looks, so a
/// request sent straight after `isolate` can still be answered over the old connection.
/// The injector's `blocked` counter moving is the proxy saying it has looked.
async fn wait_for_the_cut_to_bite(cluster: &Cluster, before: u64) -> Result<(), Failure> {
    wait_until(
        "the proxies to start refusing the cut-off member's peer traffic",
        Duration::from_millis(15_000),
        || async { cluster.stats.blocked.load(Ordering::Relaxed) > before },
    )
    .await
    .map(|_| ())
}

/// The lowest-numbered member that is not the leader, so a run is reproducible whichever
/// member happened to win the election.
fn a_follower(leader: usize, size: usize) -> usize {
    (0..size).find(|i| *i != leader).unwrap_or(0)
}

dist_test!(refuses_a_write, |ctx| {
    let key = ctx.key("refused");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let cut = a_follower(leader, cluster.initial_size);
    let name = cluster.members[cut].name.clone();
    let before_blocked = cluster.stats.blocked.load(Ordering::Relaxed);
    cluster.isolate(cut).await;
    cut_unattributable_links(cluster).await;
    wait_for_the_cut_to_bite(cluster, before_blocked).await?;
    let started = Instant::now();
    let e = expect_error(
        cluster.client(cut).put(&key, b"should-not-land").await,
        &format!("a write through {name}, which can reach nobody"),
    )?;
    let took = started.elapsed();
    let mut c = Check::new("a write through a member with no quorum");
    // Absent on the majority side, which is the obvious half of the property.
    for i in 0..cluster.initial_size {
        if i == cut {
            continue;
        }
        let other = cluster.members[i].name.clone();
        let read = ok(
            cluster.client(i).get_key(&key).await,
            &format!("a read on {other}"),
        )?;
        c.eq(&format!("{other}.range.count"), 0, read.count);
    }
    // And absent on the member that refused it, which is the half people get wrong: a
    // server that applies the write locally and *then* reports an error has still lost a
    // write, it has just been polite about it.
    let local = ok(
        cluster
            .client(cut)
            .range_req(&RangeRequest::key(&key).with("serializable", json!(true)))
            .await,
        &format!("a serializable read on {name}"),
    )?;
    c.eq(
        &format!("{name}.range.count (serializable)"),
        0,
        local.count,
    );
    let described = cluster.describe().await;
    let faults = cluster.faults.describe();
    c.note(format!(
        "{name} refused the write after {} ms with: {e}",
        took.as_millis()
    ))
    .note(described)
    .note(faults);
    c.finish()?;
    ctx.note(format!(
        "{name}, cut off from both peers, refused the write after {} ms: {e}",
        took.as_millis()
    ));
    Ok(())
});

dist_test!(refuses_a_linearizable_read, |ctx| {
    let key = ctx.key("stale");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let cut = a_follower(leader, cluster.initial_size);
    let name = cluster.members[cut].name.clone();
    let put = ok(
        cluster.client(leader).put(&key, b"old").await,
        "the first write",
    )?;
    cluster
        .wait_for_revision(put.header.revision, Duration::from_millis(10_000))
        .await?;
    let before_blocked = cluster.stats.blocked.load(Ordering::Relaxed);
    cluster.isolate(cut).await;
    cut_unattributable_links(cluster).await;
    wait_for_the_cut_to_bite(cluster, before_blocked).await?;
    // The majority moves on while the cut-off member is not looking, so its local store is
    // now provably stale. A linearizable read answered out of it would be a stale read.
    ok(
        cluster.client(leader).put(&key, b"new").await,
        "a write on the majority side",
    )?;
    let started = Instant::now();
    let e = expect_error(
        cluster.client(cut).get_key(&key).await,
        &format!("a linearizable read on {name}, which can reach nobody"),
    )?;
    let took = started.elapsed();
    let described = cluster.describe().await;
    ctx.note(format!(
        "{name} refused the linearizable read after {} ms: {e} — {described}",
        took.as_millis()
    ));
    Ok(())
});

dist_test!(serializable_still_answers, |ctx| {
    let key = ctx.key("opt-out");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let cut = a_follower(leader, cluster.initial_size);
    let name = cluster.members[cut].name.clone();
    let put = ok(cluster.client(leader).put(&key, b"before").await, "a write")?;
    cluster
        .wait_for_revision(put.header.revision, Duration::from_millis(10_000))
        .await?;
    let before_blocked = cluster.stats.blocked.load(Ordering::Relaxed);
    cluster.isolate(cut).await;
    cut_unattributable_links(cluster).await;
    wait_for_the_cut_to_bite(cluster, before_blocked).await?;
    ok(
        cluster.client(leader).put(&key, b"after").await,
        "a write the isolated member cannot see",
    )?;
    let read = ok(
        cluster
            .client(cut)
            .range_req(&RangeRequest::key(&key).with("serializable", json!(true)))
            .await,
        &format!("a serializable read on {name}, which must still be answered"),
    )?;
    let got = read.one().map(|kv| kv.value_str());
    let described = cluster.describe().await;
    let mut c = Check::new("the read that opted out of the quorum");
    // Stale is allowed here, and on a cut-off member stale is what it will usually be.
    // Inventing a value is not allowed: the only honest answers are things really written.
    c.that(
        &format!("{name}.range.kvs[0].value (serializable)"),
        "either \"before\" or \"after\", both of which were really written",
        matches!(got.as_deref(), Some("before") | Some("after")),
        got.clone(),
    );
    c.note(format!("{name} answered {got:?} while cut off"))
        .note(described);
    c.finish()
});

dist_test!(the_isolated_leader_gives_up, |ctx| {
    let key = ctx.key("deposed");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let name = cluster.members[leader].name.clone();
    // Isolating the leader is the interesting case: it is the one member that could carry
    // on answering out of its own store and be confidently wrong about being in charge.
    let before_blocked = cluster.stats.blocked.load(Ordering::Relaxed);
    cluster.isolate(leader).await;
    cut_unattributable_links(cluster).await;
    wait_for_the_cut_to_bite(cluster, before_blocked).await?;
    let client = cluster.client(leader);
    let gave_up = wait_until(
        &format!("{name} to stop naming a leader"),
        GIVE_UP_WITHIN,
        || async { matches!(client.status().await, Ok(s) if s.leader == 0) },
    )
    .await;
    // Either outcome is correct, and which one arrives depends on how far through its
    // election timeout the old leader was. Going on serving is not one of them.
    let outcome = match gave_up {
        Ok(took) => format!(
            "{name} stopped naming a leader after {} ms",
            took.as_millis()
        ),
        Err(_) => {
            let e = expect_error(
                client.put(&key, b"from-the-old-leader").await,
                &format!("a write through {name}, the deposed leader"),
            )?;
            format!("{name} still named a leader, but refused a write with: {e}")
        }
    };
    let mut c = Check::new("a leader that has been cut off from its followers");
    // Whatever it says about leadership, the majority side must have moved on without it.
    let rest: Vec<usize> = (0..cluster.initial_size).filter(|i| *i != leader).collect();
    let new_leader = leader_of(cluster, &rest, ELECT_WITHIN).await?;
    c.ne("the majority's new leader", leader, new_leader);
    let put = ok(
        cluster
            .client(new_leader)
            .put(&key, b"from-the-new-leader")
            .await,
        "a write through the majority's new leader",
    )?;
    c.at_least(
        "the new leader's put.header.revision",
        1,
        put.header.revision,
    );
    let described = cluster.describe().await;
    c.note(outcome.clone()).note(described);
    c.finish()?;
    ctx.note(outcome);
    Ok(())
});

dist_test!(the_majority_keeps_writing, |ctx| {
    let key = ctx.key("majority");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let cut = a_follower(leader, size);
    cluster.isolate(cut).await;
    cut_unattributable_links(cluster).await;
    let rest: Vec<usize> = (0..size).filter(|i| *i != cut).collect();
    let still_leading = leader_of(cluster, &rest, ELECT_WITHIN).await?;
    let mut revisions = Vec::new();
    for (n, i) in rest.iter().enumerate() {
        let name = cluster.members[*i].name.clone();
        let put = ok(
            cluster
                .client(*i)
                .put(&key, format!("v{n}").as_bytes())
                .await,
            &format!("a write through {name}, on the majority side"),
        )?;
        revisions.push(put.header.revision);
    }
    let mut c = Check::new("the majority side of a partition");
    // Two of three is a quorum, so this side should not even notice.
    c.eq("majority(3)", 2, majority(3));
    c.eq("the members that can still reach each other", 2, rest.len());
    for w in revisions.windows(2) {
        c.that(
            "the revision on the majority side",
            "to keep advancing while the partition stands",
            w[1] > w[0],
            (w[0], w[1]),
        );
    }
    let read = ok(
        cluster.client(rest[0]).get_key(&key).await,
        "a read on the majority side",
    )?;
    c.eq(
        "majority.range.kvs[0].value",
        "v1".to_string(),
        read.one().map(|kv| kv.value_str()).unwrap_or_default(),
    );
    let described = cluster.describe().await;
    c.note(format!(
        "m{} was cut off; the majority's leader is m{}",
        cut + 1,
        still_leading + 1
    ))
    .note(described);
    c.finish()
});

dist_test!(still_answers_for_itself, |ctx| {
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let cut = a_follower(leader, cluster.initial_size);
    let name = cluster.members[cut].name.clone();
    let before = ok(
        cluster.client(cut).status().await,
        &format!("{name}'s status before the partition"),
    )?;
    let before_blocked = cluster.stats.blocked.load(Ordering::Relaxed);
    cluster.isolate(cut).await;
    cut_unattributable_links(cluster).await;
    wait_for_the_cut_to_bite(cluster, before_blocked).await?;
    // Losing quorum is not the same as being dead. A cut-off member must still answer the
    // questions it can answer on its own, so an operator can find out what is wrong;
    // refusing writes is not a licence to stop talking.
    let status = ok(
        cluster.client(cut).status().await,
        &format!("{name}'s status while it is cut off"),
    )?;
    let list = ok(
        cluster.client(cut).member_list().await,
        &format!("{name}'s member list while it is cut off"),
    )?;
    let mut c = Check::new("what a cut-off member can still answer");
    c.eq(
        &format!("{name}.status.header.member_id"),
        before.header.member_id,
        status.header.member_id,
    );
    c.eq(
        &format!("{name}.status.header.cluster_id"),
        before.header.cluster_id,
        status.header.cluster_id,
    );
    c.eq(
        &format!("{name}.member_list.members.len()"),
        cluster.initial_size,
        list.members.len(),
    );
    // Its own store cannot have moved on: it has heard from nobody since.
    c.at_most(
        &format!("{name}.status.header.revision"),
        before.header.revision,
        status.header.revision,
    );
    let described = cluster.describe().await;
    c.note(described);
    c.finish()
});

dist_test!(healing_restores_writes, |ctx| {
    let key = ctx.key("healed");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let cut = a_follower(leader, cluster.initial_size);
    let name = cluster.members[cut].name.clone();
    cluster.isolate(cut).await;
    cut_unattributable_links(cluster).await;
    // Refusing writes is a state, not a verdict: a member that rejoins a quorum serves
    // again, with nothing to restart and nobody to tell it.
    cluster.heal().await;
    let still_leading = cluster.wait_for_leader(ELECT_WITHIN).await?;
    // Naming a leader again is not the same as being connected to it: the peer transport
    // reconnects on its own schedule, and a write sent before it has would sit on the
    // server's request timeout. Waiting for the cut-off member to catch up to a revision
    // only the majority could have produced is the honest way to know the link is back.
    let bridge = ok(
        cluster.client(still_leading).put(&key, b"bridge").await,
        "a write on the majority side after the heal",
    )?;
    cluster
        .wait_for_revision(bridge.header.revision, Duration::from_millis(30_000))
        .await?;
    // The first attempt can still land on a peer connection the transport has not rebuilt
    // yet and sit on the server's request timeout, so the test asks a few times rather than
    // once. What it will not accept is a member that never starts serving again.
    let mut refusals = Vec::new();
    let mut accepted = None;
    for attempt in 0..3 {
        match cluster.client(cut).put(&key, b"accepted").await {
            Ok(r) => {
                accepted = Some(r);
                break;
            }
            Err(e) => refusals.push(format!("attempt {attempt}: {e}")),
        }
    }
    let put = match accepted {
        Some(r) => r,
        None => {
            let described = cluster.describe().await;
            return Err(Failure::new(
                FailureKind::Assertion,
                format!("{name} never accepted a write again after the partition healed"),
            )
            .note(refusals.join("; "))
            .note(described));
        }
    };
    cluster
        .wait_for_revision(put.header.revision, Duration::from_millis(15_000))
        .await?;
    let mut c = Check::new("a write through a member that has rejoined the cluster");
    for i in 0..cluster.initial_size {
        let other = cluster.members[i].name.clone();
        let read = ok(
            cluster.client(i).get_key(&key).await,
            &format!("a read on {other}"),
        )?;
        c.eq(
            &format!("{other}.range.kvs[0].value"),
            "accepted".to_string(),
            read.one().map(|kv| kv.value_str()).unwrap_or_default(),
        );
    }
    let described = cluster.describe().await;
    if !refusals.is_empty() {
        c.note(format!(
            "it took {} attempts: {}",
            refusals.len() + 1,
            refusals.join("; ")
        ));
    }
    c.note(described);
    c.finish()
});

dist_test!(a_minority_of_two_in_five, |ctx| {
    let key = ctx.key("five");
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT_WITHIN).await?;
    let minority = vec![0usize, 1];
    let majority_side = vec![2usize, 3, 4];
    // Two members that can still see each other but not the other three. A minority does
    // not become a quorum by having company.
    let before_blocked = cluster.stats.blocked.load(Ordering::Relaxed);
    cluster
        .partition(&[minority.clone(), majority_side.clone()])
        .await;
    cut_unattributable_links(cluster).await;
    wait_for_the_cut_to_bite(cluster, before_blocked).await?;
    let new_leader = leader_of(cluster, &majority_side, ELECT_WITHIN).await?;
    let started = Instant::now();
    let e = expect_error(
        cluster.client(0).put(&key, b"two-of-five").await,
        "a write through m1, one of the two cut-off members",
    )?;
    let took = started.elapsed();
    let put = ok(
        cluster.client(new_leader).put(&key, b"three-of-five").await,
        "a write on the majority side of five",
    )?;
    let mut c = Check::new("a five-member cluster split two against three");
    c.eq("majority(5)", 3, majority(5));
    c.at_least(
        "the majority side's put.header.revision",
        1,
        put.header.revision,
    );
    let read = ok(
        cluster.client(majority_side[1]).get_key(&key).await,
        "a read on the majority side",
    )?;
    c.eq(
        "majority.range.kvs[0].value",
        "three-of-five".to_string(),
        read.one().map(|kv| kv.value_str()).unwrap_or_default(),
    );
    let described = cluster.describe().await;
    c.note(format!(
        "m1 refused the write after {} ms with: {e}",
        took.as_millis()
    ))
    .note(described);
    c.finish()
});
