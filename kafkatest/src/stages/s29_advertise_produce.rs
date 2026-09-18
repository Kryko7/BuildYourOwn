//! Stage 29 — ApiVersions advertises Produce(0) v11.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::kafka_test;
use crate::stages::{
    api_versions, api_versions_request, produce_request, require_api, Stage, Test, API_VERSIONS_V4,
    DESCRIBE_TOPIC_PARTITIONS_KEY, FETCH_KEY, PRODUCE_KEY, PRODUCE_V11,
};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 29,
        slug: "advertise_produce",
        name: "ApiVersions advertises Produce(0) v11",
        ext: false,
        hints: &[
            "Produce is api_key 0, which is easy to miss when you index the array by key",
            "Add {api_key: 0, min_version: 0, max_version: 11}",
            "Produce v9 and later are flexible; v11 still names topics by name, not id",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("Produce(0) is listed", listed),
            Test::new("version 11 is inside the advertised range", covers_v11),
            Test::new(
                "the earlier api keys are still advertised",
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
    require_api(&resp, PRODUCE_KEY, "Produce", &conn)?;
    Ok(())
});

kafka_test!(covers_v11, |ctx| {
    let mut conn = ctx.connect().await?;
    let resp = api_versions(&mut conn).await?;
    let (min, max) = require_api(&resp, PRODUCE_KEY, "Produce", &conn)?;
    let mut c = Check::new("the advertised Produce range", &conn);
    c.at_most("response.api_keys[api_key=0].min_version", PRODUCE_V11, min);
    c.at_least("response.api_keys[api_key=0].max_version", PRODUCE_V11, max);
    c.finish()
});

kafka_test!(others_still_there, |ctx| {
    let mut conn = ctx.connect().await?;
    let resp = api_versions(&mut conn).await?;
    require_api(&resp, 18, "ApiVersions", &conn)?;
    require_api(&resp, FETCH_KEY, "Fetch", &conn)?;
    require_api(
        &resp,
        DESCRIBE_TOPIC_PARTITIONS_KEY,
        "DescribeTopicPartitions",
        &conn,
    )?;
    require_api(&resp, PRODUCE_KEY, "Produce", &conn)?;
    Ok(())
});

kafka_test!(decodes_cleanly, |ctx| {
    let mut conn = ctx.connect().await?;
    let req = crate::stages::api_versions_request();
    let decoded = conn
        .request(4, &req)
        .await
        .map_err(|e| crate::stages::proto_fail(e, &conn))?;
    let mut c = Check::new("the ApiVersions body with four api keys", &conn);
    c.eq("response.trailing_bytes", 0usize, decoded.trailing);
    c.finish()
});

/// Worked examples: the advertisement, and the first Produce frame it unlocks.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("ApiVersions v4, read for the Produce entry", |env| {
            env.request(API_VERSIONS_V4, 291, &api_versions_request())
        })
        .request("ApiVersions v4, correlation id 291, from a client about to produce")
        .response(
            "error_code 0 (NONE) and the api_keys compact array; among its entries there is \
             now one with api_key 0 (Produce) whose min_version <= 11 and max_version >= 11, \
             alongside ApiVersions(18), Fetch(1) and DescribeTopicPartitions(75)",
        )
        .note(
            "A client picks its Produce version from this list before it sends a single \
             record. Leave api_key 0 out and no producer will ever talk to you — it fails at \
             version negotiation, not at Produce. Watch the indexing: api_key 0 is easy to \
             lose in a loop that treats 0 as 'unset'.",
        ),
        ExampleSpec::wire("A Produce v11 frame carrying no topics", |env| {
            env.request(PRODUCE_V11, 292, &produce_request(&[], -1))
        })
        .request(
            "Produce v11, correlation id 292, acks -1, timeout_ms 5000, and an empty \
             topic_data array — the smallest valid Produce request there is",
        )
        .response(
            "A normal Produce v11 response: throttle_time_ms 0 and an empty responses array. \
             There is nothing to append, so there is no error either.",
        )
        .note(
            "Advertising Produce(0) v11 is a promise to decode this frame. v9 and later are \
             flexible, so the body uses compact arrays and tagged fields; v11 still names \
             topics by name, not by topic id. Get the empty request answering correctly \
             before you touch the log.",
        ),
    ]
}
