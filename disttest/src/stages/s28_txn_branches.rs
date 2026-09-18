//! Stage 28 — Transactions: compare, success, failure.
//!
//! One request that looks at the store, decides, and then writes — atomically. Everything
//! the cluster ladder later calls a compare-and-swap, a lock or a leader election is this
//! endpoint with different comparisons in it.
//!
//! The shape is an if-then-else: every entry of `compare` has to hold for `success` to run,
//! and otherwise `failure` runs instead. Both branches are ordinary operations, so both can
//! write, and `responses` carries one entry per operation of whichever branch ran, in order
//! and tagged with the kind of operation it was.
//!
//! Two traps. `succeeded` is a boolean at its zero value when the failure branch ran, so it
//! may be omitted entirely rather than sent as `false` — which is why nothing here asserts
//! the field is present. And a transaction is **one** step of the store's clock: two puts
//! inside it share a single revision, because there is no moment between them at which a
//! reader could see one and not the other.

use crate::assert::Check;
use crate::dist_test;
use crate::etcd::{
    b64, cmp_create_eq, cmp_value_eq, cmp_version_eq, op_delete, op_put, op_range, TxnOpResponse,
};
use crate::examples::{node_example_after, ExampleSpec};
use crate::stages::{ok, Ladder, Stage, Test};
use serde_json::{json, Value};

/// Stage 28.
pub fn stage() -> Stage {
    Stage {
        number: 28,
        slug: "txn_branches",
        name: "Transactions: compare, success, failure",
        ext: false,
        ladder: Ladder::Node,
        hints: &[
            "POST /v3/kv/txn runs the success branch when every comparison holds, the failure one otherwise",
            "succeeded is false when the failure branch ran, and may be omitted rather than sent as false",
            "responses carries one entry per operation of the branch that ran, in order",
            "A transaction is one atomic step: its writes share one revision",
        ],
        examples,
        tests: vec![
            Test::new("a transaction with no comparisons succeeds", an_empty_transaction),
            Test::new("a comparison that holds runs the success branch", the_success_branch),
            Test::new("a comparison that does not hold runs the failure branch", the_failure_branch),
            Test::new("responses carries one entry per operation, in order", responses_line_up),
            Test::new("two puts in one transaction share one revision", one_atomic_step),
            Test::new("every comparison has to hold", all_comparisons_must_hold),
            Test::new("a range inside a transaction sees the transaction's own writes", a_nested_range),
            Test::new("a transaction that writes nothing does not move the revision", a_read_only_transaction),
        ],
    }
}

/// One key the examples compare against.
fn one_key() -> Vec<(String, Value)> {
    vec![(
        "/v3/kv/put".into(),
        json!({"key": b64(b"cmp"), "value": b64(b"one")}),
    )]
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        node_example_after("A comparison that holds", one_key, "/v3/kv/txn", || {
            json!({
                "compare": [cmp_value_eq(b"cmp", b"one")],
                "success": [op_put(b"cmp", b"two"), op_put(b"cmp/other", b"x")],
                "failure": [op_range(b"cmp")],
            })
        })
        .request("write both keys, but only if `cmp` still holds `one`")
        .response("succeeded true, and one response per put, both at the same revision")
        .note(
            "Both puts report the transaction's revision, not two consecutive ones. That is \
             what atomic means here: no reader can be shown a store in which the first \
             write happened and the second did not.",
        ),
        node_example_after("A comparison that does not", one_key, "/v3/kv/txn", || {
            json!({
                "compare": [cmp_value_eq(b"cmp", b"something else")],
                "success": [op_put(b"cmp", b"two")],
                "failure": [op_range(b"cmp")],
            })
        })
        .request("the same transaction, with a comparison that is now wrong")
        .response("the failure branch, and no `succeeded` field at all")
        .note(
            "`succeeded` is false, and false is the zero value of a boolean, so protobuf's \
             JSON mapping drops it. Reading a missing `succeeded` as anything but false is \
             the mistake; asserting it is present is the other one.",
        ),
    ]
}

/// The name of the kind of operation an entry of `responses` is, for a report line.
fn kind_of(r: &TxnOpResponse) -> &'static str {
    match r {
        TxnOpResponse::Put(_) => "put",
        TxnOpResponse::Range(_) => "range",
        TxnOpResponse::Delete(_) => "delete",
        TxnOpResponse::Txn(_) => "txn",
        TxnOpResponse::Unknown(_) => "unknown",
    }
}

dist_test!(an_empty_transaction, |ctx| {
    let kv = ctx.kv()?;
    let key = ctx.key("k");
    let txn = ok(
        kv.txn(vec![], vec![op_put(&key, b"v")], vec![op_delete(&key)])
            .await,
        "a transaction with no comparisons at all",
    )?;
    let read = ok(
        kv.get_key(&key).await,
        "read the key the success branch wrote",
    )?;
    let mut c = Check::new("a transaction with an empty compare list");
    // Vacuously true: nothing failed, so the success branch is the one that runs.
    c.that(
        "txn.succeeded",
        "true, because no comparison could fail",
        txn.succeeded,
        txn.succeeded,
    );
    c.eq("txn.responses.len()", 1, txn.responses.len());
    c.eq(
        "txn.responses[0] kind",
        "put",
        txn.responses.first().map(kind_of).unwrap_or("nothing"),
    );
    c.eq("range.count", 1, read.count);
    c.eq(
        "range.kvs[0].value",
        "v".to_string(),
        read.one().map(|p| p.value_str()).unwrap_or_default(),
    );
    c.finish()
});

dist_test!(the_success_branch, |ctx| {
    let kv = ctx.kv()?;
    let key = ctx.key("k");
    let other = ctx.key("other");
    ok(
        kv.put(&key, b"one").await,
        "put the key the comparison looks at",
    )?;
    let txn = ok(
        kv.txn(
            vec![cmp_value_eq(&key, b"one")],
            vec![op_put(&key, b"two")],
            vec![op_put(&other, b"failed")],
        )
        .await,
        "a transaction whose comparison holds",
    )?;
    let read = ok(kv.get_key(&key).await, "read the key back")?;
    let untouched = ok(
        kv.get_key(&other).await,
        "read the key the failure branch would have written",
    )?;
    let mut c = Check::new("a transaction whose comparison holds");
    c.that("txn.succeeded", "true", txn.succeeded, txn.succeeded);
    c.eq("txn.responses.len()", 1, txn.responses.len());
    c.eq(
        "range.kvs[0].value",
        "two".to_string(),
        read.one().map(|p| p.value_str()).unwrap_or_default(),
    );
    // The branch that did not run wrote nothing at all.
    c.eq("range(other).count", 0, untouched.count);
    c.finish()
});

dist_test!(the_failure_branch, |ctx| {
    let kv = ctx.kv()?;
    let key = ctx.key("k");
    let other = ctx.key("other");
    ok(
        kv.put(&key, b"one").await,
        "put the key the comparison looks at",
    )?;
    let txn = ok(
        kv.txn(
            vec![cmp_value_eq(&key, b"something else")],
            vec![op_put(&key, b"two")],
            vec![op_put(&other, b"failed")],
        )
        .await,
        "a transaction whose comparison does not hold",
    )?;
    let read = ok(
        kv.get_key(&key).await,
        "read the key the success branch would have written",
    )?;
    let written = ok(
        kv.get_key(&other).await,
        "read the key the failure branch wrote",
    )?;
    let mut c = Check::new("a transaction whose comparison does not hold");
    // `succeeded` may be omitted here rather than sent as false; the decoder reads a
    // missing boolean as false, so this asserts the value and never the presence.
    c.that("txn.succeeded", "false", !txn.succeeded, txn.succeeded);
    c.eq("txn.responses.len()", 1, txn.responses.len());
    c.eq(
        "range(k).kvs[0].value",
        "one".to_string(),
        read.one().map(|p| p.value_str()).unwrap_or_default(),
    );
    c.eq(
        "range(other).kvs[0].value",
        "failed".to_string(),
        written.one().map(|p| p.value_str()).unwrap_or_default(),
    );
    c.finish()
});

dist_test!(responses_line_up, |ctx| {
    let kv = ctx.kv()?;
    let (a, b, gone) = (ctx.key("a"), ctx.key("b"), ctx.key("gone"));
    ok(
        kv.put(&gone, b"doomed").await,
        "put a key for the transaction to delete",
    )?;
    let txn = ok(
        kv.txn(
            vec![],
            vec![
                op_put(&a, b"1"),
                op_range(&gone),
                op_delete(&gone),
                op_put(&b, b"2"),
            ],
            vec![],
        )
        .await,
        "a transaction with four operations of three kinds",
    )?;
    let mut c = Check::new("the responses of a four-operation branch");
    // One entry per operation, in the order they were written, each of the matching kind.
    c.eq("txn.responses.len()", 4, txn.responses.len());
    c.eq(
        "txn.responses kinds",
        vec!["put", "range", "delete", "put"],
        txn.responses.iter().map(kind_of).collect::<Vec<_>>(),
    );
    c.eq(
        "txn.responses[1].kvs[0].value",
        Some("doomed".to_string()),
        txn.range(1).and_then(|r| r.one()).map(|p| p.value_str()),
    );
    c.eq(
        "txn.responses[2].deleted",
        Some(1),
        txn.delete(2).map(|d| d.deleted),
    );
    c.finish()
});

dist_test!(one_atomic_step, |ctx| {
    let kv = ctx.kv()?;
    let (a, b) = (ctx.key("a"), ctx.key("b"));
    let before = ok(kv.put(&ctx.key("anchor"), b"v").await, "put a key first")?;
    let txn = ok(
        kv.txn(vec![], vec![op_put(&a, b"1"), op_put(&b, b"2")], vec![])
            .await,
        "a transaction holding two puts",
    )?;
    let read_a = ok(kv.get_key(&a).await, "read the first key")?;
    let read_b = ok(kv.get_key(&b).await, "read the second key")?;
    let mut c = Check::new("two puts inside one transaction");
    // One request, one revision: the two writes are simultaneous as far as the store is
    // concerned, so there is no moment at which a reader sees one without the other.
    c.eq(
        "txn.header.revision - the revision before it",
        1,
        txn.header.revision - before.header.revision,
    );
    c.eq(
        "range(a).kvs[0].mod_revision",
        txn.header.revision,
        read_a.one().map(|p| p.mod_revision).unwrap_or(0),
    );
    c.eq(
        "range(b).kvs[0].mod_revision",
        txn.header.revision,
        read_b.one().map(|p| p.mod_revision).unwrap_or(0),
    );
    c.eq(
        "the two mod_revisions",
        read_a.one().map(|p| p.mod_revision),
        read_b.one().map(|p| p.mod_revision),
    );
    c.finish()
});

dist_test!(all_comparisons_must_hold, |ctx| {
    let kv = ctx.kv()?;
    let (a, b, marker) = (ctx.key("a"), ctx.key("b"), ctx.key("marker"));
    let put_a = ok(kv.put(&a, b"one").await, "put a")?;
    ok(kv.put(&b, b"two").await, "put b")?;
    let all_hold = ok(
        kv.txn(
            vec![
                cmp_value_eq(&a, b"one"),
                cmp_value_eq(&b, b"two"),
                cmp_create_eq(&a, put_a.header.revision),
                cmp_version_eq(&marker, 0),
            ],
            vec![op_put(&marker, b"set")],
            vec![],
        )
        .await,
        "a transaction whose four comparisons all hold",
    )?;
    let one_fails = ok(
        kv.txn(
            vec![
                cmp_value_eq(&a, b"one"),
                cmp_value_eq(&b, b"the wrong value"),
            ],
            vec![op_put(&ctx.key("must/not/exist"), b"x")],
            vec![],
        )
        .await,
        "a transaction with one comparison out of two wrong",
    )?;
    let forbidden = ok(
        kv.get_key(&ctx.key("must/not/exist")).await,
        "read the key the success branch would have written",
    )?;
    let mut c = Check::new("a transaction with several comparisons");
    c.that(
        "txn(all hold).succeeded",
        "true",
        all_hold.succeeded,
        all_hold.succeeded,
    );
    // Comparisons are an AND, so one wrong entry is enough.
    c.that(
        "txn(one wrong).succeeded",
        "false",
        !one_fails.succeeded,
        one_fails.succeeded,
    );
    c.eq("range(must/not/exist).count", 0, forbidden.count);
    // cmp_version_eq against 0 is how a transaction says "this key does not exist", and it
    // held above because `marker` had never been written.
    c.eq(
        "range(marker).kvs[0].value",
        "set".to_string(),
        ok(kv.get_key(&marker).await, "read the marker")?
            .one()
            .map(|p| p.value_str())
            .unwrap_or_default(),
    );
    c.finish()
});

dist_test!(a_nested_range, |ctx| {
    let kv = ctx.kv()?;
    let key = ctx.key("k");
    ok(kv.put(&key, b"one").await, "put the key")?;
    let txn = ok(
        kv.txn(
            vec![cmp_value_eq(&key, b"one")],
            vec![op_put(&key, b"two"), op_range(&key)],
            vec![op_range(&key)],
        )
        .await,
        "a transaction that writes a key and then reads it",
    )?;
    let mut c = Check::new("a range nested inside a transaction");
    c.that("txn.succeeded", "true", txn.succeeded, txn.succeeded);
    c.eq("txn.responses.len()", 2, txn.responses.len());
    let Some(nested) = txn.range(1) else {
        return c
            .that(
                "txn.responses[1]",
                "a range response",
                false,
                txn.responses.get(1).map(kind_of),
            )
            .finish();
    };
    // The operations of a branch run in order and share the transaction's state, so the
    // range sees the put that preceded it rather than the value the comparison looked at.
    c.eq("txn.responses[1].count", 1, nested.count);
    c.eq(
        "txn.responses[1].kvs[0].value",
        "two".to_string(),
        nested.one().map(|p| p.value_str()).unwrap_or_default(),
    );
    c.eq(
        "txn.responses[1].kvs[0].mod_revision",
        txn.header.revision,
        nested.one().map(|p| p.mod_revision).unwrap_or(0),
    );
    c.finish()
});

dist_test!(a_read_only_transaction, |ctx| {
    let kv = ctx.kv()?;
    let key = ctx.key("k");
    let put = ok(kv.put(&key, b"v").await, "put a key")?;
    let txn = ok(
        kv.txn(
            vec![cmp_value_eq(&key, b"v")],
            vec![op_range(&key)],
            vec![op_range(&key)],
        )
        .await,
        "a transaction that only reads",
    )?;
    let after = ok(kv.status().await, "ask where the clock is now")?;
    let mut c = Check::new("a transaction that writes nothing");
    c.that("txn.succeeded", "true", txn.succeeded, txn.succeeded);
    // A transaction is a write only when it writes: a read-only one leaves the clock alone.
    c.eq(
        "txn.header.revision",
        put.header.revision,
        txn.header.revision,
    );
    c.eq(
        "status.header.revision after the transaction",
        put.header.revision,
        after.header.revision,
    );
    c.eq(
        "txn.responses[0].kvs[0].value",
        Some("v".to_string()),
        txn.range(0).and_then(|r| r.one()).map(|p| p.value_str()),
    );
    c.finish()
});
