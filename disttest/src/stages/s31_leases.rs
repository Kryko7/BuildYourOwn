//! Stage 31 — Leases: grant, attach, expire, revoke.
//!
//! A lease is a timer with keys hanging off it. It is how a store that knows nothing about
//! its clients expresses "this key belongs to something that is still alive": service
//! registration, leader election and locks are all one lease and one key.
//!
//! Two things make this harder than it looks. The expiry is the store's job, not the
//! client's — nobody sends a delete, the keys simply stop being there — and it is one
//! event, not one per key: every key on a lease disappears in a single revision, so a
//! watcher sees one consistent step rather than a key at a time.

use crate::assert::Check;
use crate::dist_test;
use crate::etcd::{b64, PutRequest, RangeRequest, CODE_NOT_FOUND};
use crate::examples::{node_example, node_example_after, ExampleSpec};
use crate::stages::{expect_code, ok, wait_until, Ladder, Stage, Test};
use serde_json::json;
use std::time::{Duration, Instant};

/// A TTL short enough that a test can wait for it, and long enough that a slow machine
/// does not lose the key before the test has looked at it. Real etcd will not grant less.
const SHORT_TTL: i64 = 2;

/// How long a test is prepared to wait for an expiry that is already overdue. Leases
/// expire a little late and never early, so this is a ceiling, not a delay.
const EXPIRY_PATIENCE: Duration = Duration::from_secs(20);

/// Stage 31.
pub fn stage() -> Stage {
    Stage {
        number: 31,
        slug: "leases",
        name: "Leases: grant, attach, expire, revoke",
        ext: false,
        ladder: Ladder::Node,
        hints: &[
            "POST /v3/lease/grant answers an ID and the TTL it settled on",
            "A put naming a lease binds the key to it; the kv answers with that lease id",
            "When the lease expires every key attached to it disappears in one revision",
            "Revoking is the same thing on demand, and revoking twice is an error",
        ],
        examples,
        tests: vec![
            Test::new("a grant answers an id and a ttl", grant_answers_id_and_ttl),
            Test::new(
                "a put naming a lease binds the key to it",
                put_binds_the_key,
            ),
            Test::new(
                "a key on an expired lease disappears",
                expiry_removes_the_key,
            )
            .min_timeout_ms(40_000),
            Test::new(
                "revoking a lease removes its keys at once",
                revoke_removes_the_keys,
            ),
            Test::new("revoking a lease twice is an error", revoke_twice),
            Test::new(
                "a put naming a lease that does not exist is an error",
                put_with_an_unknown_lease,
            ),
            Test::new(
                "every key on one lease disappears in a single revision",
                one_lease_one_revision,
            )
            .min_timeout_ms(40_000),
            Test::new(
                "a key overwritten without a lease survives the expiry",
                detached_key_survives,
            )
            .ext()
            .min_timeout_ms(40_000),
            Test::new("two leases keep their own keys", two_leases_are_independent).ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        node_example(
            "Asking for a lease",
            "/v3/lease/grant",
            || json!({ "TTL": "30", "ID": "0" }),
        )
        .request("thirty seconds, and ID 0 to mean \"pick an id for me\"")
        .response("the id the store picked and the TTL it settled on, both as strings")
        .note(
            "The TTL that comes back is the one the store will honour, not the one that was \
             asked for: real etcd refuses to grant less than a couple of seconds. Nothing is \
             written to the store here, so the revision has not moved.",
        ),
        node_example_after(
            "The key the lease owns",
            || {
                vec![
                    (
                        "/v3/lease/grant".to_string(),
                        json!({ "TTL": "30", "ID": "4242" }),
                    ),
                    (
                        "/v3/kv/put".to_string(),
                        json!({"key": b64(b"lk"), "value": b64(b"alive"), "lease": "4242"}),
                    ),
                ]
            },
            "/v3/kv/range",
            || json!({ "key": b64(b"lk") }),
        )
        .request("a point read of a key that was written with a lease attached")
        .response("the usual kv, plus `lease` naming the lease that owns it")
        .note(
            "A grant may name its own id instead of asking for one. The key looks like any \
             other until the lease goes, and then it goes too, with nobody having sent a \
             delete.",
        ),
    ]
}

dist_test!(grant_answers_id_and_ttl, |ctx| {
    let kv = ctx.kv()?;
    let before = ok(kv.status().await, "read the revision before granting")?;
    let lease = ok(kv.lease_grant(30).await, "grant a lease of thirty seconds")?;
    let mut c = Check::new("what a grant answers");
    c.ne("lease.ID", 0, lease.id);
    // The store may round the TTL up to whatever it is willing to police, but it may not
    // promise longer than it was asked for.
    c.within("lease.TTL", 1, 30, lease.ttl);
    c.eq("lease.error", String::new(), lease.error.clone());
    c.ne("lease.header.cluster_id", 0, lease.header.cluster_id);
    // A lease with nothing attached to it is not a write.
    c.eq(
        "lease.header.revision",
        before.header.revision,
        lease.header.revision,
    );
    c.finish()
});

dist_test!(put_binds_the_key, |ctx| {
    let leased = ctx.key("leased");
    let plain = ctx.key("plain");
    let kv = ctx.kv()?;
    let lease = ok(kv.lease_grant(60).await, "grant a lease")?;
    ok(
        kv.put_req(&PutRequest::new(&leased, b"v").lease(lease.id))
            .await,
        "put a key naming the lease",
    )?;
    ok(kv.put(&plain, b"v").await, "put a key naming no lease")?;
    let read = ok(kv.get_key(&leased).await, "read the leased key back")?;
    let other = ok(kv.get_key(&plain).await, "read the unleased key back")?;
    let mut c = Check::new("the lease a kv reports");
    c.eq(
        "range(leased).kvs[0].lease",
        lease.id,
        read.one().map(|k| k.lease).unwrap_or(0),
    );
    c.eq(
        "range(leased).kvs[0].value",
        "v".to_string(),
        read.one().map(|k| k.value_str()).unwrap_or_default(),
    );
    // A key with no lease reports 0, which is also what a missing field decodes to.
    c.eq(
        "range(plain).kvs[0].lease",
        0,
        other.one().map(|k| k.lease).unwrap_or(-1),
    );
    c.finish()
});

dist_test!(expiry_removes_the_key, |ctx| {
    let key = ctx.key("doomed");
    let (present, gone, put_revision, lived) = {
        let kv = ctx.kv()?;
        let granted_at = Instant::now();
        let lease = ok(kv.lease_grant(SHORT_TTL).await, "grant a short lease")?;
        let put = ok(
            kv.put_req(&PutRequest::new(&key, b"v").lease(lease.id))
                .await,
            "put a key on the short lease",
        )?;
        let present = ok(
            kv.get_key(&key).await,
            "read the key straight after the put",
        )?;
        let watched = &key;
        wait_until(
            "the key on the expired lease to disappear",
            EXPIRY_PATIENCE,
            || async move {
                kv.get_key(watched)
                    .await
                    .map(|r| r.count == 0)
                    .unwrap_or(false)
            },
        )
        .await?;
        let lived = granted_at.elapsed();
        let gone = ok(kv.get_key(&key).await, "read the key after the expiry")?;
        (present, gone, put.header.revision, lived)
    };
    ctx.note(format!(
        "the key lasted {} ms on a lease of {SHORT_TTL} s",
        lived.as_millis()
    ));
    let mut c = Check::new("a key whose lease ran out");
    c.eq("range(before).count", 1, present.count);
    c.eq("range(after).count", 0, gone.count);
    // Expiry is a write the store makes on its own behalf, so the clock moved.
    c.at_least(
        "range(after).header.revision",
        put_revision + 1,
        gone.header.revision,
    );
    // A lease may expire late — it never expires early.
    c.at_least(
        "how long the key lasted, in ms",
        (SHORT_TTL as u128) * 1000,
        lived.as_millis(),
    );
    c.finish()
});

dist_test!(revoke_removes_the_keys, |ctx| {
    let key = ctx.key("revoked");
    let survivor = ctx.key("survivor");
    let kv = ctx.kv()?;
    let lease = ok(
        kv.lease_grant(600).await,
        "grant a lease that will not expire",
    )?;
    let put = ok(
        kv.put_req(&PutRequest::new(&key, b"v").lease(lease.id))
            .await,
        "put a key on the lease",
    )?;
    ok(
        kv.put(&survivor, b"v").await,
        "put a key that belongs to nobody",
    )?;
    let revoke = ok(kv.lease_revoke(lease.id).await, "revoke the lease")?;
    let gone = ok(
        kv.get_key(&key).await,
        "read the leased key immediately afterwards",
    )?;
    let kept = ok(kv.get_key(&survivor).await, "read the unleased key")?;
    let mut c = Check::new("revoking a lease");
    c.eq("range(leased).count", 0, gone.count);
    c.eq("range(unleased).count", 1, kept.count);
    // The revoke's own header reports the revision the keys were removed at.
    c.at_least(
        "revoke.header.revision",
        put.header.revision + 1,
        revoke.revision,
    );
    c.finish()
});

dist_test!(revoke_twice, |ctx| {
    let key = ctx.key("once");
    let kv = ctx.kv()?;
    let lease = ok(kv.lease_grant(600).await, "grant a lease")?;
    ok(
        kv.put_req(&PutRequest::new(&key, b"v").lease(lease.id))
            .await,
        "put a key on it",
    )?;
    ok(kv.lease_revoke(lease.id).await, "revoke the lease")?;
    // The lease is not a tombstone that can be revoked again: it is gone, and asking about
    // something that is not there is a not-found, not a success.
    expect_code(
        kv.lease_revoke(lease.id).await,
        CODE_NOT_FOUND,
        "a second revoke of the same lease",
    )?;
    let read = ok(kv.get_key(&key).await, "read the key after both revokes")?;
    let mut c = Check::new("revoking a lease that is already gone");
    c.eq("range.count", 0, read.count);
    c.finish()
});

dist_test!(put_with_an_unknown_lease, |ctx| {
    let key = ctx.key("orphan");
    let kv = ctx.kv()?;
    let before = ok(kv.status().await, "read the revision before the bad put")?;
    expect_code(
        kv.put_req(&PutRequest::new(&key, b"v").lease(7_777_777_777))
            .await,
        CODE_NOT_FOUND,
        "a put naming a lease that was never granted",
    )?;
    let read = ok(
        kv.get_key(&key).await,
        "look for the key the refused put named",
    )?;
    let mut c = Check::new("a put onto a lease that does not exist");
    c.eq("range.count", 0, read.count);
    // The refusal is total: no key, and no revision spent on it.
    c.eq(
        "range.header.revision",
        before.header.revision,
        read.header.revision,
    );
    c.finish()
});

dist_test!(one_lease_one_revision, |ctx| {
    let prefix = ctx.key("herd");
    let kv = ctx.kv()?;
    let lease = ok(kv.lease_grant(SHORT_TTL).await, "grant a short lease")?;
    let mut last = 0;
    for i in 0..4 {
        let key = [prefix.as_slice(), format!("/{i}").as_bytes()].concat();
        last = ok(
            kv.put_req(&PutRequest::new(&key, b"v").lease(lease.id))
                .await,
            "put one of four keys on the same lease",
        )?
        .header
        .revision;
    }
    let request = RangeRequest::prefix(&prefix);
    let present = ok(kv.range_req(&request).await, "count the four keys")?;
    let watched = &request;
    wait_until(
        "every key on the lease to disappear",
        EXPIRY_PATIENCE,
        || async move {
            kv.range_req(watched)
                .await
                .map(|r| r.count == 0)
                .unwrap_or(false)
        },
    )
    .await?;
    let gone = ok(
        kv.range_req(&request).await,
        "read the prefix after the expiry",
    )?;
    let mut c = Check::new("four keys on one lease");
    c.eq("range(before).count", 4, present.count);
    c.eq("range(after).count", 0, gone.count);
    // One revision took all four: the store's clock moved by exactly one past the last put.
    c.eq(
        "range(after).header.revision",
        last + 1,
        gone.header.revision,
    );
    c.finish()
});

dist_test!(detached_key_survives, |ctx| {
    let key = ctx.key("adopted");
    let canary = ctx.key("canary");
    let kv = ctx.kv()?;
    let lease = ok(kv.lease_grant(SHORT_TTL).await, "grant a short lease")?;
    ok(
        kv.put_req(&PutRequest::new(&key, b"leased").lease(lease.id))
            .await,
        "put the key on the lease",
    )?;
    ok(
        kv.put_req(&PutRequest::new(&canary, b"leased").lease(lease.id))
            .await,
        "put a second key on the same lease, to tell when it goes",
    )?;
    // A put with no lease field rewrites the key with no lease at all, which is how a key
    // is taken back out of the lease's hands.
    ok(
        kv.put(&key, b"mine now").await,
        "overwrite the first key with no lease",
    )?;
    let watched = &canary;
    wait_until("the lease to expire", EXPIRY_PATIENCE, || async move {
        kv.get_key(watched)
            .await
            .map(|r| r.count == 0)
            .unwrap_or(false)
    })
    .await?;
    let read = ok(
        kv.get_key(&key).await,
        "read the detached key after the expiry",
    )?;
    let mut c = Check::new("a key taken off its lease before the lease died");
    c.eq("range.count", 1, read.count);
    c.eq(
        "range.kvs[0].value",
        "mine now".to_string(),
        read.one().map(|k| k.value_str()).unwrap_or_default(),
    );
    c.eq(
        "range.kvs[0].lease",
        0,
        read.one().map(|k| k.lease).unwrap_or(-1),
    );
    c.finish()
});

dist_test!(two_leases_are_independent, |ctx| {
    let first_key = ctx.key("first");
    let second_key = ctx.key("second");
    let kv = ctx.kv()?;
    let first = ok(kv.lease_grant(600).await, "grant the first lease")?;
    let second = ok(kv.lease_grant(600).await, "grant the second lease")?;
    ok(
        kv.put_req(&PutRequest::new(&first_key, b"a").lease(first.id))
            .await,
        "put a key on the first lease",
    )?;
    ok(
        kv.put_req(&PutRequest::new(&second_key, b"b").lease(second.id))
            .await,
        "put a key on the second lease",
    )?;
    ok(
        kv.lease_revoke(first.id).await,
        "revoke only the first lease",
    )?;
    let gone = ok(kv.get_key(&first_key).await, "read the first key")?;
    let kept = ok(kv.get_key(&second_key).await, "read the second key")?;
    let mut c = Check::new("two leases side by side");
    c.ne("the two lease ids", first.id, second.id);
    c.eq("range(first).count", 0, gone.count);
    c.eq("range(second).count", 1, kept.count);
    c.eq(
        "range(second).kvs[0].lease",
        second.id,
        kept.one().map(|k| k.lease).unwrap_or(0),
    );
    c.finish()
});
