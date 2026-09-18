//! Stage 32 — Produce several records, partitions and topics in one request.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::fixtures::{FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::proto::records::{RecordBatch, RecordItem};
use crate::stages::{
    await_high_watermark, fetch_request, produce_request, proto_fail, Stage, Test, FETCH_V16, NONE,
    PRODUCE_V11,
};

fn two_partitions() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 2))
}

fn two_topics() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 1)).and(TopicSpec::new("t2", 1))
}

fn batch(values: &[&str]) -> Vec<u8> {
    RecordBatch::of(
        0,
        1_700_000_000_000,
        values.iter().map(|v| RecordItem::value(*v)).collect(),
    )
    .encode()
}

fn keyed_batch(pairs: &[(&str, &str)]) -> Vec<u8> {
    RecordBatch::of(
        0,
        1_700_000_000_000,
        pairs
            .iter()
            .map(|(k, v)| RecordItem::keyed(*k, *v))
            .collect(),
    )
    .encode()
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 32,
        slug: "produce_many",
        name: "Produce multiple records, partitions and topics",
        ext: false,
        hints: &[
            "One Produce request can carry several topics, each with several partitions",
            "base_offset is the offset of the *first* record of the batch; the rest follow it",
            "Every partition has its own log and its own offsets — never share a counter",
            "Answer one entry per requested partition, in the order they were sent",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new(
                "three records in one batch land at offsets 0..2",
                three_records,
            )
            .with_fixtures(two_partitions),
            Test::new("two partitions keep separate offsets", separate_offsets)
                .with_fixtures(two_partitions),
            Test::new(
                "two topics in one request are both written",
                two_topics_test,
            )
            .with_fixtures(two_topics),
            Test::new("records with keys survive the round trip", keys_survive)
                .with_fixtures(two_partitions),
            Test::new(
                "every produced record can be fetched back in order",
                fetch_back,
            )
            .with_fixtures(two_partitions),
            Test::new(
                "the multi-topic response decodes with no trailing bytes",
                decodes_cleanly,
            )
            .with_fixtures(two_topics),
        ],
    }
}

kafka_test!(three_records, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = produce_request(&[(t.name.clone(), 0, batch(&["r0", "r1", "r2"]))], -1);
    let resp = conn
        .request(PRODUCE_V11, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("a three-record batch", &conn);
    if let Some(p) = resp
        .responses
        .first()
        .and_then(|t| t.partition_responses.first())
    {
        c.eq(
            "response.responses[0].partition_responses[0].error_code",
            NONE,
            p.error_code,
        );
        c.eq(
            "response.responses[0].partition_responses[0].base_offset",
            0i64,
            p.base_offset,
        );
    }
    c.finish()?;
    await_high_watermark(ctx, &t, 0, 3).await
});

kafka_test!(separate_offsets, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = produce_request(
        &[
            (t.name.clone(), 0, batch(&["p0-a", "p0-b"])),
            (t.name.clone(), 1, batch(&["p1-a"])),
        ],
        -1,
    );
    let resp = conn
        .request(PRODUCE_V11, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("two partitions written in one request", &conn);
    let entries = resp
        .responses
        .first()
        .map(|t| t.partition_responses.len())
        .unwrap_or(0);
    c.eq(
        "response.responses[0].partition_responses.len",
        2usize,
        entries,
    );
    if let Some(topic) = resp.responses.first() {
        for p in &topic.partition_responses {
            c.eq(
                &format!(
                    "response.responses[0].partition_responses[index={}].error_code",
                    p.index
                ),
                NONE,
                p.error_code,
            );
            c.eq(
                &format!(
                    "response.responses[0].partition_responses[index={}].base_offset",
                    p.index
                ),
                0i64,
                p.base_offset,
            );
        }
    }
    c.finish()?;
    await_high_watermark(ctx, &t, 0, 2).await?;
    await_high_watermark(ctx, &t, 1, 1).await
});

kafka_test!(two_topics_test, |ctx| {
    let t1 = ctx.topic("t1")?.clone();
    let t2 = ctx.topic("t2")?.clone();
    let mut conn = ctx.connect().await?;
    let req = produce_request(
        &[
            (t1.name.clone(), 0, batch(&["one"])),
            (t2.name.clone(), 0, batch(&["two", "three"])),
        ],
        -1,
    );
    let resp = conn
        .request(PRODUCE_V11, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("two topics written in one request", &conn);
    c.eq("response.responses.len", 2usize, resp.responses.len());
    for topic in &resp.responses {
        let name = topic.name.0.to_string();
        let got = topic
            .partition_responses
            .first()
            .map(|p| p.error_code)
            .unwrap_or(-1);
        c.eq(
            &format!("response.responses[name={name}].partition_responses[0].error_code"),
            NONE,
            got,
        );
    }
    c.finish()?;
    await_high_watermark(ctx, &t1, 0, 1).await?;
    await_high_watermark(ctx, &t2, 0, 2).await
});

kafka_test!(keys_survive, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = produce_request(
        &[(
            t.name.clone(),
            0,
            keyed_batch(&[("k1", "v1"), ("k2", "v2")]),
        )],
        -1,
    );
    let resp = conn
        .request(PRODUCE_V11, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("a batch of keyed records", &conn);
    if let Some(p) = resp
        .responses
        .first()
        .and_then(|t| t.partition_responses.first())
    {
        c.eq(
            "response.responses[0].partition_responses[0].error_code",
            NONE,
            p.error_code,
        );
    }
    c.finish()?;
    await_high_watermark(ctx, &t, 0, 2).await?;

    let mut conn = ctx.connect().await?;
    let fetch = fetch_request(&[(t.id, 0, 0)], 1_000);
    let resp = conn
        .request(FETCH_V16, &fetch)
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
    let pairs: Vec<(String, String)> = batches
        .iter()
        .flat_map(|b| b.records.iter())
        .map(|r| {
            (
                String::from_utf8_lossy(r.key.as_deref().unwrap_or_default()).to_string(),
                String::from_utf8_lossy(r.value.as_deref().unwrap_or_default()).to_string(),
            )
        })
        .collect();
    let mut c = Check::new("the keyed records read back", &conn);
    c.eq(
        "records[*] (key, value)",
        vec![
            ("k1".to_string(), "v1".to_string()),
            ("k2".to_string(), "v2".to_string()),
        ],
        pairs,
    );
    c.finish()
});

kafka_test!(fetch_back, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = produce_request(&[(t.name.clone(), 0, batch(&["a", "b", "c", "d"]))], -1);
    conn.request(PRODUCE_V11, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    await_high_watermark(ctx, &t, 0, 4).await?;

    let mut conn = ctx.connect().await?;
    let fetch = fetch_request(&[(t.id, 0, 0)], 1_000);
    let resp = conn
        .request(FETCH_V16, &fetch)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let p = resp
        .responses
        .first()
        .and_then(|r| r.partitions.first())
        .ok_or_else(|| Failure::harness("the fetch returned no partition entry"))?;
    let bytes = p.records.clone().map(|b| b.to_vec()).unwrap_or_default();
    let batches = RecordBatch::decode_all(&bytes)
        .map_err(|e| Failure::harness(format!("the fetched records do not decode: {e:#}")))?;
    let values: Vec<String> = batches
        .iter()
        .flat_map(|b| b.records.iter())
        .map(|r| String::from_utf8_lossy(r.value.as_deref().unwrap_or_default()).to_string())
        .collect();
    let mut c = Check::new("four produced records read back", &conn);
    c.eq(
        "records[*].value",
        vec!["a".to_string(), "b".into(), "c".into(), "d".into()],
        values,
    );
    c.eq(
        "response.responses[0].partitions[0].high_watermark",
        4i64,
        p.high_watermark,
    );
    c.finish()
});

kafka_test!(decodes_cleanly, |ctx| {
    let t1 = ctx.topic("t1")?.clone();
    let t2 = ctx.topic("t2")?.clone();
    let mut conn = ctx.connect().await?;
    let req = produce_request(
        &[
            (t1.name.clone(), 0, batch(&["one"])),
            (t2.name.clone(), 0, batch(&["two"])),
        ],
        -1,
    );
    let decoded = conn
        .request(PRODUCE_V11, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("a two-topic Produce response body", &conn);
    c.eq("response.trailing_bytes", 0usize, decoded.trailing);
    c.finish()
});

/// Worked examples: many records in one batch, and many partitions in one request.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("Three records in one batch", |env| {
            env.request(
                PRODUCE_V11,
                321,
                &produce_request(&[(env.name("t1")?, 0, batch(&["r0", "r1", "r2"]))], -1),
            )
        })
        .with_fixtures(two_partitions)
        .request(
            "Produce v11, correlation id 321, acks -1: partition 0 of one topic, one v2 \
             RecordBatch with recordCount 3 and lastOffsetDelta 2, holding 'r0', 'r1', 'r2'",
        )
        .response(
            "One partition_responses entry only: index 0, error_code 0 (NONE), base_offset \
             0, log_append_time_ms -1, log_start_offset 0. The records occupy offsets 0, 1 \
             and 2, and the high watermark moves to 3.",
        )
        .note(
            "There is one entry per partition, never one per record: the response reports \
             the base offset, and every further record is base_offset + its offsetDelta. \
             Advance the log end offset by lastOffsetDelta + 1, not by 1.",
        ),
        ExampleSpec::wire("Two partitions of one topic in one request", |env| {
            let name = env.name("t1")?;
            env.request(
                PRODUCE_V11,
                322,
                &produce_request(
                    &[
                        (name.clone(), 0, batch(&["p0-a", "p0-b"])),
                        (name, 1, batch(&["p1-a"])),
                    ],
                    -1,
                ),
            )
        })
        .with_fixtures(two_partitions)
        .request(
            "Produce v11, correlation id 322, acks -1: one topic entry whose partition_data \
             array carries two partitions — partition 0 with a two-record batch, partition 1 \
             with a one-record batch",
        )
        .response(
            "One responses entry for the topic, with two partition_responses in the same \
             order: index 0 with error_code 0 (NONE) and base_offset 0, index 1 with \
             error_code 0 and base_offset 0 as well",
        )
        .note(
            "Both base offsets are 0 because every partition is its own log with its own \
             offset counter. A single shared 'next offset' is the classic bug here, and it \
             shows up the moment a second partition is written.",
        ),
    ]
}
