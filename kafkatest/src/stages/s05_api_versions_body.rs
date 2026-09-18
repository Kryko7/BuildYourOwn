//! Stage 05 — The ApiVersions v4 response body.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::kafka_test;
use crate::stages::{
    api_versions, api_versions_request, proto_fail, require_api, Stage, Test, API_VERSIONS_KEY,
    API_VERSIONS_V4, NONE,
};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 5,
        slug: "api_versions_body",
        name: "ApiVersions v4 response body",
        ext: false,
        hints: &[
            "v4 body: error_code(int16), api_keys(COMPACT_ARRAY), throttle_time_ms(int32), \
             tagged fields",
            "A compact array is uvarint(count + 1) followed by the entries",
            "Each entry is api_key(int16) min_version(int16) max_version(int16) + tagged fields",
            "Advertise ApiVersions(18) itself with max_version >= 4",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("the error code is zero", error_code_zero),
            Test::new(
                "ApiVersions(18) is advertised with min <= 4 <= max",
                advertises_itself,
            ),
            Test::new(
                "throttle_time_ms is present and not negative",
                throttle_time,
            ),
            Test::new("the body decodes with no trailing bytes", no_trailing_bytes),
            Test::new(
                "every entry has min_version <= max_version",
                ordered_versions,
            ),
            Test::new("the api key list has no duplicates", no_duplicates),
            Test::new("ApiVersions v3 answers with a v3 body", v3_body).ext(),
            Test::new("ApiVersions v0 answers with a v0 body", v0_body).ext(),
        ],
    }
}

kafka_test!(error_code_zero, |ctx| {
    let mut conn = ctx.connect().await?;
    let resp = api_versions(&mut conn).await?;
    let mut c = Check::new("the ApiVersions v4 response body", &conn);
    c.mark(4..6);
    c.eq("response.error_code", NONE, resp.error_code);
    c.finish()
});

kafka_test!(advertises_itself, |ctx| {
    let mut conn = ctx.connect().await?;
    let resp = api_versions(&mut conn).await?;
    let (min, max) = require_api(&resp, API_VERSIONS_KEY, "ApiVersions", &conn)?;
    let mut c = Check::new("the advertised ApiVersions range", &conn);
    c.at_most(
        "response.api_keys[api_key=18].min_version",
        API_VERSIONS_V4,
        min,
    );
    c.at_least(
        "response.api_keys[api_key=18].max_version",
        API_VERSIONS_V4,
        max,
    );
    c.finish()
});

kafka_test!(throttle_time, |ctx| {
    let mut conn = ctx.connect().await?;
    let resp = api_versions(&mut conn).await?;
    let mut c = Check::new("throttle_time_ms in the v4 body", &conn);
    c.at_least("response.throttle_time_ms", 0i32, resp.throttle_time_ms);
    c.finish()
});

kafka_test!(no_trailing_bytes, |ctx| {
    let mut conn = ctx.connect().await?;
    let req = api_versions_request();
    let decoded = conn
        .request(API_VERSIONS_V4, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("that the v4 body is exactly as long as it claims", &conn);
    c.note("leftover bytes usually mean a missing or extra tagged-field section");
    c.eq("response.trailing_bytes", 0usize, decoded.trailing);
    c.finish()
});

kafka_test!(ordered_versions, |ctx| {
    let mut conn = ctx.connect().await?;
    let resp = api_versions(&mut conn).await?;
    let mut c = Check::new("every advertised version range", &conn);
    c.at_least("response.api_keys.len", 1usize, resp.api_keys.len());
    for k in &resp.api_keys {
        c.at_least(
            &format!("response.api_keys[api_key={}].max_version", k.api_key),
            k.min_version,
            k.max_version,
        );
        c.at_least(
            &format!("response.api_keys[api_key={}].min_version", k.api_key),
            0i16,
            k.min_version,
        );
    }
    c.finish()
});

kafka_test!(no_duplicates, |ctx| {
    let mut conn = ctx.connect().await?;
    let resp = api_versions(&mut conn).await?;
    let mut keys: Vec<i16> = resp.api_keys.iter().map(|k| k.api_key).collect();
    let before = keys.len();
    keys.sort_unstable();
    keys.dedup();
    let mut c = Check::new("that each api key is listed once", &conn);
    c.eq("response.api_keys.unique_len", before, keys.len());
    c.finish()
});

kafka_test!(v3_body, |ctx| {
    let mut conn = ctx.connect().await?;
    let req = api_versions_request();
    let decoded = conn
        .request(3, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the ApiVersions v3 response body", &conn);
    c.eq("response.error_code", NONE, decoded.error_code);
    c.eq("response.trailing_bytes", 0usize, decoded.trailing);
    c.finish()
});

kafka_test!(v0_body, |ctx| {
    let mut conn = ctx.connect().await?;
    let req = api_versions_request();
    let decoded = conn
        .request(0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the ApiVersions v0 response body (no throttle time)", &conn);
    c.eq("response.error_code", NONE, decoded.error_code);
    c.at_least("response.api_keys.len", 1usize, decoded.api_keys.len());
    c.eq("response.trailing_bytes", 0usize, decoded.trailing);
    c.finish()
});

/// Worked examples: the v4 body, and the v0 body it grew out of.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("ApiVersions v4, the whole exchange", |env| {
            env.request(API_VERSIONS_V4, 7, &api_versions_request())
        })
        .request(
            "ApiVersions v4, correlation id 7, client id 'kafkatest', body \
             client_software_name 'kafkatest' / client_software_version '0.1.0'",
        )
        .response(
            "error_code 0 (NONE), then a compact array of api_keys — one \
             {api_key, min_version, max_version} per API the broker serves, ApiVersions(18) \
             among them with max_version >= 4 — then throttle_time_ms 0 and the tagged fields",
        )
        .note(
            "The response header of ApiVersions is v0 (no tagged fields) at every version, \
             because a client must parse this answer before it knows what the broker supports. \
             Every other flexible API uses response header v1.",
        ),
        ExampleSpec::wire("ApiVersions v0, the non-flexible shape", |env| {
            env.request(0, 8, &api_versions_request())
        })
        .request(
            "ApiVersions v0, correlation id 8: header v1 (no tagged-field byte) and an empty body",
        )
        .response(
            "error_code 0, then api_keys as an ordinary int32-counted array of three int16s \
             each — no compact lengths, no throttle_time_ms, no tagged fields",
        )
        .note(
            "Compare the two hex dumps: v4 writes the array length as uvarint(count + 1) in \
             one byte, v0 writes it as a 4-byte count. That single difference is what 'flexible \
             versions' (KIP-482) means.",
        )
        .response_version(0),
    ]
}
