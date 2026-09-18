//! Stage 31 — Produce one record into an empty topic.

use crate::assert::{Check, Failure};
use crate::examples::{ExampleSpec, Wire};
use crate::fixtures::{FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::proto::records::{RecordBatch, RecordItem};
use crate::stages::{
    await_high_watermark, fetch_request, produce_request, proto_fail, Stage, Test, FETCH_V16, NONE,
    PRODUCE_V11,
};

fn empty_topic() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 1))
}

fn batch(base: i64, values: &[&str]) -> Vec<u8> {
    RecordBatch::of(
        base,
        1_700_000_000_000,
        values.iter().map(|v| RecordItem::value(*v)).collect(),
    )
    .encode()
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 31,
        slug: "produce_one_record",
        name: "Produce one record",
        ext: false,
        hints: &[
            "Append the batch bytes to <topic>-<partition>/00000000000000000000.log as they are",
            "Rewrite only baseOffset, to the current end of the log; the CRC covers the \
             bytes after it, so it stays valid",
            "Answer error_code 0, base_offset = the offset the first record landed on, \
             log_append_time_ms -1 (the records keep their CreateTime)",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("the produce succeeds", succeeds).with_fixtures(empty_topic),
            Test::new("the base offset of the first record is 0", base_offset_zero)
                .with_fixtures(empty_topic),
            Test::new("log_append_time_ms is -1", log_append_time).with_fixtures(empty_topic),
            Test::new("the topic name is echoed", name_echoed).with_fixtures(empty_topic),
            Test::new("a second produce continues at offset 1", second_produce)
                .with_fixtures(empty_topic),
            Test::new("the record can be fetched back", fetch_back).with_fixtures(empty_topic),
            Test::new(
                "the response decodes with no trailing bytes",
                decodes_cleanly,
            )
            .with_fixtures(empty_topic),
        ],
    }
}

async fn produce(
    ctx: &crate::stages::Ctx,
    name: &str,
    partition: i32,
    values: &[&str],
) -> Result<(i16, i64, i64), Failure> {
    let mut conn = ctx.connect().await?;
    let req = produce_request(&[(name.to_string(), partition, batch(0, values))], -1);
    let resp = conn
        .request(PRODUCE_V11, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new(
        format!("producing {} record(s) to '{name}'", values.len()),
        &conn,
    );
    let Some(p) = resp
        .responses
        .first()
        .and_then(|t| t.partition_responses.first())
    else {
        c.that(
            "response.responses[0].partition_responses[0]",
            "one partition entry",
            false,
            "none",
        );
        return Err(c
            .finish()
            .err()
            .unwrap_or_else(|| Failure::harness("no partition entry")));
    };
    c.eq(
        "response.responses[0].partition_responses[0].error_code",
        NONE,
        p.error_code,
    );
    c.finish()?;
    Ok((p.error_code, p.base_offset, p.log_append_time_ms))
}

kafka_test!(succeeds, |ctx| {
    let t = ctx.topic("t1")?.clone();
    produce(ctx, &t.name, 0, &["first"]).await?;
    Ok(())
});

kafka_test!(base_offset_zero, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let (_, base, _) = produce(ctx, &t.name, 0, &["first"]).await?;
    let mut c = Check::detached("the base offset of the first produce");
    c.eq(
        "response.responses[0].partition_responses[0].base_offset",
        0i64,
        base,
    );
    c.finish()
});

kafka_test!(log_append_time, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let (_, _, log_append) = produce(ctx, &t.name, 0, &["first"]).await?;
    let mut c = Check::detached("log_append_time_ms with the default CreateTime timestamps");
    c.eq(
        "response.responses[0].partition_responses[0].log_append_time_ms",
        -1i64,
        log_append,
    );
    c.finish()
});

kafka_test!(name_echoed, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = produce_request(&[(t.name.clone(), 0, batch(0, &["first"]))], -1);
    let resp = conn
        .request(PRODUCE_V11, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the echoed topic name", &conn);
    c.eq(
        "response.responses[0].name",
        Some(t.name.clone()),
        resp.responses.first().map(|r| r.name.0.to_string()),
    );
    c.eq(
        "response.responses[0].partition_responses[0].index",
        0i32,
        resp.responses
            .first()
            .and_then(|r| r.partition_responses.first())
            .map(|p| p.index)
            .unwrap_or(-1),
    );
    c.finish()
});

kafka_test!(second_produce, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let (_, first, _) = produce(ctx, &t.name, 0, &["first"]).await?;
    let (_, second, _) = produce(ctx, &t.name, 0, &["second"]).await?;
    let mut c = Check::detached("the base offsets of two consecutive produces");
    c.eq("produce[0].base_offset", 0i64, first);
    c.eq("produce[1].base_offset", 1i64, second);
    c.finish()
});

kafka_test!(fetch_back, |ctx| {
    let t = ctx.topic("t1")?.clone();
    produce(ctx, &t.name, 0, &["round-trip"]).await?;
    await_high_watermark(ctx, &t, 0, 1).await?;
    let mut conn = ctx.connect().await?;
    let req = fetch_request(&[(t.id, 0, 0)], 1_000);
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let bytes = resp
        .responses
        .first()
        .and_then(|r| r.partitions.first())
        .and_then(|p| p.records.clone())
        .map(|b| b.to_vec())
        .unwrap_or_default();
    let batches = RecordBatch::decode_all(&bytes)
        .map_err(|e| Failure::harness(format!("the fetched records do not decode: {e:#}")))?;
    let value = batches
        .first()
        .and_then(|b| b.records.first())
        .and_then(|r| r.value.clone())
        .unwrap_or_default();
    let mut c = Check::new("the record read back after producing it", &conn);
    c.bytes_eq("records.batches[0].records[0].value", b"round-trip", &value);
    c.eq(
        "records.batches[0].base_offset",
        0i64,
        batches.first().map(|b| b.base_offset).unwrap_or(-1),
    );
    c.finish()
});

kafka_test!(decodes_cleanly, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = produce_request(&[(t.name.clone(), 0, batch(0, &["v"]))], -1);
    let decoded = conn
        .request(PRODUCE_V11, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the Produce v11 success body", &conn);
    c.eq("response.trailing_bytes", 0usize, decoded.trailing);
    c.finish()
});

/// Worked examples: the first record ever written, and the one after it.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("One record into an empty partition", |env| {
            env.request(
                PRODUCE_V11,
                311,
                &produce_request(&[(env.name("t1")?, 0, batch(0, &["hello kafkatest"]))], -1),
            )
        })
        .with_fixtures(empty_topic)
        .request(
            "Produce v11, correlation id 311, acks -1, timeout_ms 5000: one topic, partition \
             0, one v2 RecordBatch whose baseOffset is 0, baseTimestamp 1700000000000 and \
             which holds the single record 'hello kafkatest'",
        )
        .response(
            "One responses entry with the topic name, one partition_responses entry: index \
             0, error_code 0 (NONE), base_offset 0, log_append_time_ms -1, log_start_offset \
             0, empty record_errors, null error_message; throttle_time_ms 0",
        )
        .note(
            "log_append_time_ms is -1 because the batch says CreateTime: the producer's own \
             timestamps were kept, so there is no broker time to report. base_offset is the \
             offset the *first* record of the batch landed on, which on an empty log is 0.",
        ),
        ExampleSpec::wire("Two produces in a row: offsets 0 then 1", |env| {
            let name = env.name("t1")?;
            Ok(Wire::Frames(vec![
                env.payload(
                    PRODUCE_V11,
                    312,
                    &produce_request(&[(name.clone(), 0, batch(0, &["first"]))], -1),
                )?,
                env.payload(
                    PRODUCE_V11,
                    313,
                    &produce_request(&[(name, 0, batch(0, &["second"]))], -1),
                )?,
            ]))
        })
        .with_fixtures(empty_topic)
        .request(
            "Two Produce v11 frames written back to back, correlation ids 312 and 313, each \
             one record to partition 0 of the same topic — and note that both batches carry \
             baseOffset 0 on the wire",
        )
        .response(
            "Two response frames in order: 312 answers base_offset 0, 313 answers \
             base_offset 1, both with error_code 0 (NONE) and log_append_time_ms -1",
        )
        .note(
            "A producer always writes baseOffset 0; assigning the real offset is the \
             broker's job. Rewrite that one 8-byte field to the current end of the log \
             before appending — the CRC-32C covers only the bytes after it, so it stays \
             valid and the rest of the batch is stored untouched.",
        ),
    ]
}
