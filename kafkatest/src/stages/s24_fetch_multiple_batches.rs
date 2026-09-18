//! Stage 24 — Fetch several batches and several partitions.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::fixtures::{rec, FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::proto::records::RecordBatch;
use crate::stages::{fetch_request, proto_fail, Stage, Test, FETCH_V16, NONE};

fn two_partitions() -> FixtureSpec {
    FixtureSpec::with(
        TopicSpec::new("t1", 2)
            .with_batch(0, vec![rec("a0"), rec("a1")])
            .with_batch(0, vec![rec("a2")])
            .with_batch(1, vec![rec("b0")]),
    )
}

fn two_topics() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 1).with_values(0, &["x0", "x1"]))
        .and(TopicSpec::new("t2", 1).with_values(0, &["y0"]))
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 24,
        slug: "fetch_multiple_batches",
        name: "Fetch multiple batches and partitions",
        ext: false,
        hints: &[
            "A partition's records field holds every batch, concatenated, not one batch",
            "Offsets continue across batches: batch 2's baseOffset is batch 1's \
             baseOffset + count",
            "high_watermark is the offset after the last record of the partition",
            "Each requested partition gets its own entry, in the order it was asked for",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("both batches of a partition come back", both_batches)
                .with_fixtures(two_partitions),
            Test::new("offsets continue across batches", offsets_continue)
                .with_fixtures(two_partitions),
            Test::new("the high watermark counts every record", high_watermark)
                .with_fixtures(two_partitions),
            Test::new("two partitions are answered independently", both_partitions)
                .with_fixtures(two_partitions),
            Test::new("record values arrive in order", values_in_order)
                .with_fixtures(two_partitions),
            Test::new("two topics in one fetch are both answered", two_topics_test)
                .with_fixtures(two_topics),
        ],
    }
}

async fn fetch(
    ctx: &crate::stages::Ctx,
    parts: &[(uuid::Uuid, i32, i64)],
) -> Result<kafka_protocol::messages::FetchResponse, Failure> {
    let mut conn = ctx.connect().await?;
    let req = fetch_request(parts, 1_000);
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the top level of the fetch response", &conn);
    c.eq("response.error_code", NONE, resp.error_code);
    c.finish()?;
    Ok(resp.body)
}

fn records_of(
    resp: &kafka_protocol::messages::FetchResponse,
    topic: uuid::Uuid,
    partition: i32,
) -> Result<Vec<RecordBatch>, Failure> {
    let bytes = resp
        .responses
        .iter()
        .find(|t| t.topic_id == topic)
        .and_then(|t| t.partitions.iter().find(|p| p.partition_index == partition))
        .and_then(|p| p.records.clone())
        .map(|b| b.to_vec())
        .unwrap_or_default();
    RecordBatch::decode_all(&bytes).map_err(|e| {
        Failure::harness(format!(
            "the records of partition {partition} do not decode: {e:#}"
        ))
    })
}

kafka_test!(both_batches, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let resp = fetch(ctx, &[(t.id, 0, 0)]).await?;
    let batches = records_of(&resp, t.id, 0)?;
    let mut c = Check::detached("the batches of partition 0");
    c.eq("records.batches.len", 2usize, batches.len());
    c.eq(
        "records.total_records",
        3usize,
        batches.iter().map(|b| b.records.len()).sum::<usize>(),
    );
    c.finish()
});

kafka_test!(offsets_continue, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let resp = fetch(ctx, &[(t.id, 0, 0)]).await?;
    let batches = records_of(&resp, t.id, 0)?;
    let bases: Vec<i64> = batches.iter().map(|b| b.base_offset).collect();
    let mut c = Check::detached("the base offsets of the batches");
    c.eq("records.batches[*].base_offset", vec![0i64, 2], bases);
    c.finish()
});

kafka_test!(high_watermark, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let resp = fetch(ctx, &[(t.id, 0, 0)]).await?;
    let hw = resp
        .responses
        .first()
        .and_then(|r| r.partitions.first())
        .map(|p| p.high_watermark)
        .unwrap_or(-1);
    let mut c = Check::detached("the high watermark of a partition with three records");
    c.eq(
        "response.responses[0].partitions[0].high_watermark",
        3i64,
        hw,
    );
    c.finish()
});

kafka_test!(both_partitions, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let resp = fetch(ctx, &[(t.id, 0, 0), (t.id, 1, 0)]).await?;
    let mut c = Check::detached("both partitions in one fetch");
    let partitions = resp
        .responses
        .first()
        .map(|r| r.partitions.len())
        .unwrap_or(0);
    c.eq("response.responses[0].partitions.len", 2usize, partitions);
    c.eq(
        "partition 0 record count",
        3usize,
        records_of(&resp, t.id, 0)?
            .iter()
            .map(|b| b.records.len())
            .sum::<usize>(),
    );
    c.eq(
        "partition 1 record count",
        1usize,
        records_of(&resp, t.id, 1)?
            .iter()
            .map(|b| b.records.len())
            .sum::<usize>(),
    );
    c.finish()
});

kafka_test!(values_in_order, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let resp = fetch(ctx, &[(t.id, 0, 0)]).await?;
    let batches = records_of(&resp, t.id, 0)?;
    let values: Vec<String> = batches
        .iter()
        .flat_map(|b| b.records.iter())
        .map(|r| String::from_utf8_lossy(r.value.as_deref().unwrap_or_default()).to_string())
        .collect();
    let mut c = Check::detached("the record values in offset order");
    c.eq(
        "records[*].value",
        vec!["a0".to_string(), "a1".into(), "a2".into()],
        values,
    );
    c.finish()
});

kafka_test!(two_topics_test, |ctx| {
    let t1 = ctx.topic("t1")?.clone();
    let t2 = ctx.topic("t2")?.clone();
    let resp = fetch(ctx, &[(t1.id, 0, 0), (t2.id, 0, 0)]).await?;
    let mut c = Check::detached("two topics in one fetch");
    c.eq("response.responses.len", 2usize, resp.responses.len());
    c.eq(
        "topic t1 record count",
        2usize,
        records_of(&resp, t1.id, 0)?
            .iter()
            .map(|b| b.records.len())
            .sum::<usize>(),
    );
    c.eq(
        "topic t2 record count",
        1usize,
        records_of(&resp, t2.id, 0)?
            .iter()
            .map(|b| b.records.len())
            .sum::<usize>(),
    );
    c.finish()
});

/// Worked examples: several batches in one partition, several partitions in one request.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("Two batches in one partition", |env| {
            env.request(
                FETCH_V16,
                241,
                &fetch_request(&[(env.id("t1")?, 0, 0)], 1_000),
            )
        })
        .with_fixtures(two_partitions)
        .request(
            "Fetch v16, correlation id 241: partition 0 of the fixture topic from offset 0. That \
             partition holds two batches — records 'a0' and 'a1', then 'a2'",
        )
        .response(
            "error_code 0, high_watermark 3, and a single records field carrying both batches \
             back to back: base_offset 0 with last_offset_delta 1, then base_offset 2 with \
             last_offset_delta 0",
        )
        .note(
            "records is one byte string per partition, not one per batch: concatenate the \
             segment bytes and let the client split them again. A batch's base_offset is the \
             previous base_offset plus the previous record count, which is why offsets run \
             0, 1, 2 straight across the seam.",
        ),
        ExampleSpec::wire("Two partitions in one request", |env| {
            let id = env.id("t1")?;
            env.request(
                FETCH_V16,
                242,
                &fetch_request(&[(id, 0, 0), (id, 1, 0)], 1_000),
            )
        })
        .with_fixtures(two_partitions)
        .request(
            "Fetch v16, correlation id 242: partitions 0 and 1 of the same topic, both from \
             offset 0",
        )
        .response(
            "One responses entry with two partition entries, in the order asked: partition 0 \
             with high_watermark 3 and its two batches, partition 1 with high_watermark 1 and \
             the single batch holding 'b0'",
        )
        .note(
            "Each partition is a separate log with its own offsets and its own high watermark; \
             nothing about partition 0 tells you anything about partition 1. Read them \
             independently and write the entries in the order they were requested.",
        ),
    ]
}
