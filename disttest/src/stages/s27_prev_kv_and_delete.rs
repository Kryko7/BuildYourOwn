//! Stage 27 — prev_kv and deleting ranges.
//!
//! A put and a delete both answer with a header and almost nothing else, which is fine until
//! something needs to know what was there before. `prev_kv` is that something: it turns a
//! blind write into a swap, and it is what a watch hands a subscriber that needs the old
//! value as well as the new one.
//!
//! The delete side is where the counting traps are. A `deleterange` reports how many keys it
//! removed, and that number is a fact about the store, not about the request: deleting a key
//! that was not there removes zero and — this is the part that is easy to miss — **does not
//! move the revision**, because nothing happened. A delete that does remove keys moves the
//! revision exactly once, however many keys went with it, because one request is one step of
//! the store's clock.

use crate::assert::Check;
use crate::dist_test;
use crate::etcd::{b64, prefix_end, PutRequest, RangeRequest};
use crate::examples::{node_example_after, ExampleSpec};
use crate::stages::{ok, text, Ladder, Stage, Test};
use serde_json::{json, Value};

/// Stage 27.
pub fn stage() -> Stage {
    Stage {
        number: 27,
        slug: "prev_kv_and_delete",
        name: "prev_kv and deleting ranges",
        ext: false,
        ladder: Ladder::Node,
        hints: &[
            "prev_kv on a put answers the pair as it was before the write, or nothing when it is new",
            "POST /v3/kv/deleterange answers how many keys it removed",
            "A delete over a range removes every key in it and counts them all",
            "Deleting a key that is not there is a successful request that deleted zero keys",
        ],
        examples,
        tests: vec![
            Test::new("prev_kv on a put of a key that existed answers the old pair", prev_kv_on_an_overwrite),
            Test::new("prev_kv on a put of a new key answers nothing", prev_kv_on_a_new_key),
            Test::new("a delete answers how many keys it removed", delete_counts_what_it_removed),
            Test::new("deleting a key that is not there deletes zero and moves nothing", deleting_nothing),
            Test::new("deleting a range removes every key in it", delete_over_a_range),
            Test::new("prev_kv on a delete answers everything that was removed", prev_kv_on_a_delete),
            Test::new("a delete moves the revision exactly once", one_delete_is_one_revision),
            Test::new("after a delete the key reads back as absent", the_key_is_gone),
            Test::new("a key put again after a delete starts a new life", a_second_life).ext(),
        ],
    }
}

/// One key with a value already in it, so `prev_kv` has something to answer with.
fn one_existing_key() -> Vec<(String, Value)> {
    vec![(
        "/v3/kv/put".into(),
        json!({"key": b64(b"pk"), "value": b64(b"before")}),
    )]
}

/// Three keys under `d/`, for a delete that removes more than one.
fn three_keys() -> Vec<(String, Value)> {
    (1..=3)
        .map(|i| {
            (
                "/v3/kv/put".to_string(),
                json!({
                    "key": b64(format!("d/{i}").as_bytes()),
                    "value": b64(format!("v{i}").as_bytes()),
                }),
            )
        })
        .collect()
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        node_example_after(
            "A put that asks what was there before",
            one_existing_key,
            "/v3/kv/put",
            || json!({ "key": b64(b"pk"), "value": b64(b"after"), "prev_kv": true }),
        )
        .request("a write of `pk`, and the pair it is replacing")
        .response("the usual header, plus the pair as it stood before this write")
        .note(
            "prev_kv is the pair as it was, revisions and all — not the value alone. On a \
             key that did not exist the field is simply absent, which is how a caller tells \
             a create from an overwrite without a second round trip.",
        ),
        node_example_after(
            "A delete over a range",
            three_keys,
            "/v3/kv/deleterange",
            || json!({ "key": b64(b"d/"), "range_end": b64(&prefix_end(b"d/")), "prev_kv": true }),
        )
        .request("everything under `d/`, and what it held")
        .response("a count of three, the three pairs that were removed, and one new revision")
        .note(
            "One request is one revision however many keys it removed. A server that stamps \
             a revision per key makes the store's clock depend on how a client happened to \
             batch its deletes.",
        ),
    ]
}

dist_test!(prev_kv_on_an_overwrite, |ctx| {
    let kv = ctx.kv()?;
    let key = ctx.key("k");
    let first = ok(kv.put(&key, b"before").await, "put the first value")?;
    let second = ok(
        kv.put_req(&PutRequest::new(&key, b"after").prev_kv()).await,
        "overwrite it, asking for what was there",
    )?;
    let mut c = Check::new("prev_kv on a put over an existing key");
    let Some(prev) = &second.prev_kv else {
        return c
            .that(
                "put.prev_kv",
                "the pair as it was before the write",
                false,
                (),
            )
            .finish();
    };
    c.eq("put.prev_kv.key", key.clone(), prev.key.clone());
    c.eq("put.prev_kv.value", "before".to_string(), prev.value_str());
    // prev_kv is the whole pair as it stood, so its revisions are the old ones.
    c.eq(
        "put.prev_kv.mod_revision",
        first.header.revision,
        prev.mod_revision,
    );
    c.eq(
        "put.prev_kv.create_revision",
        first.header.revision,
        prev.create_revision,
    );
    c.eq("put.prev_kv.version", 1, prev.version);
    c.eq(
        "range().kvs[0].value",
        "after".to_string(),
        ok(kv.get_key(&key).await, "read the key back")?
            .one()
            .map(|p| p.value_str())
            .unwrap_or_default(),
    );
    c.finish()
});

dist_test!(prev_kv_on_a_new_key, |ctx| {
    let kv = ctx.kv()?;
    let key = ctx.key("brand/new");
    let put = ok(
        kv.put_req(&PutRequest::new(&key, b"v").prev_kv()).await,
        "put a key that did not exist, asking for what was there",
    )?;
    let mut c = Check::new("prev_kv on a put of a key that did not exist");
    // Nothing, not an empty pair: an empty pair would be indistinguishable from a key that
    // really did hold an empty value.
    c.that(
        "put.prev_kv",
        "nothing at all, because the key is new",
        put.prev_kv.is_none(),
        put.prev_kv.as_ref().map(|p| (p.key_str(), p.value_str())),
    );
    c.at_least("put.header.revision", 1, put.header.revision);
    c.finish()
});

dist_test!(delete_counts_what_it_removed, |ctx| {
    let kv = ctx.kv()?;
    let key = ctx.key("k");
    ok(kv.put(&key, b"v").await, "put a key")?;
    let delete = ok(kv.delete(&key).await, "delete it")?;
    let mut c = Check::new("a delete of one key that was there");
    c.eq("deleterange.deleted", 1, delete.deleted);
    c.at_least("deleterange.header.revision", 1, delete.header.revision);
    c.that(
        "deleterange.prev_kvs",
        "empty, because prev_kv was not asked for",
        delete.prev_kvs.is_empty(),
        delete.prev_kvs.len(),
    );
    c.finish()
});

dist_test!(deleting_nothing, |ctx| {
    let kv = ctx.kv()?;
    let anchor = ok(
        kv.put(&ctx.key("anchor"), b"v").await,
        "put an unrelated key",
    )?;
    let delete = ok(
        kv.delete(&ctx.key("never/written")).await,
        "delete a key that is not there",
    )?;
    let after = ok(kv.status().await, "ask where the clock is now")?;
    let mut c = Check::new("a delete that removed nothing");
    c.eq("deleterange.deleted", 0, delete.deleted);
    // A request that changed nothing is not a write, so the clock does not move for it.
    c.eq(
        "deleterange.header.revision",
        anchor.header.revision,
        delete.header.revision,
    );
    c.eq(
        "status.header.revision after the delete",
        anchor.header.revision,
        after.header.revision,
    );
    c.finish()
});

dist_test!(delete_over_a_range, |ctx| {
    let kv = ctx.kv()?;
    let prefix = ctx.key("d/");
    let inside = [ctx.key("d/1"), ctx.key("d/2"), ctx.key("d/3")];
    for (i, k) in inside.iter().enumerate() {
        ok(
            kv.put(k, format!("v{i}").as_bytes()).await,
            "put a key inside the range",
        )?;
    }
    let outside = ctx.key("keep");
    ok(
        kv.put(&outside, b"kept").await,
        "put a key outside the range",
    )?;
    let delete = ok(
        kv.delete_req(&json!({
            "key": b64(&prefix),
            "range_end": b64(&prefix_end(&prefix)),
        }))
        .await,
        "delete everything under the prefix",
    )?;
    let left = ok(
        kv.range_req(&RangeRequest::all()).await,
        "read what is left",
    )?;
    let mut c = Check::new("a delete over a range");
    c.eq("deleterange.deleted", 3, delete.deleted);
    c.eq(
        "range.kvs.keys() after the delete",
        vec![text(&outside)],
        left.keys(),
    );
    c.eq("range.count after the delete", 1, left.count);
    c.finish()
});

dist_test!(prev_kv_on_a_delete, |ctx| {
    let kv = ctx.kv()?;
    let prefix = ctx.key("p/");
    let inside = [ctx.key("p/a"), ctx.key("p/b")];
    let mut revisions = Vec::new();
    for (i, k) in inside.iter().enumerate() {
        revisions.push(
            ok(kv.put(k, format!("v{i}").as_bytes()).await, "put a key")?
                .header
                .revision,
        );
    }
    let delete = ok(
        kv.delete_req(&json!({
            "key": b64(&prefix),
            "range_end": b64(&prefix_end(&prefix)),
            "prev_kv": true,
        }))
        .await,
        "delete the prefix, asking for what was removed",
    )?;
    let mut c = Check::new("prev_kv on a delete over a range");
    c.eq("deleterange.deleted", 2, delete.deleted);
    c.eq("deleterange.prev_kvs.len()", 2, delete.prev_kvs.len());
    // Everything that was removed, in key order, each with the value it held.
    c.eq(
        "deleterange.prev_kvs.keys()",
        inside.iter().map(|k| text(k)).collect::<Vec<_>>(),
        delete
            .prev_kvs
            .iter()
            .map(|p| p.key_str())
            .collect::<Vec<_>>(),
    );
    c.eq(
        "deleterange.prev_kvs.values()",
        vec!["v0".to_string(), "v1".to_string()],
        delete
            .prev_kvs
            .iter()
            .map(|p| p.value_str())
            .collect::<Vec<_>>(),
    );
    c.eq(
        "deleterange.prev_kvs[*].mod_revision",
        revisions.clone(),
        delete
            .prev_kvs
            .iter()
            .map(|p| p.mod_revision)
            .collect::<Vec<_>>(),
    );
    c.finish()
});

dist_test!(one_delete_is_one_revision, |ctx| {
    let kv = ctx.kv()?;
    let prefix = ctx.key("many/");
    let mut puts = Vec::new();
    for i in 0..6 {
        let key = [prefix.as_slice(), format!("{i:02}").as_bytes()].concat();
        puts.push(ok(kv.put(&key, b"v").await, "put a key")?.header.revision);
    }
    let last_put = puts.last().copied().unwrap_or(0);
    let delete = ok(
        kv.delete_req(&json!({
            "key": b64(&prefix),
            "range_end": b64(&prefix_end(&prefix)),
        }))
        .await,
        "delete all six in one request",
    )?;
    let mut c = Check::new("the revision a multi-key delete costs");
    c.eq("deleterange.deleted", 6, delete.deleted);
    // Exactly one, not one per key: the request is the step, not the key.
    c.eq(
        "deleterange.header.revision - the revision of the last put",
        1,
        delete.header.revision - last_put,
    );
    c.finish()
});

dist_test!(the_key_is_gone, |ctx| {
    let kv = ctx.kv()?;
    let key = ctx.key("k");
    ok(kv.put(&key, b"v").await, "put a key")?;
    ok(kv.delete(&key).await, "delete it")?;
    let read = ok(kv.get_key(&key).await, "read it back")?;
    let mut c = Check::new("a key read after it was deleted");
    // The same shape as a key that was never written: a miss is a miss.
    c.eq("range.count", 0, read.count);
    c.eq("range.kvs.len()", 0, read.kvs.len());
    c.at_least("range.header.revision", 1, read.header.revision);
    c.finish()
});

dist_test!(a_second_life, |ctx| {
    let kv = ctx.kv()?;
    let key = ctx.key("phoenix");
    ok(kv.put(&key, b"first").await, "put the key")?;
    ok(kv.put(&key, b"second").await, "put it again")?;
    ok(kv.delete(&key).await, "delete it")?;
    let reborn = ok(
        kv.put(&key, b"third").await,
        "put it a third time, after the delete",
    )?;
    let read = ok(kv.get_key(&key).await, "read it back")?;
    let mut c = Check::new("a key written again after it was deleted");
    let Some(pair) = read.one() else {
        return c
            .that("range.kvs[0]", "the key that was written again", false, ())
            .finish();
    };
    // A delete ends the key's life, so what comes back is a new key that happens to share a
    // name: version restarts at 1 and create_revision is the revision that recreated it.
    c.eq("range.kvs[0].version", 1, pair.version);
    c.eq(
        "range.kvs[0].create_revision",
        reborn.header.revision,
        pair.create_revision,
    );
    c.eq(
        "range.kvs[0].mod_revision",
        reborn.header.revision,
        pair.mod_revision,
    );
    c.eq("range.kvs[0].value", "third".to_string(), pair.value_str());
    c.finish()
});
