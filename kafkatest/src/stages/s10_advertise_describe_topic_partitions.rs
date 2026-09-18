//! Stage 10 — ApiVersions advertises DescribeTopicPartitions(75).

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::kafka_test;
use crate::stages::{
    api_versions, api_versions_request, describe_request, require_api, Stage, Test,
    API_VERSIONS_V4, DESCRIBE_TOPIC_PARTITIONS_KEY, DESCRIBE_V0, NONE,
};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 10,
        slug: "advertise_describe_topic_partitions",
        name: "ApiVersions advertises DescribeTopicPartitions(75)",
        ext: false,
        hints: &[
            "Add an entry with api_key 75, min_version 0 and max_version 0 to the api_keys array",
            "The array is compact: its length byte is count + 1, so two entries encode as 0x03",
            "Every entry ends with its own empty tagged-field section (a single 0x00)",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("DescribeTopicPartitions(75) is listed", listed),
            Test::new("version 0 is inside the advertised range", covers_v0),
            Test::new(
                "ApiVersions(18) is still advertised alongside it",
                still_has_api_versions,
            ),
            Test::new(
                "the response still decodes with no trailing bytes",
                decodes_cleanly,
            ),
        ],
    }
}

kafka_test!(listed, |ctx| {
    let mut conn = ctx.connect().await?;
    let resp = api_versions(&mut conn).await?;
    require_api(
        &resp,
        DESCRIBE_TOPIC_PARTITIONS_KEY,
        "DescribeTopicPartitions",
        &conn,
    )?;
    Ok(())
});

kafka_test!(covers_v0, |ctx| {
    let mut conn = ctx.connect().await?;
    let resp = api_versions(&mut conn).await?;
    let (min, max) = require_api(
        &resp,
        DESCRIBE_TOPIC_PARTITIONS_KEY,
        "DescribeTopicPartitions",
        &conn,
    )?;
    let mut c = Check::new("the advertised DescribeTopicPartitions range", &conn);
    c.at_most(
        "response.api_keys[api_key=75].min_version",
        DESCRIBE_V0,
        min,
    );
    c.at_least(
        "response.api_keys[api_key=75].max_version",
        DESCRIBE_V0,
        max,
    );
    c.finish()
});

kafka_test!(still_has_api_versions, |ctx| {
    let mut conn = ctx.connect().await?;
    let resp = api_versions(&mut conn).await?;
    require_api(&resp, 18, "ApiVersions", &conn)?;
    require_api(
        &resp,
        DESCRIBE_TOPIC_PARTITIONS_KEY,
        "DescribeTopicPartitions",
        &conn,
    )?;
    let mut c = Check::new("the api key list", &conn);
    c.at_least("response.api_keys.len", 2usize, resp.api_keys.len());
    c.eq("response.error_code", NONE, resp.error_code);
    c.finish()
});

kafka_test!(decodes_cleanly, |ctx| {
    let mut conn = ctx.connect().await?;
    let req = crate::stages::api_versions_request();
    let decoded = conn
        .request(4, &req)
        .await
        .map_err(|e| crate::stages::proto_fail(e, &conn))?;
    let mut c = Check::new("the ApiVersions body after adding a second api key", &conn);
    c.note("a wrong compact-array length is the usual cause of leftover bytes here");
    c.eq("response.trailing_bytes", 0usize, decoded.trailing);
    c.finish()
});

/// Worked examples: the advertisement, and the request it unlocks.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("The api_keys entry for key 75", |env| {
            env.request(API_VERSIONS_V4, 101, &api_versions_request())
        })
        .request("ApiVersions v4, correlation id 101, client id 'kafkatest'")
        .response(
            "error_code 0 (NONE) and the compact api_keys array, which now carries an entry \
             {api_key 75, min_version 0, max_version 0} beside {api_key 18, ...}; then \
             throttle_time_ms and the tagged fields",
        )
        .note(
            "Adding an api key means adding an entry and bumping the compact array length, \
             which is uvarint(count + 1). Each entry ends with its own empty tagged-field \
             byte (0x00) — forgetting it shifts every following entry.",
        ),
        ExampleSpec::wire("The request the advertisement unlocks", |env| {
            env.request(
                DESCRIBE_V0,
                102,
                &describe_request(&["kafkatest-probe"], 100),
            )
        })
        .request(
            "DescribeTopicPartitions v0, correlation id 102, one topic name, \
             response_partition_limit 100 — what a client sends once it has seen key 75 in \
             the list",
        )
        .response(
            "A normal DescribeTopicPartitions v0 body: throttle_time_ms, then one topics \
             entry for the requested name with error_code 3 (UNKNOWN_TOPIC_OR_PARTITION), \
             because nothing created that topic",
        )
        .note(
            "A client that cannot find key 75 in ApiVersions gives up before it sends this \
             frame at all, which is why the advertisement is its own stage. What the body \
             has to contain is stage 11's job.",
        ),
    ]
}
