//! Stage 41 — The majority of a partition keeps serving.
//!
//! Stage 40 was about the side that must stop. This one is about the side that must not.
//! A quorum is a *majority*, not everybody: two members of three, or three of five, are
//! enough to commit an entry, so a cluster that stops writing the moment one member goes
//! quiet has thrown away the only thing replication bought it.
//!
//! Two failures live here and they pull in opposite directions. The timid one is a cluster
//! that waits for every member before committing, which turns one unreachable member into
//! an outage. The careless one is the majority forgetting that the cut-off member exists at
//! all — reusing its slot, or counting two of three as unanimous. The tests below hold both
//! ends: the majority keeps electing, committing and reading, while the member on the other
//! side of the cut learns nothing at all.
//!
//! Every test cuts the network, so every one is `.fresh()` with a generous floor under its
//! timeout. Where a test needs the cut to be provably in effect before it writes, it waits
//! for the proxies' `blocked` counter to move rather than sleeping: that counter rising is
//! the fault injector saying the link really is down.

use crate::assert::{Check, Failure, FailureKind};
use crate::cluster::proxy::LinkFault;
use crate::cluster::Cluster;
use crate::dist_test;
use crate::etcd::{b64, RangeRequest};
use crate::examples::{cluster_example, step, ExampleSpec};
use crate::stages::{check_strictly_increasing, majority, ok, wait_until, Ladder, Stage, Test};
use serde_json::json;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// How long the cluster is given to settle on one leader.
const ELECT_WITHIN: Duration = Duration::from_millis(15_000);
/// The floor under every test in this stage.
const SLOW: u64 = 60_000;

/// Stage 41.
pub fn stage() -> Stage {
    Stage {
        number: 41,
        slug: "majority_keeps_serving",
        name: "The majority of a partition keeps serving",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "A quorum of members is enough: the cluster does not need everyone",
            "The majority side elects a leader among itself if the old one was cut off",
            "Writes keep succeeding on the majority side while the partition stands",
            "The revision keeps advancing, and the minority side knows nothing about it",
        ],
        examples,
        tests: vec![
            Test::new(
                "the majority accepts writes with one member cut off",
                the_majority_accepts_writes,
            )
            .ext()
            .fresh()
            .min_timeout_ms(SLOW),
            Test::new(
                "the majority elects a leader of its own when the old one is cut off",
                the_majority_elects_its_own,
            )
            .ext()
            .fresh()
            .min_timeout_ms(SLOW),
            Test::new(
                "the revision keeps advancing on the majority side",
                the_revision_keeps_advancing,
            )
            .ext()
            .fresh()
            .min_timeout_ms(SLOW),
            Test::new(
                "reads on the majority side see every write",
                reads_see_every_write,
            )
            .ext()
            .fresh()
            .min_timeout_ms(SLOW),
            Test::new(
                "a write on the majority side is invisible to the cut-off member",
                invisible_to_the_minority,
            )
            .ext()
            .fresh()
            .min_timeout_ms(SLOW),
            Test::new(
                "both sides of the split name the same leader on the majority side",
                the_majority_agrees_on_its_leader,
            )
            .ext()
            .fresh()
            .min_timeout_ms(SLOW),
            Test::new(
                "the majority survives the whole partition with no restart",
                no_restart_needed,
            )
            .ext()
            .fresh()
            .min_timeout_ms(SLOW),
            Test::new(
                "three of five keep serving with two cut off",
                three_of_five_keep_serving,
            )
            .ext()
            .cluster(5)
            .min_timeout_ms(90_000),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![cluster_example("Two members are enough", || {
        vec![
            step(
                1,
                "/v3/kv/put",
                json!({"key": b64(b"still-serving"), "value": b64(b"yes")}),
                "write through m2",
            ),
            step(
                2,
                "/v3/kv/range",
                json!({"key": b64(b"still-serving")}),
                "read it on m3",
            ),
            step(
                2,
                "/v3/maintenance/status",
                json!({}),
                "and ask m3 how far the log has got",
            ),
        ]
    })
    .request("a write and a read between two members of a three-member cluster")
    .response("both succeed, and the revision has moved")
    .note(
        "Picture m1 cut off before this exchange. Nothing above changes: m2 and m3 are two \
         of three, which is a quorum, so the entry commits and the revision advances \
         exactly as it does here. What a captured transcript cannot show is the other \
         half of the property — m1 is still running, still answering `serializable` reads, \
         and still holding the value from before the cut, because nothing has reached it \
         since.",
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

/// The lowest-numbered member that is not the leader.
fn a_follower(leader: usize, size: usize) -> usize {
    (0..size).find(|i| *i != leader).unwrap_or(0)
}

dist_test!(the_majority_accepts_writes, |ctx| {
    let key = ctx.key("serving");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let cut = a_follower(leader, size);
    cluster.isolate(cut).await;
    cut_unattributable_links(cluster).await;
    let rest: Vec<usize> = (0..size).filter(|i| *i != cut).collect();
    leader_of(cluster, &rest, ELECT_WITHIN).await?;
    let mut c = Check::new("writes through each member of the majority side");
    for (n, i) in rest.iter().enumerate() {
        let name = cluster.members[*i].name.clone();
        let value = format!("v{n}");
        ok(
            cluster.client(*i).put(&key, value.as_bytes()).await,
            &format!("a write through {name} with one member cut off"),
        )?;
        // Read it back on the *other* member of the pair: a quorum of two has to agree,
        // not just the member that took the request.
        let other = rest[(n + 1) % rest.len()];
        let other_name = cluster.members[other].name.clone();
        let read = ok(
            cluster.client(other).get_key(&key).await,
            &format!("a read on {other_name}"),
        )?;
        c.eq(
            &format!("{other_name}.range.kvs[0].value after a write on {name}"),
            value,
            read.one().map(|kv| kv.value_str()).unwrap_or_default(),
        );
    }
    c.eq("the quorum of a three-member cluster", 2, majority(3));
    let described = cluster.describe().await;
    c.note(format!("m{} is cut off", cut + 1)).note(described);
    c.finish()
});

dist_test!(the_majority_elects_its_own, |ctx| {
    let key = ctx.key("new-leader");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let old_term = ok(
        cluster.client(leader).status().await,
        "the old leader's status",
    )?
    .raft_term;
    let size = cluster.initial_size;
    let rest: Vec<usize> = (0..size).filter(|i| *i != leader).collect();
    let started = Instant::now();
    cluster.isolate(leader).await;
    cut_unattributable_links(cluster).await;
    // The two members left have never been leader and have to work it out between them.
    let new_leader = leader_of(cluster, &rest, ELECT_WITHIN).await?;
    let took = started.elapsed();
    let status = ok(
        cluster.client(new_leader).status().await,
        "the new leader's status",
    )?;
    let mut c = Check::new("the leader the majority side chose for itself");
    c.ne("the new leader", leader, new_leader);
    c.that(
        "the new leader",
        "one of the two members still able to reach each other",
        rest.contains(&new_leader),
        new_leader + 1,
    );
    // A new leader means a new term, and a strictly larger one: that is what stops the old
    // leader's entries and the new one's from being confused for each other.
    c.that(
        "the term after the election",
        "a strictly larger raft term than the cut-off leader's",
        status.raft_term > old_term,
        (old_term, status.raft_term),
    );
    let put = ok(
        cluster.client(new_leader).put(&key, b"elected").await,
        "a write through the newly elected leader",
    )?;
    c.at_least("put.header.revision", 1, put.header.revision);
    let described = cluster.describe().await;
    c.note(format!(
        "m{} was cut off; m{} took over after {} ms",
        leader + 1,
        new_leader + 1,
        took.as_millis()
    ))
    .note(described);
    c.finish()?;
    ctx.note(format!(
        "the majority replaced the cut-off leader m{} with m{} after {} ms, term {old_term} to {}",
        leader + 1,
        new_leader + 1,
        took.as_millis(),
        status.raft_term
    ));
    Ok(())
});

dist_test!(the_revision_keeps_advancing, |ctx| {
    let prefix = ctx.key("advancing");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let cut = a_follower(leader, size);
    cluster.isolate(cut).await;
    cut_unattributable_links(cluster).await;
    let rest: Vec<usize> = (0..size).filter(|i| *i != cut).collect();
    let serving = leader_of(cluster, &rest, ELECT_WITHIN).await?;
    let mut revisions = Vec::new();
    for n in 0..10 {
        let key = [prefix.as_slice(), format!("/{n:02}").as_bytes()].concat();
        let through = rest[n % rest.len()];
        let put = ok(
            cluster
                .client(through)
                .put(&key, format!("v{n}").as_bytes())
                .await,
            "a write on the majority side",
        )?;
        revisions.push(put.header.revision);
    }
    let mut c = Check::new("ten writes committed by two members of three");
    check_strictly_increasing(&mut c, "put.header.revision", &revisions);
    // Both members of the quorum must have the whole run, not just the one that led it.
    for i in &rest {
        let name = cluster.members[*i].name.clone();
        let read = ok(
            cluster
                .client(*i)
                .range_req(&RangeRequest::prefix(&prefix))
                .await,
            &format!("a prefix read on {name}"),
        )?;
        c.eq(&format!("{name}.range.count"), 10, read.count);
    }
    let described = cluster.describe().await;
    c.note(format!(
        "m{} is cut off; the majority's leader is m{}",
        cut + 1,
        serving + 1
    ))
    .note(described);
    c.finish()
});

dist_test!(reads_see_every_write, |ctx| {
    let prefix = ctx.key("readback");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let cut = a_follower(leader, size);
    cluster.isolate(cut).await;
    cut_unattributable_links(cluster).await;
    let rest: Vec<usize> = (0..size).filter(|i| *i != cut).collect();
    leader_of(cluster, &rest, ELECT_WITHIN).await?;
    let mut c = Check::new("a read-after-write loop on the majority side");
    for n in 0..8 {
        let key = [prefix.as_slice(), format!("/{n:02}").as_bytes()].concat();
        let value = format!("v{n}");
        let writer = rest[n % rest.len()];
        let reader = rest[(n + 1) % rest.len()];
        ok(
            cluster.client(writer).put(&key, value.as_bytes()).await,
            "a write on the majority side",
        )?;
        // No wait: a partition does not make a linearizable read any less linearizable on
        // the side that still has a quorum.
        let read = ok(
            cluster.client(reader).get_key(&key).await,
            "a read on the majority side",
        )?;
        c.eq(
            &format!("m{}.range({n:02}).kvs[0].value", reader + 1),
            value,
            read.one().map(|kv| kv.value_str()).unwrap_or_default(),
        );
    }
    let described = cluster.describe().await;
    c.note(described);
    c.finish()
});

dist_test!(invisible_to_the_minority, |ctx| {
    let key = ctx.key("unseen");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let cut = a_follower(leader, size);
    let name = cluster.members[cut].name.clone();
    let before_blocked = cluster.stats.blocked.load(Ordering::Relaxed);
    cluster.isolate(cut).await;
    cut_unattributable_links(cluster).await;
    // Wait for the injector to have refused something before writing, so this test is
    // about replication and not about a race with the fault it just asked for.
    wait_for_the_cut_to_bite(cluster, before_blocked).await?;
    let before = ok(
        cluster.client(cut).status().await,
        &format!("{name}'s revision while cut off"),
    )?
    .header
    .revision;
    let rest: Vec<usize> = (0..size).filter(|i| *i != cut).collect();
    let serving = leader_of(cluster, &rest, ELECT_WITHIN).await?;
    let put = ok(
        cluster.client(serving).put(&key, b"majority-only").await,
        "a write the cut-off member must never learn about",
    )?;
    let read = ok(
        cluster
            .client(cut)
            .range_req(&RangeRequest::key(&key).with("serializable", json!(true)))
            .await,
        &format!("a serializable read on {name}"),
    )?;
    let after = ok(
        cluster.client(cut).status().await,
        &format!("{name}'s revision after the majority wrote"),
    )?
    .header
    .revision;
    let mut c = Check::new("what the cut-off member knows about the majority's writes");
    c.eq(&format!("{name}.range.count (serializable)"), 0, read.count);
    c.eq(&format!("{name}.status.header.revision"), before, after);
    c.that(
        "the majority's revision against the cut-off member's",
        "the majority to be strictly ahead",
        put.header.revision > after,
        (put.header.revision, after),
    );
    let described = cluster.describe().await;
    c.note(described);
    c.finish()
});

dist_test!(the_majority_agrees_on_its_leader, |ctx| {
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let cut = a_follower(leader, size);
    cluster.isolate(cut).await;
    cut_unattributable_links(cluster).await;
    let rest: Vec<usize> = (0..size).filter(|i| *i != cut).collect();
    let serving = leader_of(cluster, &rest, ELECT_WITHIN).await?;
    let mut statuses = Vec::new();
    for i in &rest {
        let name = cluster.members[*i].name.clone();
        let s = ok(
            cluster.client(*i).status().await,
            &format!("{name}'s status"),
        )?;
        statuses.push((name, s.leader, s.raft_term));
    }
    let mut c = Check::new("what the two members of the quorum say about each other");
    let leader_id = cluster.members[serving].member_id;
    for (name, named, term) in &statuses {
        c.eq(&format!("{name}.status.leader"), leader_id, *named);
        c.eq(&format!("{name}.status.raft_term"), statuses[0].2, *term);
    }
    let described = cluster.describe().await;
    c.note(format!("m{} is cut off", cut + 1)).note(described);
    c.finish()
});

dist_test!(no_restart_needed, |ctx| {
    let prefix = ctx.key("endurance");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let cut = a_follower(leader, size);
    cluster.isolate(cut).await;
    cut_unattributable_links(cluster).await;
    let rest: Vec<usize> = (0..size).filter(|i| *i != cut).collect();
    let serving = leader_of(cluster, &rest, ELECT_WITHIN).await?;
    // Six rounds spread over a couple of seconds, which is several election timeouts: a
    // majority that quietly re-elects every time the cut-off member fails to answer would
    // show up here as a changing leader.
    let mut leaders = Vec::new();
    let mut revisions = Vec::new();
    for round in 0..6 {
        let key = [prefix.as_slice(), format!("/{round}").as_bytes()].concat();
        let through = rest[round % rest.len()];
        let put = ok(
            cluster.client(through).put(&key, b"still here").await,
            "a write during a long partition",
        )?;
        revisions.push(put.header.revision);
        leaders.push(cluster.leader_according_to(rest[0]).await);
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    let running = cluster.running();
    let mut c = Check::new("a majority that was never restarted");
    check_strictly_increasing(&mut c, "put.header.revision", &revisions);
    c.that(
        "the leader over six rounds",
        "the same leader throughout, with no election on the majority side",
        leaders.iter().all(|l| *l == Some(serving)),
        leaders.iter().map(|l| l.map(|x| x + 1)).collect::<Vec<_>>(),
    );
    // Nobody was killed and nobody was started: all three processes are still the ones the
    // cluster began with, the cut-off member included.
    c.eq("the members still running", size, running.len());
    let described = cluster.describe().await;
    c.note(format!(
        "m{} was cut off for the whole test; m{} led throughout",
        cut + 1,
        serving + 1
    ))
    .note(described);
    c.finish()
});

dist_test!(three_of_five_keep_serving, |ctx| {
    let prefix = ctx.key("five");
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
    let mut revisions = Vec::new();
    for n in 0..6 {
        let key = [prefix.as_slice(), format!("/{n}").as_bytes()].concat();
        let through = majority_side[n % majority_side.len()];
        let put = ok(
            cluster
                .client(through)
                .put(&key, format!("v{n}").as_bytes())
                .await,
            "a write on the three-member side of a five-member split",
        )?;
        revisions.push(put.header.revision);
    }
    let mut c = Check::new("three of five, carrying on without the other two");
    c.eq("the quorum of a five-member cluster", 3, majority(5));
    check_strictly_increasing(&mut c, "put.header.revision", &revisions);
    for i in &majority_side {
        let name = cluster.members[*i].name.clone();
        let read = ok(
            cluster
                .client(*i)
                .range_req(&RangeRequest::prefix(&prefix))
                .await,
            &format!("a prefix read on {name}"),
        )?;
        c.eq(&format!("{name}.range.count"), 6, read.count);
    }
    // And the two on the other side have learnt nothing, however many of them there are.
    for i in &minority {
        let name = cluster.members[*i].name.clone();
        let read = ok(
            cluster
                .client(*i)
                .range_req(&RangeRequest::prefix(&prefix).with("serializable", json!(true)))
                .await,
            &format!("a serializable prefix read on {name}"),
        )?;
        c.eq(&format!("{name}.range.count (serializable)"), 0, read.count);
    }
    let described = cluster.describe().await;
    c.note(format!("the majority side is led by m{}", serving + 1))
        .note(described);
    c.finish()
});
