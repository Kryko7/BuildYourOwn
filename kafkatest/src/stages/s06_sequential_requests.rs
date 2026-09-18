//! Stage 06 — Several requests on one connection.

use crate::assert::Check;
use crate::examples::{ExampleSpec, Wire};
use crate::kafka_test;
use crate::stages::{
    api_versions_body_v4, api_versions_request, correlation_id_of, proto_fail, raw_flexible_frame,
    roundtrip_raw, Stage, Test, API_VERSIONS_KEY, API_VERSIONS_V4, NONE,
};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 6,
        slug: "sequential_requests",
        name: "Sequential requests on one connection",
        ext: false,
        hints: &[
            "Loop inside the connection handler instead of closing after one response",
            "Read exactly message_size bytes each time; never assume one read = one request",
            "Flush the response before reading the next request",
            "Treat read() returning 0 as the client closing, not as an error",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new(
                "five requests on one connection are all answered",
                five_requests,
            ),
            Test::new("the correlation ids stay in order", ids_in_order),
            Test::new("the connection survives twenty requests", twenty_requests),
            Test::new("each response body still decodes", bodies_decode),
            Test::new("two connections can be used alternately", alternating),
        ],
    }
}

kafka_test!(five_requests, |ctx| {
    let mut conn = ctx.connect().await?;
    for i in 0..5i32 {
        let frame = raw_flexible_frame(
            API_VERSIONS_KEY,
            API_VERSIONS_V4,
            1000 + i,
            Some("kafkatest"),
            &[],
            &api_versions_body_v4(),
        );
        let resp = roundtrip_raw(&mut conn, &frame)
            .await
            .map_err(|f| f.note(format!("request {} of 5 on one connection", i + 1)))?;
        let mut c = Check::new(format!("request {} of 5", i + 1), &conn);
        c.mark(0..4);
        c.eq(
            "response.correlation_id",
            1000 + i,
            correlation_id_of(&resp, &conn)?,
        );
        c.finish()?;
    }
    Ok(())
});

kafka_test!(ids_in_order, |ctx| {
    let mut conn = ctx.connect().await?;
    let ids = [7i32, 3, 9, 1];
    let mut seen = Vec::new();
    for id in ids {
        let frame = raw_flexible_frame(
            API_VERSIONS_KEY,
            API_VERSIONS_V4,
            id,
            Some("kafkatest"),
            &[],
            &api_versions_body_v4(),
        );
        let resp = roundtrip_raw(&mut conn, &frame).await?;
        seen.push(correlation_id_of(&resp, &conn)?);
    }
    let mut c = Check::new("the order responses came back in", &conn);
    c.eq("responses.correlation_ids", ids.to_vec(), seen);
    c.finish()
});

kafka_test!(twenty_requests, |ctx| {
    let mut conn = ctx.connect().await?;
    for i in 0..20i32 {
        let frame = raw_flexible_frame(
            API_VERSIONS_KEY,
            API_VERSIONS_V4,
            2000 + i,
            Some("kafkatest"),
            &[],
            &api_versions_body_v4(),
        );
        let resp = roundtrip_raw(&mut conn, &frame)
            .await
            .map_err(|f| f.note(format!("request {} of 20", i + 1)))?;
        let got = correlation_id_of(&resp, &conn)?;
        if got != 2000 + i {
            let mut c = Check::new(format!("request {} of 20", i + 1), &conn);
            c.mark(0..4);
            c.eq("response.correlation_id", 2000 + i, got);
            return c.finish();
        }
    }
    Ok(())
});

kafka_test!(bodies_decode, |ctx| {
    let mut conn = ctx.connect().await?;
    let req = api_versions_request();
    for i in 0..3 {
        let decoded = conn
            .request(API_VERSIONS_V4, &req)
            .await
            .map_err(|e| proto_fail(e, &conn).note(format!("request {} of 3", i + 1)))?;
        let mut c = Check::new(format!("the body of response {} of 3", i + 1), &conn);
        c.eq("response.error_code", NONE, decoded.error_code);
        c.eq("response.trailing_bytes", 0usize, decoded.trailing);
        c.finish()?;
    }
    Ok(())
});

kafka_test!(alternating, |ctx| {
    let mut a = ctx.connect().await?;
    let mut b = ctx.connect().await?;
    for i in 0..4i32 {
        for (label, conn, base) in [("A", &mut a, 3000i32), ("B", &mut b, 4000)] {
            let id = base + i;
            let frame = raw_flexible_frame(
                API_VERSIONS_KEY,
                API_VERSIONS_V4,
                id,
                Some("kafkatest"),
                &[],
                &api_versions_body_v4(),
            );
            let resp = roundtrip_raw(conn, &frame)
                .await
                .map_err(|f| f.note(format!("connection {label}, round {}", i + 1)))?;
            let got = correlation_id_of(&resp, conn)?;
            let mut c = Check::new(format!("connection {label}, round {}", i + 1), conn);
            c.mark(0..4);
            c.eq("response.correlation_id", id, got);
            c.finish()?;
        }
    }
    Ok(())
});

/// Worked examples: more than one request on a single connection.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("Two requests, one connection", |env| {
            Ok(Wire::Frames(vec![
                env.payload(API_VERSIONS_V4, 1, &api_versions_request())?,
                env.payload(API_VERSIONS_V4, 2, &api_versions_request())?,
            ]))
        })
        .request(
            "Two complete ApiVersions v4 frames on the same socket, correlation ids 1 and 2, \
             each with its own 4-byte size prefix",
        )
        .response(
            "Two response frames in the same order: correlation_id 1 then correlation_id 2, \
             both with error_code 0",
        )
        .note(
            "Loop on the same socket: read the size, read the body, answer, read the next \
             size. A broker that closes after the first response, or that treats one read() \
             as one request, fails here.",
        ),
        ExampleSpec::wire("Correlation ids the broker does not choose", |env| {
            Ok(Wire::Frames(vec![
                env.payload(API_VERSIONS_V4, 77, &api_versions_request())?,
                env.payload(API_VERSIONS_V4, 5, &api_versions_request())?,
            ]))
        })
        .request("Two ApiVersions v4 frames with correlation ids 77 and then 5")
        .response("Two responses, echoing 77 and then 5, in that order")
        .note(
            "The ids are the client's, and they do not have to ascend. Answer in arrival \
             order and echo what you were given.",
        ),
    ]
}
