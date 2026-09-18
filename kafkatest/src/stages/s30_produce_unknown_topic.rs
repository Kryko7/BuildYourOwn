//! Stage 30 — Produce to a topic that does not exist.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::fixtures::{FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::proto::records::{RecordBatch, RecordItem};
use crate::stages::{
    produce_request, proto_fail, Stage, Test, NONE, PRODUCE_V11, UNKNOWN_TOPIC_OR_PARTITION,
};

fn one_topic() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 1))
}

fn batch(values: &[&str]) -> Vec<u8> {
    RecordBatch::of(
        0,
        1_700_000_000_000,
        values.iter().map(|v| RecordItem::value(*v)).collect(),
    )
    .encode()
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 30,
        slug: "produce_unknown_topic",
        name: "Produce to an unknown topic → error 3",
        ext: false,
        hints: &[
            "Check the topic exists before touching the log; do not create it implicitly",
            "The error lives in the per-partition entry, not at the top level",
            "Answer error_code 3, base_offset -1, log_append_time_ms -1, log_start_offset -1",
            "Echo the topic name so the producer can match the entry",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("an unknown topic returns error 3", error_code),
            Test::new("the base offset of a failed produce is -1", base_offset),
            Test::new("the topic name is echoed", name_echoed),
            Test::new(
                "an unknown partition of a known topic returns error 3",
                unknown_partition,
            )
            .with_fixtures(one_topic),
            Test::new("a known and an unknown topic can be mixed", mixed).with_fixtures(one_topic),
            Test::new(
                "the response decodes with no trailing bytes",
                decodes_cleanly,
            ),
        ],
    }
}

kafka_test!(error_code, |ctx| {
    let name = ctx.unique("produce-unknown");
    let mut conn = ctx.connect().await?;
    let req = produce_request(&[(name.clone(), 0, batch(&["v"]))], 1);
    let resp = conn
        .request(PRODUCE_V11, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new(format!("producing to the absent topic '{name}'"), &conn);
    c.eq("response.responses.len", 1usize, resp.responses.len());
    match resp
        .responses
        .first()
        .and_then(|t| t.partition_responses.first())
    {
        Some(p) => c.eq(
            "response.responses[0].partition_responses[0].error_code",
            UNKNOWN_TOPIC_OR_PARTITION,
            p.error_code,
        ),
        None => c.that(
            "response.responses[0].partition_responses[0]",
            "one partition entry",
            false,
            "none",
        ),
    };
    c.finish()
});

kafka_test!(base_offset, |ctx| {
    let name = ctx.unique("produce-unknown-off");
    let mut conn = ctx.connect().await?;
    let req = produce_request(&[(name, 0, batch(&["v"]))], 1);
    let resp = conn
        .request(PRODUCE_V11, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the offsets reported for a failed produce", &conn);
    if let Some(p) = resp
        .responses
        .first()
        .and_then(|t| t.partition_responses.first())
    {
        c.eq(
            "response.responses[0].partition_responses[0].base_offset",
            -1i64,
            p.base_offset,
        );
        c.eq(
            "response.responses[0].partition_responses[0].log_append_time_ms",
            -1i64,
            p.log_append_time_ms,
        );
    }
    c.finish()
});

kafka_test!(name_echoed, |ctx| {
    let name = ctx.unique("produce-unknown-name");
    let mut conn = ctx.connect().await?;
    let req = produce_request(&[(name.clone(), 0, batch(&["v"]))], 1);
    let resp = conn
        .request(PRODUCE_V11, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the echoed topic name", &conn);
    c.eq(
        "response.responses[0].name",
        Some(name),
        resp.responses.first().map(|t| t.name.0.to_string()),
    );
    c.finish()
});

kafka_test!(unknown_partition, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = produce_request(&[(t.name.clone(), 99, batch(&["v"]))], 1);
    let resp = conn
        .request(PRODUCE_V11, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new(
        format!(
            "producing to partition 99 of '{}', which has 1 partition",
            t.name
        ),
        &conn,
    );
    if let Some(p) = resp
        .responses
        .first()
        .and_then(|t| t.partition_responses.first())
    {
        c.eq(
            "response.responses[0].partition_responses[0].error_code",
            UNKNOWN_TOPIC_OR_PARTITION,
            p.error_code,
        );
    } else {
        c.that(
            "response.responses[0].partition_responses[0]",
            "one entry",
            false,
            "none",
        );
    }
    c.finish()
});

kafka_test!(mixed, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let unknown = ctx.unique("produce-absent");
    let mut conn = ctx.connect().await?;
    let req = produce_request(
        &[
            (t.name.clone(), 0, batch(&["good"])),
            (unknown.clone(), 0, batch(&["bad"])),
        ],
        1,
    );
    let resp = conn
        .request(PRODUCE_V11, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("a known and an unknown topic in one Produce", &conn);
    c.eq("response.responses.len", 2usize, resp.responses.len());
    for topic in &resp.responses {
        let name = topic.name.0.to_string();
        let want = if name == t.name {
            NONE
        } else {
            UNKNOWN_TOPIC_OR_PARTITION
        };
        let got = topic
            .partition_responses
            .first()
            .map(|p| p.error_code)
            .unwrap_or(-1);
        c.eq(
            &format!("response.responses[name={name}].partition_responses[0].error_code"),
            want,
            got,
        );
    }
    c.finish()
});

kafka_test!(decodes_cleanly, |ctx| {
    let name = ctx.unique("produce-unknown-trailing");
    let mut conn = ctx.connect().await?;
    let req = produce_request(&[(name, 0, batch(&["v"]))], 1);
    let decoded = conn
        .request(PRODUCE_V11, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the Produce v11 response body", &conn);
    c.eq("response.trailing_bytes", 0usize, decoded.trailing);
    c.at_least("response.throttle_time_ms", 0i32, decoded.throttle_time_ms);
    c.finish()
});

/// Worked examples: the error on its own, and the same error next to a success.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("Produce to a topic that was never created", |env| {
            env.request(
                PRODUCE_V11,
                301,
                &produce_request(
                    &[("kafkatest-no-such-topic".to_string(), 0, batch(&["hello"]))],
                    1,
                ),
            )
        })
        .request(
            "Produce v11, correlation id 301, acks 1, one topic 'kafkatest-no-such-topic', \
             partition 0, carrying a one-record v2 batch",
        )
        .response(
            "One responses entry echoing the name, holding one partition_responses entry: \
             index 0, error_code 3 (UNKNOWN_TOPIC_OR_PARTITION), base_offset -1, \
             log_append_time_ms -1, log_start_offset -1, an empty record_errors array and a \
             null error_message; then throttle_time_ms 0",
        )
        .note(
            "The error is per partition, not at the top of the response — there is no \
             top-level error_code in Produce at all. And do not create the topic: \
             auto-creation is off, so an unknown name is an error, never a side effect.",
        ),
        ExampleSpec::wire("One known topic and one unknown, in one request", |env| {
            env.request(
                PRODUCE_V11,
                302,
                &produce_request(
                    &[
                        (env.name("t1")?, 0, batch(&["good"])),
                        ("kafkatest-no-such-topic".to_string(), 0, batch(&["bad"])),
                    ],
                    1,
                ),
            )
        })
        .with_fixtures(one_topic)
        .request(
            "Produce v11, correlation id 302, acks 1: partition 0 of the fixture topic \
             (which exists) and partition 0 of 'kafkatest-no-such-topic' (which does not), \
             each with a one-record batch",
        )
        .response(
            "Two responses entries, one per requested topic: the fixture topic answers \
             error_code 0 (NONE) with base_offset 0, and 'kafkatest-no-such-topic' answers \
             error_code 3 (UNKNOWN_TOPIC_OR_PARTITION) with base_offset -1",
        )
        .note(
            "One bad topic must not fail the whole request. Resolve and append each \
             partition independently, and answer one entry for every partition that was \
             asked for, so the producer can match them up by name and index.",
        ),
    ]
}
