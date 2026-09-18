//! Stage 35 — Produce to Fetch round trip, and offsets that survive a broker restart.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::fixtures::{rec, FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::proto::records::{RecordBatch, RecordItem};
use crate::stages::{
    await_high_watermark, error_label, expect_still_serving, fetch_request, produce_request,
    proto_fail, Ctx, Stage, Test, FETCH_V16, NONE, PRODUCE_V11,
};

/// Restarting the reference broker means a JVM shutdown and a full KRaft boot.
const RESTART_TIMEOUT_MS: u64 = 120_000;
/// Base timestamp of every batch this stage produces (a fixed instant, so runs compare).
const BASE_TIMESTAMP: i64 = 1_700_000_000_000;

fn empty_topic() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 1))
}

/// A topic that already holds three records, the way a log does after a restart.
fn seeded_topic() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 1).with_batch(0, vec![rec("a"), rec("b"), rec("c")]))
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 35,
        slug: "produce_fetch_restart",
        name: "Produce to fetch round trip across restarts",
        ext: true,
        hints: &[
            "A Fetch must hand back what Produce was given: keys, values, headers and \
             CreateTime timestamps, in offset order",
            "At startup, read the last segment of every partition to recover its end \
             offset; offsets continue from there, they never reset to 0",
            "The high watermark comes back from the log too, and the log start offset \
             stays 0 until something is deleted",
            "Only baseOffset and partitionLeaderEpoch are the broker's to rewrite; the \
             record bytes inside the batch belong to the producer",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new(
                "keys, values, headers and timestamps come back unchanged",
                round_trip,
            )
            .with_fixtures(empty_topic)
            .ext(),
            Test::new("a null key and a tombstone value survive", tombstones)
                .with_fixtures(empty_topic)
                .ext(),
            Test::new(
                "the records are still there after a restart",
                survives_restart,
            )
            .with_fixtures(empty_topic)
            .timeout_ms(RESTART_TIMEOUT_MS)
            .ext(),
            Test::new(
                "offsets continue from the persisted log after a restart",
                offsets_continue,
            )
            .with_fixtures(empty_topic)
            .timeout_ms(RESTART_TIMEOUT_MS)
            .ext(),
            Test::new(
                "a fetch from the middle of the log works after a restart",
                fetch_middle_after_restart,
            )
            .with_fixtures(empty_topic)
            .timeout_ms(RESTART_TIMEOUT_MS)
            .ext(),
            Test::new(
                "the high watermark is restored and the broker serves again",
                watermark_restored,
            )
            .with_fixtures(empty_topic)
            .timeout_ms(RESTART_TIMEOUT_MS)
            .ext(),
        ],
    }
}

/// One record, rendered as the single line a failure message shows.
fn render(offset: i64, timestamp: i64, r: &RecordItem) -> String {
    let text = |b: &Option<Vec<u8>>| match b {
        None => "null".to_string(),
        Some(v) => String::from_utf8_lossy(v).to_string(),
    };
    let headers: Vec<String> = r
        .headers
        .iter()
        .map(|(k, v)| format!("{k}={}", text(v)))
        .collect();
    format!(
        "offset={offset} timestamp={timestamp} key={} value={} headers=[{}]",
        text(&r.key),
        text(&r.value),
        headers.join(", ")
    )
}

/// A batch of `n` records with keys, values, two headers each and rising timestamps.
fn rich_batch(n: usize) -> RecordBatch {
    let mut b = RecordBatch {
        base_offset: 0,
        base_timestamp: BASE_TIMESTAMP,
        max_timestamp: BASE_TIMESTAMP + n.saturating_sub(1) as i64,
        ..Default::default()
    };
    for i in 0..n {
        b.records.push(RecordItem {
            attributes: 0,
            timestamp_delta: i as i64,
            offset_delta: i as i32,
            key: Some(format!("key-{i}").into_bytes()),
            value: Some(format!("value-{i}-{}", "x".repeat(i)).into_bytes()),
            headers: vec![
                ("trace".to_string(), Some(format!("t{i}").into_bytes())),
                ("empty".to_string(), None),
            ],
        });
    }
    b.last_offset_delta = n.saturating_sub(1) as i32;
    b
}

/// What the records of a batch look like once the broker has assigned `base` as the first
/// offset — the same rendering the fetched records go through.
fn expected(batch: &RecordBatch, base: i64) -> Vec<String> {
    batch
        .records
        .iter()
        .map(|r| {
            render(
                base + r.offset_delta as i64,
                batch.base_timestamp + r.timestamp_delta,
                r,
            )
        })
        .collect()
}

/// Produce one batch with acks=-1, returning the base offset the broker assigned.
async fn produce(ctx: &Ctx, name: &str, batch: &RecordBatch) -> Result<i64, Failure> {
    let mut conn = ctx.connect().await?;
    let req = produce_request(&[(name.to_string(), 0, batch.encode())], -1);
    let resp = conn
        .request(PRODUCE_V11, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new(
        format!("producing {} record(s) to '{name}'", batch.records.len()),
        &conn,
    );
    let Some(p) = resp
        .responses
        .first()
        .and_then(|t| t.partition_responses.first())
    else {
        c.that(
            "response.responses[0].partition_responses[0]",
            "one entry for the partition",
            false,
            "no entry at all",
        );
        return Err(c
            .finish()
            .err()
            .unwrap_or_else(|| Failure::harness("no partition entry")));
    };
    c.that(
        "response.responses[0].partition_responses[0].error_code",
        &error_label(NONE),
        p.error_code == NONE,
        error_label(p.error_code),
    );
    c.finish()?;
    Ok(p.base_offset)
}

/// A partition's records from `from`, plus its high watermark and log start offset.
async fn fetch_from(
    ctx: &Ctx,
    topic: &crate::fixtures::TopicInfo,
    from: i64,
) -> Result<(Vec<String>, i64, i64), Failure> {
    let mut conn = ctx.connect().await?;
    let req = fetch_request(&[(topic.id, 0, from)], 2_000);
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new(
        format!("fetching '{}'-0 from offset {from}", topic.name),
        &conn,
    );
    let Some(p) = resp
        .responses
        .first()
        .and_then(|t| t.partitions.first())
        .cloned()
    else {
        c.that(
            "response.responses[0].partitions[0]",
            "one entry for the partition that was fetched",
            false,
            "no entry at all",
        );
        return Err(c
            .finish()
            .err()
            .unwrap_or_else(|| Failure::harness("no partition entry")));
    };
    c.that(
        "response.responses[0].partitions[0].error_code",
        &error_label(NONE),
        p.error_code == NONE,
        error_label(p.error_code),
    );
    c.finish()?;
    let bytes = p.records.clone().map(|b| b.to_vec()).unwrap_or_default();
    let batches = RecordBatch::decode_all(&bytes).map_err(|e| {
        Failure::harness(format!(
            "the records fetched from '{}'-0 do not decode: {e:#}",
            topic.name
        ))
    })?;
    let seen: Vec<String> = batches
        .iter()
        .flat_map(|b| {
            b.records.iter().map(move |r| {
                render(
                    b.base_offset + r.offset_delta as i64,
                    b.base_timestamp + r.timestamp_delta,
                    r,
                )
            })
        })
        // A fetch may start inside a batch: Kafka hands back whole batches and lets the
        // client drop what it has already seen.
        .filter(|line| {
            line.split_whitespace()
                .next()
                .and_then(|f| f.strip_prefix("offset="))
                .and_then(|n| n.parse::<i64>().ok())
                .map(|o| o >= from)
                .unwrap_or(true)
        })
        .collect();
    Ok((seen, p.high_watermark, p.log_start_offset))
}

kafka_test!(round_trip, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let batch = rich_batch(4);
    let base = produce(ctx, &t.name, &batch).await?;
    await_high_watermark(ctx, &t, 0, 4).await?;
    let (seen, hw, _) = fetch_from(ctx, &t, 0).await?;
    let mut c = Check::detached("the records fetched back after producing them");
    c.eq(
        "response.responses[0].partitions[0].base_offset",
        0i64,
        base,
    );
    c.eq("records[*]", expected(&batch, base), seen);
    c.eq(
        "response.responses[0].partitions[0].high_watermark",
        4i64,
        hw,
    );
    c.finish()
});

kafka_test!(tombstones, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut batch = RecordBatch {
        base_offset: 0,
        base_timestamp: BASE_TIMESTAMP,
        max_timestamp: BASE_TIMESTAMP,
        ..Default::default()
    };
    batch.records = vec![
        RecordItem {
            key: None,
            value: Some(b"no key here".to_vec()),
            ..Default::default()
        },
        RecordItem {
            offset_delta: 1,
            key: Some(b"deleted".to_vec()),
            value: None,
            ..Default::default()
        },
    ];
    batch.last_offset_delta = 1;
    let base = produce(ctx, &t.name, &batch).await?;
    await_high_watermark(ctx, &t, 0, 2).await?;
    let (seen, _, _) = fetch_from(ctx, &t, 0).await?;
    let mut c = Check::detached("a null key and a null (tombstone) value");
    c.eq("records[*]", expected(&batch, base), seen);
    c.finish()
});

kafka_test!(survives_restart, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let batch = rich_batch(5);
    let base = produce(ctx, &t.name, &batch).await?;
    await_high_watermark(ctx, &t, 0, 5).await?;

    ctx.restart_broker().await?;
    await_high_watermark(ctx, &t, 0, 5)
        .await
        .map_err(|f| f.note("the records were produced before the broker was restarted"))?;

    let (seen, hw, log_start) = fetch_from(ctx, &t, 0).await?;
    let mut c = Check::detached("the log after the broker was restarted");
    c.eq("records[*]", expected(&batch, base), seen);
    c.eq(
        "response.responses[0].partitions[0].high_watermark",
        5i64,
        hw,
    );
    c.eq(
        "response.responses[0].partitions[0].log_start_offset",
        0i64,
        log_start,
    );
    c.finish()
});

kafka_test!(offsets_continue, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let first = rich_batch(3);
    let before = produce(ctx, &t.name, &first).await?;
    await_high_watermark(ctx, &t, 0, 3).await?;

    ctx.restart_broker().await?;
    await_high_watermark(ctx, &t, 0, 3).await?;

    let second = rich_batch(2);
    let after = produce(ctx, &t.name, &second).await?;
    let mut c = Check::detached("the base offset of a produce after a restart");
    c.eq("produce[before restart].base_offset", 0i64, before);
    c.eq("produce[after restart].base_offset", 3i64, after);
    c.finish()?;

    await_high_watermark(ctx, &t, 0, 5).await?;
    let (seen, _, _) = fetch_from(ctx, &t, 0).await?;
    let mut want = expected(&first, before);
    want.extend(expected(&second, after));
    let mut c = Check::detached("the whole log, written on both sides of a restart");
    c.eq("records[*]", want, seen);
    c.finish()
});

kafka_test!(fetch_middle_after_restart, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let batch = rich_batch(5);
    let base = produce(ctx, &t.name, &batch).await?;
    await_high_watermark(ctx, &t, 0, 5).await?;

    ctx.restart_broker().await?;
    await_high_watermark(ctx, &t, 0, 5).await?;

    let (seen, hw, _) = fetch_from(ctx, &t, 3).await?;
    let want: Vec<String> = expected(&batch, base).into_iter().skip(3).collect();
    let mut c = Check::detached("a fetch from offset 3 after a restart");
    c.eq("records[*]", want, seen);
    c.eq(
        "response.responses[0].partitions[0].high_watermark",
        5i64,
        hw,
    );
    c.finish()
});

kafka_test!(watermark_restored, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let batch = rich_batch(2);
    produce(ctx, &t.name, &batch).await?;
    await_high_watermark(ctx, &t, 0, 2).await?;

    ctx.restart_broker().await?;
    expect_still_serving(ctx, "a restart").await?;
    await_high_watermark(ctx, &t, 0, 2).await?;

    // And the restarted broker still accepts new records into the recovered log.
    let more = rich_batch(1);
    let base = produce(ctx, &t.name, &more).await?;
    let mut c = Check::detached("a produce into the recovered log");
    c.eq(
        "response.responses[0].partition_responses[0].base_offset",
        2i64,
        base,
    );
    c.finish()?;
    let (_, hw, _) = fetch_from(ctx, &t, 0).await?;
    let mut c = Check::detached("the high watermark after the restart");
    c.eq(
        "response.responses[0].partitions[0].high_watermark",
        3i64,
        hw,
    );
    c.finish()
});

/// Worked examples: offsets that continue, and the fetch that reads them back.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("Producing into a log that already holds records", |env| {
            let records = RecordBatch::of(
                0,
                BASE_TIMESTAMP,
                vec![RecordItem::value("after the existing three")],
            )
            .encode();
            env.request(
                PRODUCE_V11,
                351,
                &produce_request(&[(env.name("t1")?, 0, records)], -1),
            )
        })
        .with_fixtures(seeded_topic)
        .request(
            "Produce v11, correlation id 351, acks -1: one record to partition 0 of a topic \
             whose log already holds three records at offsets 0, 1 and 2",
        )
        .response(
            "index 0, error_code 0 (NONE), base_offset 3 — not 0 — with log_append_time_ms \
             -1 and log_start_offset 0",
        )
        .note(
            "base_offset is the current end of the log, and the end of the log is whatever \
             is on disk. That is why this must still answer 3 after a restart: recover the \
             end offset by reading the last segment at startup, never from a counter that \
             begins at 0 when the process does.",
        ),
        ExampleSpec::wire("Fetching the records back", |env| {
            env.request(
                FETCH_V16,
                352,
                &fetch_request(&[(env.id("t1")?, 0, 0)], 1_000),
            )
        })
        .with_fixtures(seeded_topic)
        .request(
            "Fetch v16, correlation id 352, max_wait_ms 1000, min_bytes 1: topic by topic id \
             (v13 and later name topics by id, not by name), partition 0, fetch_offset 0",
        )
        .response(
            "error_code 0 (NONE) at the top, then one partitions entry: partition_index 0, \
             error_code 0, high_watermark 3, last_stable_offset 3, log_start_offset 0, and a \
             records field carrying the stored v2 batch with the three records at offsets 0, \
             1 and 2, keys, values and CreateTime timestamps as they were produced",
        )
        .note(
            "The records field is the segment's bytes handed back as they are — that is why \
             a Fetch after a restart is the real test of stage 34's on-disk format. Only \
             baseOffset and partitionLeaderEpoch were ever the broker's to write; everything \
             inside the batch belongs to the producer and must come back byte for byte.",
        ),
    ]
}
