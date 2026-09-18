//! Stage 20 — Fetch with no topics at all.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::kafka_test;
use crate::stages::{fetch_request, proto_fail, Stage, Test, FETCH_V16, NONE};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 20,
        slug: "fetch_no_topics",
        name: "Fetch with no topics",
        ext: false,
        hints: &[
            "v16 response: throttle_time_ms(int32), error_code(int16), session_id(int32), \
             responses(COMPACT_ARRAY), tagged fields",
            "An empty compact array is the single byte 0x01 (count + 1)",
            "Answer immediately when there is nothing to wait for, whatever max_wait_ms says",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("the top-level error code is 0", error_code),
            Test::new("the responses array is empty", empty_responses),
            Test::new("throttle_time_ms is not negative", throttle),
            Test::new("the body decodes with no trailing bytes", decodes_cleanly),
            Test::new("an empty fetch answers quickly", answers_quickly),
        ],
    }
}

kafka_test!(error_code, |ctx| {
    let mut conn = ctx.connect().await?;
    let req = fetch_request(&[], 500);
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("a Fetch v16 request with no topics", &conn);
    c.eq("response.error_code", NONE, resp.error_code);
    c.finish()
});

kafka_test!(empty_responses, |ctx| {
    let mut conn = ctx.connect().await?;
    let req = fetch_request(&[], 500);
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the responses array of an empty fetch", &conn);
    c.eq("response.responses.len", 0usize, resp.responses.len());
    c.finish()
});

kafka_test!(throttle, |ctx| {
    let mut conn = ctx.connect().await?;
    let req = fetch_request(&[], 500);
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("throttle_time_ms of an empty fetch", &conn);
    c.at_least("response.throttle_time_ms", 0i32, resp.throttle_time_ms);
    c.finish()
});

kafka_test!(decodes_cleanly, |ctx| {
    let mut conn = ctx.connect().await?;
    let req = fetch_request(&[], 500);
    let decoded = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the Fetch v16 response body", &conn);
    c.eq("response.trailing_bytes", 0usize, decoded.trailing);
    c.finish()
});

kafka_test!(answers_quickly, |ctx| {
    let mut conn = ctx.connect().await?;
    let req = fetch_request(&[], 5_000);
    let started = std::time::Instant::now();
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let elapsed = started.elapsed().as_millis();
    let mut c = Check::new("how long an empty fetch with max_wait_ms=5000 takes", &conn);
    c.note("with no partitions there is nothing to wait for, so the broker should not sleep");
    c.eq("response.error_code", NONE, resp.error_code);
    c.that(
        "response.elapsed_ms",
        "well under max_wait_ms (5000)",
        elapsed < 3_000,
        elapsed,
    );
    c.finish()
});

/// Worked examples: the smallest fetch there is, and why it must not wait.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("Fetch v16 with an empty topics array", |env| {
            env.request(FETCH_V16, 201, &fetch_request(&[], 500))
        })
        .request(
            "Fetch v16, correlation id 201: max_wait_ms 500, min_bytes 1, max_bytes 10485760, \
             session_id 0, session_epoch 0, replica_id -1, and a topics array of length zero",
        )
        .response(
            "A response header v1 — correlation_id then a tagged-field byte — and then \
             throttle_time_ms 0, error_code 0 (NONE), the session_id, an empty responses \
             compact array (the single byte 0x01) and the body's own tagged-field byte",
        )
        .note(
            "Fetch v16 is flexible, so its response header carries a tagged-field byte that the \
             ApiVersions response header does not. Forgetting that one byte shifts every field \
             after it and the client decodes garbage.",
        ),
        ExampleSpec::wire("An empty fetch that asks to wait five seconds", |env| {
            env.request(FETCH_V16, 202, &fetch_request(&[], 5_000))
        })
        .request("The same empty Fetch v16, correlation id 202, but with max_wait_ms 5000")
        .response(
            "The same body — throttle_time_ms 0, error_code 0, empty responses — and it arrives \
             in milliseconds, not in five seconds",
        )
        .note(
            "max_wait_ms is how long the broker may hold the request waiting for records on the \
             partitions it was asked about. With no partitions there is nothing to wait for, so \
             answer at once; sleeping here is the classic first long-poll bug.",
        ),
    ]
}
