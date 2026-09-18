//! Stage 29 — Compare-and-swap semantics.
//!
//! Stage 28 proved a transaction picks a branch. This one is about the comparison itself:
//! four targets over four different fields, six operators between them, and the one
//! encoding everybody trips over — `VERSION` against `0`, which is how a store with no
//! "does this key exist" query says "this key does not exist".
//!
//! The property underneath every test here is that a comparison and the branch it guards
//! happen together. A swap that loses the race must leave the store exactly as it found
//! it: not the value, not the revision, not even a key the success branch would have
//! created.

use crate::assert::Check;
use crate::dist_test;
use crate::etcd::{
    b64, cmp_create_eq, cmp_mod_eq, cmp_value_eq, cmp_version_eq, compare, op_put, op_range,
};
use crate::examples::{node_example, node_example_after, ExampleSpec};
use crate::stages::{ok, Ladder, Stage, Test};
use serde_json::{json, Value};

/// Stage 29.
pub fn stage() -> Stage {
    Stage {
        number: 29,
        slug: "txn_compare_and_swap",
        name: "Compare-and-swap semantics",
        ext: true,
        ladder: Ladder::Node,
        hints: &[
            "VERSION = 0 is how you say 'this key does not exist'",
            "CREATE, MOD, VALUE and VERSION are four different comparisons with four different fields",
            "Two concurrent swaps from the same value: exactly one may succeed",
            "A comparison naming a key that does not exist compares against zeros, it does not fail the request",
        ],
        examples,
        tests: vec![
            Test::new(
                "a create-if-absent transaction succeeds exactly once",
                create_if_absent,
            ),
            Test::new("a VERSION comparison holds only at that version", version_compare),
            Test::new(
                "a CREATE comparison names the revision the key was born at",
                create_compare,
            ),
            Test::new("a MOD comparison names the revision of the last put", mod_compare),
            Test::new("a VALUE comparison swaps only from the value it names", value_compare),
            Test::new(
                "a comparison on a key that does not exist compares against zeros",
                absent_key_compares_against_zeros,
            ),
            Test::new("two swaps from the same value cannot both succeed", one_swap_wins),
            Test::new("a swap that fails leaves the store untouched", failed_swap_changes_nothing),
            Test::new(
                "GREATER, LESS and NOT_EQUAL compare the same four fields",
                other_operators,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        node_example("Create the key only if nobody else has", "/v3/kv/txn", || {
            json!({
                "compare": [{"key": b64(b"cas"), "target": "VERSION", "result": "EQUAL", "version": "0"}],
                "success": [{"requestPut": {"key": b64(b"cas"), "value": b64(b"mine")}}],
                "failure": [{"requestRange": {"key": b64(b"cas")}}],
            })
        })
        .request("version 0 — the key must not exist — and a put if that holds")
        .response("succeeded true, and the put's own response inside `responses`")
        .note(
            "There is no `version` field to compare against on a key that was never \
             written, so the comparison is made against zeros. That is what makes \
             `VERSION EQUAL 0` mean \"absent\", and what makes this one request a lock.",
        ),
        node_example_after(
            "The same request, once the key is there",
            || vec![("/v3/kv/put".to_string(), json!({"key": b64(b"cas"), "value": b64(b"mine")}))],
            "/v3/kv/txn",
            || {
                json!({
                    "compare": [{"key": b64(b"cas"), "target": "VERSION", "result": "EQUAL", "version": "0"}],
                    "success": [{"requestPut": {"key": b64(b"cas"), "value": b64(b"yours")}}],
                    "failure": [{"requestRange": {"key": b64(b"cas")}}],
                })
            },
        )
        .request("the identical transaction, sent after the key has been created")
        .response("no `succeeded` at all — false is the zero value — and the failure branch's range")
        .note(
            "The loser learns who won without a second round trip, because the failure \
             branch read the key. Nothing was written: the header's revision is the same \
             one the put before it reported.",
        ),
    ]
}

dist_test!(create_if_absent, |ctx| {
    let key = ctx.key("cas");
    let kv = ctx.kv()?;
    let first = ok(
        kv.txn(
            vec![cmp_version_eq(&key, 0)],
            vec![op_put(&key, b"first")],
            vec![op_range(&key)],
        )
        .await,
        "create the key if it is absent",
    )?;
    let second = ok(
        kv.txn(
            vec![cmp_version_eq(&key, 0)],
            vec![op_put(&key, b"second")],
            vec![op_range(&key)],
        )
        .await,
        "try to create the same key again",
    )?;
    let read = ok(kv.get_key(&key).await, "read the key back")?;
    let mut c = Check::new("create-if-absent, twice");
    c.eq("txn(first).succeeded", true, first.succeeded);
    c.eq("txn(second).succeeded", false, second.succeeded);
    c.eq(
        "range.kvs[0].value",
        "first".to_string(),
        read.one().map(|k| k.value_str()).unwrap_or_default(),
    );
    c.eq(
        "txn(second).responses[0].response_range.count",
        1,
        second.range(0).map(|r| r.count).unwrap_or(0),
    );
    c.eq(
        "txn(second).responses[0].response_range.kvs[0].value",
        "first".to_string(),
        second
            .range(0)
            .and_then(|r| r.one())
            .map(|k| k.value_str())
            .unwrap_or_default(),
    );
    c.finish()
});

dist_test!(version_compare, |ctx| {
    let key = ctx.key("v");
    let kv = ctx.kv()?;
    ok(kv.put(&key, b"one").await, "put the key")?;
    ok(kv.put(&key, b"two").await, "put it again, so version is 2")?;
    let at_two = ok(
        kv.txn(vec![cmp_version_eq(&key, 2)], vec![op_range(&key)], vec![])
            .await,
        "compare VERSION against 2",
    )?;
    let at_one = ok(
        kv.txn(vec![cmp_version_eq(&key, 1)], vec![op_range(&key)], vec![])
            .await,
        "compare VERSION against 1",
    )?;
    let at_three = ok(
        kv.txn(vec![cmp_version_eq(&key, 3)], vec![op_range(&key)], vec![])
            .await,
        "compare VERSION against 3",
    )?;
    let mut c = Check::new("VERSION after two puts");
    c.eq("txn(VERSION = 2).succeeded", true, at_two.succeeded);
    c.eq("txn(VERSION = 1).succeeded", false, at_one.succeeded);
    c.eq("txn(VERSION = 3).succeeded", false, at_three.succeeded);
    c.finish()
});

dist_test!(create_compare, |ctx| {
    let key = ctx.key("c");
    let kv = ctx.kv()?;
    let born = ok(kv.put(&key, b"one").await, "create the key")?;
    let moved = ok(kv.put(&key, b"two").await, "write it a second time")?;
    let at_create = ok(
        kv.txn(
            vec![cmp_create_eq(&key, born.header.revision)],
            vec![op_range(&key)],
            vec![],
        )
        .await,
        "compare CREATE against the revision the key was created at",
    )?;
    let at_mod = ok(
        kv.txn(
            vec![cmp_create_eq(&key, moved.header.revision)],
            vec![op_range(&key)],
            vec![],
        )
        .await,
        "compare CREATE against the revision of the second put",
    )?;
    let mut c = Check::new("CREATE against a key that has been written twice");
    c.eq(
        "txn(CREATE = create_revision).succeeded",
        true,
        at_create.succeeded,
    );
    // The trap: the second put moved mod_revision and left create_revision alone, so
    // comparing CREATE against the newer revision must not hold.
    c.eq(
        "txn(CREATE = mod_revision).succeeded",
        false,
        at_mod.succeeded,
    );
    c.finish()
});

dist_test!(mod_compare, |ctx| {
    let key = ctx.key("m");
    let kv = ctx.kv()?;
    let born = ok(kv.put(&key, b"one").await, "create the key")?;
    let moved = ok(kv.put(&key, b"two").await, "write it a second time")?;
    let at_mod = ok(
        kv.txn(
            vec![cmp_mod_eq(&key, moved.header.revision)],
            vec![op_put(&key, b"three")],
            vec![op_range(&key)],
        )
        .await,
        "swap on MOD equal to the latest revision",
    )?;
    let stale = ok(
        kv.txn(
            vec![cmp_mod_eq(&key, born.header.revision)],
            vec![op_put(&key, b"four")],
            vec![op_range(&key)],
        )
        .await,
        "swap on MOD equal to the revision the key was created at",
    )?;
    let read = ok(kv.get_key(&key).await, "read the key back")?;
    let mut c = Check::new("MOD as an optimistic-concurrency token");
    c.eq("txn(MOD = latest).succeeded", true, at_mod.succeeded);
    c.eq("txn(MOD = stale).succeeded", false, stale.succeeded);
    c.eq(
        "range.kvs[0].value",
        "three".to_string(),
        read.one().map(|k| k.value_str()).unwrap_or_default(),
    );
    c.finish()
});

dist_test!(value_compare, |ctx| {
    let key = ctx.key("val");
    let kv = ctx.kv()?;
    ok(kv.put(&key, b"one").await, "put the key")?;
    let swap = ok(
        kv.txn(
            vec![cmp_value_eq(&key, b"one")],
            vec![op_put(&key, b"two")],
            vec![op_range(&key)],
        )
        .await,
        "swap one for two",
    )?;
    let again = ok(
        kv.txn(
            vec![cmp_value_eq(&key, b"one")],
            vec![op_put(&key, b"three")],
            vec![op_range(&key)],
        )
        .await,
        "try to swap one for three, when the value is now two",
    )?;
    let read = ok(kv.get_key(&key).await, "read the key back")?;
    let mut c = Check::new("VALUE as the thing being swapped from");
    c.eq("txn(VALUE = one).succeeded", true, swap.succeeded);
    c.eq("txn(VALUE = one, again).succeeded", false, again.succeeded);
    c.eq(
        "range.kvs[0].value",
        "two".to_string(),
        read.one().map(|k| k.value_str()).unwrap_or_default(),
    );
    c.finish()
});

dist_test!(absent_key_compares_against_zeros, |ctx| {
    let key = ctx.key("ghost");
    let kv = ctx.kv()?;
    let version = ok(
        kv.txn(vec![cmp_version_eq(&key, 0)], vec![op_range(&key)], vec![])
            .await,
        "compare VERSION against 0 on a key that was never written",
    )?;
    let create = ok(
        kv.txn(vec![cmp_create_eq(&key, 0)], vec![op_range(&key)], vec![])
            .await,
        "compare CREATE against 0 on the same key",
    )?;
    let modified = ok(
        kv.txn(vec![cmp_mod_eq(&key, 0)], vec![op_range(&key)], vec![])
            .await,
        "compare MOD against 0 on the same key",
    )?;
    let nonzero = ok(
        kv.txn(vec![cmp_version_eq(&key, 1)], vec![op_range(&key)], vec![])
            .await,
        "compare VERSION against 1 on the same key",
    )?;
    let mut c = Check::new("comparisons against a key that does not exist");
    // Each of these is a 200 carrying a verdict, never an error: a comparison over a
    // missing key reads zeros out of a key/value that is not there.
    c.eq("txn(VERSION = 0).succeeded", true, version.succeeded);
    c.eq("txn(CREATE = 0).succeeded", true, create.succeeded);
    c.eq("txn(MOD = 0).succeeded", true, modified.succeeded);
    c.eq("txn(VERSION = 1).succeeded", false, nonzero.succeeded);
    c.eq(
        "txn(VERSION = 0).responses[0].response_range.count",
        0,
        version.range(0).map(|r| r.count).unwrap_or(-1),
    );
    c.finish()
});

dist_test!(one_swap_wins, |ctx| {
    let key = ctx.key("race");
    let kv = ctx.kv()?;
    ok(
        kv.put(&key, b"v0").await,
        "put the value both swaps start from",
    )?;
    let a = ok(
        kv.txn(
            vec![cmp_value_eq(&key, b"v0")],
            vec![op_put(&key, b"a")],
            vec![op_range(&key)],
        )
        .await,
        "swap v0 for a",
    )?;
    let b = ok(
        kv.txn(
            vec![cmp_value_eq(&key, b"v0")],
            vec![op_put(&key, b"b")],
            vec![op_range(&key)],
        )
        .await,
        "swap v0 for b, from the same starting value",
    )?;
    let read = ok(kv.get_key(&key).await, "read the winner")?;
    let winners = [a.succeeded, b.succeeded].iter().filter(|x| **x).count();
    let mut c = Check::new("two swaps from one value");
    c.eq("how many of the two swaps succeeded", 1, winners);
    c.eq(
        "range.kvs[0].value",
        "a".to_string(),
        read.one().map(|k| k.value_str()).unwrap_or_default(),
    );
    c.eq(
        "range.kvs[0].version",
        2,
        read.one().map(|k| k.version).unwrap_or(0),
    );
    c.finish()
});

dist_test!(failed_swap_changes_nothing, |ctx| {
    let key = ctx.key("kept");
    let other = ctx.key("never");
    let kv = ctx.kv()?;
    let put = ok(
        kv.put(&key, b"keep").await,
        "put the key the swap will not touch",
    )?;
    let failed = ok(
        kv.txn(
            // The key is at version 1, so this cannot hold.
            vec![cmp_version_eq(&key, 99)],
            vec![op_put(&key, b"clobbered"), op_put(&other, b"leaked")],
            vec![],
        )
        .await,
        "run a swap whose comparison cannot hold",
    )?;
    let read = ok(kv.get_key(&key).await, "read the key back")?;
    let leaked = ok(
        kv.get_key(&other).await,
        "look for the key the success branch would have written",
    )?;
    let mut c = Check::new("what a losing swap leaves behind");
    c.eq("txn.succeeded", false, failed.succeeded);
    c.eq("txn.responses.len()", 0, failed.responses.len());
    c.eq(
        "range.kvs[0].value",
        "keep".to_string(),
        read.one().map(|k| k.value_str()).unwrap_or_default(),
    );
    c.eq(
        "range.kvs[0].mod_revision",
        put.header.revision,
        read.one().map(|k| k.mod_revision).unwrap_or(0),
    );
    // Nothing was written, so the store's clock has not moved either.
    c.eq(
        "range.header.revision",
        put.header.revision,
        read.header.revision,
    );
    c.eq("range(the other key).count", 0, leaked.count);
    c.finish()
});

dist_test!(other_operators, |ctx| {
    let key = ctx.key("ops");
    let kv = ctx.kv()?;
    let born = ok(kv.put(&key, b"bbb").await, "create the key")?;
    let moved = ok(
        kv.put(&key, b"bbb").await,
        "write it again, so version is 2",
    )?;
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "VERSION GREATER 1",
            compare(&key, "VERSION", "GREATER", "version", json!("1")),
            true,
        ),
        (
            "VERSION GREATER 2",
            compare(&key, "VERSION", "GREATER", "version", json!("2")),
            false,
        ),
        (
            "VERSION LESS 3",
            compare(&key, "VERSION", "LESS", "version", json!("3")),
            true,
        ),
        (
            "VERSION NOT_EQUAL 1",
            compare(&key, "VERSION", "NOT_EQUAL", "version", json!("1")),
            true,
        ),
        (
            "VERSION NOT_EQUAL 2",
            compare(&key, "VERSION", "NOT_EQUAL", "version", json!("2")),
            false,
        ),
        (
            "CREATE LESS mod_revision",
            compare(
                &key,
                "CREATE",
                "LESS",
                "create_revision",
                json!(moved.header.revision.to_string()),
            ),
            true,
        ),
        (
            "MOD GREATER create_revision",
            compare(
                &key,
                "MOD",
                "GREATER",
                "mod_revision",
                json!(born.header.revision.to_string()),
            ),
            true,
        ),
        (
            "VALUE NOT_EQUAL aaa",
            compare(&key, "VALUE", "NOT_EQUAL", "value", json!(b64(b"aaa"))),
            true,
        ),
        (
            "VALUE GREATER aaa",
            compare(&key, "VALUE", "GREATER", "value", json!(b64(b"aaa"))),
            true,
        ),
    ];
    let mut c = Check::new("the operators other than EQUAL");
    for (name, cmp, want) in cases {
        let t = ok(
            kv.txn(vec![cmp], vec![op_range(&key)], vec![]).await,
            &format!("compare {name}"),
        )?;
        c.eq(&format!("txn({name}).succeeded"), want, t.succeeded);
    }
    c.finish()
});
