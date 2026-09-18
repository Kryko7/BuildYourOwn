//! Stage 23 — Range: reading one key.
//!
//! A point read is the smallest range there is, and it is where the three numbers every
//! later stage leans on come from. `create_revision` is stamped once and never moves,
//! `mod_revision` follows every put, and `version` counts the puts since creation — which
//! makes `version == 0` the canonical way to say "this key does not exist", the comparison
//! stage 29 builds compare-and-swap out of.
//!
//! The trap that catches most hand-written servers is the miss. A key that was never written
//! is not an error and not a 404: it is a perfectly ordinary answer with no `kvs` and a
//! `count` of zero, and because a zero-valued field may be omitted entirely, that answer
//! looks like nothing but a header. The other trap is that a point read is *exact*: a
//! request for `k` must not drag in `k/child` just because a range request is what carries
//! it.

use crate::assert::Check;
use crate::dist_test;
use crate::etcd::b64;
use crate::examples::{node_example, node_example_after, ExampleSpec};
use crate::stages::{ok, Ladder, Stage, Test};
use serde_json::json;

/// Stage 23.
pub fn stage() -> Stage {
    Stage {
        number: 23,
        slug: "range_one_key",
        name: "Range: reading one key",
        ext: false,
        ladder: Ladder::Node,
        hints: &[
            "POST /v3/kv/range with just a key is a point read",
            "A key that is not there answers no kvs and count 0 — not an error",
            "create_revision is set once, mod_revision moves with every put, version counts the puts",
            "count is the number of keys the range holds, whatever limit was asked for",
        ],
        examples,
        tests: vec![
            Test::new("a key that was never written is not an error", a_miss_is_not_an_error),
            Test::new("a key that was written reads back with its revisions", a_hit_carries_its_revisions),
            Test::new("version counts the puts since the key was created", version_counts_the_puts),
            Test::new("create_revision never moves while mod_revision does", create_stays_mod_moves),
            Test::new("the revision of a read is at least the revision of the write", read_is_not_older_than_the_write),
            Test::new("a key holding an empty value is still one key", an_empty_value_is_still_a_key),
            Test::new("two keys do not see each other", two_keys_are_separate),
            Test::new("a point read is exact, not a prefix", a_point_read_is_not_a_prefix),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        node_example_after(
            "A point read that hits",
            || {
                vec![(
                    "/v3/kv/put".into(),
                    json!({ "key": b64(b"one"), "value": b64(b"first") }),
                )]
            },
            "/v3/kv/range",
            || json!({ "key": b64(b"one") }),
        )
        .request("one key, base64, and nothing else")
        .response("one kv with its three revisions, and a count of 1")
        .note(
            "version is 1 after the first put, not 0: it counts the puts the key has seen \
             since it was created, which is why comparing version against 0 is how a \
             transaction asks whether a key exists at all.",
        ),
        node_example(
            "A point read that misses",
            "/v3/kv/range",
            || json!({ "key": b64(b"gone23") }),
        )
        .request("a key that was never written")
        .response("a header, and nothing else at all")
        .note(
            "This is a success, not an error. count is 0 and kvs is empty, and because a \
             field at its zero value may be omitted, both disappear from the body entirely.",
        ),
    ]
}

dist_test!(a_miss_is_not_an_error, |ctx| {
    let read = ok(
        ctx.kv()?.get_key(&ctx.key("missing")).await,
        "read a key that is not there",
    )?;
    let mut c = Check::new("a point read that misses");
    c.eq("range.count", 0, read.count);
    c.eq("range.kvs.len()", 0, read.kvs.len());
    c.at_least("range.header.revision", 1, read.header.revision);
    c.finish()
});

dist_test!(a_hit_carries_its_revisions, |ctx| {
    let kv = ctx.kv()?;
    let key = ctx.key("k");
    let put = ok(kv.put(&key, b"v").await, "put k")?;
    let read = ok(kv.get_key(&key).await, "read k back")?;
    let mut c = Check::new("a point read that hits");
    c.eq("range.count", 1, read.count);
    c.eq("range.kvs.len()", 1, read.kvs.len());
    let Some(pair) = read.one() else {
        return c.finish();
    };
    c.eq("range.kvs[0].key", key.clone(), pair.key.clone());
    c.eq("range.kvs[0].value", "v".to_string(), pair.value_str());
    c.eq(
        "range.kvs[0].create_revision",
        put.header.revision,
        pair.create_revision,
    );
    c.eq(
        "range.kvs[0].mod_revision",
        put.header.revision,
        pair.mod_revision,
    );
    c.eq("range.kvs[0].version", 1, pair.version);
    c.finish()
});

dist_test!(version_counts_the_puts, |ctx| {
    let kv = ctx.kv()?;
    let key = ctx.key("counted");
    let mut versions = Vec::new();
    for i in 1..=4 {
        ok(
            kv.put(&key, format!("v{i}").as_bytes()).await,
            "put the same key again",
        )?;
        let read = ok(kv.get_key(&key).await, "read it back")?;
        versions.push(read.one().map(|p| p.version).unwrap_or(-1));
    }
    let mut c = Check::new("four puts of one key");
    c.eq(
        "range.kvs[0].version after each put",
        vec![1, 2, 3, 4],
        versions,
    );
    let read = ok(kv.get_key(&key).await, "read the final value")?;
    c.eq(
        "range.kvs[0].value",
        "v4".to_string(),
        read.one().map(|p| p.value_str()).unwrap_or_default(),
    );
    c.eq("range.count", 1, read.count);
    c.finish()
});

dist_test!(create_stays_mod_moves, |ctx| {
    let kv = ctx.kv()?;
    let key = ctx.key("moving");
    let first = ok(kv.put(&key, b"a").await, "put the key the first time")?;
    let second = ok(kv.put(&key, b"b").await, "put it a second time")?;
    let third = ok(kv.put(&key, b"c").await, "put it a third time")?;
    let read = ok(kv.get_key(&key).await, "read it back")?;
    let mut c = Check::new("three puts of one key");
    let Some(pair) = read.one() else {
        return c
            .that(
                "range.kvs[0]",
                "the key that was written three times",
                false,
                (),
            )
            .finish();
    };
    // create_revision is stamped at the first put and is the key's identity afterwards;
    // mod_revision is the clock of the most recent one.
    c.eq(
        "range.kvs[0].create_revision",
        first.header.revision,
        pair.create_revision,
    );
    c.eq(
        "range.kvs[0].mod_revision",
        third.header.revision,
        pair.mod_revision,
    );
    c.ne(
        "range.kvs[0].mod_revision",
        second.header.revision,
        pair.mod_revision,
    );
    c.that(
        "range.kvs[0].create_revision < range.kvs[0].mod_revision",
        "a create_revision older than the last write",
        pair.create_revision < pair.mod_revision,
        (pair.create_revision, pair.mod_revision),
    );
    c.eq("range.kvs[0].version", 3, pair.version);
    c.finish()
});

dist_test!(read_is_not_older_than_the_write, |ctx| {
    let kv = ctx.kv()?;
    let key = ctx.key("fresh");
    let put = ok(kv.put(&key, b"v").await, "put the key")?;
    let read = ok(kv.get_key(&key).await, "read it back straight away")?;
    let miss = ok(
        kv.get_key(&ctx.key("never")).await,
        "read a key that is not there",
    )?;
    let mut c = Check::new("the header of a read that followed a write");
    // A read never reports a revision older than a write that has already been acknowledged;
    // a read that misses reports the same clock, because the clock is the store's, not the
    // key's.
    c.at_least(
        "range.header.revision",
        put.header.revision,
        read.header.revision,
    );
    c.at_least(
        "range.header.revision (a miss)",
        put.header.revision,
        miss.header.revision,
    );
    c.eq(
        "range.header.cluster_id",
        put.header.cluster_id,
        read.header.cluster_id,
    );
    c.finish()
});

dist_test!(an_empty_value_is_still_a_key, |ctx| {
    let kv = ctx.kv()?;
    let key = ctx.key("hollow");
    let put = ok(kv.put(&key, b"").await, "put a key with an empty value")?;
    let read = ok(kv.get_key(&key).await, "read it back")?;
    let mut c = Check::new("a key whose value is empty");
    // An empty value is a value: the key exists, count is 1, and only the `value` field is
    // gone from the body because a zero-valued field may be omitted.
    c.eq("range.count", 1, read.count);
    c.eq("range.kvs.len()", 1, read.kvs.len());
    c.eq(
        "range.kvs[0].value.len()",
        0,
        read.one().map(|p| p.value.len()).unwrap_or(999),
    );
    c.eq(
        "range.kvs[0].key",
        key.clone(),
        read.one().map(|p| p.key.clone()).unwrap_or_default(),
    );
    c.eq(
        "range.kvs[0].mod_revision",
        put.header.revision,
        read.one().map(|p| p.mod_revision).unwrap_or(0),
    );
    c.finish()
});

dist_test!(two_keys_are_separate, |ctx| {
    let kv = ctx.kv()?;
    let (a, b) = (ctx.key("a"), ctx.key("b"));
    ok(kv.put(&a, b"alpha").await, "put a")?;
    ok(kv.put(&b, b"beta").await, "put b")?;
    let read_a = ok(kv.get_key(&a).await, "read a")?;
    let read_b = ok(kv.get_key(&b).await, "read b")?;
    let mut c = Check::new("two keys written to one store");
    c.eq("range(a).count", 1, read_a.count);
    c.eq(
        "range(a).kvs.keys()",
        vec![String::from_utf8_lossy(&a).to_string()],
        read_a.keys(),
    );
    c.eq(
        "range(a).kvs[0].value",
        "alpha".to_string(),
        read_a.one().map(|p| p.value_str()).unwrap_or_default(),
    );
    c.eq("range(b).count", 1, read_b.count);
    c.eq(
        "range(b).kvs[0].value",
        "beta".to_string(),
        read_b.one().map(|p| p.value_str()).unwrap_or_default(),
    );
    c.finish()
});

dist_test!(a_point_read_is_not_a_prefix, |ctx| {
    let kv = ctx.kv()?;
    let short = ctx.key("k");
    let long = ctx.key("k/child");
    ok(kv.put(&short, b"parent").await, "put the short key")?;
    ok(kv.put(&long, b"child").await, "put the longer key")?;
    let read = ok(kv.get_key(&short).await, "point-read the short key")?;
    let mut c = Check::new("a point read of a key another key starts with");
    // A range request with no range_end is exactly one key, however many keys share its
    // bytes as a prefix. Answering both is the mistake a prefix-first implementation makes.
    c.eq("range.count", 1, read.count);
    c.eq(
        "range.kvs.keys()",
        vec![String::from_utf8_lossy(&short).to_string()],
        read.keys(),
    );
    c.eq(
        "range.kvs[0].value",
        "parent".to_string(),
        read.one().map(|p| p.value_str()).unwrap_or_default(),
    );
    c.finish()
});
