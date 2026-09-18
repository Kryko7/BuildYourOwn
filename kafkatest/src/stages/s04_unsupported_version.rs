//! Stage 04 — UNSUPPORTED_VERSION for an ApiVersions request the broker cannot serve.

use crate::assert::{Check, Failure};
use crate::examples::{ExampleSpec, Wire};
use crate::kafka_test;
use crate::stages::{
    api_versions_body_v4, be_i16, correlation_id_of, raw_flexible_frame, roundtrip_raw, Ctx, Stage,
    Test, API_VERSIONS_KEY, API_VERSIONS_V4, NONE, UNSUPPORTED_VERSION,
};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 4,
        slug: "unsupported_version",
        name: "UNSUPPORTED_VERSION (35) for a bad ApiVersions version",
        ext: false,
        hints: &[
            "Check api_version before decoding the body; ApiVersions is valid for v0-v4",
            "Answer correlation_id(int32) then error_code(int16) = 35",
            "The error response for ApiVersions always uses the v0 header (no tagged fields)",
            "Keep the connection open afterwards: one bad request is not a fatal error",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("version 5 returns error 35", v5),
            Test::new("version 1234 returns error 35", v1234),
            Test::new("a negative version returns error 35", negative),
            Test::new(
                "the error response still echoes the correlation id",
                echoes_id,
            ),
            Test::new(
                "the connection stays usable after an unsupported version",
                still_usable,
            ),
            Test::new(
                "a supported version does not return error 35",
                supported_is_fine,
            ),
        ],
    }
}

async fn error_code_for(ctx: &Ctx, version: i16, id: i32) -> Result<(i32, i16), Failure> {
    let mut conn = ctx.connect().await?;
    let frame = raw_flexible_frame(
        API_VERSIONS_KEY,
        version,
        id,
        Some("kafkatest"),
        &[],
        &api_versions_body_v4(),
    );
    let resp = roundtrip_raw(&mut conn, &frame).await?;
    let correlation = correlation_id_of(&resp, &conn)?;
    let code = be_i16(&resp, 4).ok_or_else(|| {
        let mut c = Check::new(format!("the ApiVersions v{version} error response"), &conn);
        c.at_least("response.length", 6usize, resp.len());
        c.note("an ApiVersions error response is correlation_id(int32) then error_code(int16)");
        c.finish()
            .err()
            .unwrap_or_else(|| Failure::harness("short response"))
    })?;
    let mut c = Check::new(format!("the ApiVersions v{version} error response"), &conn);
    c.mark(4..6);
    c.eq("response.error_code", UNSUPPORTED_VERSION, code);
    c.finish()?;
    Ok((correlation, code))
}

kafka_test!(v5, |ctx| {
    error_code_for(ctx, 5, 21).await?;
    Ok(())
});

kafka_test!(v1234, |ctx| {
    error_code_for(ctx, 1234, 22).await?;
    Ok(())
});

kafka_test!(negative, |ctx| {
    error_code_for(ctx, -1, 23).await?;
    Ok(())
});

kafka_test!(echoes_id, |ctx| {
    let (correlation, _) = error_code_for(ctx, 9, 0x1234_5678).await?;
    let conn = ctx.connect().await?;
    let mut c = Check::new("the correlation id of an error response", &conn);
    c.eq("response.correlation_id", 0x1234_5678i32, correlation);
    c.finish()
});

kafka_test!(still_usable, |ctx| {
    let mut conn = ctx.connect().await?;
    let bad = raw_flexible_frame(
        API_VERSIONS_KEY,
        7,
        31,
        Some("kafkatest"),
        &[],
        &api_versions_body_v4(),
    );
    let resp = roundtrip_raw(&mut conn, &bad).await?;
    let mut c = Check::new("the error response before reusing the connection", &conn);
    c.mark(4..6);
    c.eq(
        "response.error_code",
        UNSUPPORTED_VERSION,
        be_i16(&resp, 4).unwrap_or(-1),
    );
    c.finish()?;

    let good = raw_flexible_frame(
        API_VERSIONS_KEY,
        API_VERSIONS_V4,
        32,
        Some("kafkatest"),
        &[],
        &api_versions_body_v4(),
    );
    let resp = roundtrip_raw(&mut conn, &good)
        .await
        .map_err(|f| f.note("the broker hung up after answering UNSUPPORTED_VERSION"))?;
    let mut c = Check::new("the follow-up request on the same connection", &conn);
    c.mark(0..4);
    c.eq(
        "response.correlation_id",
        32i32,
        correlation_id_of(&resp, &conn)?,
    );
    c.eq("response.error_code", NONE, be_i16(&resp, 4).unwrap_or(-1));
    c.finish()
});

kafka_test!(supported_is_fine, |ctx| {
    let mut conn = ctx.connect().await?;
    let frame = raw_flexible_frame(
        API_VERSIONS_KEY,
        API_VERSIONS_V4,
        41,
        Some("kafkatest"),
        &[],
        &api_versions_body_v4(),
    );
    let resp = roundtrip_raw(&mut conn, &frame).await?;
    let mut c = Check::new("ApiVersions v4, which every broker must support", &conn);
    c.mark(4..6);
    c.eq("response.error_code", NONE, be_i16(&resp, 4).unwrap_or(-1));
    c.finish()
});

/// Worked examples: a version the broker cannot speak, and the error that says so.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("ApiVersions v1234", |_env| {
            Ok(Wire::Frames(vec![raw_flexible_frame(
                API_VERSIONS_KEY,
                1234,
                22,
                Some("kafkatest"),
                &[],
                &api_versions_body_v4(),
            )]))
        })
        .request("ApiVersions with api_version 1234, correlation id 22")
        .response(
            "correlation_id 22, then error_code 35 (UNSUPPORTED_VERSION), then a one-entry \
             api_keys array — ApiVersions(18), min_version 0, max_version 4 — so the client \
             can retry with a version the broker speaks",
        )
        .note(
            "Check api_version before you decode the body. The reply is written in the v0 \
             shape — a v0 response header with no tagged fields — because the client has not \
             yet been told what the broker supports.",
        )
        .response_version(0),
        ExampleSpec::wire("ApiVersions v5, one past the maximum", |_env| {
            Ok(Wire::Frames(vec![raw_flexible_frame(
                API_VERSIONS_KEY,
                5,
                21,
                Some("kafkatest"),
                &[],
                &api_versions_body_v4(),
            )]))
        })
        .request("ApiVersions with api_version 5, correlation id 21")
        .response("correlation_id 21, then error_code 35 (UNSUPPORTED_VERSION)")
        .note(
            "v4 is the highest ApiVersions this suite asks for, so 5 is the first bad one. \
             The connection stays open afterwards: one unsupported version is not a fatal \
             protocol error.",
        )
        .response_version(0),
    ]
}
