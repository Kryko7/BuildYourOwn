//! Stage 25 — Fetch from an offset in the middle, and OFFSET_OUT_OF_RANGE.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::fixtures::{rec, FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::proto::records::RecordBatch;
use crate::stages::{fetch_request, proto_fail, Ctx, Stage, Test, FETCH_V16, NONE};

/// `OFFSET_OUT_OF_RANGE`.
const OFFSET_OUT_OF_RANGE: i16 = 1;

/// Three batches: offsets 0-2, 3-4 and 5. The high watermark is 6.
fn three_batches() -> FixtureSpec {
    FixtureSpec::with(
        TopicSpec::new("t1", 1)
            .with_batch(0, vec![rec("a0"), rec("a1"), rec("a2")])
            .with_batch(0, vec![rec("a3"), rec("a4")])
            .with_batch(0, vec![rec("a5")]),
    )
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 25,
        slug: "fetch_from_offset",
        name: "Fetch from the middle, and OFFSET_OUT_OF_RANGE (1)",
        ext: true,
        hints: &[
            "A fetch offset inside the log starts at the first batch whose last offset is \
             >= it, so the response may begin before the offset that was asked for",
            "Never split a batch: the client drops the records below its fetch offset",
            "fetch_offset == high_watermark is not an error — answer with error 0, no \
             records and the same high watermark",
            "fetch_offset above the high watermark or below log_start_offset (a negative \
             offset included) is error 1 OFFSET_OUT_OF_RANGE",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("a fetch from a batch boundary starts there", from_boundary)
                .with_fixtures(three_batches)
                .ext(),
            Test::new(
                "an offset inside a batch returns that whole batch",
                inside_batch,
            )
            .with_fixtures(three_batches)
            .ext(),
            Test::new(
                "a fetch of the last offset returns only the last batch",
                last_batch,
            )
            .with_fixtures(three_batches)
            .ext(),
            Test::new(
                "fetching at the high watermark is empty, not an error",
                at_watermark,
            )
            .with_fixtures(three_batches)
            .ext(),
            Test::new(
                "an offset past the high watermark is error 1",
                past_watermark,
            )
            .with_fixtures(three_batches)
            .ext(),
            Test::new("a negative fetch offset is error 1", negative_offset)
                .with_fixtures(three_batches)
                .ext(),
            Test::new(
                "log_start_offset is reported alongside the records",
                log_start,
            )
            .with_fixtures(three_batches)
            .ext(),
        ],
    }
}

/// What one partition of a fetch response said.
struct Fetched {
    error_code: i16,
    high_watermark: i64,
    log_start_offset: i64,
    records: Vec<u8>,
}

impl Fetched {
    /// The batches of the partition, decoded.
    fn batches(&self) -> Result<Vec<RecordBatch>, Failure> {
        RecordBatch::decode_all(&self.records)
            .map_err(|e| Failure::harness(format!("the returned records do not decode: {e:#}")))
    }

    /// The base offset of every returned batch.
    fn base_offsets(&self) -> Result<Vec<i64>, Failure> {
        Ok(self.batches()?.iter().map(|b| b.base_offset).collect())
    }

    /// Every record offset in the response, in order.
    fn record_offsets(&self) -> Result<Vec<i64>, Failure> {
        Ok(self
            .batches()?
            .iter()
            .flat_map(|b| {
                b.records
                    .iter()
                    .map(|r| b.base_offset + i64::from(r.offset_delta))
                    .collect::<Vec<i64>>()
            })
            .collect())
    }
}

/// Fetch partition 0 of `t1` from `offset`, without asserting anything about the result.
async fn fetch_at(ctx: &Ctx, offset: i64) -> Result<Fetched, Failure> {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = fetch_request(&[(t.id, 0, offset)], 500);
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
            .unwrap_or_else(|| Failure::harness("no partition entry")));
    };
    c.finish()?;
    Ok(Fetched {
        error_code: p.error_code,
        high_watermark: p.high_watermark,
        log_start_offset: p.log_start_offset,
        records: p.records.as_ref().map(|r| r.to_vec()).unwrap_or_default(),
    })
}

kafka_test!(from_boundary, |ctx| {
    let got = fetch_at(ctx, 3).await?;
    let mut c = Check::detached("a fetch from offset 3, which begins a batch");
    c.eq(
        "response.responses[0].partitions[0].error_code",
        NONE,
        got.error_code,
    );
    c.note("the fixture holds three batches: offsets 0-2, 3-4 and 5");
    c.eq(
        "records.batches[*].base_offset",
        vec![3i64, 5],
        got.base_offsets()?,
    );
    c.finish()
});

kafka_test!(inside_batch, |ctx| {
    let got = fetch_at(ctx, 4).await?;
    let mut c = Check::detached("a fetch from offset 4, which sits inside the batch at 3");
    c.note("Kafka never splits a batch, so offset 3 comes back too and the client drops it");
    c.eq(
        "records.batches[0].base_offset",
        3i64,
        got.base_offsets()?.first().copied().unwrap_or(-1),
    );
    c.eq("records[*].offset", vec![3i64, 4, 5], got.record_offsets()?);
    c.finish()
});

kafka_test!(last_batch, |ctx| {
    let got = fetch_at(ctx, 5).await?;
    let mut c = Check::detached("a fetch of the last offset in the log");
    c.eq(
        "records.batches[*].base_offset",
        vec![5i64],
        got.base_offsets()?,
    );
    c.eq("records[*].offset", vec![5i64], got.record_offsets()?);
    c.finish()
});

kafka_test!(at_watermark, |ctx| {
    let got = fetch_at(ctx, 6).await?;
    let mut c = Check::detached("a fetch at the high watermark, one past the last record");
    c.eq(
        "response.responses[0].partitions[0].error_code",
        NONE,
        got.error_code,
    );
    c.eq(
        "response.responses[0].partitions[0].records.len",
        0usize,
        got.records.len(),
    );
    c.eq(
        "response.responses[0].partitions[0].high_watermark",
        6i64,
        got.high_watermark,
    );
    c.finish()
});

kafka_test!(past_watermark, |ctx| {
    let got = fetch_at(ctx, 12).await?;
    let mut c = Check::detached("a fetch from offset 12, well past the high watermark of 6");
    c.eq(
        "response.responses[0].partitions[0].error_code",
        OFFSET_OUT_OF_RANGE,
        got.error_code,
    );
    c.eq(
        "response.responses[0].partitions[0].records.len",
        0usize,
        got.records.len(),
    );
    c.finish()
});

kafka_test!(negative_offset, |ctx| {
    let got = fetch_at(ctx, -1).await?;
    let mut c = Check::detached("a fetch from offset -1, below log_start_offset");
    c.note("-1 is below log_start_offset, which is the same kind of error as too high");
    c.eq(
        "response.responses[0].partitions[0].error_code",
        OFFSET_OUT_OF_RANGE,
        got.error_code,
    );
    c.finish()
});

kafka_test!(log_start, |ctx| {
    let got = fetch_at(ctx, 3).await?;
    let mut c = Check::detached("the offsets reported with a mid-log fetch");
    c.eq(
        "response.responses[0].partitions[0].log_start_offset",
        0i64,
        got.log_start_offset,
    );
    c.eq(
        "response.responses[0].partitions[0].high_watermark",
        6i64,
        got.high_watermark,
    );
    c.finish()
});

/// Worked examples: an offset inside a batch, and an offset past the end of the log.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("Fetch from an offset inside a batch", |env| {
            env.request(
                FETCH_V16,
                251,
                &fetch_request(&[(env.id("t1")?, 0, 4)], 500),
            )
        })
        .with_fixtures(three_batches)
        .request(
            "Fetch v16, correlation id 251, fetch_offset 4. The partition holds three batches: \
             offsets 0-2, 3-4 and 5",
        )
        .response(
            "error_code 0, log_start_offset 0, high_watermark 6, and records that begin at \
             base_offset 3 — the whole batch containing offset 4 — followed by the batch at 5",
        )
        .note(
            "Never split a batch. Find the first batch whose last offset is >= fetch_offset and \
             start sending there; record 3 comes back although nobody asked for it, and the \
             client discards it. Cutting the batch open would break its CRC and its record \
             count.",
        ),
        ExampleSpec::wire("Fetch past the end of the log", |env| {
            env.request(
                FETCH_V16,
                252,
                &fetch_request(&[(env.id("t1")?, 0, 12)], 500),
            )
        })
        .with_fixtures(three_batches)
        .request("The same partition, fetch_offset 12, while the high watermark is 6")
        .response(
            "Top-level error_code 0 still, and on the partition error_code 1 \
             (OFFSET_OUT_OF_RANGE) with no records",
        )
        .note(
            "Above the high watermark, or below log_start_offset (a negative offset included), \
             is error 1. Exactly *at* the high watermark is not: that is the caught-up consumer, \
             and it must get error 0 with an empty records field.",
        ),
    ]
}
