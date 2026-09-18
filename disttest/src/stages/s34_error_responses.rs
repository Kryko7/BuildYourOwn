//! Stage 34 — Error responses.
//!
//! Everything so far has been about what the store does when it is asked something
//! sensible. This stage is about the other half of an API: what it says when it is not, and
//! what it must not do while saying it.
//!
//! The shape is fixed — a 4xx status and `{"code": <grpc code>, "message": "..."}` — and
//! the codes are the gRPC ones, so a client that already speaks etcd can tell "you sent
//! rubbish" (3) from "that lease is gone" (5) from "that was too big" (8) without reading
//! English. The harder half is the part no status code shows: a request the store refuses
//! must leave the store exactly as it was, and must not take the connection down with it.

use crate::assert::Check;
use crate::dist_test;
use crate::etcd::{
    b64, PutRequest, CODE_INVALID_ARGUMENT, CODE_NOT_FOUND, CODE_RESOURCE_EXHAUSTED,
};
use crate::examples::{node_example, ExampleSpec};
use crate::stages::{expect_code, ok, Ladder, Stage, Test};
use serde_json::{json, Value};

/// Comfortably over the 1.5 MiB or so a real etcd will accept in one request.
const TOO_BIG: usize = 2_500_000;

/// Stage 34.
pub fn stage() -> Stage {
    Stage {
        number: 34,
        slug: "error_responses",
        name: "Error responses",
        ext: true,
        ladder: Ladder::Node,
        hints: &[
            "Errors are a 4xx status and a body of {\"code\": <grpc code>, \"message\": \"...\"}",
            "Bad base64 is code 3, a lease that does not exist is code 5, too large is code 8",
            "An unknown path is 404 and never a 200 with an empty body",
            "A bad request must not change the store, and must not close the connection",
        ],
        examples,
        tests: vec![
            Test::new(
                "a key that is not base64 is code 3",
                bad_base64_is_invalid_argument,
            ),
            Test::new(
                "a lease that does not exist is code 5",
                unknown_lease_is_not_found,
            ),
            Test::new(
                "a request that is too large is code 8",
                too_large_is_resource_exhausted,
            ),
            Test::new("an unknown path is a 404", unknown_path_is_404),
            Test::new("a body that is not JSON is refused", not_json_is_refused),
            Test::new(
                "an error body carries a code and a message",
                error_bodies_have_a_shape,
            ),
            Test::new("a refused request changes nothing", refusals_change_nothing),
            Test::new(
                "the store keeps serving afterwards",
                the_store_survives_a_battering,
            ),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        node_example(
            "A key that is not base64",
            "/v3/kv/range",
            || json!({ "key": "not base64!" }),
        )
        .request("a range whose key is plain text where base64 was required")
        .response("HTTP 400 and `{\"code\":3,\"message\":\"...\"}` — invalid argument")
        .note(
            "Every key and value on this wire is base64, and the decoder rejects the \
             request before the store ever sees it. Answering 200 with an empty result \
             here is the bug this test exists to catch: the caller would read it as \
             \"no such key\".",
        ),
        node_example(
            "A put onto a lease nobody granted",
            "/v3/kv/put",
            || json!({"key": b64(b"oops"), "value": b64(b"v"), "lease": "999999"}),
        )
        .request("a perfectly well-formed put naming a lease id that does not exist")
        .response(
            "HTTP 404 and `{\"code\":5,\"message\":\"etcdserver: requested lease not found\"}`",
        )
        .note(
            "Code 5 is not-found, and the request is refused whole: the key is not \
             written, not even without the lease. The HTTP status tracks the gRPC code, \
             which is why this one is a 404 and the bad-base64 one is a 400.",
        ),
    ]
}

dist_test!(bad_base64_is_invalid_argument, |ctx| {
    let kv = ctx.kv()?;
    expect_code(
        kv.post("/v3/kv/range", &json!({ "key": "!! not base64 !!" }))
            .await,
        CODE_INVALID_ARGUMENT,
        "a range whose key is not base64",
    )?;
    expect_code(
        kv.post(
            "/v3/kv/put",
            &json!({"key": b64(b"k"), "value": "also not base64"}),
        )
        .await,
        CODE_INVALID_ARGUMENT,
        "a put whose value is not base64",
    )?;
    Ok(())
});

dist_test!(unknown_lease_is_not_found, |ctx| {
    let key = ctx.key("leaseless");
    let kv = ctx.kv()?;
    expect_code(
        kv.put_req(&PutRequest::new(&key, b"v").lease(424_242_424))
            .await,
        CODE_NOT_FOUND,
        "a put naming a lease that was never granted",
    )?;
    expect_code(
        kv.lease_revoke(424_242_424).await,
        CODE_NOT_FOUND,
        "a revoke of a lease that was never granted",
    )?;
    let read = ok(
        kv.get_key(&key).await,
        "look for the key the refused put named",
    )?;
    let mut c = Check::new("what a refused lease leaves behind");
    c.eq("range.count", 0, read.count);
    c.finish()
});

dist_test!(too_large_is_resource_exhausted, |ctx| {
    let key = ctx.key("whale");
    let kv = ctx.kv()?;
    let huge = vec![b'x'; TOO_BIG];
    expect_code(
        kv.put(&key, &huge).await,
        CODE_RESOURCE_EXHAUSTED,
        "a put of a value far larger than the server accepts",
    )?;
    let read = ok(
        kv.get_key(&key).await,
        "look for the key the huge put named",
    )?;
    let mut c = Check::new("a request the server will not carry");
    c.note(format!("the value was {TOO_BIG} bytes before base64"));
    // A limit that is enforced by truncating, or by storing it anyway, is not a limit.
    c.eq("range.count", 0, read.count);
    c.finish()
});

dist_test!(unknown_path_is_404, |ctx| {
    let kv = ctx.kv()?;
    let mut c = Check::new("paths the server does not serve");
    for path in ["/v3/kv/nonsense", "/v3/does/not/exist", "/nope"] {
        let resp = ok(
            kv.post_raw(path, &json!({})).await,
            "post to a path that does not exist",
        )?;
        c.eq(&format!("POST {path} status"), 404, resp.status);
        c.block(format!("POST {path}"), resp.text());
    }
    c.finish()
});

dist_test!(not_json_is_refused, |ctx| {
    let kv = ctx.kv()?;
    let mut c = Check::new("bodies that are not JSON");
    for body in [
        &b"this is not json at all"[..],
        &b"{\"key\": "[..],
        &[0x00, 0xff, 0xfe, 0x01][..],
    ] {
        let resp = ok(
            kv.post_bytes("/v3/kv/range", body).await,
            "post a body that is not JSON",
        )?;
        let label = String::from_utf8_lossy(body).to_string();
        c.that(
            &format!("status of a request whose body is {label:?}"),
            "a 4xx",
            (400..500).contains(&resp.status),
            resp.status,
        );
        c.block(format!("the answer to {label:?}"), resp.text());
    }
    c.finish()
});

dist_test!(error_bodies_have_a_shape, |ctx| {
    let kv = ctx.kv()?;
    let resp = ok(
        kv.post_raw("/v3/kv/range", &json!({ "key": "%%%%" })).await,
        "send a request that must be refused",
    )?;
    let body: Value = serde_json::from_str(&resp.text()).unwrap_or(Value::Null);
    let mut c = Check::new("the shape of an error body");
    c.block("the answer", resp.text());
    c.that(
        "the status",
        "a 4xx",
        (400..500).contains(&resp.status),
        resp.status,
    );
    c.eq(
        "error.code",
        Some(CODE_INVALID_ARGUMENT),
        body.get("code").and_then(Value::as_i64),
    );
    c.that(
        "error.message",
        "a non-empty string saying what was wrong",
        body.get("message")
            .and_then(Value::as_str)
            .is_some_and(|m| !m.trim().is_empty()),
        body.get("message")
            .cloned()
            .unwrap_or(Value::Null)
            .to_string(),
    );
    c.finish()
});

dist_test!(refusals_change_nothing, |ctx| {
    let key = ctx.key("witness");
    let kv = ctx.kv()?;
    let put = ok(
        kv.put(&key, b"untouched").await,
        "write the key the refusals must not touch",
    )?;
    // Every way of being wrong this stage knows about, one after another.
    let _ = kv
        .post("/v3/kv/put", &json!({"key": "!!!", "value": "!!!"}))
        .await;
    let _ = kv
        .post(
            "/v3/kv/deleterange",
            &json!({"key": "not base64", "range_end": "also not"}),
        )
        .await;
    let _ = kv.post_bytes("/v3/kv/put", b"{ this is not json").await;
    let _ = kv.post_raw("/v3/kv/no-such-endpoint", &json!({})).await;
    let _ = kv
        .put_req(&PutRequest::new(&key, b"clobbered").lease(31_415_926))
        .await;
    let _ = kv
        .post("/v3/kv/txn", &json!({"compare": "not a list"}))
        .await;
    let read = ok(kv.get_key(&key).await, "read the witness key back")?;
    let status = ok(kv.status().await, "ask the store where its clock is")?;
    let mut c = Check::new("the store after six refused requests");
    c.eq("range.count", 1, read.count);
    c.eq(
        "range.kvs[0].value",
        "untouched".to_string(),
        read.one().map(|k| k.value_str()).unwrap_or_default(),
    );
    c.eq(
        "range.kvs[0].mod_revision",
        put.header.revision,
        read.one().map(|k| k.mod_revision).unwrap_or(0),
    );
    // Nothing was written, so nothing moved the clock either.
    c.eq(
        "status.header.revision",
        put.header.revision,
        status.header.revision,
    );
    c.finish()
});

dist_test!(the_store_survives_a_battering, |ctx| {
    let before_key = ctx.key("before");
    let after_key = ctx.key("after");
    let kv = ctx.kv()?;
    let before = ok(
        kv.put(&before_key, b"v").await,
        "write a key before the battering",
    )?;
    for _ in 0..10 {
        let _ = kv.post("/v3/kv/range", &json!({ "key": "!!!" })).await;
        let _ = kv.post_bytes("/v3/kv/range", b"garbage").await;
        let _ = kv.post_raw("/v3/nowhere", &json!({})).await;
    }
    // The connection is a keep-alive one: a refusal that closes it turns every later
    // request into a reconnect, and a refusal that wedges it turns them into timeouts.
    let after = ok(kv.put(&after_key, b"v").await, "write a key afterwards")?;
    let read = ok(kv.get_key(&before_key).await, "read the earlier key")?;
    let health = ok(kv.health().await, "ask for health")?;
    let mut c = Check::new("the store after thirty bad requests");
    c.eq(
        "put(after).header.revision",
        before.header.revision + 1,
        after.header.revision,
    );
    c.eq("range(before).count", 1, read.count);
    c.that(
        "the health body",
        "something that parses as JSON",
        health.is_object() || health.is_string(),
        health.to_string(),
    );
    c.finish()
});
