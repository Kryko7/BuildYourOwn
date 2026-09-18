//! Stage 25 — limit, sort order, count_only and keys_only.
//!
//! The four options a range request carries beyond its interval, and every one of them has a
//! way of being subtly wrong.
//!
//! `limit` truncates the *answer*, never the *question*: `count` keeps reporting how many
//! keys the range holds, and `more` says the answer was cut. Sorting happens before the
//! truncation, so a descending sort with a limit hands back the last keys of the range
//! rather than the first ones reversed. `count_only` must answer with no pairs at all, and
//! `keys_only` with pairs whose values are left out — which, since a zero-valued field may be
//! omitted, is the same thing on the wire as a value of zero bytes and a very different thing
//! in the store.

use crate::assert::{Check, Failure};
use crate::dist_test;
use crate::etcd::{b64, prefix_end, RangeRequest};
use crate::examples::{node_example_after, ExampleSpec};
use crate::stages::{ok, put_series, text, Ctx, Ladder, Stage, Test};
use serde_json::{json, Value};

/// The four keys both examples read back.
fn four_keys() -> Vec<(String, Value)> {
    (0..4)
        .map(|i| {
            (
                "/v3/kv/put".to_string(),
                json!({
                    "key": b64(format!("n/{i}").as_bytes()),
                    "value": b64(format!("v{i}").as_bytes()),
                }),
            )
        })
        .collect()
}

/// Stage 25.
pub fn stage() -> Stage {
    Stage {
        number: 25,
        slug: "range_options",
        name: "limit, sort order, count_only and keys_only",
        ext: true,
        ladder: Ladder::Node,
        hints: &[
            "limit cuts the answer and sets more, but never changes count",
            "sort_order and sort_target together decide the order; the default is ascending by key",
            "count_only answers the count and no kvs at all",
            "keys_only answers the kvs with their values left out, not with empty values invented",
        ],
        examples,
        tests: vec![
            Test::new(
                "a limit cuts the answer but never the count",
                limit_cuts_the_answer,
            )
            .ext(),
            Test::new(
                "a limit as large as the range leaves more false",
                limit_that_does_not_bite,
            )
            .ext(),
            Test::new(
                "a descending sort by key reverses the answer",
                descending_by_key,
            )
            .ext(),
            Test::new("sorting by mod revision follows the writes", sort_by_mod).ext(),
            Test::new(
                "sorting by create revision follows the creations",
                sort_by_create,
            )
            .ext(),
            Test::new("count_only answers a count and no pairs at all", count_only).ext(),
            Test::new(
                "keys_only answers keys with their values left out",
                keys_only,
            )
            .ext(),
            Test::new(
                "a limit with a descending sort takes the last keys",
                limit_with_a_descending_sort,
            )
            .ext(),
            Test::new(
                "the options are harmless over a range that holds nothing",
                options_on_an_empty_range,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        node_example_after(
            "A limit, and the count it does not change",
            four_keys,
            "/v3/kv/range",
            || {
                json!({
                    "key": b64(b"n/"),
                    "range_end": b64(&prefix_end(b"n/")),
                    "limit": "2",
                })
            },
        )
        .request("the four keys under `n/`, but at most two of them")
        .response("two pairs, `more` true, and a count that still says four")
        .note(
            "count answers the question that was asked — how many keys are in this range — \
             and limit only says how many of them to send. A server that reports the \
             truncated length as the count makes paging impossible.",
        ),
        node_example_after(
            "The last two keys, by sorting before truncating",
            four_keys,
            "/v3/kv/range",
            || {
                json!({
                    "key": b64(b"n/"),
                    "range_end": b64(&prefix_end(b"n/")),
                    "limit": "2",
                    "sort_order": "DESCEND",
                    "sort_target": "KEY",
                })
            },
        )
        .request("at most two keys under `n/`, largest first")
        .response("`n/3` and `n/2`, in that order, with `more` still true")
        .note(
            "The sort happens first and the limit second. Truncating to the first two keys \
             and then reversing them would answer `n/1` and `n/0`, which is a different \
             pair of keys, not a different order.",
        ),
    ]
}

/// The five keys every test in this stage reads back, and the prefix that covers them.
async fn five_keys(ctx: &Ctx) -> Result<(Vec<u8>, Vec<Vec<u8>>), Failure> {
    let base = ctx.key("n");
    put_series(ctx.kv()?, &base, 5).await?;
    let prefix = [base.as_slice(), b"/"].concat();
    let keys = (0..5)
        .map(|i| [base.as_slice(), format!("/{i:02}").as_bytes()].concat())
        .collect();
    Ok((prefix, keys))
}

dist_test!(limit_cuts_the_answer, |ctx| {
    let (prefix, keys) = five_keys(ctx).await?;
    let read = ok(
        ctx.kv()?
            .range_req(&RangeRequest::prefix(&prefix).limit(2))
            .await,
        "read the prefix with a limit of two",
    )?;
    let mut c = Check::new("a limit smaller than the range");
    c.eq("range.kvs.len()", 2, read.kvs.len());
    c.eq(
        "range.kvs.keys()",
        keys[..2].iter().map(|k| text(k)).collect::<Vec<_>>(),
        read.keys(),
    );
    c.that(
        "range.more",
        "true, because two of five were sent",
        read.more,
        read.more,
    );
    // The whole point: count answers the question, not the answer.
    c.eq("range.count", 5, read.count);
    c.finish()
});

dist_test!(limit_that_does_not_bite, |ctx| {
    let (prefix, keys) = five_keys(ctx).await?;
    let kv = ctx.kv()?;
    let exact = ok(
        kv.range_req(&RangeRequest::prefix(&prefix).limit(5)).await,
        "read the prefix with a limit of exactly five",
    )?;
    let generous = ok(
        kv.range_req(&RangeRequest::prefix(&prefix).limit(50)).await,
        "read the prefix with a limit of fifty",
    )?;
    let mut c = Check::new("a limit at or above the size of the range");
    c.eq("range(limit 5).kvs.len()", 5, exact.kvs.len());
    c.that(
        "range(limit 5).more",
        "false, because nothing was left out",
        !exact.more,
        exact.more,
    );
    c.eq("range(limit 50).kvs.len()", 5, generous.kvs.len());
    c.that(
        "range(limit 50).more",
        "false, because nothing was left out",
        !generous.more,
        generous.more,
    );
    c.eq(
        "range(limit 50).kvs.keys()",
        keys.iter().map(|k| text(k)).collect::<Vec<_>>(),
        generous.keys(),
    );
    c.finish()
});

dist_test!(descending_by_key, |ctx| {
    let (prefix, keys) = five_keys(ctx).await?;
    let read = ok(
        ctx.kv()?
            .range_req(&RangeRequest::prefix(&prefix).sort("DESCEND", "KEY"))
            .await,
        "read the prefix largest key first",
    )?;
    let mut expected: Vec<String> = keys.iter().map(|k| text(k)).collect();
    expected.reverse();
    let mut c = Check::new("a descending sort by key");
    c.eq("range.kvs.keys()", expected, read.keys());
    c.eq("range.count", 5, read.count);
    c.finish()
});

dist_test!(sort_by_mod, |ctx| {
    let kv = ctx.kv()?;
    let base = ctx.key("m/");
    // Written in an order that has nothing to do with the key order, so sorting by MOD and
    // sorting by KEY cannot accidentally agree.
    let write_order = ["c", "a", "e", "b", "d"];
    for name in write_order {
        let key = [base.as_slice(), name.as_bytes()].concat();
        ok(kv.put(&key, name.as_bytes()).await, "put a key")?;
    }
    let read = ok(
        kv.range_req(&RangeRequest::prefix(&base).sort("ASCEND", "MOD"))
            .await,
        "read the prefix oldest write first",
    )?;
    let descending = ok(
        kv.range_req(&RangeRequest::prefix(&base).sort("DESCEND", "MOD"))
            .await,
        "read the prefix newest write first",
    )?;
    let mut expected: Vec<String> = write_order
        .iter()
        .map(|n| text(&[base.as_slice(), n.as_bytes()].concat()))
        .collect();
    let mut c = Check::new("a sort by mod revision");
    c.eq(
        "range(ASCEND MOD).kvs.keys()",
        expected.clone(),
        read.keys(),
    );
    expected.reverse();
    c.eq("range(DESCEND MOD).kvs.keys()", expected, descending.keys());
    c.finish()
});

dist_test!(sort_by_create, |ctx| {
    let kv = ctx.kv()?;
    let base = ctx.key("c/");
    let creation_order = ["d", "b", "e", "a", "c"];
    for name in creation_order {
        let key = [base.as_slice(), name.as_bytes()].concat();
        ok(kv.put(&key, b"first").await, "create a key")?;
    }
    // Rewriting in the opposite order moves every mod_revision and leaves every
    // create_revision alone, so a server sorting by the wrong field answers backwards.
    for name in creation_order.iter().rev() {
        let key = [base.as_slice(), name.as_bytes()].concat();
        ok(kv.put(&key, b"second").await, "rewrite the key")?;
    }
    let read = ok(
        kv.range_req(&RangeRequest::prefix(&base).sort("ASCEND", "CREATE"))
            .await,
        "read the prefix oldest creation first",
    )?;
    let expected: Vec<String> = creation_order
        .iter()
        .map(|n| text(&[base.as_slice(), n.as_bytes()].concat()))
        .collect();
    let mut c = Check::new("a sort by create revision");
    c.eq("range(ASCEND CREATE).kvs.keys()", expected, read.keys());
    c.eq("range.count", 5, read.count);
    c.finish()
});

dist_test!(count_only, |ctx| {
    let (prefix, _) = five_keys(ctx).await?;
    let read = ok(
        ctx.kv()?
            .range_req(&RangeRequest::prefix(&prefix).count_only())
            .await,
        "ask for the count and nothing else",
    )?;
    let mut c = Check::new("count_only");
    c.eq("range.count", 5, read.count);
    // No pairs at all: the point of count_only is not sending them.
    c.eq("range.kvs.len()", 0, read.kvs.len());
    c.finish()
});

dist_test!(keys_only, |ctx| {
    let (prefix, keys) = five_keys(ctx).await?;
    let read = ok(
        ctx.kv()?
            .range_req(&RangeRequest::prefix(&prefix).keys_only())
            .await,
        "ask for the keys without their values",
    )?;
    let mut c = Check::new("keys_only");
    c.eq("range.kvs.len()", 5, read.kvs.len());
    c.eq(
        "range.kvs.keys()",
        keys.iter().map(|k| text(k)).collect::<Vec<_>>(),
        read.keys(),
    );
    c.eq("range.count", 5, read.count);
    let value_bytes: usize = read.kvs.iter().map(|p| p.value.len()).sum();
    c.eq("total bytes of value across the answer", 0, value_bytes);
    // The revisions still come back: it is the value that is left out, not the pair.
    c.that(
        "range.kvs[*].mod_revision",
        "a real revision on every pair, because only the value was dropped",
        read.kvs.iter().all(|p| p.mod_revision > 0),
        read.kvs.iter().map(|p| p.mod_revision).collect::<Vec<_>>(),
    );
    c.finish()
});

dist_test!(limit_with_a_descending_sort, |ctx| {
    let (prefix, keys) = five_keys(ctx).await?;
    let read = ok(
        ctx.kv()?
            .range_req(
                &RangeRequest::prefix(&prefix)
                    .sort("DESCEND", "KEY")
                    .limit(2),
            )
            .await,
        "read at most two keys, largest first",
    )?;
    let mut c = Check::new("a limit applied after a descending sort");
    // The last two keys of the range, newest first — not the first two reversed.
    c.eq(
        "range.kvs.keys()",
        vec![text(&keys[4]), text(&keys[3])],
        read.keys(),
    );
    c.that(
        "range.more",
        "true, because three keys were left out",
        read.more,
        read.more,
    );
    c.eq("range.count", 5, read.count);
    c.finish()
});

dist_test!(options_on_an_empty_range, |ctx| {
    let kv = ctx.kv()?;
    ok(
        kv.put(&ctx.key("kept"), b"v").await,
        "put a key outside the range",
    )?;
    let empty = ctx.key("nothing/");
    let limited = ok(
        kv.range_req(&RangeRequest::prefix(&empty).limit(3).sort("DESCEND", "KEY"))
            .await,
        "read an empty range with a limit and a sort",
    )?;
    let counted = ok(
        kv.range_req(&RangeRequest::prefix(&empty).count_only())
            .await,
        "count an empty range",
    )?;
    let bare = ok(
        kv.range_req(&RangeRequest::prefix(&empty).keys_only())
            .await,
        "ask for the keys of an empty range",
    )?;
    let mut c = Check::new("the options over a range that holds nothing");
    c.eq("range(limit, sort).kvs.len()", 0, limited.kvs.len());
    c.eq("range(limit, sort).count", 0, limited.count);
    c.that(
        "range(limit, sort).more",
        "false, because nothing was cut short",
        !limited.more,
        limited.more,
    );
    c.eq("range(count_only).count", 0, counted.count);
    c.eq("range(count_only).kvs.len()", 0, counted.kvs.len());
    c.eq("range(keys_only).kvs.len()", 0, bare.kvs.len());
    c.finish()
});
