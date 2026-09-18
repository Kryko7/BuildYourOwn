//! Stage 22 — Put and the response header.
//!
//! The first write. Everything later in the track hangs off the header this stage checks:
//! the revision is the store's clock, and every test that says "and then it was visible"
//! says it in terms of a revision.

use crate::assert::Check;
use crate::dist_test;
use crate::etcd::{b64, PutRequest};
use crate::examples::{node_example, ExampleSpec};
use crate::stages::{check_strictly_increasing, describe, ok, Ladder, Stage, Test};
use serde_json::json;

/// Stage 22.
pub fn stage() -> Stage {
    Stage {
        number: 22,
        slug: "put_header",
        name: "Put and the response header",
        ext: false,
        ladder: Ladder::Node,
        hints: &[
            "POST /v3/kv/put with base64 key and value, answering a header",
            "Every write increments the store revision by one, and the header reports the new value",
            "64-bit numbers are JSON strings; a field at its zero value may be left out entirely",
            "raft_term is in the header too, and it never goes backwards",
        ],
        examples,
        tests: vec![
            Test::new("a put answers with a header", answers_with_a_header),
            Test::new("the revision increases by exactly one", revision_increases_by_one),
            Test::new("the revision is the same for every member id", header_is_self_consistent),
            Test::new("writing the same key again still moves the revision", rewriting_moves_the_revision),
            Test::new("an empty value is a value", empty_value),
            Test::new("a key with awkward bytes survives the round trip", awkward_bytes),
            Test::new("a large value is stored whole", large_value).ext(),
            Test::new("many writes produce one increasing sequence", many_writes),
            Test::new("the header travels on every endpoint", header_everywhere).ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        node_example("A put and the header it answers with", "/v3/kv/put", || {
            json!({ "key": b64(b"foo"), "value": b64(b"bar") })
        })
        .request("`foo` set to `bar`, both base64")
        .response("a header whose revision is the store's clock after the write")
        .note(
            "There is no body beyond the header: a put tells you when it happened, not what \
             it wrote. Ask for `prev_kv` when you want the value that was there before.",
        ),
        node_example("Reading the write back", "/v3/kv/range", || {
            json!({ "key": b64(b"foo") })
        })
        .request("a point read of the same key")
        .response("one kv with create_revision, mod_revision and version, and count 1")
        .note(
            "create_revision is set once and never moves; mod_revision follows every put; \
             version counts the puts since the key was created, so it is 1 after the first.",
        ),
    ]
}

dist_test!(answers_with_a_header, |ctx| {
    let key = ctx.key("k");
    let put = ok(ctx.kv()?.put(&key, b"v").await, "put the first key")?;
    let mut c = Check::new("the header of a put");
    c.at_least("put.header.revision", 1, put.header.revision);
    c.ne("put.header.cluster_id", 0, put.header.cluster_id);
    c.ne("put.header.member_id", 0, put.header.member_id);
    c.at_least("put.header.raft_term", 1, put.header.raft_term);
    c.that(
        "put.prev_kv",
        "nothing, because the key was new and prev_kv was not asked for",
        put.prev_kv.is_none(),
        put.prev_kv.as_ref().map(|kv| kv.value_str()),
    );
    c.finish()
});

dist_test!(revision_increases_by_one, |ctx| {
    let kv = ctx.kv()?;
    let first = ok(kv.put(&ctx.key("a"), b"1").await, "put a")?;
    let second = ok(kv.put(&ctx.key("b"), b"2").await, "put b")?;
    let third = ok(kv.put(&ctx.key("c"), b"3").await, "put c")?;
    let mut c = Check::new("three puts in a row");
    c.eq(
        "put(b).header.revision - put(a).header.revision",
        1,
        second.header.revision - first.header.revision,
    );
    c.eq(
        "put(c).header.revision - put(b).header.revision",
        1,
        third.header.revision - second.header.revision,
    );
    c.finish()
});

dist_test!(header_is_self_consistent, |ctx| {
    let kv = ctx.kv()?;
    let put = ok(kv.put(&ctx.key("k"), b"v").await, "put k")?;
    let range = ok(kv.get_key(&ctx.key("k")).await, "read k back")?;
    let status = ok(kv.status().await, "ask for the status")?;
    let mut c = Check::new("the same header from three endpoints");
    c.eq(
        "range.header.cluster_id",
        put.header.cluster_id,
        range.header.cluster_id,
    );
    c.eq(
        "range.header.member_id",
        put.header.member_id,
        range.header.member_id,
    );
    c.eq(
        "status.header.cluster_id",
        put.header.cluster_id,
        status.header.cluster_id,
    );
    c.at_least(
        "range.header.revision",
        put.header.revision,
        range.header.revision,
    );
    c.finish()
});

dist_test!(rewriting_moves_the_revision, |ctx| {
    let kv = ctx.kv()?;
    let key = ctx.key("k");
    let first = ok(kv.put(&key, b"one").await, "put k=one")?;
    let second = ok(kv.put(&key, b"two").await, "put k=two")?;
    let read = ok(kv.get_key(&key).await, "read k")?;
    let mut c = Check::new("writing one key twice");
    c.eq(
        "second put.header.revision",
        first.header.revision + 1,
        second.header.revision,
    );
    let Some(kv_pair) = read.one() else {
        return c
            .that("range.kvs[0]", "the key that was just written twice", false, ())
            .finish();
    };
    c.eq("range.kvs[0].value", "two".to_string(), kv_pair.value_str());
    c.eq(
        "range.kvs[0].create_revision",
        first.header.revision,
        kv_pair.create_revision,
    );
    c.eq(
        "range.kvs[0].mod_revision",
        second.header.revision,
        kv_pair.mod_revision,
    );
    c.eq("range.kvs[0].version", 2, kv_pair.version);
    c.finish()
});

dist_test!(empty_value, |ctx| {
    let kv = ctx.kv()?;
    let key = ctx.key("empty");
    ok(kv.put(&key, b"").await, "put an empty value")?;
    let read = ok(kv.get_key(&key).await, "read the empty value back")?;
    let mut c = Check::new("a key holding an empty value");
    c.eq("range.count", 1, read.count);
    c.eq(
        "range.kvs[0].value.len()",
        0,
        read.one().map(|k| k.value.len()).unwrap_or(999),
    );
    c.eq(
        "range.kvs[0].version",
        1,
        read.one().map(|k| k.version).unwrap_or(0),
    );
    c.finish()
});

dist_test!(awkward_bytes, |ctx| {
    let kv = ctx.kv()?;
    // Bytes that break anything treating a key as a C string, a UTF-8 string or a path.
    let key = [ctx.key("odd").as_slice(), &[0x00, 0xff, b'/', b'\n', 0x80]].concat();
    let value = vec![0x00u8, 0x01, 0xfe, 0xff, b'"', b'\\'];
    ok(kv.put(&key, &value).await, "put a key with awkward bytes")?;
    let read = ok(kv.get_key(&key).await, "read it back")?;
    let mut c = Check::new("a key and value that are not text");
    c.eq("range.count", 1, read.count);
    match read.one() {
        Some(pair) => {
            c.that(
                "range.kvs[0].key",
                "the exact bytes that were written",
                pair.key == key,
                String::from_utf8_lossy(&pair.key).to_string(),
            );
            c.that(
                "range.kvs[0].value",
                "the exact bytes that were written",
                pair.value == value,
                pair.value.clone(),
            );
        }
        None => {
            c.that("range.kvs[0]", "the key that was written", false, ());
        }
    }
    c.finish()
});

dist_test!(large_value, |ctx| {
    let kv = ctx.kv()?;
    let key = ctx.key("large");
    let value: Vec<u8> = (0..64 * 1024).map(|i| (i % 251) as u8).collect();
    ok(kv.put(&key, &value).await, "put a 64 KiB value")?;
    let read = ok(kv.get_key(&key).await, "read the 64 KiB value back")?;
    let mut c = Check::new("a value of 64 KiB");
    c.eq(
        "range.kvs[0].value.len()",
        value.len(),
        read.one().map(|k| k.value.len()).unwrap_or(0),
    );
    c.that(
        "range.kvs[0].value",
        "the same 64 KiB, byte for byte",
        read.one().map(|k| k.value.as_slice()) == Some(value.as_slice()),
        read.one().map(|k| k.value.len()),
    );
    c.finish()
});

dist_test!(many_writes, |ctx| {
    let kv = ctx.kv()?;
    let mut revisions = Vec::new();
    for i in 0..20 {
        let key = ctx.key(&format!("seq{i:02}"));
        revisions.push(
            ok(kv.put(&key, format!("v{i}").as_bytes()).await, "put in a loop")?
                .header
                .revision,
        );
    }
    let mut c = Check::new("twenty writes in a row");
    check_strictly_increasing(&mut c, "revisions", &revisions);
    let span = revisions.last().copied().unwrap_or(0) - revisions.first().copied().unwrap_or(0);
    c.eq("last revision - first revision", 19, span);
    c.finish()
});

dist_test!(header_everywhere, |ctx| {
    let kv = ctx.kv()?;
    let key = ctx.key("k");
    let put = ok(kv.put(&key, b"v").await, "put k")?;
    let delete = ok(kv.delete(&key).await, "delete k")?;
    let txn = ok(kv.txn(vec![], vec![], vec![]).await, "an empty transaction")?;
    let put_again = ok(
        kv.put_req(&PutRequest::new(&key, b"v2").prev_kv()).await,
        "put k again",
    )?;
    let mut c = Check::new("every endpoint answers with a header");
    c.at_least(
        "deleterange.header.revision",
        put.header.revision + 1,
        delete.header.revision,
    );
    c.at_least(
        "txn.header.revision",
        delete.header.revision,
        txn.header.revision,
    );
    c.at_least(
        "put.header.revision",
        txn.header.revision,
        put_again.header.revision,
    );
    c.eq(
        "txn.header.cluster_id",
        put.header.cluster_id,
        txn.header.cluster_id,
    );
    c.that(
        "put(prev_kv).prev_kv",
        "nothing, because the key had been deleted",
        put_again.prev_kv.is_none(),
        put_again.prev_kv.as_ref().map(|k| k.value_str()),
    );
    if let Err(e) = kv.version().await {
        return Err(describe(e, "GET /version"));
    }
    c.finish()
});
