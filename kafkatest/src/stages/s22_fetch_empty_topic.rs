//! Stage 22 — Fetch a topic that exists but has no records.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::fixtures::{FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::stages::{fetch_request, proto_fail, Stage, Test, FETCH_V16, NONE};

fn empty_topic() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 1))
}

fn empty_two_partitions() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 2))
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 22,
        slug: "fetch_empty_topic",
        name: "Fetch an empty topic",
        ext: false,
        hints: &[
            "A topic that exists but holds nothing is error_code 0, not 3 or 100",
            "high_watermark and log_start_offset are both 0 for an empty partition",
            "records may be null or a zero-length compact bytes field; both mean 'nothing'",
            "The partition file <topic>-<n>/00000000000000000000.log may be zero bytes long",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("the partition error code is 0", error_code).with_fixtures(empty_topic),
            Test::new("the high watermark is 0", high_watermark).with_fixtures(empty_topic),
            Test::new("no records come back", no_records).with_fixtures(empty_topic),
            Test::new("the log start offset is 0", log_start_offset).with_fixtures(empty_topic),
            Test::new("the topic id is echoed", echoes_id).with_fixtures(empty_topic),
            Test::new("both partitions of an empty topic answer", two_partitions)
                .with_fixtures(empty_two_partitions),
            Test::new("the body decodes with no trailing bytes", decodes_cleanly)
                .with_fixtures(empty_topic),
        ],
    }
}

kafka_test!(error_code, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = fetch_request(&[(t.id, 0, 0)], 200);
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new(format!("fetching the empty topic '{}'", t.name), &conn);
    c.eq("response.error_code", NONE, resp.error_code);
    match resp.responses.first().and_then(|r| r.partitions.first()) {
        Some(p) => c.eq(
            "response.responses[0].partitions[0].error_code",
            NONE,
            p.error_code,
        ),
        None => c.that(
            "response.responses[0].partitions[0]",
            "one partition entry",
            false,
            "none",
        ),
    };
    c.finish()
});

kafka_test!(high_watermark, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = fetch_request(&[(t.id, 0, 0)], 200);
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the high watermark of an empty partition", &conn);
    if let Some(p) = resp.responses.first().and_then(|r| r.partitions.first()) {
        c.eq(
            "response.responses[0].partitions[0].high_watermark",
            0i64,
            p.high_watermark,
        );
    }
    c.finish()
});

kafka_test!(no_records, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = fetch_request(&[(t.id, 0, 0)], 200);
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the records of an empty partition", &conn);
    if let Some(p) = resp.responses.first().and_then(|r| r.partitions.first()) {
        let len = p.records.as_ref().map(|r| r.len()).unwrap_or(0);
        c.eq(
            "response.responses[0].partitions[0].records.len",
            0usize,
            len,
        );
    }
    c.finish()
});

kafka_test!(log_start_offset, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = fetch_request(&[(t.id, 0, 0)], 200);
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the log start offset of an empty partition", &conn);
    if let Some(p) = resp.responses.first().and_then(|r| r.partitions.first()) {
        c.eq(
            "response.responses[0].partitions[0].log_start_offset",
            0i64,
            p.log_start_offset,
        );
    }
    c.finish()
});

kafka_test!(echoes_id, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = fetch_request(&[(t.id, 0, 0)], 200);
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the topic id of an empty topic", &conn);
    c.eq(
        "response.responses[0].topic_id",
        t.id,
        resp.responses
            .first()
            .map(|r| r.topic_id)
            .unwrap_or_default(),
    );
    c.finish()
});

kafka_test!(two_partitions, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = fetch_request(&[(t.id, 0, 0), (t.id, 1, 0)], 200);
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("both partitions of an empty two-partition topic", &conn);
    let partitions = resp
        .responses
        .first()
        .map(|r| r.partitions.len())
        .unwrap_or(0);
    c.eq("response.responses[0].partitions.len", 2usize, partitions);
    if let Some(r) = resp.responses.first() {
        for p in &r.partitions {
            c.eq(
                &format!(
                    "response.responses[0].partitions[{}].error_code",
                    p.partition_index
                ),
                NONE,
                p.error_code,
            );
            c.eq(
                &format!(
                    "response.responses[0].partitions[{}].high_watermark",
                    p.partition_index
                ),
                0i64,
                p.high_watermark,
            );
        }
    }
    c.finish()
});

kafka_test!(decodes_cleanly, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = fetch_request(&[(t.id, 0, 0)], 200);
    let decoded = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the Fetch v16 body for an empty partition", &conn);
    c.eq("response.trailing_bytes", 0usize, decoded.trailing);
    c.finish()
});

/// Worked examples: a topic that exists and holds nothing at all.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("Fetch an existing topic with an empty log", |env| {
            env.request(
                FETCH_V16,
                221,
                &fetch_request(&[(env.id("t1")?, 0, 0)], 200),
            )
        })
        .with_fixtures(empty_topic)
        .request(
            "Fetch v16, correlation id 221: the fixture topic's id, partition 0, fetch_offset 0, \
             max_wait_ms 200",
        )
        .response(
            "Top-level error_code 0, one responses entry echoing the topic id, and one \
             partition: partition_index 0, error_code 0 (NONE), high_watermark 0, \
             log_start_offset 0 and an empty records field",
        )
        .note(
            "Empty is not an error: 0, not 3 (UNKNOWN_TOPIC_OR_PARTITION) and not 100 \
             (UNKNOWN_TOPIC_ID). The segment file 00000000000000000000.log is zero bytes long, \
             and zero bytes of records is a perfectly good answer.",
        ),
        ExampleSpec::wire("Both partitions of an empty topic", |env| {
            let id = env.id("t1")?;
            env.request(
                FETCH_V16,
                222,
                &fetch_request(&[(id, 0, 0), (id, 1, 0)], 200),
            )
        })
        .with_fixtures(empty_two_partitions)
        .request(
            "Fetch v16, correlation id 222: one topic entry carrying two partitions, 0 and 1, \
             both from offset 0",
        )
        .response(
            "One responses entry with two partition entries, in the order they were asked for, \
             each with error_code 0, high_watermark 0, log_start_offset 0 and no records",
        )
        .note(
            "Partitions of one topic share a single topic entry — the id is written once, the \
             partitions array holds the rest. Every partition that was asked about gets an \
             answer, even when there is nothing in it.",
        ),
    ]
}
