//! Stage 30 — Compaction and ErrCompacted.
//!
//! An MVCC store that keeps every revision for ever runs out of disk, so there has to be a
//! way to throw history away. Compaction is that way, and the interesting part is not the
//! forgetting but what the store says afterwards: a read of a revision that is gone is a
//! specific error, code 11, and not an empty answer. A reader that was paging through
//! history has to be able to tell "there was nothing there" from "there was something
//! there and you are too late".
//!
//! The other half is what compaction must never take: the newest version of a live key is
//! the store's current state however old the revision that wrote it is.

use crate::assert::Check;
use crate::dist_test;
use crate::etcd::{b64, RangeRequest, CODE_OUT_OF_RANGE};
use crate::examples::{node_example_after, ExampleSpec};
use crate::stages::{expect_code, ok, Ladder, Stage, Test};
use serde_json::{json, Value};

/// Stage 30.
pub fn stage() -> Stage {
    Stage {
        number: 30,
        slug: "compaction",
        name: "Compaction and ErrCompacted",
        ext: true,
        ladder: Ladder::Node,
        hints: &[
            "POST /v3/kv/compaction drops every revision below the one given",
            "A read below the compacted revision is code 11, not an empty answer",
            "Compaction never removes the newest version of a live key",
            "Compacting twice, or below what is already compacted, is an error, not a no-op",
        ],
        examples,
        tests: vec![
            Test::new(
                "a read below the compacted revision is code 11",
                read_below_is_out_of_range,
            ),
            Test::new(
                "a read at the compacted revision still works",
                read_at_the_boundary,
            ),
            Test::new(
                "a read above the compacted revision still works",
                read_above_the_boundary,
            ),
            Test::new(
                "the newest version of a live key survives compaction",
                live_keys_survive,
            ),
            Test::new(
                "compacting to a revision above the current one is an error",
                compact_into_the_future,
            ),
            Test::new(
                "compacting twice to the same revision is an error",
                compact_twice,
            ),
            Test::new(
                "a physical compaction answers a header",
                physical_compaction,
            ),
            Test::new(
                "the store keeps writing after a compaction",
                writes_continue,
            ),
            Test::new(
                "a compaction does not move the revision",
                compaction_is_not_a_write,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    fn two_puts() -> Vec<(String, Value)> {
        vec![
            (
                "/v3/kv/put".to_string(),
                json!({"key": b64(b"hist"), "value": b64(b"v1")}),
            ),
            (
                "/v3/kv/put".to_string(),
                json!({"key": b64(b"hist"), "value": b64(b"v2")}),
            ),
        ]
    }
    vec![
        node_example_after(
            "Throwing away everything below revision 3",
            two_puts,
            "/v3/kv/compaction",
            || json!({ "revision": "3" }),
        )
        .request("compact up to revision 3, after two writes have happened")
        .response("a bare header: compaction answers when it happened, not what it removed")
        .note(
            "Compaction is not a write. The revision in this header is the store's clock as \
             it already was; nothing was appended to make room.",
        ),
        node_example_after(
            "Reading a revision that has been compacted away",
            || {
                let mut v = two_puts();
                v.push(("/v3/kv/compaction".to_string(), json!({ "revision": "3" })));
                v
            },
            "/v3/kv/range",
            || json!({ "key": b64(b"hist"), "revision": "2" }),
        )
        .request("a point read as of revision 2, which compaction has just dropped")
        .response("HTTP 400 and code 11, `mvcc: required revision has been compacted`")
        .note(
            "An empty answer would be a lie: there was a value at revision 2, and the store \
             no longer knows it. Code 11 is how a paging reader learns it has fallen behind \
             and must start again from the current revision.",
        ),
    ]
}

/// Write one key `n` times and hand back the revision of each write.
async fn history(
    ctx: &mut crate::stages::Ctx,
    key: &[u8],
    n: usize,
) -> Result<Vec<i64>, crate::assert::Failure> {
    let kv = ctx.kv()?;
    let mut revisions = Vec::new();
    for i in 0..n {
        let put = ok(
            kv.put(key, format!("v{i}").as_bytes()).await,
            "write one version of the key",
        )?;
        revisions.push(put.header.revision);
    }
    Ok(revisions)
}

dist_test!(read_below_is_out_of_range, |ctx| {
    let key = ctx.key("hist");
    let revs = history(ctx, &key, 3).await?;
    let (first, last) = (revs[0], revs[2]);
    let kv = ctx.kv()?;
    // The oldest revision is readable right up until the compaction takes it.
    let before = ok(
        kv.range_req(&RangeRequest::key(&key).at_revision(first))
            .await,
        "read the key as it was at its first revision",
    )?;
    ok(
        kv.compact(last, false).await,
        "compact up to the newest revision",
    )?;
    let e = expect_code(
        kv.range_req(&RangeRequest::key(&key).at_revision(first))
            .await,
        CODE_OUT_OF_RANGE,
        "a read of a revision that has been compacted away",
    )?;
    let mut c = Check::new("history below a compaction");
    c.eq(
        "range(before compaction).kvs[0].value",
        "v0".to_string(),
        before.one().map(|k| k.value_str()).unwrap_or_default(),
    );
    c.that(
        "error.message",
        "a message saying the revision has been compacted",
        e.body().to_lowercase().contains("compact"),
        e.body(),
    );
    c.finish()
});

dist_test!(read_at_the_boundary, |ctx| {
    let key = ctx.key("edge");
    let revs = history(ctx, &key, 3).await?;
    let middle = revs[1];
    let kv = ctx.kv()?;
    ok(
        kv.compact(middle, false).await,
        "compact up to the middle revision",
    )?;
    let read = ok(
        kv.range_req(&RangeRequest::key(&key).at_revision(middle))
            .await,
        "read the key at exactly the compacted revision",
    )?;
    let mut c = Check::new("the compacted revision itself");
    // The boundary is inclusive: revision N survives a compaction to N, and only what is
    // strictly below it is gone.
    c.eq("range.count", 1, read.count);
    c.eq(
        "range.kvs[0].value",
        "v1".to_string(),
        read.one().map(|k| k.value_str()).unwrap_or_default(),
    );
    c.eq(
        "range.kvs[0].mod_revision",
        middle,
        read.one().map(|k| k.mod_revision).unwrap_or(0),
    );
    c.finish()
});

dist_test!(read_above_the_boundary, |ctx| {
    let key = ctx.key("above");
    let revs = history(ctx, &key, 3).await?;
    let (middle, last) = (revs[1], revs[2]);
    let kv = ctx.kv()?;
    ok(
        kv.compact(middle, false).await,
        "compact up to the middle revision",
    )?;
    let at_last = ok(
        kv.range_req(&RangeRequest::key(&key).at_revision(last))
            .await,
        "read the key at the newest revision",
    )?;
    let now = ok(
        kv.get_key(&key).await,
        "read the key with no revision at all",
    )?;
    let mut c = Check::new("everything above the compacted revision");
    c.eq(
        "range(at last).kvs[0].value",
        "v2".to_string(),
        at_last.one().map(|k| k.value_str()).unwrap_or_default(),
    );
    c.eq(
        "range(now).kvs[0].value",
        "v2".to_string(),
        now.one().map(|k| k.value_str()).unwrap_or_default(),
    );
    c.eq(
        "range(now).kvs[0].version",
        3,
        now.one().map(|k| k.version).unwrap_or(0),
    );
    c.finish()
});

dist_test!(live_keys_survive, |ctx| {
    let old = ctx.key("ancient");
    let busy = ctx.key("busy");
    let kv = ctx.kv()?;
    let born = ok(
        kv.put(&old, b"still here").await,
        "write a key and then leave it alone",
    )?;
    // Push the store's clock a long way past the revision that wrote it.
    for i in 0..30 {
        ok(
            kv.put(&busy, format!("v{i}").as_bytes()).await,
            "write a different key over and over",
        )?;
    }
    let head = ok(kv.get_key(&busy).await, "find the current revision")?;
    ok(
        kv.compact(head.header.revision, false).await,
        "compact everything below the current revision",
    )?;
    let read = ok(kv.get_key(&old).await, "read the untouched key back")?;
    let mut c = Check::new("a key whose only version is older than the compaction");
    c.eq("range.count", 1, read.count);
    c.eq(
        "range.kvs[0].value",
        "still here".to_string(),
        read.one().map(|k| k.value_str()).unwrap_or_default(),
    );
    // Its revisions are the ones it was written at, not the compaction's.
    c.eq(
        "range.kvs[0].create_revision",
        born.header.revision,
        read.one().map(|k| k.create_revision).unwrap_or(0),
    );
    c.eq(
        "range.kvs[0].mod_revision",
        born.header.revision,
        read.one().map(|k| k.mod_revision).unwrap_or(0),
    );
    c.finish()
});

dist_test!(compact_into_the_future, |ctx| {
    let key = ctx.key("future");
    let revs = history(ctx, &key, 2).await?;
    let last = revs[1];
    let kv = ctx.kv()?;
    expect_code(
        kv.compact(last + 50, false).await,
        CODE_OUT_OF_RANGE,
        "a compaction to a revision the store has not reached",
    )?;
    let read = ok(
        kv.get_key(&key).await,
        "read the key after the refused compaction",
    )?;
    let still_old = ok(
        kv.range_req(&RangeRequest::key(&key).at_revision(revs[0]))
            .await,
        "read the oldest revision, which nothing was allowed to drop",
    )?;
    let mut c = Check::new("a compaction that names a revision from the future");
    c.eq("range.count", 1, read.count);
    c.eq(
        "range(at the first revision).kvs[0].value",
        "v0".to_string(),
        still_old.one().map(|k| k.value_str()).unwrap_or_default(),
    );
    c.finish()
});

dist_test!(compact_twice, |ctx| {
    let key = ctx.key("twice");
    let revs = history(ctx, &key, 3).await?;
    let middle = revs[1];
    let kv = ctx.kv()?;
    ok(
        kv.compact(middle, false).await,
        "compact up to the middle revision",
    )?;
    // Compaction is not idempotent: the second call names a revision that is already gone,
    // and that is the same error as reading one.
    expect_code(
        kv.compact(middle, false).await,
        CODE_OUT_OF_RANGE,
        "a second compaction to the same revision",
    )?;
    expect_code(
        kv.compact(revs[0], false).await,
        CODE_OUT_OF_RANGE,
        "a compaction to a revision below the one already compacted",
    )?;
    let read = ok(kv.get_key(&key).await, "read the key after both refusals")?;
    let mut c = Check::new("compacting the same revision twice");
    c.eq(
        "range.kvs[0].value",
        "v2".to_string(),
        read.one().map(|k| k.value_str()).unwrap_or_default(),
    );
    c.finish()
});

dist_test!(physical_compaction, |ctx| {
    let key = ctx.key("phys");
    let revs = history(ctx, &key, 3).await?;
    let last = revs[2];
    let kv = ctx.kv()?;
    let header = ok(
        kv.compact(last, true).await,
        "compact with physical set, which waits for the space to be reclaimed",
    )?;
    let read = ok(
        kv.get_key(&key).await,
        "read the key after the physical compaction",
    )?;
    let mut c = Check::new("physical: true");
    c.at_least("compaction.header.revision", last, header.revision);
    c.ne("compaction.header.cluster_id", 0, header.cluster_id);
    c.eq(
        "range.kvs[0].value",
        "v2".to_string(),
        read.one().map(|k| k.value_str()).unwrap_or_default(),
    );
    expect_code(
        kv.range_req(&RangeRequest::key(&key).at_revision(revs[0]))
            .await,
        CODE_OUT_OF_RANGE,
        "a read of history the physical compaction dropped",
    )?;
    c.finish()
});

dist_test!(writes_continue, |ctx| {
    let key = ctx.key("after");
    let revs = history(ctx, &key, 2).await?;
    let last = revs[1];
    let kv = ctx.kv()?;
    ok(
        kv.compact(last, false).await,
        "compact up to the newest revision",
    )?;
    let next = ok(
        kv.put(&key, b"post-compaction").await,
        "write the key again",
    )?;
    let other = ok(
        kv.put(&ctx.key("fresh"), b"new key").await,
        "write a key that did not exist",
    )?;
    let read = ok(ctx.kv()?.get_key(&key).await, "read the compacted key back")?;
    let mut c = Check::new("the store after a compaction");
    c.eq("put(after).header.revision", last + 1, next.header.revision);
    c.eq(
        "put(fresh).header.revision",
        last + 2,
        other.header.revision,
    );
    c.eq(
        "range.kvs[0].value",
        "post-compaction".to_string(),
        read.one().map(|k| k.value_str()).unwrap_or_default(),
    );
    // version counts every put the key has ever seen, and compaction is not a put.
    c.eq(
        "range.kvs[0].version",
        3,
        read.one().map(|k| k.version).unwrap_or(0),
    );
    c.finish()
});

dist_test!(compaction_is_not_a_write, |ctx| {
    let key = ctx.key("clock");
    let revs = history(ctx, &key, 2).await?;
    let last = revs[1];
    let kv = ctx.kv()?;
    let before = ok(kv.status().await, "read the revision before compacting")?;
    ok(
        kv.compact(last, false).await,
        "compact up to the newest revision",
    )?;
    let after = ok(kv.status().await, "read the revision after compacting")?;
    let mut c = Check::new("the store's clock across a compaction");
    c.eq(
        "status(before).header.revision",
        last,
        before.header.revision,
    );
    c.eq("status(after).header.revision", last, after.header.revision);
    c.finish()
});
