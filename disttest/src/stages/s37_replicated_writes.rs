//! Stage 37 — A write on any member is visible on every member.
//!
//! The first stage that asks the cluster to be a cluster rather than three servers that
//! happen to know each other's addresses. Every test here writes through one member and
//! then reads the same key from all three.
//!
//! Two traps live in this stage. The first is the follower write: a member that is not the
//! leader may not apply a write locally and answer, it has to forward the request and wait
//! for the leader's answer, because a locally applied write belongs to no raft log and
//! would vanish the moment anyone else wrote. The second is the revision: revisions are a
//! property of the cluster's log, not of the member that happened to answer, so a follower
//! that has never accepted a write of its own still answers with the cluster's revision.
//!
//! Nothing here kills or partitions anything, so every test shares one cluster and none of
//! them is `.fresh()`. Replication is fast but not instantaneous, which is why every
//! cross-member read polls with [`wait_until`] instead of reading once and hoping.

use crate::assert::Check;
use crate::dist_test;
use crate::etcd::{b64, Client};
use crate::examples::{cluster_example, step, ExampleSpec};
use crate::stages::{check_strictly_increasing, ok, text, wait_until, Ladder, Stage, Test};
use serde_json::json;
use std::time::Duration;

/// How long a replicated write is given to reach every member.
const REPLICATE_WITHIN: Duration = Duration::from_millis(10_000);
/// How long the cluster is given to settle on one leader.
const ELECT_WITHIN: Duration = Duration::from_millis(15_000);

/// Stage 37.
pub fn stage() -> Stage {
    Stage {
        number: 37,
        slug: "replicated_writes",
        name: "A write on any member is visible on every member",
        ext: false,
        ladder: Ladder::Cluster,
        hints: &[
            "A write accepted by a follower is forwarded to the leader, not applied locally",
            "The revision a member answers with is the cluster's, not its own counter",
            "Every member must end up holding the same value for the key",
            "Writes through different members still produce one increasing revision sequence",
        ],
        examples,
        tests: vec![
            Test::new(
                "a write through the leader reaches every member",
                write_through_the_leader,
            )
            .min_timeout_ms(40_000),
            Test::new(
                "a write through a follower is accepted and reaches every member",
                write_through_a_follower,
            )
            .min_timeout_ms(40_000),
            Test::new(
                "writes through three members make one revision sequence",
                one_revision_sequence,
            ),
            Test::new(
                "every member holds the same value for the key",
                same_value_everywhere,
            ),
            Test::new("a delete replicates too", a_delete_replicates),
            Test::new(
                "a batch of writes through rotating members all land",
                a_rotating_batch,
            ),
            Test::new(
                "the revision a member answers with is the cluster's",
                the_revision_is_the_clusters,
            ),
            Test::new(
                "create and mod revisions are the same on every member",
                revisions_of_the_key_agree,
            )
            .ext(),
            Test::new(
                "forty writes through rotating members keep one order",
                forty_rotating_writes,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        cluster_example("A write on m1, read back on m3", || {
            vec![
                step(
                    0,
                    "/v3/kv/put",
                    json!({"key": b64(b"replicated"), "value": b64(b"yes")}),
                    "write through m1",
                ),
                step(
                    2,
                    "/v3/kv/range",
                    json!({"key": b64(b"replicated")}),
                    "and read it on m3",
                ),
            ]
        })
        .request("a put on one member, then a range for the same key on another")
        .response("the value the put wrote, with the revision the put answered with")
        .note(
            "The read is served by a member that never saw the client's put. It answers \
             from the entry the leader replicated to it, and its header carries the \
             cluster's revision rather than a count of the writes this member handled.",
        ),
        cluster_example("A write through a follower", || {
            vec![
                step(
                    1,
                    "/v3/kv/put",
                    json!({"key": b64(b"through-m2"), "value": b64(b"forwarded")}),
                    "write through m2, which is usually not the leader",
                ),
                step(
                    0,
                    "/v3/kv/range",
                    json!({"key": b64(b"through-m2")}),
                    "read it back on m1",
                ),
                step(
                    2,
                    "/v3/kv/range",
                    json!({"key": b64(b"through-m2")}),
                    "and on m3",
                ),
            ]
        })
        .request("a put sent to a member that is not the leader")
        .response("a normal put response, and the key readable on all three members")
        .note(
            "A follower does not apply the write itself. It forwards the request to the \
             leader and answers when the leader's entry is committed, which is why the \
             revision in the answer is one the whole cluster agrees on. A member that \
             applied the write locally would pass this one read and fail the moment \
             another member wrote the same key.",
        ),
    ]
}

/// Poll one member until the key holds `want`, or give up with a failure naming the member.
async fn wait_for_value(
    client: &Client,
    member: &str,
    key: &[u8],
    want: &[u8],
) -> Result<(), crate::assert::Failure> {
    wait_until(
        &format!("{member} to hold {}={}", text(key), text(want)),
        REPLICATE_WITHIN,
        || async { matches!(client.value_of(key).await, Ok(Some(v)) if v == want) },
    )
    .await
    .map(|_| ())
}

/// Poll one member until the key is gone.
async fn wait_for_absence(
    client: &Client,
    member: &str,
    key: &[u8],
) -> Result<(), crate::assert::Failure> {
    wait_until(
        &format!("{member} to stop holding {}", text(key)),
        REPLICATE_WITHIN,
        || async { matches!(client.value_of(key).await, Ok(None)) },
    )
    .await
    .map(|_| ())
}

dist_test!(write_through_the_leader, |ctx| {
    let key = ctx.key("leader-write");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let put = ok(
        cluster.client(leader).put(&key, b"v1").await,
        "a write through the leader",
    )?;
    cluster
        .wait_for_revision(put.header.revision, REPLICATE_WITHIN)
        .await?;
    let mut c = Check::new("the key read back from every member");
    for i in 0..cluster.initial_size {
        let name = cluster.members[i].name.clone();
        let read = ok(
            cluster.client(i).get_key(&key).await,
            &format!("a read on {name}"),
        )?;
        c.eq(
            &format!("{name}.range.kvs[0].value"),
            "v1".to_string(),
            read.one().map(|kv| kv.value_str()).unwrap_or_default(),
        );
        c.at_least(
            &format!("{name}.range.header.revision"),
            put.header.revision,
            read.header.revision,
        );
    }
    let described = cluster.describe().await;
    let faults = cluster.faults.describe();
    c.note(described).note(faults);
    c.finish()
});

dist_test!(write_through_a_follower, |ctx| {
    let key = ctx.key("follower-write");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    // Any member that is not the leader will do; the lowest index keeps the run
    // reproducible whichever member happened to win the election.
    let follower = (0..cluster.initial_size)
        .find(|i| *i != leader)
        .unwrap_or(0);
    let follower_name = cluster.members[follower].name.clone();
    let put = ok(
        cluster.client(follower).put(&key, b"v2").await,
        &format!("a write through {follower_name}, which is not the leader"),
    )?;
    cluster
        .wait_for_revision(put.header.revision, REPLICATE_WITHIN)
        .await?;
    let mut c = Check::new("a write a follower accepted");
    c.at_least("put.header.revision", 1, put.header.revision);
    for i in 0..cluster.initial_size {
        let name = cluster.members[i].name.clone();
        let read = ok(
            cluster.client(i).get_key(&key).await,
            &format!("a read on {name}"),
        )?;
        c.eq(
            &format!("{name}.range.kvs[0].value"),
            "v2".to_string(),
            read.one().map(|kv| kv.value_str()).unwrap_or_default(),
        );
    }
    let described = cluster.describe().await;
    c.note(format!(
        "the write went through {follower_name}; the leader was m{}",
        leader + 1
    ))
    .note(described);
    c.finish()
});

dist_test!(one_revision_sequence, |ctx| {
    let key = ctx.key("sequence");
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT_WITHIN).await?;
    let mut revisions = Vec::new();
    for i in 0..cluster.initial_size {
        let name = cluster.members[i].name.clone();
        let put = ok(
            cluster
                .client(i)
                .put(&key, format!("from-{name}").as_bytes())
                .await,
            &format!("a write through {name}"),
        )?;
        revisions.push(put.header.revision);
    }
    let mut c = Check::new("three writes through three different members");
    // One log means one sequence: whichever member took the request, the revision it
    // answers with comes from the same counter.
    check_strictly_increasing(&mut c, "put.header.revision", &revisions);
    c.observe("revisions", &revisions);
    c.finish()
});

dist_test!(same_value_everywhere, |ctx| {
    let key = ctx.key("agreed");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    // Three writes in a row through three members: whoever wrote last, every member must
    // end up holding that value and no other.
    for (round, value) in ["a", "b", "final"].iter().enumerate() {
        let through = round % cluster.initial_size;
        let name = cluster.members[through].name.clone();
        ok(
            cluster.client(through).put(&key, value.as_bytes()).await,
            &format!("write {value} through {name}"),
        )?;
    }
    for i in 0..cluster.initial_size {
        let name = cluster.members[i].name.clone();
        wait_for_value(cluster.client(i), &name, &key, b"final").await?;
    }
    let mut c = Check::new("the last value, on every member");
    let mut values = Vec::new();
    for i in 0..cluster.initial_size {
        let name = cluster.members[i].name.clone();
        let read = ok(
            cluster.client(i).get_key(&key).await,
            &format!("a read on {name}"),
        )?;
        values.push(read.one().map(|kv| kv.value_str()).unwrap_or_default());
    }
    c.eq(
        "the values every member holds",
        vec!["final".to_string(); cluster.initial_size],
        values,
    );
    c.note(format!("the leader was m{}", leader + 1));
    c.finish()
});

dist_test!(a_delete_replicates, |ctx| {
    let key = ctx.key("deleted");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    ok(
        cluster.client(leader).put(&key, b"here").await,
        "the write that is about to be deleted",
    )?;
    for i in 0..cluster.initial_size {
        let name = cluster.members[i].name.clone();
        wait_for_value(cluster.client(i), &name, &key, b"here").await?;
    }
    let follower = (0..cluster.initial_size)
        .find(|i| *i != leader)
        .unwrap_or(0);
    let follower_name = cluster.members[follower].name.clone();
    let del = ok(
        cluster.client(follower).delete(&key).await,
        &format!("a delete through {follower_name}"),
    )?;
    for i in 0..cluster.initial_size {
        let name = cluster.members[i].name.clone();
        wait_for_absence(cluster.client(i), &name, &key).await?;
    }
    let mut c = Check::new("a delete sent through a follower");
    c.eq("deleterange.deleted", 1, del.deleted);
    c.at_least("deleterange.header.revision", 1, del.header.revision);
    c.note(format!("the delete went through {follower_name}"));
    c.finish()
});

dist_test!(a_rotating_batch, |ctx| {
    let prefix = ctx.key("batch");
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let mut last = 0;
    for n in 0..9 {
        let key = [prefix.as_slice(), format!("/{n:02}").as_bytes()].concat();
        let through = n % size;
        let name = cluster.members[through].name.clone();
        let put = ok(
            cluster
                .client(through)
                .put(&key, format!("v{n}").as_bytes())
                .await,
            &format!("write {n} through {name}"),
        )?;
        last = put.header.revision;
    }
    cluster.wait_for_revision(last, REPLICATE_WITHIN).await?;
    let mut c = Check::new("nine writes spread over three members");
    for i in 0..size {
        let name = cluster.members[i].name.clone();
        let read = ok(
            cluster
                .client(i)
                .range_req(&crate::etcd::RangeRequest::prefix(&prefix))
                .await,
            &format!("a prefix read on {name}"),
        )?;
        c.eq(&format!("{name}.range.count"), 9, read.count);
        c.eq(
            &format!("{name}.range.values"),
            (0..9).map(|n| format!("v{n}")).collect::<Vec<_>>(),
            read.values(),
        );
    }
    c.finish()
});

dist_test!(the_revision_is_the_clusters, |ctx| {
    let prefix = ctx.key("counter");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    // Every write goes through the leader, so a member counting only the writes it
    // handled itself would answer a much smaller number than the cluster's revision.
    let mut last = 0;
    for n in 0..5 {
        let key = [prefix.as_slice(), format!("/{n}").as_bytes()].concat();
        last = ok(
            cluster.client(leader).put(&key, b"x").await,
            "a write through the leader",
        )?
        .header
        .revision;
    }
    cluster.wait_for_revision(last, REPLICATE_WITHIN).await?;
    let mut c = Check::new("the revision every member answers with");
    for i in 0..cluster.initial_size {
        let name = cluster.members[i].name.clone();
        let status = ok(
            cluster.client(i).status().await,
            &format!("the status of {name}"),
        )?;
        c.at_least(
            &format!("{name}.status.header.revision"),
            last,
            status.header.revision,
        );
    }
    c.note(format!(
        "every one of the five writes went through m{}",
        leader + 1
    ));
    c.finish()
});

dist_test!(revisions_of_the_key_agree, |ctx| {
    let key = ctx.key("mvcc");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    let first = ok(
        cluster.client(leader).put(&key, b"one").await,
        "the first write",
    )?;
    let second = ok(
        cluster.client(leader).put(&key, b"two").await,
        "the second write",
    )?;
    cluster
        .wait_for_revision(second.header.revision, REPLICATE_WITHIN)
        .await?;
    let mut c = Check::new("the revisions attached to a twice-written key");
    for i in 0..cluster.initial_size {
        let name = cluster.members[i].name.clone();
        let read = ok(
            cluster.client(i).get_key(&key).await,
            &format!("a read on {name}"),
        )?;
        let kv = read.one().cloned().unwrap_or_default();
        c.eq(
            &format!("{name}.range.kvs[0].create_revision"),
            first.header.revision,
            kv.create_revision,
        );
        c.eq(
            &format!("{name}.range.kvs[0].mod_revision"),
            second.header.revision,
            kv.mod_revision,
        );
        c.eq(&format!("{name}.range.kvs[0].version"), 2, kv.version);
    }
    c.finish()
});

dist_test!(forty_rotating_writes, |ctx| {
    let key = ctx.key("forty");
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let mut revisions = Vec::new();
    for n in 0..40 {
        let through = n % size;
        let name = cluster.members[through].name.clone();
        let put = ok(
            cluster
                .client(through)
                .put(&key, format!("v{n}").as_bytes())
                .await,
            &format!("write {n} through {name}"),
        )?;
        revisions.push(put.header.revision);
    }
    let last = revisions.last().copied().unwrap_or(0);
    cluster.wait_for_revision(last, REPLICATE_WITHIN).await?;
    let mut c = Check::new("forty writes to one key, spread over three members");
    check_strictly_increasing(&mut c, "put.header.revision", &revisions);
    for i in 0..size {
        let name = cluster.members[i].name.clone();
        let read = ok(
            cluster.client(i).get_key(&key).await,
            &format!("a read on {name}"),
        )?;
        let kv = read.one().cloned().unwrap_or_default();
        c.eq(
            &format!("{name}.range.kvs[0].value"),
            "v39".to_string(),
            kv.value_str(),
        );
        c.eq(&format!("{name}.range.kvs[0].version"), 40, kv.version);
        c.eq(
            &format!("{name}.range.kvs[0].mod_revision"),
            last,
            kv.mod_revision,
        );
    }
    c.finish()
});
