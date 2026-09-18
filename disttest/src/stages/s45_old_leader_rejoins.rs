//! Stage 45 — an old leader rejoins without resurrecting anything.
//!
//! A leader that is cut off does not know it has been replaced. It may still hold entries it
//! appended locally and never got committed, and it still believes its own term is current.
//! When the link comes back, the direction of the correction is the whole point: the old
//! leader learns the higher term, steps down, throws away whatever the majority never
//! accepted, and takes the majority's values. It never pushes its own back.
//!
//! The trap is that both sides look alive the whole time. The isolated member answers status
//! requests, holds a key/value store, and will happily serve reads from it if reads are
//! allowed to be served locally — which is why a linearizable read has to reach a quorum, and
//! why a write on a member that has lost its quorum has to be refused rather than queued.
//! A test that only checks the majority's side would pass against an implementation that
//! quietly resurrects the old leader's writes on the heal.

use crate::assert::{Check, Failure, FailureKind};
use crate::cluster::Cluster;
use crate::dist_test;
use crate::etcd::Client;
use crate::examples::{cluster_example, step, ExampleSpec};
use crate::stages::{expect_error, ok, Ladder, Stage, Test};
use serde_json::json;
use std::time::{Duration, Instant};

/// Stage 45.
pub fn stage() -> Stage {
    Stage {
        number: 45,
        slug: "old_leader_rejoins",
        name: "An old leader rejoins without resurrecting anything",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "A leader that was cut off may hold entries no quorum ever accepted",
            "On rejoining it sees a higher term, steps down, and truncates those entries",
            "A client that read from the new majority must never see the old entries appear",
            "Its own view of the key must become the cluster's, not the other way round",
        ],
        examples,
        tests: vec![
            Test::new(
                "the old leader's view of a key becomes the cluster's",
                the_old_leader_adopts_the_majority,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "a write on the isolated old leader is refused",
                the_isolated_leader_refuses_writes,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "nothing the isolated side was told to write appears after the heal",
                nothing_from_the_isolated_side_appears,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "a value read from the new majority never turns back into the old one",
                the_majority_value_never_reverts,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "the term the old leader ends in is at least the new one",
                the_old_leader_adopts_the_new_term,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "the old leader's revision catches up with the majority's",
                the_old_leader_catches_up,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "isolating and healing the same member twice works",
                twice_in_a_row,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
            Test::new(
                "the cluster names exactly one leader once the link is back",
                one_leader_after_the_heal,
            )
            .ext()
            .fresh()
            .min_timeout_ms(60_000),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        cluster_example("Who leads, and what the term is, on each member", || {
            vec![
                step(
                    0,
                    "/v3/kv/put",
                    json!({"key": "ZXg0NS9r", "value": "bWFqb3JpdHk="}),
                    "the majority's value for ex45/k",
                ),
                step(
                    0,
                    "/v3/maintenance/status",
                    json!({}),
                    "m1's leader and term",
                ),
                step(
                    1,
                    "/v3/maintenance/status",
                    json!({}),
                    "m2's leader and term",
                ),
                step(
                    2,
                    "/v3/kv/range",
                    json!({"key": "ZXg0NS9r"}),
                    "and the value m3 answers with",
                ),
            ]
        })
        .request("a write, then the term and leader every member reports")
        .response("one leader id, one term, and the majority's value everywhere")
        .note(
            "An example cannot cut a link, so this is the healthy shape the stage compares \
         against. The stage isolates whichever member this status call names as leader, lets \
         the other two elect a successor in a higher term, and then heals. What must be true \
         afterwards is that the isolated member reports the successor's id and the successor's \
         term here, and the successor's value there — never the term or the value it was \
         holding while it was alone.",
        ),
    ]
}

/// Wait until every member in `among` names the same leader, ignoring everyone else.
///
/// [`Cluster::wait_for_leader`] insists that every *running* member agrees, which can never
/// happen while one of them is cut off: the isolated member names either the leader it last
/// heard from or nobody at all. A partition test has to ask only the side that still holds a
/// quorum, and has to insist the leader it names is on that side.
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
/// [`Cluster::wait_for_leader`] is satisfied as soon as the running members agree, and they
/// agree on a member that has just stopped leading for as long as it takes them to miss a
/// heartbeat. Insisting that the leader named is itself still running is what makes the
/// answer usable.
async fn live_leader(cluster: &mut Cluster, within: Duration) -> Result<usize, Failure> {
    let alive = cluster.running();
    leader_among(cluster, &alive, within).await
}

/// A second client for one member, with a deadline far shorter than the member's own.
///
/// A member that has lost its quorum does not say no. It takes the request, tries to get it
/// committed, and gives up only when its own commit timeout expires seven seconds later.
/// Waiting that out on every attempt would hold the network cut for half a minute, and a cut
/// that long is not what this stage is about, so the write is given a deadline of its own and
/// the deadline is read as the refusal it is: what matters is that the write was never
/// acknowledged, and that it is not there afterwards either.
fn impatient_client(cluster: &Cluster, member: usize) -> Result<Client, Failure> {
    let url = cluster.client(member).url.clone();
    let name = cluster.members[member].name.clone();
    Client::new(&url, &name, Duration::from_millis(1_200))
        .map_err(|e| Failure::harness(format!("cannot build a short-deadline client: {e}")))
}

dist_test!(the_old_leader_adopts_the_majority, |ctx| {
    let key = ctx.key("k");
    let cluster = ctx.cluster()?;
    let old = live_leader(cluster, Duration::from_millis(20_000)).await?;
    ok(
        cluster.client(old).put(&key, b"before").await,
        "the write every member saw",
    )?;
    let majority: Vec<usize> = (0..cluster.initial_size).filter(|i| *i != old).collect();
    let everyone: Vec<usize> = (0..cluster.initial_size).collect();
    cluster.isolate(old).await;
    let fresh = leader_among(cluster, &majority, Duration::from_millis(25_000)).await?;
    let put = ok(
        cluster.client(fresh).put(&key, b"after").await,
        "the majority's write while the old leader was away",
    )?;
    cluster.heal().await;
    leader_among(cluster, &everyone, Duration::from_millis(30_000)).await?;
    cluster
        .wait_for_revision(put.header.revision, Duration::from_millis(30_000))
        .await?;
    let seen = ok(
        cluster.client(old).value_of(&key).await,
        "reading the key from the member that used to lead",
    )?;
    let names = cluster.leader_according_to(old).await;
    let described = cluster.describe().await;
    let mut c = Check::new("the old leader's view of the key once the link is back");
    c.note(format!(
        "m{} was isolated, m{} was elected in its place",
        old + 1,
        fresh + 1
    ));
    c.note(described);
    c.eq("the old leader's value", Some(b"after".to_vec()), seen);
    c.eq("the leader the old leader names", Some(fresh), names);
    c.finish()
});

dist_test!(the_isolated_leader_refuses_writes, |ctx| {
    let key = ctx.key("refused");
    let marker = ctx.key("marker");
    let cluster = ctx.cluster()?;
    let old = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let majority: Vec<usize> = (0..cluster.initial_size).filter(|i| *i != old).collect();
    let everyone: Vec<usize> = (0..cluster.initial_size).collect();
    let impatient = impatient_client(cluster, old)?;
    cluster.isolate(old).await;
    let fresh = leader_among(cluster, &majority, Duration::from_millis(25_000)).await?;
    // A member alone on its side of the cut has no quorum, so it can neither commit anything
    // nor promise anything. Answering this put at all would be the bug the whole stage is
    // about, and the heal afterwards is what proves the answer was a refusal rather than a
    // slow acknowledgement: a write that really was refused is not there later either.
    let refused = expect_error(
        impatient.put(&key, b"cannot be accepted").await,
        "a write on the isolated old leader",
    )?;
    let code = refused.code();
    let put = ok(
        cluster
            .client(fresh)
            .put(&marker, b"the majority carries on")
            .await,
        "a write on the majority side",
    )?;
    cluster.heal().await;
    leader_among(cluster, &everyone, Duration::from_millis(30_000)).await?;
    cluster
        .wait_for_revision(put.header.revision, Duration::from_millis(30_000))
        .await?;
    let mut present = Vec::new();
    for i in 0..cluster.initial_size {
        let name = cluster.members[i].name.clone();
        match cluster.client(i).value_of(&key).await {
            Ok(None) => {}
            other => present.push(format!("{name} -> {other:?}")),
        }
    }
    let described = cluster.describe().await;
    let faults = cluster.faults.describe();
    let mut c = Check::new("a write sent to a member with no quorum");
    c.note(format!("m{} is the isolated old leader", old + 1));
    c.note(described).note(faults);
    c.block("what the isolated member answered", refused.to_string());
    c.that(
        "error.code",
        "14 (unavailable) or 4 (deadline exceeded), or no code at all when the request simply \
         never came back",
        code.is_none() || code == Some(14) || code == Some(4),
        code,
    );
    c.that(
        "the refused key after the heal",
        "absent on every member",
        present.is_empty(),
        present,
    );
    c.finish()
});

dist_test!(nothing_from_the_isolated_side_appears, |ctx| {
    let ghost = ctx.key("ghost");
    let real = ctx.key("real");
    let cluster = ctx.cluster()?;
    let old = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let majority: Vec<usize> = (0..cluster.initial_size).filter(|i| *i != old).collect();
    let everyone: Vec<usize> = (0..cluster.initial_size).collect();
    let impatient = impatient_client(cluster, old)?;
    cluster.isolate(old).await;
    let fresh = leader_among(cluster, &majority, Duration::from_millis(25_000)).await?;
    // Two attempts at writing on the wrong side of the cut, made without waiting for the old
    // leader to notice it has been replaced. That is deliberate: a member that still believes
    // it leads will append these entries to its own log before discovering that no quorum
    // will ever accept them, and truncating them on the heal is the thing this stage is
    // named after. Neither write may be answered, and neither may turn up afterwards.
    let mut answered = Vec::new();
    for i in 0..2 {
        if impatient
            .put(&ghost, format!("ghost-{i}").as_bytes())
            .await
            .is_ok()
        {
            answered.push(i);
        }
    }
    let put = ok(
        cluster
            .client(fresh)
            .put(&real, b"the majority's own write")
            .await,
        "a write on the majority side",
    )?;
    cluster.heal().await;
    leader_among(cluster, &everyone, Duration::from_millis(30_000)).await?;
    cluster
        .wait_for_revision(put.header.revision, Duration::from_millis(30_000))
        .await?;
    let mut present = Vec::new();
    let mut missing_real = Vec::new();
    for i in 0..cluster.initial_size {
        let name = cluster.members[i].name.clone();
        match cluster.client(i).value_of(&ghost).await {
            Ok(None) => {}
            other => present.push(format!("{name} -> {other:?}")),
        }
        match cluster.client(i).value_of(&real).await {
            Ok(Some(v)) if v == b"the majority's own write" => {}
            other => missing_real.push(format!("{name} -> {other:?}")),
        }
    }
    let described = cluster.describe().await;
    let mut c = Check::new("the key the isolated member was asked to write");
    c.note(format!(
        "m{} was isolated, m{} was elected in its place",
        old + 1,
        fresh + 1
    ));
    c.note(described);
    c.eq(
        "writes the isolated member acknowledged",
        Vec::new(),
        answered,
    );
    c.that(
        "the ghost key after the heal",
        "absent on every member",
        present.is_empty(),
        present,
    );
    c.that(
        "the majority's own key after the heal",
        "present on every member",
        missing_real.is_empty(),
        missing_real,
    );
    c.finish()
});

dist_test!(the_majority_value_never_reverts, |ctx| {
    let key = ctx.key("stable");
    let cluster = ctx.cluster()?;
    let old = live_leader(cluster, Duration::from_millis(20_000)).await?;
    ok(
        cluster.client(old).put(&key, b"old").await,
        "the value the old leader knows",
    )?;
    let majority: Vec<usize> = (0..cluster.initial_size).filter(|i| *i != old).collect();
    let everyone: Vec<usize> = (0..cluster.initial_size).collect();
    cluster.isolate(old).await;
    let fresh = leader_among(cluster, &majority, Duration::from_millis(25_000)).await?;
    let put = ok(
        cluster.client(fresh).put(&key, b"new").await,
        "the value the new majority wrote",
    )?;
    let read_before_the_heal = ok(
        cluster.client(fresh).value_of(&key).await,
        "the value a client read from the new majority",
    )?;
    cluster.heal().await;
    leader_among(cluster, &everyone, Duration::from_millis(30_000)).await?;
    cluster
        .wait_for_revision(put.header.revision, Duration::from_millis(30_000))
        .await?;
    // A client that has already been shown "new" may never be shown "old" again, on any
    // member, at any point after the heal. Errors are allowed while a member is catching up;
    // an older value is not.
    let mut wrong = Vec::new();
    for _ in 0..8 {
        for i in 0..cluster.initial_size {
            let name = cluster.members[i].name.clone();
            if let Ok(Some(v)) = cluster.client(i).value_of(&key).await {
                if v != b"new" {
                    wrong.push(format!("{name} -> {}", String::from_utf8_lossy(&v)));
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let described = cluster.describe().await;
    let mut c = Check::new("the value every member serves after the old leader came back");
    c.note(format!(
        "m{} was isolated, m{} was elected in its place",
        old + 1,
        fresh + 1
    ));
    c.note(described);
    c.eq(
        "what the client read from the new majority",
        Some(b"new".to_vec()),
        read_before_the_heal,
    );
    c.that(
        "the value over eight rounds of reading every member",
        "never anything but the majority's",
        wrong.is_empty(),
        wrong,
    );
    c.finish()
});

dist_test!(the_old_leader_adopts_the_new_term, |ctx| {
    let key = ctx.key("term");
    let cluster = ctx.cluster()?;
    let old = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let before = ok(
        cluster.client(old).status().await,
        "the old leader's status",
    )?;
    let majority: Vec<usize> = (0..cluster.initial_size).filter(|i| *i != old).collect();
    let everyone: Vec<usize> = (0..cluster.initial_size).collect();
    cluster.isolate(old).await;
    let fresh = leader_among(cluster, &majority, Duration::from_millis(25_000)).await?;
    let put = ok(
        cluster.client(fresh).put(&key, b"in the new term").await,
        "a write in the new term",
    )?;
    let new_term = ok(
        cluster.client(fresh).status().await,
        "the new leader's status",
    )?
    .raft_term;
    cluster.heal().await;
    leader_among(cluster, &everyone, Duration::from_millis(30_000)).await?;
    cluster
        .wait_for_revision(put.header.revision, Duration::from_millis(30_000))
        .await?;
    let after = ok(
        cluster.client(old).status().await,
        "the old leader's status after the heal",
    )?;
    let described = cluster.describe().await;
    let mut c = Check::new("the term the old leader is in once it has rejoined");
    c.note(format!(
        "m{} led in term {}, m{} was elected in term {new_term}",
        old + 1,
        before.raft_term,
        fresh + 1
    ));
    c.note(described);
    c.at_least("the new leader's term", before.raft_term + 1, new_term);
    c.at_least(
        "the old leader's term after the heal",
        new_term,
        after.raft_term,
    );
    c.ne(
        "the old leader still calls itself leader",
        after.header.member_id,
        after.leader,
    );
    c.finish()
});

dist_test!(the_old_leader_catches_up, |ctx| {
    let prefix = ctx.prefix();
    let cluster = ctx.cluster()?;
    let old = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let majority: Vec<usize> = (0..cluster.initial_size).filter(|i| *i != old).collect();
    let everyone: Vec<usize> = (0..cluster.initial_size).collect();
    cluster.isolate(old).await;
    let fresh = leader_among(cluster, &majority, Duration::from_millis(25_000)).await?;
    let mut last = 0;
    for i in 0..40 {
        let key = format!("{prefix}/c{i:03}").into_bytes();
        last = ok(
            cluster
                .client(fresh)
                .put(&key, format!("v{i}").as_bytes())
                .await,
            "a write the isolated member cannot see",
        )?
        .header
        .revision;
    }
    let behind = ok(
        cluster.client(old).status().await,
        "the isolated member's status",
    )?
    .header
    .revision;
    cluster.heal().await;
    leader_among(cluster, &everyone, Duration::from_millis(30_000)).await?;
    cluster
        .wait_for_revision(last, Duration::from_millis(30_000))
        .await?;
    let mut missing = Vec::new();
    for i in 0..40 {
        let key = format!("{prefix}/c{i:03}").into_bytes();
        let want = format!("v{i}");
        match cluster.client(old).value_of(&key).await {
            Ok(Some(v)) if v == want.as_bytes() => {}
            other => missing.push(format!("{} -> {other:?}", String::from_utf8_lossy(&key))),
        }
    }
    let described = cluster.describe().await;
    let mut c = Check::new("the forty writes the old leader slept through");
    c.note(format!(
        "m{} was isolated at revision {behind}, the majority reached {last}",
        old + 1
    ));
    c.note(described);
    c.at_most(
        "the isolated member's revision while it was away",
        last - 1,
        behind,
    );
    c.that(
        "the keys written while it was away",
        "all forty present and correct on the old leader",
        missing.is_empty(),
        missing,
    );
    c.finish()
});

dist_test!(twice_in_a_row, |ctx| {
    let key = ctx.key("twice");
    let cluster = ctx.cluster()?;
    let old = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let majority: Vec<usize> = (0..cluster.initial_size).filter(|i| *i != old).collect();
    let everyone: Vec<usize> = (0..cluster.initial_size).collect();
    // The same member loses and regains contact twice. Nothing in the recovery path may
    // depend on it being the first time: the second heal has to converge just like the first.
    let mut values = Vec::new();
    for round in 0..2 {
        cluster.isolate(old).await;
        let fresh = leader_among(cluster, &majority, Duration::from_millis(25_000)).await?;
        let value = format!("round-{round}");
        let put = ok(
            cluster.client(fresh).put(&key, value.as_bytes()).await,
            "a write on the majority side",
        )?;
        cluster.heal().await;
        leader_among(cluster, &everyone, Duration::from_millis(30_000)).await?;
        cluster
            .wait_for_revision(put.header.revision, Duration::from_millis(30_000))
            .await?;
        let seen = ok(
            cluster.client(old).value_of(&key).await,
            "reading the key from the member that keeps being isolated",
        )?;
        values.push((value, seen));
    }
    let described = cluster.describe().await;
    let mut c = Check::new("two isolations and two heals of the same member");
    c.note(format!("m{} was the member cut off twice", old + 1));
    c.note(described);
    for (want, got) in &values {
        c.eq(
            &format!("what m{} answers after the {want} heal", old + 1),
            Some(want.clone().into_bytes()),
            got.clone(),
        );
    }
    c.finish()
});

dist_test!(one_leader_after_the_heal, |ctx| {
    let cluster = ctx.cluster()?;
    let old = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let majority: Vec<usize> = (0..cluster.initial_size).filter(|i| *i != old).collect();
    let everyone: Vec<usize> = (0..cluster.initial_size).collect();
    cluster.isolate(old).await;
    let fresh = leader_among(cluster, &majority, Duration::from_millis(25_000)).await?;
    cluster.heal().await;
    let settled = leader_among(cluster, &everyone, Duration::from_millis(30_000)).await?;
    // Two leaders at once is the failure this whole section exists to rule out, and the heal
    // is the moment it would show: the returning member still believes it leads, and the
    // majority's leader is not going to stand down for it. What is checked is therefore that
    // no two members ever name different leaders — not that the leader never changes, since
    // an election is allowed to happen at any time and is not a split brain.
    let mut rounds = Vec::new();
    for _ in 0..8 {
        let mut round = Vec::new();
        for i in &everyone {
            round.push(cluster.leader_according_to(*i).await);
        }
        rounds.push(round);
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    let last = live_leader(cluster, Duration::from_millis(20_000)).await?;
    let described = cluster.describe().await;
    let mut c = Check::new("who leads, on every member, once the link is back");
    c.note(format!(
        "m{} was isolated, m{} was elected, m{} led once the link was back",
        old + 1,
        fresh + 1,
        settled + 1
    ));
    c.note(described);
    for (round, leaders) in rounds.iter().enumerate() {
        let mut named: Vec<usize> = leaders.iter().flatten().copied().collect();
        let voices = named.len();
        named.sort_unstable();
        named.dedup();
        c.eq(
            &format!("round {round}: how many different leaders were named"),
            1,
            named.len().max(1),
        );
        c.at_least(
            &format!("round {round}: how many members named a leader at all"),
            2,
            voices,
        );
    }
    c.that(
        "the leader once the polling was over",
        "still one leader named by everyone",
        last < cluster.initial_size,
        last + 1,
    );
    c.finish()
});
