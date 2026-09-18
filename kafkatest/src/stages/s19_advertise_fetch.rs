//! Stage 19 — ApiVersions advertises Fetch(1) v16.

use crate::assert::Check;
use crate::examples::{ExampleSpec, Wire};
use crate::kafka_test;
use crate::stages::{
    api_versions, api_versions_request, fetch_request, require_api, Stage, Test, API_VERSIONS_V4,
    DESCRIBE_TOPIC_PARTITIONS_KEY, FETCH_KEY, FETCH_V16,
};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 19,
        slug: "advertise_fetch",
        name: "ApiVersions advertises Fetch(1) v16",
        ext: false,
        hints: &[
            "Add {api_key: 1, min_version: 0, max_version: 16} to the api_keys array",
            "Fetch v12 and later are flexible versions: the request and response use \
             compact arrays and tagged fields",
            "From v13 the request names topics by topic id, not by name",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("Fetch(1) is listed", listed),
            Test::new("version 16 is inside the advertised range", covers_v16),
            Test::new(
                "the other api keys are still advertised",
                others_still_there,
            ),
            Test::new(
                "the response decodes with no trailing bytes",
                decodes_cleanly,
            ),
        ],
    }
}

kafka_test!(listed, |ctx| {
    let mut conn = ctx.connect().await?;
    let resp = api_versions(&mut conn).await?;
    require_api(&resp, FETCH_KEY, "Fetch", &conn)?;
    Ok(())
});

kafka_test!(covers_v16, |ctx| {
    let mut conn = ctx.connect().await?;
    let resp = api_versions(&mut conn).await?;
    let (min, max) = require_api(&resp, FETCH_KEY, "Fetch", &conn)?;
    let mut c = Check::new("the advertised Fetch range", &conn);
    c.at_most("response.api_keys[api_key=1].min_version", FETCH_V16, min);
    c.at_least("response.api_keys[api_key=1].max_version", FETCH_V16, max);
    c.finish()
});

kafka_test!(others_still_there, |ctx| {
    let mut conn = ctx.connect().await?;
    let resp = api_versions(&mut conn).await?;
    require_api(&resp, 18, "ApiVersions", &conn)?;
    require_api(
        &resp,
        DESCRIBE_TOPIC_PARTITIONS_KEY,
        "DescribeTopicPartitions",
        &conn,
    )?;
    require_api(&resp, FETCH_KEY, "Fetch", &conn)?;
    Ok(())
});

kafka_test!(decodes_cleanly, |ctx| {
    let mut conn = ctx.connect().await?;
    let req = crate::stages::api_versions_request();
    let decoded = conn
        .request(4, &req)
        .await
        .map_err(|e| crate::stages::proto_fail(e, &conn))?;
    let mut c = Check::new("the ApiVersions body with three api keys", &conn);
    c.eq("response.trailing_bytes", 0usize, decoded.trailing);
    c.finish()
});

/// Worked examples: reading the Fetch entry, and then using the version it promises.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("ApiVersions v4, read for the Fetch entry", |env| {
            env.request(API_VERSIONS_V4, 191, &api_versions_request())
        })
        .request("ApiVersions v4, correlation id 191, client id 'kafkatest'")
        .response(
            "error_code 0 (NONE) and the api_keys array, which now carries an entry \
             {api_key 1 (Fetch), min_version 0, max_version >= 16} beside ApiVersions(18) and \
             DescribeTopicPartitions(75)",
        )
        .note(
            "An entry is three int16s, six bytes: 00 01 00 00 00 10 is Fetch, 0, 16. Advertise \
             only what you actually serve — a client picks the highest version you claim and \
             never asks again on that connection.",
        ),
        ExampleSpec::wire("The handshake a consumer really performs", |env| {
            Ok(Wire::Frames(vec![
                env.payload(API_VERSIONS_V4, 192, &api_versions_request())?,
                env.payload(FETCH_V16, 193, &fetch_request(&[], 500))?,
            ]))
        })
        .request(
            "Two frames written back to back on one connection: ApiVersions v4 (correlation id \
             192), then a Fetch v16 with an empty topics array (correlation id 193)",
        )
        .response(
            "Two frames, in order: the ApiVersions body listing Fetch(1), then a Fetch v16 body \
             with throttle_time_ms 0, error_code 0 and an empty responses array",
        )
        .note(
            "Advertising and serving are one job. A broker that lists Fetch(1) v16 and then \
             cannot decode a v16 request fails at the second frame, not the first — and the \
             client has no way back, because it already chose the version.",
        ),
    ]
}
