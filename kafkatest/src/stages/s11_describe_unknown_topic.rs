//! Stage 11 — DescribeTopicPartitions for a topic that does not exist.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::kafka_test;
use crate::stages::{
    describe_request, proto_fail, Stage, Test, DESCRIBE_V0, NONE, UNKNOWN_TOPIC_OR_PARTITION,
};
use uuid::Uuid;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 11,
        slug: "describe_unknown_topic",
        name: "DescribeTopicPartitions: unknown topic",
        ext: false,
        hints: &[
            "Look the name up in the metadata you loaded; a miss is not an error for the \
             connection, only for that topic entry",
            "error_code 3 (UNKNOWN_TOPIC_OR_PARTITION), topic_id all-zero, partitions empty",
            "The response topic name echoes the requested name, so the client can match entries",
            "next_cursor is null (0xff) when there is nothing left to page through",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("an unknown topic returns error 3", error_code),
            Test::new("the topic id of an unknown topic is all zeroes", zero_uuid),
            Test::new(
                "the partitions array of an unknown topic is empty",
                no_partitions,
            ),
            Test::new("the requested name is echoed back", name_echoed),
            Test::new("next_cursor is null", cursor_null),
            Test::new("two unknown topics both return error 3", two_unknown),
            Test::new(
                "the response body decodes with no trailing bytes",
                decodes_cleanly,
            ),
        ],
    }
}

kafka_test!(error_code, |ctx| {
    let name = ctx.unique("unknown-topic");
    let mut conn = ctx.connect().await?;
    let req = describe_request(&[&name], 100);
    let resp = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new(
        format!("DescribeTopicPartitions for the absent '{name}'"),
        &conn,
    );
    c.eq("response.topics.len", 1usize, resp.topics.len());
    if let Some(t) = resp.topics.first() {
        c.eq(
            "response.topics[0].error_code",
            UNKNOWN_TOPIC_OR_PARTITION,
            t.error_code,
        );
    }
    c.finish()
});

kafka_test!(zero_uuid, |ctx| {
    let name = ctx.unique("unknown-topic-id");
    let mut conn = ctx.connect().await?;
    let req = describe_request(&[&name], 100);
    let resp = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the topic id of an unknown topic", &conn);
    if let Some(t) = resp.topics.first() {
        c.eq("response.topics[0].topic_id", Uuid::nil(), t.topic_id);
    } else {
        c.that(
            "response.topics[0]",
            "one topic entry",
            false,
            resp.topics.len(),
        );
    }
    c.finish()
});

kafka_test!(no_partitions, |ctx| {
    let name = ctx.unique("unknown-topic-parts");
    let mut conn = ctx.connect().await?;
    let req = describe_request(&[&name], 100);
    let resp = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the partitions of an unknown topic", &conn);
    if let Some(t) = resp.topics.first() {
        c.eq(
            "response.topics[0].partitions.len",
            0usize,
            t.partitions.len(),
        );
    }
    c.finish()
});

kafka_test!(name_echoed, |ctx| {
    let name = ctx.unique("unknown-topic-name");
    let mut conn = ctx.connect().await?;
    let req = describe_request(&[&name], 100);
    let resp = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the echoed topic name", &conn);
    let got = resp
        .topics
        .first()
        .and_then(|t| t.name.as_ref())
        .map(|n| n.0.to_string());
    c.eq("response.topics[0].name", Some(name.clone()), got);
    c.finish()
});

kafka_test!(cursor_null, |ctx| {
    let name = ctx.unique("unknown-topic-cursor");
    let mut conn = ctx.connect().await?;
    let req = describe_request(&[&name], 100);
    let resp = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("next_cursor when nothing is left to page", &conn);
    c.that(
        "response.next_cursor",
        "null",
        resp.next_cursor.is_none(),
        resp.next_cursor
            .as_ref()
            .map(|c| format!("{}-{}", c.topic_name.0.as_str(), c.partition_index)),
    );
    c.finish()
});

kafka_test!(two_unknown, |ctx| {
    let a = ctx.unique("unknown-a");
    let b = ctx.unique("unknown-b");
    let mut names = [a.as_str(), b.as_str()];
    names.sort_unstable();
    let mut conn = ctx.connect().await?;
    let req = describe_request(&names, 100);
    let resp = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("two unknown topics in one request", &conn);
    c.eq("response.topics.len", 2usize, resp.topics.len());
    for (i, t) in resp.topics.iter().enumerate() {
        c.eq(
            &format!("response.topics[{i}].error_code"),
            UNKNOWN_TOPIC_OR_PARTITION,
            t.error_code,
        );
        c.eq(
            &format!("response.topics[{i}].topic_id"),
            Uuid::nil(),
            t.topic_id,
        );
    }
    c.finish()
});

kafka_test!(decodes_cleanly, |ctx| {
    let name = ctx.unique("unknown-trailing");
    let mut conn = ctx.connect().await?;
    let req = describe_request(&[&name], 100);
    let decoded = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the DescribeTopicPartitions v0 body", &conn);
    c.eq("response.trailing_bytes", 0usize, decoded.trailing);
    c.at_least("response.throttle_time_ms", 0i32, decoded.throttle_time_ms);
    c.eq(
        "response.topics[0].error_code",
        UNKNOWN_TOPIC_OR_PARTITION,
        decoded.topics.first().map(|t| t.error_code).unwrap_or(NONE),
    );
    c.finish()
});

/// Worked examples: names the broker has never heard of.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("One topic that does not exist", |env| {
            env.request(
                DESCRIBE_V0,
                111,
                &describe_request(&["kafkatest-missing"], 100),
            )
        })
        .request(
            "DescribeTopicPartitions v0, correlation id 111, topics ['kafkatest-missing'], \
             response_partition_limit 100, cursor null",
        )
        .response(
            "throttle_time_ms, then one topics entry: error_code 3 \
             (UNKNOWN_TOPIC_OR_PARTITION), the requested name echoed back, topic_id all \
             zeroes, an empty partitions array (compact length 0x01), \
             topic_authorized_operations, and finally next_cursor written as the null byte \
             0xff",
        )
        .note(
            "A missing topic is an error inside its own entry, never an error for the \
             request: the header, the throttle time and the connection are exactly as they \
             would be for a topic that exists.",
        ),
        ExampleSpec::wire("Two unknown topics in one request", |env| {
            env.request(
                DESCRIBE_V0,
                112,
                &describe_request(&["kafkatest-missing-a", "kafkatest-missing-b"], 100),
            )
        })
        .request(
            "DescribeTopicPartitions v0, correlation id 112, two topic names in one compact \
             array (length byte 0x03), response_partition_limit 100",
        )
        .response(
            "Two topics entries, one per requested name and in name order, each with \
             error_code 3, the name echoed, an all-zero topic_id and no partitions; \
             next_cursor is still null",
        )
        .note(
            "Answer one entry per requested name — never collapse the misses into a single \
             error, and never drop them. The response array's compact length is count + 1, \
             so two entries encode as 0x03.",
        ),
    ]
}
