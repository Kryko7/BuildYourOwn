//! Stage 38 — Linearizable reads after a write elsewhere.
//!
//! One sentence is the whole stage: write on m1, read on m2 straight away, see the write.
//! No polling, no "eventually", no retry loop — the read is issued the instant the put's
//! answer arrives and it must already carry the new value.
//!
//! That is harder than it looks, and the trap is a member serving the read out of its own
//! store. A follower's store is always slightly behind; a leader's store may be stale too,
//! because a leader that has been partitioned away does not yet know it has been replaced.
//! The fix is the same in both cases: confirm with a quorum before answering. A follower
//! asks the leader for a read index and waits until it has applied that far; a leader
//! confirms it is still the leader with a round of heartbeats. Only `serializable: true`
//! opts out, and a serializable read is allowed to be stale — but it is never allowed to
//! invent a value nobody ever wrote.
//!
//! Every test here shares one healthy cluster; nothing is killed or cut, so nothing is
//! `.fresh()`.

use crate::assert::Check;
use crate::dist_test;
use crate::etcd::{b64, RangeRequest};
use crate::examples::{cluster_example, step, ExampleSpec};
use crate::stages::{ok, wait_until, Ladder, Stage, Test};
use serde_json::json;
use std::time::Duration;

/// How long the cluster is given to settle on one leader.
const ELECT_WITHIN: Duration = Duration::from_millis(15_000);

/// Stage 38.
pub fn stage() -> Stage {
    Stage {
        number: 38,
        slug: "linearizable_reads",
        name: "Linearizable reads after a write elsewhere",
        ext: false,
        ladder: Ladder::Cluster,
        hints: &[
            "A read that is not serializable must not answer from a stale local state",
            "Confirm leadership (a read index, or a round of heartbeats) before answering",
            "A write acknowledged by one member must be visible to a read on any other, immediately",
            "serializable: true is the opt-out, and it is the only way to answer locally",
        ],
        examples,
        tests: vec![
            Test::new(
                "a write on one member is on the next read of another",
                write_here_read_there,
            )
            .min_timeout_ms(40_000),
            Test::new(
                "every ordered pair of members sees the write at once",
                every_ordered_pair,
            )
            .min_timeout_ms(40_000),
            Test::new(
                "a member that handled neither the write nor the last read still sees it",
                the_uninvolved_member,
            ),
            Test::new(
                "a delete is visible to the next read on another member",
                a_delete_is_visible_at_once,
            ),
            Test::new(
                "a read-after-write loop across members never misses",
                twenty_rounds,
            ),
            Test::new(
                "a serializable read may be stale but never invents a value",
                serializable_may_be_stale,
            ),
            Test::new(
                "a serializable read catches up on its own",
                serializable_catches_up,
            ),
            Test::new(
                "the read's header revision is at least the write's",
                the_read_header_is_not_behind,
            )
            .ext(),
            Test::new(
                "a key that was never written is absent on every member",
                an_absent_key_is_absent_everywhere,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        cluster_example("Write on m1, read on m2 with no wait", || {
            vec![
                step(
                    0,
                    "/v3/kv/put",
                    json!({"key": b64(b"lin"), "value": b64(b"fresh")}),
                    "write through m1",
                ),
                step(
                    1,
                    "/v3/kv/range",
                    json!({"key": b64(b"lin")}),
                    "read on m2, the very next request",
                ),
            ]
        })
        .request("a put, then immediately a range on a different member")
        .response("the value the put wrote, on the first read, with no retry")
        .note(
            "There is no sleep between the two steps. A member answering this read out of \
             its own store would have to have applied the entry already, which it has no \
             way of guaranteeing; a correct member asks the leader how far to read and \
             waits until it has applied that far.",
        ),
        cluster_example("The serializable opt-out", || {
            vec![
                step(
                    0,
                    "/v3/kv/put",
                    json!({"key": b64(b"lin-opt"), "value": b64(b"v")}),
                    "write through m1",
                ),
                step(
                    2,
                    "/v3/kv/range",
                    json!({"key": b64(b"lin-opt"), "serializable": true}),
                    "read on m3 without confirming with a quorum",
                ),
            ]
        })
        .request("the same read, with serializable: true")
        .response("either the new value or nothing at all — both are correct")
        .note(
            "This is the one read that is allowed to be behind, because it skips the round \
             trip to the leader. What it may never do is answer with a value nobody wrote, \
             or with a value older than one this same client already read here. On a quiet \
             three-member cluster it usually looks identical to the linearizable read, \
             which is exactly why the suite does not assert that it is stale.",
        ),
    ]
}

dist_test!(write_here_read_there, |ctx| {
    let key = ctx.key("k");
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT_WITHIN).await?;
    let put = ok(cluster.client(0).put(&key, b"fresh").await, "a write on m1")?;
    // No wait of any kind: the whole point is that the next read already sees it.
    let read = ok(cluster.client(1).get_key(&key).await, "a read on m2")?;
    let mut c = Check::new("a read on m2 of a write that m1 acknowledged");
    c.eq(
        "m2.range.kvs[0].value",
        "fresh".to_string(),
        read.one().map(|kv| kv.value_str()).unwrap_or_default(),
    );
    c.at_least(
        "m2.range.header.revision",
        put.header.revision,
        read.header.revision,
    );
    let faults = cluster.faults.describe();
    let described = cluster.describe().await;
    c.note(faults).note(described);
    c.finish()
});

dist_test!(every_ordered_pair, |ctx| {
    let prefix = ctx.key("pair");
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let mut c = Check::new("a write on every member, read at once on every other");
    for w in 0..size {
        for r in 0..size {
            if w == r {
                continue;
            }
            let key = [prefix.as_slice(), format!("/{w}{r}").as_bytes()].concat();
            let value = format!("m{}->m{}", w + 1, r + 1);
            let writer = cluster.members[w].name.clone();
            let reader = cluster.members[r].name.clone();
            ok(
                cluster.client(w).put(&key, value.as_bytes()).await,
                &format!("a write on {writer}"),
            )?;
            let read = ok(
                cluster.client(r).get_key(&key).await,
                &format!("a read on {reader}"),
            )?;
            c.eq(
                &format!("{writer} wrote, {reader} read: range.kvs[0].value"),
                value,
                read.one().map(|kv| kv.value_str()).unwrap_or_default(),
            );
        }
    }
    c.finish()
});

dist_test!(the_uninvolved_member, |ctx| {
    let key = ctx.key("bystander");
    let cluster = ctx.cluster()?;
    let leader = cluster.wait_for_leader(ELECT_WITHIN).await?;
    // Write through the leader, then read on whichever member is furthest from the
    // exchange: not the leader, and not the member the previous test happened to use.
    let reader = (0..cluster.initial_size)
        .rev()
        .find(|i| *i != leader)
        .unwrap_or(0);
    let reader_name = cluster.members[reader].name.clone();
    let put = ok(
        cluster.client(leader).put(&key, b"seen").await,
        "a write through the leader",
    )?;
    let read = ok(
        cluster.client(reader).get_key(&key).await,
        &format!("a read on {reader_name}"),
    )?;
    let mut c = Check::new("a read on a member that took no part in the write");
    c.eq(
        &format!("{reader_name}.range.kvs[0].value"),
        "seen".to_string(),
        read.one().map(|kv| kv.value_str()).unwrap_or_default(),
    );
    c.eq(
        &format!("{reader_name}.range.kvs[0].mod_revision"),
        put.header.revision,
        read.one().map(|kv| kv.mod_revision).unwrap_or_default(),
    );
    c.note(format!(
        "the write went through m{}, the read through {reader_name}",
        leader + 1
    ));
    c.finish()
});

dist_test!(a_delete_is_visible_at_once, |ctx| {
    let key = ctx.key("gone");
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT_WITHIN).await?;
    ok(cluster.client(0).put(&key, b"here").await, "a write on m1")?;
    let del = ok(cluster.client(1).delete(&key).await, "a delete on m2")?;
    let read = ok(cluster.client(2).get_key(&key).await, "a read on m3")?;
    let mut c = Check::new("a read on m3 of a key m2 has just deleted");
    c.eq("m2.deleterange.deleted", 1, del.deleted);
    c.eq("m3.range.count", 0, read.count);
    c.eq("m3.range.kvs.len()", 0, read.kvs.len());
    c.finish()
});

dist_test!(twenty_rounds, |ctx| {
    let key = ctx.key("loop");
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT_WITHIN).await?;
    let size = cluster.initial_size;
    let mut c = Check::new("twenty rounds of write here, read there");
    let mut misses = Vec::new();
    for round in 0..20 {
        let writer = round % size;
        let reader = (round + 1) % size;
        let value = format!("r{round}");
        ok(
            cluster.client(writer).put(&key, value.as_bytes()).await,
            "a write",
        )?;
        let read = ok(cluster.client(reader).get_key(&key).await, "a read")?;
        let got = read.one().map(|kv| kv.value_str()).unwrap_or_default();
        if got != value {
            misses.push(format!(
                "round {round}: m{} wrote {value}, m{} read {got:?}",
                writer + 1,
                reader + 1
            ));
        }
    }
    c.that(
        "rounds that read something other than the value just written",
        "no round to miss the write that had already been acknowledged",
        misses.is_empty(),
        misses.clone(),
    );
    if !misses.is_empty() {
        c.block("the rounds that missed", misses.join("\n"));
    }
    c.finish()
});

dist_test!(serializable_may_be_stale, |ctx| {
    let key = ctx.key("serializable");
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT_WITHIN).await?;
    ok(
        cluster.client(0).put(&key, b"written").await,
        "a write on m1",
    )?;
    let mut c = Check::new("a serializable read on every member");
    let mut answers = Vec::new();
    for i in 0..cluster.initial_size {
        let name = cluster.members[i].name.clone();
        let read = ok(
            cluster
                .client(i)
                .range_req(&RangeRequest::key(&key).with("serializable", json!(true)))
                .await,
            &format!("a serializable read on {name}"),
        )?;
        let got = read.one().map(|kv| kv.value_str());
        // The only two honest answers are the value that was written and nothing at all.
        // A serializable read is allowed to be behind; it is not allowed to make things up.
        c.that(
            &format!("{name}.range.kvs[0].value (serializable)"),
            "either \"written\" or no key at all",
            matches!(got.as_deref(), None | Some("written")),
            got.clone(),
        );
        answers.push(format!("{name}: {got:?}"));
    }
    c.observe("what each member answered", &answers);
    c.finish()
});

dist_test!(serializable_catches_up, |ctx| {
    let key = ctx.key("catch-up");
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT_WITHIN).await?;
    ok(
        cluster.client(0).put(&key, b"eventually").await,
        "a write on m1",
    )?;
    // Staleness is allowed, but it is not allowed to last: the entry is replicated whether
    // anyone reads it or not, so a serializable read settles on the value shortly after.
    for i in 0..cluster.initial_size {
        let name = cluster.members[i].name.clone();
        let client = cluster.client(i);
        let req = RangeRequest::key(&key).with("serializable", json!(true));
        wait_until(
            &format!("a serializable read on {name} to see the write"),
            Duration::from_millis(10_000),
            || async {
                matches!(
                    client.range_req(&req).await,
                    Ok(r) if r.one().map(|kv| kv.value_str()).as_deref() == Some("eventually")
                )
            },
        )
        .await?;
    }
    Ok(())
});

dist_test!(the_read_header_is_not_behind, |ctx| {
    let key = ctx.key("header");
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT_WITHIN).await?;
    let mut c = Check::new("the header of a linearizable read taken right after a write");
    for w in 0..cluster.initial_size {
        let r = (w + 1) % cluster.initial_size;
        let writer = cluster.members[w].name.clone();
        let reader = cluster.members[r].name.clone();
        let put = ok(
            cluster
                .client(w)
                .put(&key, format!("h{w}").as_bytes())
                .await,
            &format!("a write on {writer}"),
        )?;
        let read = ok(
            cluster.client(r).get_key(&key).await,
            &format!("a read on {reader}"),
        )?;
        // A read that answers with a revision lower than a write it already reflects would
        // be telling the client the store has gone backwards.
        c.at_least(
            &format!("{reader}.range.header.revision after a write on {writer}"),
            put.header.revision,
            read.header.revision,
        );
    }
    c.finish()
});

dist_test!(an_absent_key_is_absent_everywhere, |ctx| {
    let key = ctx.key("never-written");
    let cluster = ctx.cluster()?;
    cluster.wait_for_leader(ELECT_WITHIN).await?;
    let mut c = Check::new("a key nobody ever wrote");
    for i in 0..cluster.initial_size {
        let name = cluster.members[i].name.clone();
        let read = ok(
            cluster.client(i).get_key(&key).await,
            &format!("a read on {name}"),
        )?;
        c.eq(&format!("{name}.range.count"), 0, read.count);
        c.eq(&format!("{name}.range.kvs.len()"), 0, read.kvs.len());
        let stale = ok(
            cluster
                .client(i)
                .range_req(&RangeRequest::key(&key).with("serializable", json!(true)))
                .await,
            &format!("a serializable read on {name}"),
        )?;
        c.eq(
            &format!("{name}.range.count (serializable)"),
            0,
            stale.count,
        );
    }
    c.finish()
});
