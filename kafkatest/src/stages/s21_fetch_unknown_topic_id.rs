//! Stage 21 — Fetch for a topic id the broker has never seen.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::fixtures::{derive_topic_id, FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::stages::{fetch_request, proto_fail, Stage, Test, FETCH_V16, NONE, UNKNOWN_TOPIC_ID};
use uuid::Uuid;

fn one_topic() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 1))
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 21,
        slug: "fetch_unknown_topic_id",
        name: "Fetch an unknown topic id → UNKNOWN_TOPIC_ID (100)",
        ext: false,
        hints: &[
            "The top-level error_code stays 0: the failure belongs to the partition entry",
            "Per partition: error_code 100, high_watermark 0, records null (0xff as a compact \
             nullable bytes length)",
            "Echo the topic id you were given; there is no name to fall back on in v13+",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("the top-level error code is still 0", top_level_ok),
            Test::new("the partition error code is 100", partition_error),
            Test::new("the unknown topic id is echoed", echoes_id),
            Test::new("no records are returned", no_records),
            Test::new("the all-zero topic id is also unknown", nil_uuid),
            Test::new(
                "an unknown id next to a known one only fails its own entry",
                mixed,
            )
            .with_fixtures(one_topic),
        ],
    }
}

fn unknown_id(ctx: &crate::stages::Ctx) -> Uuid {
    derive_topic_id(ctx.seed ^ 0xdead_beef, "definitely-not-a-real-topic")
}

kafka_test!(top_level_ok, |ctx| {
    let id = unknown_id(ctx);
    let mut conn = ctx.connect().await?;
    let req = fetch_request(&[(id, 0, 0)], 500);
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the top level of a fetch for an unknown topic id", &conn);
    c.eq("response.error_code", NONE, resp.error_code);
    c.eq("response.responses.len", 1usize, resp.responses.len());
    c.finish()
});

kafka_test!(partition_error, |ctx| {
    let id = unknown_id(ctx);
    let mut conn = ctx.connect().await?;
    let req = fetch_request(&[(id, 0, 0)], 500);
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the partition entry of an unknown topic id", &conn);
    match resp.responses.first().and_then(|t| t.partitions.first()) {
        Some(p) => {
            c.eq(
                "response.responses[0].partitions[0].error_code",
                UNKNOWN_TOPIC_ID,
                p.error_code,
            );
            c.eq(
                "response.responses[0].partitions[0].partition_index",
                0i32,
                p.partition_index,
            );
        }
        None => {
            c.that(
                "response.responses[0].partitions[0]",
                "one partition entry",
                false,
                "none",
            );
        }
    }
    c.finish()
});

kafka_test!(echoes_id, |ctx| {
    let id = unknown_id(ctx);
    let mut conn = ctx.connect().await?;
    let req = fetch_request(&[(id, 0, 0)], 500);
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the echoed topic id", &conn);
    c.eq(
        "response.responses[0].topic_id",
        id,
        resp.responses
            .first()
            .map(|t| t.topic_id)
            .unwrap_or_default(),
    );
    c.finish()
});

kafka_test!(no_records, |ctx| {
    let id = unknown_id(ctx);
    let mut conn = ctx.connect().await?;
    let req = fetch_request(&[(id, 0, 0)], 500);
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the records of an unknown topic id", &conn);
    if let Some(p) = resp.responses.first().and_then(|t| t.partitions.first()) {
        let len = p.records.as_ref().map(|r| r.len()).unwrap_or(0);
        c.eq(
            "response.responses[0].partitions[0].records.len",
            0usize,
            len,
        );
        // Apache Kafka reports -1 for a partition it does not host; hand-written brokers
        // report 0. Both are "nothing here", so only non-positive is required.
        c.at_most(
            "response.responses[0].partitions[0].high_watermark",
            0i64,
            p.high_watermark,
        );
    }
    c.finish()
});

kafka_test!(nil_uuid, |ctx| {
    let mut conn = ctx.connect().await?;
    let req = fetch_request(&[(Uuid::nil(), 0, 0)], 500);
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("a fetch for the all-zero topic id", &conn);
    c.note(
        "the zero UUID is reserved: Apache Kafka rejects the whole request with a top-level \
         error, hand-written brokers report UNKNOWN_TOPIC_ID per partition — either is fine, \
         hanging or crashing is not",
    );
    let per_partition = resp
        .responses
        .first()
        .and_then(|t| t.partitions.first())
        .map(|p| p.error_code);
    let handled = resp.error_code != NONE || per_partition == Some(UNKNOWN_TOPIC_ID);
    c.that(
        "response.error_code / responses[0].partitions[0].error_code",
        "a top-level error, or UNKNOWN_TOPIC_ID (100) on the partition",
        handled,
        (resp.error_code, per_partition),
    );
    c.finish()
});

kafka_test!(mixed, |ctx| {
    let known = ctx.topic("t1")?.clone();
    let unknown = unknown_id(ctx);
    let mut conn = ctx.connect().await?;
    let req = fetch_request(&[(known.id, 0, 0), (unknown, 0, 0)], 500);
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("a known and an unknown topic id in one fetch", &conn);
    c.eq("response.error_code", NONE, resp.error_code);
    c.eq("response.responses.len", 2usize, resp.responses.len());
    for t in &resp.responses {
        let want = if t.topic_id == known.id {
            NONE
        } else {
            UNKNOWN_TOPIC_ID
        };
        let got = t.partitions.first().map(|p| p.error_code).unwrap_or(-1);
        c.eq(
            &format!(
                "response.responses[topic_id={}].partitions[0].error_code",
                t.topic_id
            ),
            want,
            got,
        );
    }
    c.finish()
});

/// A topic id no broker has ever created, fixed so the example bytes never move.
const EXAMPLE_UNKNOWN_ID: Uuid = Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0001);

/// Worked examples: an id that is not there, and one that is not there beside one that is.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("Fetch a topic id the broker has never seen", |env| {
            env.request(
                FETCH_V16,
                211,
                &fetch_request(&[(EXAMPLE_UNKNOWN_ID, 0, 0)], 500),
            )
        })
        .request(
            "Fetch v16, correlation id 211, one topic: topic_id \
             00000000-0000-4000-8000-000000000001, partition 0, fetch_offset 0",
        )
        .response(
            "throttle_time_ms 0 and a top-level error_code of 0, then one responses entry \
             echoing those same 16 id bytes, holding one partition: partition_index 0, \
             error_code 100 (UNKNOWN_TOPIC_ID), high_watermark -1 and an empty records field",
        )
        .note(
            "The top level stays 0 — the request was well formed, the topic was not there. \
             From Fetch v13 there is no topic name to fall back on, so echo the id you were \
             given: that is the only thing the client can match the entry against.",
        ),
        ExampleSpec::wire("An unknown id beside a known one", |env| {
            env.request(
                FETCH_V16,
                212,
                &fetch_request(&[(env.id("t1")?, 0, 0), (EXAMPLE_UNKNOWN_ID, 0, 0)], 500),
            )
        })
        .with_fixtures(one_topic)
        .request(
            "Fetch v16, correlation id 212, two topics: the real (empty) fixture topic's id and \
             the same made-up id, each asking for partition 0 at offset 0",
        )
        .response(
            "Top-level error_code 0 and two responses entries, in the order they were asked \
             for: the fixture topic with error_code 0, high_watermark 0 and no records, then \
             the made-up id with error_code 100 (UNKNOWN_TOPIC_ID)",
        )
        .note(
            "One unknown topic must not spoil the rest of the answer. In Fetch every failure \
             that belongs to a partition is reported on that partition; the top-level \
             error_code is about the request and its fetch session, nothing else.",
        ),
    ]
}
