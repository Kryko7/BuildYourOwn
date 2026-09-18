//! Stage 23 — Fetch a single record from the log.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::fixtures::{rec, FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::proto::records::RecordBatch;
use crate::stages::{fetch_request, proto_fail, Stage, Test, FETCH_V16, NONE};

const VALUE: &str = "hello kafkatest";

fn one_record() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 1).with_batch(0, vec![rec(VALUE)]))
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 23,
        slug: "fetch_single_record",
        name: "Fetch a single record from disk",
        ext: false,
        hints: &[
            "Read <topic>-<partition>/00000000000000000000.log and send the batch bytes \
             straight through: no re-encoding is needed",
            "records is a compact nullable bytes field: uvarint(length + 1) then the bytes",
            "high_watermark is the offset after the last record, so 1 for a single record",
            "The batch header already carries baseOffset, lastOffsetDelta and the CRC-32C",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("the fetch succeeds", succeeds).with_fixtures(one_record),
            Test::new("one record comes back", one_record_back).with_fixtures(one_record),
            Test::new("the record value is intact", value_intact).with_fixtures(one_record),
            Test::new("the batch base offset is 0", base_offset).with_fixtures(one_record),
            Test::new("the high watermark is 1", high_watermark).with_fixtures(one_record),
            Test::new("the batch CRC verifies", crc_ok).with_fixtures(one_record),
            Test::new("the returned bytes match the log segment", matches_segment)
                .with_fixtures(one_record)
                .ext(),
        ],
    }
}

async fn fetch_records(
    ctx: &crate::stages::Ctx,
    offset: i64,
) -> Result<(Vec<u8>, i64, i16), Failure> {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = fetch_request(&[(t.id, 0, offset)], 1_000);
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new(format!("fetching '{}' from offset {offset}", t.name), &conn);
    c.eq("response.error_code", NONE, resp.error_code);
    let Some(p) = resp.responses.first().and_then(|r| r.partitions.first()) else {
        c.that(
            "response.responses[0].partitions[0]",
            "one partition entry",
            false,
            "none",
        );
        return Err(c
            .finish()
            .err()
            .unwrap_or_else(|| Failure::harness("no partition")));
    };
    c.eq(
        "response.responses[0].partitions[0].error_code",
        NONE,
        p.error_code,
    );
    c.finish()?;
    Ok((
        p.records.as_ref().map(|r| r.to_vec()).unwrap_or_default(),
        p.high_watermark,
        p.error_code,
    ))
}

kafka_test!(succeeds, |ctx| {
    let (records, _, error) = fetch_records(ctx, 0).await?;
    let mut c = Check::detached("that a fetch of a non-empty partition succeeds");
    c.eq(
        "response.responses[0].partitions[0].error_code",
        NONE,
        error,
    );
    c.at_least(
        "response.responses[0].partitions[0].records.len",
        1usize,
        records.len(),
    );
    c.finish()
});

kafka_test!(one_record_back, |ctx| {
    let (records, _, _) = fetch_records(ctx, 0).await?;
    let batches = RecordBatch::decode_all(&records)
        .map_err(|e| Failure::harness(format!("the returned records do not decode: {e:#}")))?;
    let mut c = Check::detached("the decoded record batches");
    c.eq("records.batches.len", 1usize, batches.len());
    c.eq(
        "records.batches[0].records.len",
        1usize,
        batches.first().map(|b| b.records.len()).unwrap_or(0),
    );
    c.finish()
});

kafka_test!(value_intact, |ctx| {
    let (records, _, _) = fetch_records(ctx, 0).await?;
    let batches = RecordBatch::decode_all(&records)
        .map_err(|e| Failure::harness(format!("the returned records do not decode: {e:#}")))?;
    let got = batches
        .first()
        .and_then(|b| b.records.first())
        .and_then(|r| r.value.clone())
        .unwrap_or_default();
    let mut c = Check::detached("the record value");
    c.bytes_eq(
        "records.batches[0].records[0].value",
        VALUE.as_bytes(),
        &got,
    );
    c.finish()
});

kafka_test!(base_offset, |ctx| {
    let (records, _, _) = fetch_records(ctx, 0).await?;
    let batches = RecordBatch::decode_all(&records)
        .map_err(|e| Failure::harness(format!("the returned records do not decode: {e:#}")))?;
    let mut c = Check::detached("the batch header");
    c.eq(
        "records.batches[0].base_offset",
        0i64,
        batches.first().map(|b| b.base_offset).unwrap_or(-1),
    );
    c.eq(
        "records.batches[0].last_offset_delta",
        0i32,
        batches.first().map(|b| b.last_offset_delta).unwrap_or(-1),
    );
    c.finish()
});

kafka_test!(high_watermark, |ctx| {
    let (_, hw, _) = fetch_records(ctx, 0).await?;
    let mut c = Check::detached("the high watermark after one record");
    c.eq(
        "response.responses[0].partitions[0].high_watermark",
        1i64,
        hw,
    );
    c.finish()
});

kafka_test!(crc_ok, |ctx| {
    let (records, _, _) = fetch_records(ctx, 0).await?;
    // RecordBatch::decode verifies the CRC-32C and refuses the batch when it is wrong.
    RecordBatch::decode_all(&records).map_err(|e| {
        Failure::harness(format!(
            "the returned batch failed its own integrity check: {e:#}"
        ))
    })?;
    Ok(())
});

kafka_test!(matches_segment, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let (records, _, _) = fetch_records(ctx, 0).await?;
    let segment = ctx.fixtures.segment_bytes(&t.name, 0)?;
    let mut c = Check::detached("the fetched bytes against the log segment on disk");
    c.note(format!(
        "segment: {}",
        ctx.fixtures.segment_path(&t.name, 0).display()
    ));
    c.note("Kafka sends the segment bytes untouched; re-encoding them changes the CRC");
    let prefix = segment.get(..records.len()).unwrap_or(&segment);
    c.bytes_eq("records", prefix, &records);
    c.finish()
});

/// Worked examples: one record on the wire, and the offset just past it.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("Fetch the only record in the log", |env| {
            env.request(
                FETCH_V16,
                231,
                &fetch_request(&[(env.id("t1")?, 0, 0)], 1_000),
            )
        })
        .with_fixtures(one_record)
        .request(
            "Fetch v16, correlation id 231: the fixture topic's id, partition 0, fetch_offset 0, \
             partition_max_bytes 1048576",
        )
        .response(
            "error_code 0, log_start_offset 0, high_watermark 1, and a records field holding one \
             v2 RecordBatch exactly as it sits in the segment: base_offset 0, \
             last_offset_delta 0, magic 2, attributes 0, a CRC-32C over everything that follows \
             it, and one record whose value is 'hello kafkatest'",
        )
        .note(
            "high_watermark is the offset *after* the last record, so one record means 1. The \
             batch is copied out of 00000000000000000000.log untouched — re-encoding it, even \
             field for field, is how the CRC stops matching and clients start reporting corrupt \
             messages.",
        ),
        ExampleSpec::wire("Fetch at the offset after that record", |env| {
            env.request(FETCH_V16, 232, &fetch_request(&[(env.id("t1")?, 0, 1)], 0))
        })
        .with_fixtures(one_record)
        .request("The same topic and partition, fetch_offset 1, max_wait_ms 0")
        .response("error_code 0 again, high_watermark still 1, and an empty records field")
        .note(
            "fetch_offset == high_watermark means 'caught up', not an error. This is the request \
             a consumer repeats once it has read everything; answering 1 (OFFSET_OUT_OF_RANGE) \
             here sends it back to the start of the log.",
        ),
    ]
}
