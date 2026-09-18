//! Stage 03 — Parse the request header.

use crate::assert::{Check, Failure};
use crate::examples::{ExampleSpec, Wire};
use crate::kafka_test;
use crate::stages::{
    api_versions_body_v4, api_versions_request, correlation_id_of, raw_flexible_frame,
    roundtrip_raw, Stage, Test, API_VERSIONS_KEY, API_VERSIONS_V4, NONE,
};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 3,
        slug: "request_header",
        name: "Parse the request header",
        ext: false,
        hints: &[
            "Header v2: api_key(int16) api_version(int16) correlation_id(int32) \
             client_id(nullable string: int16 length, -1 = null) then tagged fields",
            "client_id is a plain (non-compact) string even in flexible versions",
            "Tagged fields are a uvarint count followed by (uvarint tag, uvarint length, bytes)",
            "Skip tagged fields you do not know instead of rejecting the request",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("a normal client id is accepted", normal_client_id),
            Test::new("an empty client id is accepted", empty_client_id),
            Test::new("a null client id is accepted", null_client_id),
            Test::new("a long client id is accepted", long_client_id).ext(),
            Test::new(
                "unknown header tagged fields are ignored",
                unknown_tagged_field,
            )
            .ext(),
            Test::new(
                "several requests with different client ids are answered",
                mixed_client_ids,
            ),
            Test::new(
                "the api version in the header selects the response",
                version_selects_body,
            ),
        ],
    }
}

async fn header_roundtrip(
    ctx: &crate::stages::Ctx,
    client_id: Option<&str>,
    id: i32,
) -> Result<(), Failure> {
    let mut conn = ctx.connect().await?;
    let frame = raw_flexible_frame(
        API_VERSIONS_KEY,
        API_VERSIONS_V4,
        id,
        client_id,
        &[],
        &api_versions_body_v4(),
    );
    let resp = roundtrip_raw(&mut conn, &frame).await?;
    let got = correlation_id_of(&resp, &conn)?;
    let mut c = Check::new(
        format!(
            "a request header with client_id {}",
            client_id
                .map(|s| format!("{s:?}"))
                .unwrap_or_else(|| "null".into())
        ),
        &conn,
    );
    c.mark(0..4);
    c.eq("response.correlation_id", id, got);
    c.finish()
}

kafka_test!(normal_client_id, |ctx| {
    header_roundtrip(ctx, Some("kafkatest-client"), 11).await
});

kafka_test!(empty_client_id, |ctx| {
    header_roundtrip(ctx, Some(""), 12).await
});

kafka_test!(null_client_id, |ctx| {
    header_roundtrip(ctx, None, 13).await
});

kafka_test!(long_client_id, |ctx| {
    let long = "c".repeat(200);
    header_roundtrip(ctx, Some(&long), 14).await
});

kafka_test!(unknown_tagged_field, |ctx| {
    let mut conn = ctx.connect().await?;
    let frame = raw_flexible_frame(
        API_VERSIONS_KEY,
        API_VERSIONS_V4,
        15,
        Some("kafkatest"),
        &[(0xbe, vec![0xde, 0xad]), (0xef, vec![])],
        &api_versions_body_v4(),
    );
    let resp = roundtrip_raw(&mut conn, &frame).await?;
    let got = correlation_id_of(&resp, &conn)?;
    let mut c = Check::new("a header carrying two unknown tagged fields", &conn);
    c.note("a broker must skip tagged fields it does not recognise, not reject the request");
    c.mark(0..4);
    c.eq("response.correlation_id", 15i32, got);
    c.at_least("response.length", 6usize, resp.len());
    c.finish()
});

kafka_test!(mixed_client_ids, |ctx| {
    let mut conn = ctx.connect().await?;
    for (i, client) in [Some("a"), None, Some(""), Some("a-longer-client-id")]
        .into_iter()
        .enumerate()
    {
        let id = 100 + i as i32;
        let frame = raw_flexible_frame(
            API_VERSIONS_KEY,
            API_VERSIONS_V4,
            id,
            client,
            &[],
            &api_versions_body_v4(),
        );
        let resp = roundtrip_raw(&mut conn, &frame).await?;
        let got = correlation_id_of(&resp, &conn)?;
        let mut c = Check::new(format!("request {} of 4 on one connection", i + 1), &conn);
        c.mark(0..4);
        c.eq("response.correlation_id", id, got);
        c.finish()?;
    }
    Ok(())
});

kafka_test!(version_selects_body, |ctx| {
    // ApiVersions v0 has a non-flexible header and a body without throttle time; a broker
    // that ignores api_version and always answers v4 fails here.
    let mut conn = ctx.connect().await?;
    let req = api_versions_request();
    let resp = conn
        .request(0, &req)
        .await
        .map_err(|e| crate::stages::proto_fail(e, &conn))?;
    let mut c = Check::new(
        "ApiVersions v0, whose header and body differ from v4",
        &conn,
    );
    c.eq("response.error_code", NONE, resp.error_code);
    c.at_least("response.api_keys.len", 1usize, resp.api_keys.len());
    c.eq("response.trailing_bytes", 0usize, resp.trailing);
    c.finish()
});

/// Worked examples: request headers a broker must parse without complaint.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("A header with an unknown tagged field", |_env| {
            Ok(Wire::Frames(vec![raw_flexible_frame(
                API_VERSIONS_KEY,
                API_VERSIONS_V4,
                311,
                Some("kafkatest"),
                &[(99, vec![0x01, 0x02, 0x03])],
                &api_versions_body_v4(),
            )]))
        })
        .request(
            "ApiVersions v4, correlation id 311, client id 'kafkatest', and one header tagged \
             field: tag 99, 3 bytes of payload the broker has never heard of",
        )
        .response("An ordinary ApiVersions v4 response: correlation id 311, error_code 0")
        .note(
            "Unknown tagged fields are skipped, not rejected: read the count, then \
             uvarint(tag) + uvarint(size) + size bytes, and throw them away. A broker that \
             assumes the tagged-field section is always the single byte 0x00 breaks here.",
        ),
        ExampleSpec::wire("A null client id", |_env| {
            Ok(Wire::Frames(vec![raw_flexible_frame(
                API_VERSIONS_KEY,
                API_VERSIONS_V4,
                312,
                None,
                &[],
                &api_versions_body_v4(),
            )]))
        })
        .request("ApiVersions v4, correlation id 312, client_id encoded as the length -1 (0xffff)")
        .response("An ordinary ApiVersions v4 response, correlation id 312, error_code 0")
        .note(
            "client_id stays a *nullable* int16-length string even in the flexible header v2, \
             where every other string is compact. -1 means null and no bytes follow.",
        ),
    ]
}
