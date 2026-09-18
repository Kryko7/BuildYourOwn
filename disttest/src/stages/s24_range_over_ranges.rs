//! Stage 24 — Ranges, prefixes and the whole store.
//!
//! One key becomes many the moment `range_end` appears, and the whole of the rest of the API
//! is built on the same half-open interval: a prefix, a delete over a range, the span a watch
//! covers. Getting `[key, range_end)` right once gets all of them right.
//!
//! There are two traps. The first is the end itself: the interval excludes it, so a prefix
//! range ends at the prefix with its **last byte incremented**, not at the prefix followed by
//! `0xff` and not at the prefix itself. The second is the order. Keys are byte strings, not
//! text, so the answer is sorted by unsigned byte value: `B` before `a`, and `0x80` after
//! `0x7f` rather than before everything, which is where a language that compares bytes as
//! signed quietly produces the wrong answer.

use crate::assert::Check;
use crate::dist_test;
use crate::etcd::{b64, prefix_end, RangeRequest};
use crate::examples::{node_example_after, ExampleSpec};
use crate::stages::{ok, text, Ladder, Stage, Test};
use serde_json::json;

/// Stage 24.
pub fn stage() -> Stage {
    Stage {
        number: 24,
        slug: "range_over_ranges",
        name: "Ranges, prefixes and the whole store",
        ext: false,
        ladder: Ladder::Node,
        hints: &[
            "range_end makes the request a half-open range [key, range_end)",
            "A prefix range ends at the key with its last byte incremented",
            "key and range_end both \0 means every key in the store",
            "Results come back in key order, which is byte order, not string order",
        ],
        examples,
        tests: vec![
            Test::new(
                "range_end is excluded, because the range is half-open",
                range_end_is_excluded,
            ),
            Test::new(
                "a prefix range takes everything under the prefix",
                a_prefix_range,
            ),
            Test::new(
                "a prefix range stops at the prefix boundary",
                a_prefix_stops_at_its_boundary,
            ),
            Test::new(
                "a zero byte at both ends is every key in the store",
                the_whole_store,
            ),
            Test::new(
                "keys come back in byte order, not string order",
                byte_order_not_string_order,
            ),
            Test::new("a range that covers no key answers nothing", an_empty_range),
            Test::new(
                "a range whose end sorts before its start is empty, not an error",
                end_before_start,
            ),
            Test::new(
                "keys written in a scrambled order still come back sorted",
                scrambled_writes_sort,
            ),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        node_example_after(
            "A prefix range",
            || {
                vec![
                    (
                        "/v3/kv/put".into(),
                        json!({"key": b64(b"k/a"), "value": b64(b"1")}),
                    ),
                    (
                        "/v3/kv/put".into(),
                        json!({"key": b64(b"k/b"), "value": b64(b"2")}),
                    ),
                    (
                        "/v3/kv/put".into(),
                        json!({"key": b64(b"k/c"), "value": b64(b"3")}),
                    ),
                ]
            },
            "/v3/kv/range",
            || json!({ "key": b64(b"k/"), "range_end": b64(&prefix_end(b"k/")) }),
        )
        .request("everything under the prefix `k/`, whose end key is `k0`")
        .response("the pairs in byte order, with a count of how many the range holds")
        .note(
            "`k0` is `k/` with its last byte incremented, and the range excludes it. Ending \
             the range at `k/\u{ff}` instead looks equivalent and is not: a key of `k/` \
             followed by two 0xff bytes sorts after it and would be missed.",
        ),
        node_example_after(
            "The range that ends before it starts",
            || {
                vec![(
                    "/v3/kv/put".into(),
                    json!({"key": b64(b"k/a"), "value": b64(b"1")}),
                )]
            },
            "/v3/kv/range",
            || json!({ "key": b64(b"z"), "range_end": b64(b"a") }),
        )
        .request("a range from `z` to `a`, which covers nothing")
        .response("a header and nothing else, at HTTP 200")
        .note(
            "An interval that is empty because its end sorts before its start is still a \
             well-formed question, and the answer is simply that no key is in it. Refusing \
             it with an error is the mistake.",
        ),
    ]
}

dist_test!(range_end_is_excluded, |ctx| {
    let kv = ctx.kv()?;
    let (a, b, c_key) = (ctx.key("r/a"), ctx.key("r/b"), ctx.key("r/c"));
    ok(kv.put(&a, b"1").await, "put r/a")?;
    ok(kv.put(&b, b"2").await, "put r/b")?;
    ok(kv.put(&c_key, b"3").await, "put r/c")?;
    let read = ok(
        kv.range_req(&RangeRequest::range(&a, &c_key)).await,
        "read the range [r/a, r/c)",
    )?;
    let mut c = Check::new("a half-open range");
    // The end key is the one thing everybody gets wrong first: [a, c) holds a and b.
    c.eq("range.kvs.keys()", vec![text(&a), text(&b)], read.keys());
    c.eq("range.count", 2, read.count);
    c.finish()
});

dist_test!(a_prefix_range, |ctx| {
    let kv = ctx.kv()?;
    let prefix = ctx.key("p/");
    let inside = [ctx.key("p/a"), ctx.key("p/b"), ctx.key("p/b/deeper")];
    for (i, k) in inside.iter().enumerate() {
        ok(
            kv.put(k, format!("v{i}").as_bytes()).await,
            "put a key under the prefix",
        )?;
    }
    ok(
        kv.put(&ctx.key("other"), b"x").await,
        "put a key outside the prefix",
    )?;
    let read = ok(
        kv.range_req(&RangeRequest::prefix(&prefix)).await,
        "read everything under the prefix",
    )?;
    let mut c = Check::new("a prefix range");
    c.eq(
        "range.kvs.keys()",
        inside.iter().map(|k| text(k)).collect::<Vec<_>>(),
        read.keys(),
    );
    c.eq("range.count", 3, read.count);
    c.note(format!(
        "the prefix was {} and its end key {}",
        text(&prefix),
        text(&prefix_end(&prefix))
    ));
    c.finish()
});

dist_test!(a_prefix_stops_at_its_boundary, |ctx| {
    let kv = ctx.kv()?;
    let prefix = ctx.key("b/");
    let end = prefix_end(&prefix);
    let inside = ctx.key("b/a");
    // `end` is the prefix with its last byte incremented, so a key spelled exactly that way
    // sits on the boundary and is excluded; anything after it is further out still.
    let beyond = ctx.key("bz");
    ok(kv.put(&inside, b"in").await, "put a key under the prefix")?;
    ok(kv.put(&end, b"boundary").await, "put the end key itself")?;
    ok(kv.put(&beyond, b"out").await, "put a key past the prefix")?;
    let read = ok(
        kv.range_req(&RangeRequest::prefix(&prefix)).await,
        "read everything under the prefix",
    )?;
    let mut c = Check::new("the boundary of a prefix range");
    c.eq("range.kvs.keys()", vec![text(&inside)], read.keys());
    c.eq("range.count", 1, read.count);
    c.note(format!(
        "the end key {} was written and must be excluded",
        text(&end)
    ));
    c.finish()
});

dist_test!(the_whole_store, |ctx| {
    let kv = ctx.kv()?;
    let keys = [ctx.key("a"), ctx.key("m/deep/er"), ctx.key("z")];
    for (i, k) in keys.iter().enumerate() {
        ok(kv.put(k, format!("v{i}").as_bytes()).await, "put a key")?;
    }
    let read = ok(kv.range_req(&RangeRequest::all()).await, "read every key")?;
    let mut c = Check::new("a range over the whole store");
    // A single zero byte at both ends is the only way to say "everything": it is the
    // smallest key there is, and as a range_end it is the one value that means "no end".
    c.eq(
        "range.kvs.keys()",
        keys.iter().map(|k| text(k)).collect::<Vec<_>>(),
        read.keys(),
    );
    c.eq("range.count", 3, read.count);
    c.finish()
});

dist_test!(byte_order_not_string_order, |ctx| {
    let kv = ctx.kv()?;
    let base = ctx.key("o/");
    let suffix_in_order: [&[u8]; 6] = [b"A", b"B", b"a", b"b", &[0x7f], &[0x80]];
    // Written back to front, so nothing but a real sort can produce the order below.
    for (i, s) in suffix_in_order.iter().enumerate().rev() {
        let key = [base.as_slice(), s].concat();
        ok(
            kv.put(&key, format!("v{i}").as_bytes()).await,
            "put a key with an awkward suffix",
        )?;
    }
    let read = ok(
        kv.range_req(&RangeRequest::prefix(&base)).await,
        "read the whole prefix",
    )?;
    let expected: Vec<Vec<u8>> = suffix_in_order
        .iter()
        .map(|s| [base.as_slice(), s].concat())
        .collect();
    let actual: Vec<Vec<u8>> = read.kvs.iter().map(|p| p.key.clone()).collect();
    let mut c = Check::new("the order keys come back in");
    // `B` before `a` rules out case-insensitive ordering; 0x80 after 0x7f rules out
    // comparing bytes as signed, which would drop every high byte to the front.
    c.that(
        "range.kvs.keys()",
        "the keys sorted by unsigned byte value",
        actual == expected,
        actual.iter().map(|k| format!("{k:?}")).collect::<Vec<_>>(),
    );
    c.eq("range.count", 6, read.count);
    c.finish()
});

dist_test!(an_empty_range, |ctx| {
    let kv = ctx.kv()?;
    ok(kv.put(&ctx.key("e/a"), b"1").await, "put one key")?;
    ok(kv.put(&ctx.key("e/z"), b"2").await, "put another")?;
    let read = ok(
        kv.range_req(&RangeRequest::range(&ctx.key("e/m"), &ctx.key("e/n")))
            .await,
        "read a range between the two",
    )?;
    let mut c = Check::new("a range that covers no key");
    c.eq("range.kvs.len()", 0, read.kvs.len());
    c.eq("range.count", 0, read.count);
    c.that(
        "range.more",
        "false, because nothing was cut short",
        !read.more,
        read.more,
    );
    c.at_least("range.header.revision", 1, read.header.revision);
    c.finish()
});

dist_test!(end_before_start, |ctx| {
    let kv = ctx.kv()?;
    ok(kv.put(&ctx.key("x"), b"1").await, "put a key")?;
    // Not an error: an interval whose end sorts before its start simply holds nothing.
    let read = ok(
        kv.range_req(&RangeRequest::range(&ctx.key("z"), &ctx.key("a")))
            .await,
        "read a range from z to a",
    )?;
    let mut c = Check::new("a range whose end sorts before its start");
    c.eq("range.kvs.len()", 0, read.kvs.len());
    c.eq("range.count", 0, read.count);
    c.at_least("range.header.revision", 1, read.header.revision);
    c.finish()
});

dist_test!(scrambled_writes_sort, |ctx| {
    let kv = ctx.kv()?;
    let base = ctx.key("s/");
    let names = ["04", "09", "01", "07", "02", "08", "03", "06", "05", "00"];
    for name in names {
        let key = [base.as_slice(), name.as_bytes()].concat();
        ok(
            kv.put(&key, name.as_bytes()).await,
            "put a key out of order",
        )?;
    }
    let read = ok(
        kv.range_req(&RangeRequest::prefix(&base)).await,
        "read the prefix",
    )?;
    let mut sorted = names.to_vec();
    sorted.sort_unstable();
    let mut c = Check::new("keys written out of order");
    // The answer is sorted by key, never by the order the writes arrived in.
    c.eq(
        "range.kvs.keys()",
        sorted
            .iter()
            .map(|n| text(&[base.as_slice(), n.as_bytes()].concat()))
            .collect::<Vec<_>>(),
        read.keys(),
    );
    c.eq("range.count", 10, read.count);
    c.finish()
});
