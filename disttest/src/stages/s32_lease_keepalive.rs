//! Stage 32 — Lease keepalive and TTL.
//!
//! A lease is only useful if something can say "I am still here". That is the keepalive,
//! and it is the first streaming endpoint on the ladder: the answer is a chunked body of
//! one JSON object per line, each wrapped in `{"result": ...}`, even when — as here —
//! exactly one line ever comes back.
//!
//! The trap is the dead lease. A keepalive for a lease that has expired or been revoked is
//! not an error: it is a perfectly ordinary message carrying the id that was asked about
//! and a TTL of zero, because the caller's question — "how long have I got?" — has a real
//! answer, and the answer is "none".

use crate::assert::Check;
use crate::dist_test;
use crate::etcd::PutRequest;
use crate::examples::{node_example, node_example_after, ExampleSpec};
use crate::stages::{ok, wait_until, Ladder, Stage, Test};
use serde_json::json;
use std::time::{Duration, Instant};

/// The TTL the renewal tests race against; real etcd will not grant less than this.
const SHORT_TTL: i64 = 2;

/// How often a test renews a lease it means to keep: comfortably inside [`SHORT_TTL`].
const RENEW_EVERY: Duration = Duration::from_millis(700);

/// How long a test is prepared to wait for an expiry that is already overdue.
const EXPIRY_PATIENCE: Duration = Duration::from_secs(20);

/// Stage 32.
pub fn stage() -> Stage {
    Stage {
        number: 32,
        slug: "lease_keepalive",
        name: "Lease keepalive and TTL",
        ext: true,
        ladder: Ladder::Node,
        hints: &[
            "POST /v3/lease/keepalive is a stream: the answer is wrapped in {\"result\": ...}",
            "A keepalive resets the lease's remaining time to its TTL",
            "A keepalive for a lease that is gone answers TTL 0, not an error",
            "Keys survive exactly as long as something keeps the lease alive",
        ],
        examples,
        tests: vec![
            Test::new(
                "a keepalive answers the id it was asked about",
                answers_the_id,
            ),
            Test::new(
                "a keepalive message is wrapped in a result",
                wrapped_in_a_result,
            ),
            Test::new(
                "keeping a lease alive keeps its key past the original ttl",
                renewal_keeps_the_key,
            )
            .min_timeout_ms(40_000),
            Test::new(
                "the key goes once the keepalives stop",
                the_key_goes_when_renewal_stops,
            )
            .min_timeout_ms(40_000),
            Test::new(
                "a keepalive for a lease that was never granted answers a ttl of zero",
                unknown_lease_answers_zero,
            ),
            Test::new(
                "a keepalive for a revoked lease answers a ttl of zero",
                revoked_lease_answers_zero,
            ),
            Test::new(
                "a keepalive does not move the store revision",
                renewal_is_not_a_write,
            ),
            Test::new(
                "keeping one lease alive does nothing for another",
                renewal_is_per_lease,
            )
            .ext()
            .min_timeout_ms(40_000),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        node_example_after(
            "Saying \"I am still here\"",
            || {
                vec![(
                    "/v3/lease/grant".to_string(),
                    json!({ "TTL": "30", "ID": "4343" }),
                )]
            },
            "/v3/lease/keepalive",
            || json!({ "ID": "4343" }),
        )
        .request("the id of a lease that was granted a moment ago")
        .response("one chunked line: `{\"result\": {header, ID, TTL}}`, and the TTL is full again")
        .note(
            "This is a stream, not a plain request: `Transfer-Encoding: chunked`, one JSON \
             object per line, and every one of them wrapped in `result`. A client that \
             writes several ids down this one body gets one answer per id.",
        ),
        node_example(
            "Renewing a lease that is already gone",
            "/v3/lease/keepalive",
            || json!({ "ID": "999999" }),
        )
        .request("an id that was never granted — or was, and has since expired")
        .response("the same shape, carrying the id and no TTL at all")
        .note(
            "No error, and no TTL field: zero is the default, so it is left out. A client \
             renewing in a loop sees TTL 0 and knows its keys are gone, without having to \
             tell an error apart from a network failure.",
        ),
    ]
}

dist_test!(answers_the_id, |ctx| {
    let kv = ctx.kv()?;
    let lease = ok(kv.lease_grant(30).await, "grant a lease of thirty seconds")?;
    let alive = ok(kv.lease_keepalive(lease.id).await, "renew the lease once")?;
    let mut c = Check::new("what a keepalive answers");
    c.eq("keepalive.ID", lease.id, alive.id);
    // The renewed TTL is the lease's own, not some remainder counting down.
    c.within("keepalive.TTL", 1, 30, alive.ttl);
    c.eq("keepalive.error", String::new(), alive.error.clone());
    c.eq(
        "keepalive.header.cluster_id",
        lease.header.cluster_id,
        alive.header.cluster_id,
    );
    c.finish()
});

dist_test!(wrapped_in_a_result, |ctx| {
    let kv = ctx.kv()?;
    let lease = ok(kv.lease_grant(30).await, "grant a lease")?;
    let raw = ok(
        kv.post_raw(
            "/v3/lease/keepalive",
            &json!({ "ID": lease.id.to_string() }),
        )
        .await,
        "renew it, keeping the body exactly as it arrived",
    )?;
    let text = raw.text();
    let first = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default();
    let parsed: serde_json::Value = serde_json::from_str(first).unwrap_or(serde_json::Value::Null);
    let mut c = Check::new("the envelope a streaming endpoint uses");
    c.block("the body as it arrived", text.clone());
    c.eq("HTTP status", 200, raw.status);
    c.that(
        "the first line of the body",
        "an object with a `result` field",
        parsed.get("result").is_some(),
        first.to_string(),
    );
    let inner = parsed
        .get("result")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    c.eq(
        "result.ID",
        lease.id.to_string(),
        inner
            .get("ID")
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| v.to_string())
            })
            .unwrap_or_default(),
    );
    c.that(
        "result.header",
        "the header every response carries",
        inner.get("header").is_some(),
        inner.to_string(),
    );
    c.finish()
});

dist_test!(renewal_keeps_the_key, |ctx| {
    let key = ctx.key("renewed");
    let kv = ctx.kv()?;
    let lease = ok(kv.lease_grant(SHORT_TTL).await, "grant a short lease")?;
    ok(
        kv.put_req(&PutRequest::new(&key, b"v").lease(lease.id))
            .await,
        "put a key on it",
    )?;
    let started = Instant::now();
    let mut renewals = 0;
    // Keep it alive well past twice the TTL, renewing faster than it can run out.
    while started.elapsed() < Duration::from_millis((SHORT_TTL as u64) * 2_000 + 500) {
        tokio::time::sleep(RENEW_EVERY).await;
        let alive = ok(kv.lease_keepalive(lease.id).await, "renew the lease")?;
        renewals += 1;
        if alive.ttl <= 0 {
            break;
        }
    }
    let lived = started.elapsed();
    let read = ok(
        kv.get_key(&key).await,
        "read the key after all that renewing",
    )?;
    let mut c = Check::new("a lease held open by keepalives");
    c.note(format!(
        "{renewals} keepalives over {} ms, on a lease of {SHORT_TTL} s",
        lived.as_millis()
    ));
    c.at_least(
        "how long the key was held, in ms",
        (SHORT_TTL as u128) * 2_000,
        lived.as_millis(),
    );
    c.eq("range.count", 1, read.count);
    c.eq(
        "range.kvs[0].lease",
        lease.id,
        read.one().map(|k| k.lease).unwrap_or(0),
    );
    c.finish()
});

dist_test!(the_key_goes_when_renewal_stops, |ctx| {
    let key = ctx.key("abandoned");
    let kv = ctx.kv()?;
    let lease = ok(kv.lease_grant(SHORT_TTL).await, "grant a short lease")?;
    ok(
        kv.put_req(&PutRequest::new(&key, b"v").lease(lease.id))
            .await,
        "put a key on it",
    )?;
    for _ in 0..3 {
        tokio::time::sleep(RENEW_EVERY).await;
        ok(kv.lease_keepalive(lease.id).await, "renew the lease")?;
    }
    let held = ok(
        kv.get_key(&key).await,
        "read the key while it is still being renewed",
    )?;
    // Now stop, and let the last renewal run out.
    let stopped = Instant::now();
    let watched = &key;
    wait_until(
        "the key to go once nothing renews the lease",
        EXPIRY_PATIENCE,
        || async move {
            kv.get_key(watched)
                .await
                .map(|r| r.count == 0)
                .unwrap_or(false)
        },
    )
    .await?;
    let after = stopped.elapsed();
    let gone = ok(
        kv.get_key(&key).await,
        "read the key after the last renewal ran out",
    )?;
    let mut c = Check::new("a lease nobody renews any more");
    c.note(format!(
        "the key went {} ms after the last keepalive",
        after.as_millis()
    ));
    c.eq("range(while renewed).count", 1, held.count);
    c.eq("range(after).count", 0, gone.count);
    // The last keepalive bought a full TTL, so the key cannot have gone immediately.
    c.at_least(
        "how long the key outlived the last keepalive, in ms",
        (SHORT_TTL as u128) * 1000,
        after.as_millis(),
    );
    c.finish()
});

dist_test!(unknown_lease_answers_zero, |ctx| {
    let kv = ctx.kv()?;
    let alive = ok(
        kv.lease_keepalive(6_543_210).await,
        "renew a lease that was never granted",
    )?;
    let mut c = Check::new("a keepalive for an id nobody has");
    c.eq("keepalive.ID", 6_543_210, alive.id);
    c.eq("keepalive.TTL", 0, alive.ttl);
    c.ne("keepalive.header.cluster_id", 0, alive.header.cluster_id);
    c.finish()
});

dist_test!(revoked_lease_answers_zero, |ctx| {
    let key = ctx.key("was-here");
    let kv = ctx.kv()?;
    let lease = ok(kv.lease_grant(600).await, "grant a lease")?;
    ok(
        kv.put_req(&PutRequest::new(&key, b"v").lease(lease.id))
            .await,
        "put a key on it",
    )?;
    ok(kv.lease_revoke(lease.id).await, "revoke the lease")?;
    let alive = ok(
        kv.lease_keepalive(lease.id).await,
        "renew the lease that has just been revoked",
    )?;
    let read = ok(kv.get_key(&key).await, "read the key the lease owned")?;
    let mut c = Check::new("renewing a lease that has been revoked");
    // A renewal cannot bring a lease back, and saying so is not an error.
    c.eq("keepalive.ID", lease.id, alive.id);
    c.eq("keepalive.TTL", 0, alive.ttl);
    c.eq("range.count", 0, read.count);
    c.finish()
});

dist_test!(renewal_is_not_a_write, |ctx| {
    let kv = ctx.kv()?;
    let lease = ok(kv.lease_grant(60).await, "grant a lease")?;
    let before = ok(kv.status().await, "read the revision before renewing")?;
    for _ in 0..3 {
        ok(kv.lease_keepalive(lease.id).await, "renew the lease")?;
    }
    let after = ok(kv.status().await, "read the revision after renewing")?;
    let mut c = Check::new("the store's clock across three keepalives");
    // Nothing about the keys changed, so nothing about the store's history did either.
    c.eq(
        "status(after).header.revision",
        before.header.revision,
        after.header.revision,
    );
    c.finish()
});

dist_test!(renewal_is_per_lease, |ctx| {
    let kept = ctx.key("kept");
    let dropped = ctx.key("dropped");
    let kv = ctx.kv()?;
    let held = ok(
        kv.lease_grant(SHORT_TTL).await,
        "grant the lease that will be renewed",
    )?;
    let abandoned = ok(
        kv.lease_grant(SHORT_TTL).await,
        "grant the lease that will not be",
    )?;
    ok(
        kv.put_req(&PutRequest::new(&kept, b"v").lease(held.id))
            .await,
        "put a key on the renewed lease",
    )?;
    ok(
        kv.put_req(&PutRequest::new(&dropped, b"v").lease(abandoned.id))
            .await,
        "put a key on the abandoned lease",
    )?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis((SHORT_TTL as u64) * 2_000 + 500) {
        tokio::time::sleep(RENEW_EVERY).await;
        ok(
            kv.lease_keepalive(held.id).await,
            "renew only the first lease",
        )?;
    }
    let watched = &dropped;
    wait_until(
        "the key on the lease nobody renewed to disappear",
        EXPIRY_PATIENCE,
        || async move {
            kv.get_key(watched)
                .await
                .map(|r| r.count == 0)
                .unwrap_or(false)
        },
    )
    .await?;
    let survivor = ok(kv.get_key(&kept).await, "read the key on the renewed lease")?;
    let gone = ok(
        kv.get_key(&dropped).await,
        "read the key on the abandoned lease",
    )?;
    let mut c = Check::new("two leases, one of them renewed");
    c.eq("range(kept).count", 1, survivor.count);
    c.eq(
        "range(kept).kvs[0].lease",
        held.id,
        survivor.one().map(|k| k.lease).unwrap_or(0),
    );
    c.eq("range(dropped).count", 0, gone.count);
    c.finish()
});
