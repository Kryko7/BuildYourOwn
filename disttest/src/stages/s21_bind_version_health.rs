//! Stage 21 — Bind, /version and /health.
//!
//! Before any key exists there is a socket. This stage is about being a server at all: one
//! process listening on the address the harness chose, answering HTTP/1.1, and still
//! answering on the same connection fifty requests later.
//!
//! Two traps live here. The first is keep-alive: a server that closes the connection after
//! every answer passes a single request and then runs the ephemeral port range dry under the
//! cluster workload, so the suite checks reuse now rather than at stage 54. The second is
//! what happens when the request is wrong — an unknown path, the wrong method, a body that
//! is not JSON. All three have to be refused *and survived*; a server that answers 200 with
//! an empty body, or that drops the connection rather than the request, fails every later
//! stage in a way that looks like something else.

use crate::assert::Check;
use crate::dist_test;
use crate::etcd::RangeRequest;
use crate::examples::{node_example, ExampleSpec};
use crate::stages::{ok, Ladder, Stage, Test};
use serde_json::{json, Value};

/// Stage 21.
pub fn stage() -> Stage {
    Stage {
        number: 21,
        slug: "bind_version_health",
        name: "Bind, /version and /health",
        ext: false,
        ladder: Ladder::Node,
        hints: &[
            "Listen on the --listen-client-urls address and answer HTTP/1.1 on it",
            "GET /version answers {\"etcdserver\":..,\"etcdcluster\":..} and GET /health {\"health\":\"true\"}",
            "Keep the connection alive: the suite reuses one connection for thousands of requests",
            "An unknown path is 404, and a GET on a POST-only endpoint is not a 200",
        ],
        examples,
        tests: vec![
            Test::new("GET /version answers both version strings", version_endpoint),
            Test::new("GET /health says the node is healthy", health_endpoint),
            Test::new("an unknown path is a 404, not an empty 200", unknown_path_is_404),
            Test::new("a refused path leaves the connection usable", refusal_keeps_the_connection).ext(),
            Test::new("a GET on a POST-only endpoint is not a 200", get_on_a_post_endpoint),
            Test::new("fifty requests in a row are all answered", fifty_requests),
            Test::new("a body that is not JSON is refused, and the node keeps serving", not_json),
            Test::new("the status endpoint reports a version, a member id and a revision", status_endpoint),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        node_example(
            "What the status endpoint reports",
            "/v3/maintenance/status",
            || json!({}),
        )
        .request("an empty object, because status takes no arguments")
        .response("the header every endpoint carries, plus the server's own version and sizes")
        .note(
            "The revision in the header is the store's clock and is at least 1 on an empty \
             store, because bringing the cluster up is itself a write. member_id identifies \
             the server that answered and is never zero.",
        ),
        node_example("An endpoint that is not there", "/v3/kv/nonesuch", || {
            json!({})
        })
        .request("a well-formed POST to a path the API does not define")
        .response("404, and nothing that could be mistaken for a successful empty answer")
        .note(
            "A catch-all that answers 200 with an empty body is the trap: every decoder \
                 in this suite reads a missing field as its zero, so an empty 200 looks \
                 exactly like a successful request that found nothing.",
        ),
    ]
}

/// Parse a response body as JSON, or hand back `Null` so the check can report what arrived.
fn body_json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}

/// True when `field` of `v` is a string with something in it.
fn non_empty_string(v: &Value, field: &str) -> bool {
    v.get(field)
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty())
}

dist_test!(version_endpoint, |ctx| {
    let resp = ok(ctx.kv()?.get("/version").await, "GET /version")?;
    let body = body_json(&resp.text());
    let mut c = Check::new("GET /version");
    c.eq("GET /version: status", 200u16, resp.status);
    c.that(
        "version.etcdserver",
        "a non-empty version string",
        non_empty_string(&body, "etcdserver"),
        body.get("etcdserver").cloned(),
    );
    c.that(
        "version.etcdcluster",
        "a non-empty version string",
        non_empty_string(&body, "etcdcluster"),
        body.get("etcdcluster").cloned(),
    );
    c.block(
        "the server answered",
        format!("{}\n\n{}", resp.head(), resp.text()),
    );
    c.finish()
});

dist_test!(health_endpoint, |ctx| {
    let resp = ok(ctx.kv()?.get("/health").await, "GET /health")?;
    let body = body_json(&resp.text());
    let mut c = Check::new("GET /health");
    c.eq("GET /health: status", 200u16, resp.status);
    // The field is the string "true", not the boolean: this endpoint predates the v3 API
    // and never adopted its types. Real etcd sends a `reason` alongside it, so the check
    // reads one field rather than comparing the whole object.
    c.eq(
        "health.health",
        Some("true"),
        body.get("health").and_then(Value::as_str),
    );
    c.block(
        "the server answered",
        format!("{}\n\n{}", resp.head(), resp.text()),
    );
    c.finish()
});

dist_test!(unknown_path_is_404, |ctx| {
    let resp = ok(
        ctx.kv()?.get("/v3/kv/no/such/endpoint").await,
        "GET a path the API does not define",
    )?;
    let mut c = Check::new("a path that is not part of the API");
    c.that(
        "GET /v3/kv/no/such/endpoint: status",
        "not a success, because nothing was found to succeed at",
        !(200..300).contains(&resp.status),
        resp.status,
    );
    c.eq("GET /v3/kv/no/such/endpoint: status", 404u16, resp.status);
    c.block(
        "the server answered",
        format!("{}\n\n{}", resp.head(), resp.text()),
    );
    c.finish()
});

dist_test!(refusal_keeps_the_connection, |ctx| {
    let kv = ctx.kv()?;
    let key = ctx.key("after404");
    let refused = ok(kv.get("/definitely/not/here").await, "GET an unknown path")?;
    // The point is what happens next: the harness pools connections, so a server that
    // closes the socket instead of answering the request loses the one after it too.
    let put = ok(kv.put(&key, b"v").await, "put a key after the refusal")?;
    let read = ok(kv.get_key(&key).await, "read that key back")?;
    let mut c = Check::new("a request that follows a refused one");
    c.eq("GET /definitely/not/here: status", 404u16, refused.status);
    c.at_least("put.header.revision", 1, put.header.revision);
    c.eq("range.count", 1, read.count);
    c.eq(
        "range.kvs[0].value",
        "v".to_string(),
        read.one().map(|kv| kv.value_str()).unwrap_or_default(),
    );
    c.finish()
});

dist_test!(get_on_a_post_endpoint, |ctx| {
    let resp = ok(
        ctx.kv()?.get("/v3/kv/range").await,
        "GET an endpoint that only takes POST",
    )?;
    let mut c = Check::new("the wrong method on a real endpoint");
    // Real etcd answers 501; 404 and 405 are equally defensible. What is not defensible is
    // a 200, because a GET carries no body and so asked for nothing.
    c.that(
        "GET /v3/kv/range: status",
        "anything but a success, because a GET carries no request body",
        !(200..300).contains(&resp.status),
        resp.status,
    );
    c.block(
        "the server answered",
        format!("{}\n\n{}", resp.head(), resp.text()),
    );
    c.finish()
});

dist_test!(fifty_requests, |ctx| {
    let kv = ctx.kv()?;
    let mut statuses = Vec::new();
    let mut closed = Vec::new();
    let mut written = 0usize;
    for i in 0..50 {
        if i % 2 == 0 {
            let resp = ok(kv.get("/health").await, "GET /health in a loop")?;
            statuses.push(resp.status);
            if let Some(v) = resp.header("connection") {
                if v.eq_ignore_ascii_case("close") {
                    closed.push(i);
                }
            }
        } else {
            let key = ctx.key(&format!("busy{i:02}"));
            ok(
                kv.put(&key, format!("v{i}").as_bytes()).await,
                "put in a loop",
            )?;
            written += 1;
        }
    }
    let prefix = ctx.key("busy");
    let read = ok(
        kv.range_req(&RangeRequest::prefix(&prefix)).await,
        "read every key the loop wrote",
    )?;
    let mut c = Check::new("fifty requests over one client");
    c.eq(
        "GET /health answered 200 this many times",
        25,
        statuses.iter().filter(|s| **s == 200).count(),
    );
    c.that(
        "responses carrying Connection: close",
        "none, because the harness reuses the connection it opened",
        closed.is_empty(),
        &closed,
    );
    c.eq(
        "range.count over the keys the loop wrote",
        written as i64,
        read.count,
    );
    c.finish()
});

dist_test!(not_json, |ctx| {
    let kv = ctx.kv()?;
    let key = ctx.key("afterjunk");
    let resp = ok(
        kv.post_bytes("/v3/kv/put", b"this is not JSON, it is a sentence")
            .await,
        "POST a body that is not JSON",
    )?;
    let put = ok(kv.put(&key, b"v").await, "put a key after the bad request")?;
    let mut c = Check::new("a request body the server cannot parse");
    c.that(
        "POST /v3/kv/put with a non-JSON body: status",
        "a 4xx, because the request was the client's mistake",
        (400..500).contains(&resp.status),
        resp.status,
    );
    c.at_least(
        "the put that followed: header.revision",
        1,
        put.header.revision,
    );
    c.block(
        "the server answered",
        format!("{}\n\n{}", resp.head(), resp.text()),
    );
    c.finish()
});

dist_test!(status_endpoint, |ctx| {
    let status = ok(ctx.kv()?.status().await, "POST /v3/maintenance/status")?;
    let mut c = Check::new("the status of an empty store");
    c.that(
        "status.version",
        "a non-empty version string",
        !status.version.is_empty(),
        status.version.clone(),
    );
    c.ne("status.header.member_id", 0, status.header.member_id);
    // Not zero: bringing a cluster up is itself a write, so an empty store is already at 1.
    c.at_least("status.header.revision", 1, status.header.revision);
    c.at_least("status.header.raft_term", 1, status.header.raft_term);
    c.finish()
});
