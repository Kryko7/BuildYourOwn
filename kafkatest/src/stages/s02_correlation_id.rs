//! Stage 02 — Echo the correlation id.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::kafka_test;
use crate::stages::{
    api_versions_body_v4, api_versions_request, correlation_id_of, raw_flexible_frame,
    roundtrip_raw, Stage, Test, API_VERSIONS_KEY, API_VERSIONS_V4,
};
use rand::Rng;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 2,
        slug: "correlation_id",
        name: "Respond with the correlation id",
        ext: false,
        hints: &[
            "Read the 4-byte big-endian message size, then that many bytes",
            "The request header starts api_key(int16) api_version(int16) correlation_id(int32)",
            "Write back size(int32) then the same correlation id, unchanged, big-endian",
            "The correlation id is signed: do not clamp, mask or renumber it",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("the correlation id comes back unchanged", echoes),
            Test::new("the response is at least a 4-byte header", header_length),
            Test::new("correlation id 0 is echoed", zero),
            Test::new("the largest correlation id is echoed", max_id),
            Test::new("a negative correlation id is echoed", negative),
            Test::new("seeded random correlation ids are echoed", random_ids),
        ],
    }
}

async fn echo_once(ctx: &crate::stages::Ctx, id: i32) -> Result<(), crate::assert::Failure> {
    let mut conn = ctx.connect().await?;
    let frame = raw_flexible_frame(
        API_VERSIONS_KEY,
        API_VERSIONS_V4,
        id,
        Some("kafkatest"),
        &[],
        &api_versions_body_v4(),
    );
    let resp = roundtrip_raw(&mut conn, &frame).await?;
    let got = correlation_id_of(&resp, &conn)?;
    let mut c = Check::new(format!("that correlation id {id} is echoed"), &conn);
    c.mark(0..4); // the correlation id is the first four bytes of every response
    c.eq("response.correlation_id", id, got);
    c.finish()
}

kafka_test!(echoes, |ctx| { echo_once(ctx, 0x6f4a_12f3).await });

kafka_test!(header_length, |ctx| {
    let mut conn = ctx.connect().await?;
    let frame = raw_flexible_frame(
        API_VERSIONS_KEY,
        API_VERSIONS_V4,
        7,
        Some("kafkatest"),
        &[],
        &api_versions_body_v4(),
    );
    let resp = roundtrip_raw(&mut conn, &frame).await?;
    let mut c = Check::new("that the response carries a full header", &conn);
    c.at_least("response.length", 4usize, resp.len());
    c.finish()
});

kafka_test!(zero, |ctx| { echo_once(ctx, 0).await });

kafka_test!(max_id, |ctx| { echo_once(ctx, i32::MAX).await });

kafka_test!(negative, |ctx| { echo_once(ctx, -12345).await });

kafka_test!(random_ids, |ctx| {
    let ids: Vec<i32> = (0..3).map(|_| ctx.rng.random::<i32>()).collect();
    for id in ids {
        echo_once(ctx, id).await?;
    }
    Ok(())
});

/// Worked examples: what the broker sees, and what a correct broker answers.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("ApiVersions, correlation id 42", |env| {
            env.request(API_VERSIONS_V4, 42, &api_versions_request())
        })
        .request("ApiVersions v4, correlation id 42, client id 'kafkatest'")
        .response(
            "4-byte size, then correlation_id 42 echoed unchanged as the first field of the \
             response header, then the v4 body (error_code 0)",
        )
        .note(
            "This is the whole stage: read the 4-byte size, read that many bytes, copy bytes \
             4..8 of the header into your reply. The size prefix counts the bytes after it, \
             itself excluded.",
        ),
        ExampleSpec::wire("The largest correlation id there is", |env| {
            env.request(API_VERSIONS_V4, i32::MAX, &api_versions_request())
        })
        .request("ApiVersions v4, correlation id 2147483647 (0x7fffffff)")
        .response("The same id comes back: 7f ff ff ff at offset 4 of the response frame")
        .note(
            "correlation_id is a signed int32 and the client picks it. Echo the four bytes; \
             never renumber them, and never assume they count up from one.",
        ),
    ]
}
