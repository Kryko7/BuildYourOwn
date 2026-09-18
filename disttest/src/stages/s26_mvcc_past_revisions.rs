//! Stage 26 — MVCC: reading at a past revision.
//!
//! The store keeps every version of every key, and the revision in the header is the handle
//! on that history. Naming a revision in a range request asks what the store looked like
//! when its clock read that number: the value a key held then, keys that have since been
//! deleted, and nothing of what has happened since.
//!
//! Two things are easy to get backwards. The **header** of a historical read still reports
//! the store's current revision, not the one that was asked for — the answer is old, the
//! clock is not. And `revision: 0` means *now*, not *the beginning*, because zero is the
//! zero value of the field and a request that left it out has to mean the same thing as one
//! that sent it. A revision the store has not reached yet is an error rather than an empty
//! answer, because the honest answer to "what did the future look like" is that there is no
//! such state.

use crate::assert::Check;
use crate::dist_test;
use crate::etcd::{b64, RangeRequest, CODE_OUT_OF_RANGE};
use crate::examples::{node_example_after, ExampleSpec};
use crate::stages::{expect_code, ok, put_series, text, Ladder, Stage, Test};
use serde_json::{json, Value};

/// Stage 26.
pub fn stage() -> Stage {
    Stage {
        number: 26,
        slug: "mvcc_past_revisions",
        name: "MVCC: reading at a past revision",
        ext: true,
        ladder: Ladder::Node,
        hints: &[
            "revision in a range request reads the store as it was at that revision",
            "A key deleted later is still there when read at a revision before the delete",
            "The header of a historical read still reports the store's current revision",
            "Revision 0 means 'now', because 0 is the zero value and cannot mean 'the beginning'",
        ],
        examples,
        tests: vec![
            Test::new(
                "a read at an old revision sees the old value",
                the_old_value,
            )
            .ext(),
            Test::new(
                "a key deleted later is still there before the delete",
                deleted_later_still_there,
            )
            .ext(),
            Test::new(
                "a key created later is absent before its creation",
                created_later_is_absent,
            )
            .ext(),
            Test::new(
                "the header of a historical read reports the current revision",
                header_is_current,
            )
            .ext(),
            Test::new("revision zero means now", revision_zero_means_now).ext(),
            Test::new(
                "a revision the store has not reached is an error",
                a_future_revision,
            )
            .ext(),
            Test::new(
                "a historical range sees the store as it was",
                a_historical_range,
            )
            .ext(),
            Test::new(
                "a historical range respects limit and sorting too",
                historical_options,
            )
            .ext(),
        ],
    }
}

/// Two writes to one key, so the example has a history to read back.
fn two_writes() -> Vec<(String, Value)> {
    vec![
        (
            "/v3/kv/put".into(),
            json!({"key": b64(b"mv"), "value": b64(b"one")}),
        ),
        (
            "/v3/kv/put".into(),
            json!({"key": b64(b"mv"), "value": b64(b"two")}),
        ),
    ]
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        node_example_after(
            "Reading a key as it was",
            two_writes,
            "/v3/kv/range",
            || json!({ "key": b64(b"mv"), "revision": "2" }),
        )
        .request("the key `mv` as the store held it at revision 2")
        .response("the first value, alongside a header whose revision is the current one")
        .note(
            "The kv carries the revisions it had then — mod_revision 2, version 1 — while \
             the header carries the store's clock now. Nothing about a historical read \
             rewinds the header.",
        ),
        node_example_after(
            "A revision the store has not reached",
            two_writes,
            "/v3/kv/range",
            || json!({ "key": b64(b"mv"), "revision": "99999" }),
        )
        .request("a revision far beyond anything that has happened")
        .response("HTTP 400 and code 11, out of range")
        .note(
            "A future revision is refused rather than answered empty: an empty answer would \
             claim the key did not exist at a moment that has not happened. The same code \
             comes back for a revision compacted away at the other end of the history.",
        ),
    ]
}

dist_test!(the_old_value, |ctx| {
    let kv = ctx.kv()?;
    let key = ctx.key("k");
    let first = ok(kv.put(&key, b"one").await, "put the first value")?;
    let second = ok(kv.put(&key, b"two").await, "put the second value")?;
    let old = ok(
        kv.range_req(&RangeRequest::key(&key).at_revision(first.header.revision))
            .await,
        "read the key at the first revision",
    )?;
    let now = ok(kv.get_key(&key).await, "read the key now")?;
    let mut c = Check::new("a read at an old revision");
    c.eq("range(revision=first).count", 1, old.count);
    c.eq(
        "range(revision=first).kvs[0].value",
        "one".to_string(),
        old.one().map(|p| p.value_str()).unwrap_or_default(),
    );
    // The pair carries the revisions it had then, not the ones it has now.
    c.eq(
        "range(revision=first).kvs[0].mod_revision",
        first.header.revision,
        old.one().map(|p| p.mod_revision).unwrap_or(0),
    );
    c.eq(
        "range(revision=first).kvs[0].version",
        1,
        old.one().map(|p| p.version).unwrap_or(0),
    );
    c.eq(
        "range().kvs[0].value",
        "two".to_string(),
        now.one().map(|p| p.value_str()).unwrap_or_default(),
    );
    c.eq(
        "range().kvs[0].mod_revision",
        second.header.revision,
        now.one().map(|p| p.mod_revision).unwrap_or(0),
    );
    c.finish()
});

dist_test!(deleted_later_still_there, |ctx| {
    let kv = ctx.kv()?;
    let key = ctx.key("doomed");
    let put = ok(kv.put(&key, b"alive").await, "put the key")?;
    let delete = ok(kv.delete(&key).await, "delete it")?;
    let before = ok(
        kv.range_req(&RangeRequest::key(&key).at_revision(put.header.revision))
            .await,
        "read the key at a revision before the delete",
    )?;
    let now = ok(kv.get_key(&key).await, "read the key now")?;
    let mut c = Check::new("a key read at a revision before it was deleted");
    // A delete is a new revision, not an erasure: everything before it still holds the key.
    c.eq("range(revision=put).count", 1, before.count);
    c.eq(
        "range(revision=put).kvs[0].value",
        "alive".to_string(),
        before.one().map(|p| p.value_str()).unwrap_or_default(),
    );
    c.eq("range().count", 0, now.count);
    c.at_least(
        "deleterange.header.revision",
        put.header.revision + 1,
        delete.header.revision,
    );
    c.finish()
});

dist_test!(created_later_is_absent, |ctx| {
    let kv = ctx.kv()?;
    let anchor = ok(
        kv.put(&ctx.key("anchor"), b"v").await,
        "put an unrelated key",
    )?;
    let late = ctx.key("late");
    let created = ok(kv.put(&late, b"v").await, "create the key afterwards")?;
    let before = ok(
        kv.range_req(&RangeRequest::key(&late).at_revision(anchor.header.revision))
            .await,
        "read the key at a revision before it existed",
    )?;
    let mut c = Check::new("a key read before it was created");
    c.eq("range(revision=anchor).count", 0, before.count);
    c.eq("range(revision=anchor).kvs.len()", 0, before.kvs.len());
    // The key exists at its own creating revision, and only from then on.
    let at_creation = ok(
        kv.range_req(&RangeRequest::key(&late).at_revision(created.header.revision))
            .await,
        "read the key at the revision that created it",
    )?;
    c.eq("range(revision=created).count", 1, at_creation.count);
    c.finish()
});

dist_test!(header_is_current, |ctx| {
    let kv = ctx.kv()?;
    let key = ctx.key("k");
    let first = ok(kv.put(&key, b"one").await, "put the first value")?;
    for i in 2..=5 {
        ok(kv.put(&key, format!("v{i}").as_bytes()).await, "put again")?;
    }
    let latest = ok(kv.get_key(&key).await, "read the key now")?;
    let old = ok(
        kv.range_req(&RangeRequest::key(&key).at_revision(first.header.revision))
            .await,
        "read the key at the first revision",
    )?;
    let mut c = Check::new("the header of a historical read");
    // The answer is from the past; the clock in the header is not.
    c.eq(
        "range(revision=first).header.revision",
        latest.header.revision,
        old.header.revision,
    );
    c.that(
        "range(revision=first).header.revision > the revision asked for",
        "the store's current clock, not the revision the read named",
        old.header.revision > first.header.revision,
        (first.header.revision, old.header.revision),
    );
    c.finish()
});

dist_test!(revision_zero_means_now, |ctx| {
    let kv = ctx.kv()?;
    let key = ctx.key("k");
    ok(kv.put(&key, b"one").await, "put the first value")?;
    ok(kv.put(&key, b"latest").await, "put the latest value")?;
    let zero = ok(
        kv.range_req(&RangeRequest::key(&key).at_revision(0)).await,
        "read the key at revision 0",
    )?;
    let omitted = ok(
        kv.get_key(&key).await,
        "read the key with no revision at all",
    )?;
    let mut c = Check::new("revision 0");
    // Zero is the zero value of the field, so sending it and leaving it out must agree.
    c.eq("range(revision=0).count", 1, zero.count);
    c.eq(
        "range(revision=0).kvs[0].value",
        "latest".to_string(),
        zero.one().map(|p| p.value_str()).unwrap_or_default(),
    );
    c.eq(
        "range(revision=0).kvs",
        omitted.kvs.clone(),
        zero.kvs.clone(),
    );
    c.eq(
        "range(revision=0).header.revision",
        omitted.header.revision,
        zero.header.revision,
    );
    c.finish()
});

dist_test!(a_future_revision, |ctx| {
    let kv = ctx.kv()?;
    ok(kv.put(&ctx.key("k"), b"v").await, "put a key")?;
    let status = ok(kv.status().await, "ask how far the clock has got")?;
    let beyond = status.header.revision + 1_000;
    // Real etcd answers code 11, out of range — the same code a revision compacted away at
    // the other end of the history gets, because both name a revision that is not readable.
    let err = expect_code(
        kv.range_req(&RangeRequest::all().at_revision(beyond)).await,
        CODE_OUT_OF_RANGE,
        "a range at a revision far in the future",
    )?;
    let next = expect_code(
        kv.range_req(&RangeRequest::all().at_revision(status.header.revision + 1))
            .await,
        CODE_OUT_OF_RANGE,
        "a range at the revision one past the clock",
    )?;
    let mut c = Check::new("a revision the store has not reached");
    c.observe("error(far future).body", err.body());
    c.observe("error(one past).body", next.body());
    // The current revision itself is readable; only what is past it is not.
    let at_now = ok(
        kv.range_req(&RangeRequest::all().at_revision(status.header.revision))
            .await,
        "a range at exactly the current revision",
    )?;
    c.eq("range(revision=current).count", 1, at_now.count);
    c.finish()
});

dist_test!(a_historical_range, |ctx| {
    let kv = ctx.kv()?;
    let base = ctx.key("h");
    let revisions = put_series(kv, &base, 3).await?;
    let then = revisions.last().copied().unwrap_or(0);
    let prefix = [base.as_slice(), b"/"].concat();
    // Everything after this point must be invisible to a read at `then`.
    ok(
        kv.put(&[prefix.as_slice(), b"03"].concat(), b"late").await,
        "add a fourth key",
    )?;
    ok(
        kv.delete(&[prefix.as_slice(), b"01"].concat()).await,
        "delete one of the three",
    )?;
    let old = ok(
        kv.range_req(&RangeRequest::prefix(&prefix).at_revision(then))
            .await,
        "read the prefix as it was",
    )?;
    let now = ok(
        kv.range_req(&RangeRequest::prefix(&prefix)).await,
        "read the prefix now",
    )?;
    let mut c = Check::new("a range read at an old revision");
    c.eq(
        "range(revision=then).kvs.keys()",
        (0..3)
            .map(|i| text(&[prefix.as_slice(), format!("{i:02}").as_bytes()].concat()))
            .collect::<Vec<_>>(),
        old.keys(),
    );
    c.eq("range(revision=then).count", 3, old.count);
    c.eq(
        "range().kvs.keys()",
        vec![
            text(&[prefix.as_slice(), b"00"].concat()),
            text(&[prefix.as_slice(), b"02"].concat()),
            text(&[prefix.as_slice(), b"03"].concat()),
        ],
        now.keys(),
    );
    c.finish()
});

dist_test!(historical_options, |ctx| {
    let kv = ctx.kv()?;
    let base = ctx.key("o");
    let revisions = put_series(kv, &base, 5).await?;
    let then = revisions.last().copied().unwrap_or(0);
    let prefix = [base.as_slice(), b"/"].concat();
    for i in 5..8 {
        let key = [prefix.as_slice(), format!("{i:02}").as_bytes()].concat();
        ok(kv.put(&key, b"late").await, "add a key after the snapshot")?;
    }
    let limited = ok(
        kv.range_req(
            &RangeRequest::prefix(&prefix)
                .at_revision(then)
                .sort("DESCEND", "KEY")
                .limit(2),
        )
        .await,
        "read the prefix as it was, largest key first, at most two",
    )?;
    let mut c = Check::new("limit and sorting over a historical range");
    // The limit and the sort apply to the old state, so the largest key is the largest of
    // the five that existed then, not of the eight that exist now.
    c.eq(
        "range(revision=then, DESCEND, limit 2).kvs.keys()",
        vec![
            text(&[prefix.as_slice(), b"04"].concat()),
            text(&[prefix.as_slice(), b"03"].concat()),
        ],
        limited.keys(),
    );
    c.eq(
        "range(revision=then, DESCEND, limit 2).count",
        5,
        limited.count,
    );
    c.that(
        "range(revision=then, DESCEND, limit 2).more",
        "true, because three of the five were left out",
        limited.more,
        limited.more,
    );
    c.finish()
});
