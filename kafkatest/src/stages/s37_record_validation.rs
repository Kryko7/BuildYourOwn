//! Stage 37 — record validation: bad CRCs, bad counts, bad magic and oversized batches.

use crate::assert::{Check, Failure};
use crate::broker::reference::MESSAGE_MAX_BYTES;
use crate::examples::ExampleSpec;
use crate::fixtures::{FixtureSpec, TopicInfo, TopicSpec};
use crate::kafka_test;
use crate::proto::records::{RecordBatch, RecordItem};
use crate::stages::{
    error_is_one_of, error_label, expect_still_serving, fetch_request, produce_request, proto_fail,
    Ctx, Stage, Test, CORRUPT_MESSAGE, FETCH_V16, INVALID_RECORD, MESSAGE_TOO_LARGE, NONE,
    PRODUCE_V11,
};

/// Offset of the magic byte inside an encoded batch: baseOffset(8) + batchLength(4) +
/// partitionLeaderEpoch(4).
const MAGIC_AT: usize = 16;

fn empty_topic() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 1))
}

fn good_batch(values: &[&str]) -> RecordBatch {
    RecordBatch::of(
        0,
        1_700_000_000_000,
        values.iter().map(|v| RecordItem::value(*v)).collect(),
    )
}

/// A one-record batch whose encoded size is about `size` bytes.
fn batch_of_size(size: usize) -> RecordBatch {
    // The v2 header is 61 bytes; the record adds ~15 on top of its value.
    let payload = size.saturating_sub(80);
    RecordBatch::of(
        0,
        1_700_000_000_000,
        vec![RecordItem::value(vec![b'p'; payload])],
    )
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 37,
        slug: "record_validation",
        name: "Record validation: CORRUPT_MESSAGE (2) and MESSAGE_TOO_LARGE (10)",
        ext: true,
        hints: &[
            "Recompute the CRC-32C over everything after the crc field and compare: a \
             mismatch is error 2 CORRUPT_MESSAGE and nothing is appended",
            "A recordCount that does not match the records, or a magic other than 2, is \
             rejected too: real Kafka answers 87 INVALID_RECORD there, and 2 \
             CORRUPT_MESSAGE is accepted as well",
            "A batch bigger than message.max.bytes from the properties file is error 10 \
             MESSAGE_TOO_LARGE, per partition",
            "Every one of these is a per-partition error_code in a normal response: never \
             close the connection, never stop the accept loop",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new(
                "a batch with a wrong CRC is error 2 CORRUPT_MESSAGE",
                bad_crc,
            )
            .with_fixtures(empty_topic)
            .ext(),
            Test::new("a wrong record count in the header is rejected", bad_count)
                .with_fixtures(empty_topic)
                .ext(),
            Test::new("a batch whose magic is not 2 is rejected", bad_magic)
                .with_fixtures(empty_topic)
                .ext(),
            Test::new(
                "a batch over message.max.bytes is error 10 MESSAGE_TOO_LARGE",
                too_large,
            )
            .with_fixtures(empty_topic)
            .ext(),
            Test::new(
                "a batch just under message.max.bytes is accepted",
                just_fits,
            )
            .with_fixtures(empty_topic)
            .ext(),
            Test::new(
                "a good batch on the same connection still works after a bad one",
                good_after_bad,
            )
            .with_fixtures(empty_topic)
            .ext(),
            Test::new(
                "the broker survives every malformed batch",
                survives_them_all,
            )
            .with_fixtures(empty_topic)
            .ext(),
        ],
    }
}

/// Produce raw batch bytes and return `(error_code, base_offset)`.
async fn produce_bytes(ctx: &Ctx, name: &str, bytes: Vec<u8>) -> Result<(i16, i64), Failure> {
    let mut conn = ctx.connect().await?;
    let len = bytes.len();
    let req = produce_request(&[(name.to_string(), 0, bytes)], -1);
    let resp = conn
        .request(PRODUCE_V11, &req)
        .await
        .map_err(|e| proto_fail(e, &conn).note(format!("while producing a {len}-byte batch")))?;
    let mut c = Check::new(format!("producing a {len}-byte batch to '{name}'"), &conn);
    let Some(p) = resp
        .responses
        .first()
        .and_then(|t| t.partition_responses.first())
    else {
        c.that(
            "response.responses[0].partition_responses[0]",
            "one entry for the partition, carrying the error code",
            false,
            "no entry at all",
        );
        return Err(c
            .finish()
            .err()
            .unwrap_or_else(|| Failure::harness("no partition entry")));
    };
    c.finish()?;
    Ok((p.error_code, p.base_offset))
}

/// The partition's high watermark, to prove a rejected batch was not appended.
async fn high_watermark(ctx: &Ctx, topic: &TopicInfo) -> Result<i64, Failure> {
    let mut conn = ctx.connect().await?;
    let req = fetch_request(&[(topic.id, 0, 0)], 500);
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    resp.responses
        .first()
        .and_then(|t| t.partitions.first())
        .map(|p| p.high_watermark)
        .ok_or_else(|| {
            Failure::harness(format!(
                "the Fetch for '{}'-0 returned no partition entry",
                topic.name
            ))
        })
}

kafka_test!(bad_crc, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut batch = good_batch(&["this will not survive"]);
    batch.crc_override = Some(0xdead_beef);
    let (error, _) = produce_bytes(ctx, &t.name, batch.encode()).await?;
    let mut c = Check::detached("a batch whose crc field does not match its bytes");
    c.that(
        "response.responses[0].partition_responses[0].error_code",
        &error_label(CORRUPT_MESSAGE),
        error == CORRUPT_MESSAGE,
        error_label(error),
    );
    c.finish()?;
    let hw = high_watermark(ctx, &t).await?;
    let mut c = Check::detached("the log after a corrupt batch");
    c.eq(
        "response.responses[0].partitions[0].high_watermark",
        0i64,
        hw,
    );
    c.finish()
});

kafka_test!(bad_count, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut batch = good_batch(&["a", "b"]);
    // The CRC is computed over the header too, so this batch is "valid" until something
    // tries to read the records the count promises.
    batch.record_count_override = Some(5);
    let (error, _) = produce_bytes(ctx, &t.name, batch.encode()).await?;
    let mut c = Check::detached("a batch whose recordCount says 5 but which holds 2 records");
    c.note(
        "the CRC still matches, so this is only noticed while reading the records: Apache \
         Kafka 4.1.2 answers 87 INVALID_RECORD, and a broker that reports the batch as \
         corrupt instead (2 CORRUPT_MESSAGE) is accepted too",
    );
    error_is_one_of(
        &mut c,
        "response.responses[0].partition_responses[0].error_code",
        &[CORRUPT_MESSAGE, INVALID_RECORD],
        error,
    );
    c.finish()?;
    let hw = high_watermark(ctx, &t).await?;
    let mut c = Check::detached("the log after a batch with a wrong record count");
    c.eq(
        "response.responses[0].partitions[0].high_watermark",
        0i64,
        hw,
    );
    c.finish()
});

kafka_test!(bad_magic, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut bytes = good_batch(&["wrong format"]).encode();
    if let Some(b) = bytes.get_mut(MAGIC_AT) {
        *b = 1;
    }
    let (error, _) = produce_bytes(ctx, &t.name, bytes).await?;
    let mut c = Check::detached("a batch claiming magic 1");
    c.note(
        "Produce v3 and later carry v2 batches only: Apache Kafka 4.1.2 answers 87 \
         INVALID_RECORD, and a broker that runs its CRC check first and reports 2 \
         CORRUPT_MESSAGE is accepted too",
    );
    error_is_one_of(
        &mut c,
        "response.responses[0].partition_responses[0].error_code",
        &[CORRUPT_MESSAGE, INVALID_RECORD],
        error,
    );
    c.finish()?;
    let hw = high_watermark(ctx, &t).await?;
    let mut c = Check::detached("the log after a batch with the wrong magic");
    c.eq(
        "response.responses[0].partitions[0].high_watermark",
        0i64,
        hw,
    );
    c.finish()
});

kafka_test!(too_large, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let bytes = batch_of_size(MESSAGE_MAX_BYTES * 2).encode();
    let mut c = Check::detached("the oversized batch this test builds");
    c.at_least("batch.size_in_bytes", MESSAGE_MAX_BYTES + 1, bytes.len());
    c.finish()?;
    let (error, _) = produce_bytes(ctx, &t.name, bytes).await?;
    let mut c = Check::detached(format!(
        "a batch larger than the message.max.bytes={MESSAGE_MAX_BYTES} in server.properties"
    ));
    c.that(
        "response.responses[0].partition_responses[0].error_code",
        &error_label(MESSAGE_TOO_LARGE),
        error == MESSAGE_TOO_LARGE,
        error_label(error),
    );
    c.finish()?;
    let hw = high_watermark(ctx, &t).await?;
    let mut c = Check::detached("the log after an oversized batch");
    c.eq(
        "response.responses[0].partitions[0].high_watermark",
        0i64,
        hw,
    );
    c.finish()
});

kafka_test!(just_fits, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let bytes = batch_of_size(MESSAGE_MAX_BYTES - 6_000).encode();
    let mut c = Check::detached("the batch this test builds");
    c.at_most("batch.size_in_bytes", MESSAGE_MAX_BYTES, bytes.len());
    c.finish()?;
    let (error, base) = produce_bytes(ctx, &t.name, bytes).await?;
    let mut c = Check::detached(format!(
        "a batch just under the message.max.bytes={MESSAGE_MAX_BYTES} limit"
    ));
    c.that(
        "response.responses[0].partition_responses[0].error_code",
        &error_label(NONE),
        error == NONE,
        error_label(error),
    );
    c.eq(
        "response.responses[0].partition_responses[0].base_offset",
        0i64,
        base,
    );
    c.finish()
});

kafka_test!(good_after_bad, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut corrupt = good_batch(&["corrupt"]);
    corrupt.crc_override = Some(1);
    let mut conn = ctx.connect().await?;

    let bad = produce_request(&[(t.name.clone(), 0, corrupt.encode())], -1);
    let bad_resp = conn
        .request(PRODUCE_V11, &bad)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let bad_code = bad_resp
        .responses
        .first()
        .and_then(|t| t.partition_responses.first())
        .map(|p| p.error_code)
        .unwrap_or(i16::MIN);

    // Same connection, next request: the broker must not have lost its place in the stream.
    let good = produce_request(&[(t.name.clone(), 0, good_batch(&["fine"]).encode())], -1);
    let good_resp = conn.request(PRODUCE_V11, &good).await.map_err(|e| {
        proto_fail(e, &conn).note("the previous request on this connection carried a corrupt batch")
    })?;
    let mut c = Check::new("a valid Produce right after a corrupt one", &conn);
    c.that(
        "response[0].responses[0].partition_responses[0].error_code",
        &error_label(CORRUPT_MESSAGE),
        bad_code == CORRUPT_MESSAGE,
        error_label(bad_code),
    );
    if let Some(p) = good_resp
        .responses
        .first()
        .and_then(|t| t.partition_responses.first())
    {
        c.that(
            "response[1].responses[0].partition_responses[0].error_code",
            &error_label(NONE),
            p.error_code == NONE,
            error_label(p.error_code),
        );
        c.eq(
            "response[1].responses[0].partition_responses[0].base_offset",
            0i64,
            p.base_offset,
        );
    }
    c.finish()
});

kafka_test!(survives_them_all, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut corrupt = good_batch(&["x"]);
    corrupt.crc_override = Some(0);
    let mut counted = good_batch(&["x"]);
    counted.record_count_override = Some(-1);
    let mut magic = good_batch(&["x"]).encode();
    if let Some(b) = magic.get_mut(MAGIC_AT) {
        *b = 0;
    }
    let mut truncated = good_batch(&["x", "y"]).encode();
    truncated.truncate(truncated.len() / 2);

    for (what, bytes) in [
        ("a wrong CRC", corrupt.encode()),
        ("a negative record count", counted.encode()),
        ("magic 0", magic),
        ("a truncated batch", truncated),
        ("an empty records field", Vec::new()),
    ] {
        let (error, _) = produce_bytes(ctx, &t.name, bytes).await?;
        let mut c = Check::detached(format!("a Produce carrying {what}"));
        // Any error code is fine here; a crash, a hang or a closed accept loop is not.
        c.observe(
            "response.responses[0].partition_responses[0].error_code",
            error_label(error),
        );
        c.finish()?;
        expect_still_serving(ctx, &format!("a Produce carrying {what}")).await?;
    }

    let hw = high_watermark(ctx, &t).await?;
    let mut c = Check::detached("the log after five malformed batches");
    c.eq(
        "response.responses[0].partitions[0].high_watermark",
        0i64,
        hw,
    );
    c.finish()
});

/// Worked examples: a batch the CRC catches, and one only the records catch.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("A batch whose CRC does not match its bytes", |env| {
            let mut bad = good_batch(&["this will not survive"]);
            bad.crc_override = Some(0xdead_beef);
            env.request(
                PRODUCE_V11,
                371,
                &produce_request(&[(env.name("t1")?, 0, bad.encode())], -1),
            )
        })
        .with_fixtures(empty_topic)
        .request(
            "Produce v11, correlation id 371, acks -1: a well-formed one-record v2 batch \
             whose crc field has been overwritten with deadbeef instead of the CRC-32C of \
             the bytes that follow it",
        )
        .response(
            "index 0, error_code 2 (CORRUPT_MESSAGE), base_offset -1. Nothing is appended — \
             the partition's high watermark stays 0 — and the connection stays open.",
        )
        .note(
            "Recompute the CRC-32C (Castagnoli, not the CRC-32 of zip) over everything after \
             the crc field — from attributes to the last record — and compare before you \
             append. This is a per-partition error code in an otherwise normal response: \
             never close the connection over a bad batch.",
        ),
        ExampleSpec::wire("A recordCount that lies", |env| {
            let mut bad = good_batch(&["a", "b"]);
            bad.record_count_override = Some(5);
            env.request(
                PRODUCE_V11,
                372,
                &produce_request(&[(env.name("t1")?, 0, bad.encode())], -1),
            )
        })
        .with_fixtures(empty_topic)
        .request(
            "Produce v11, correlation id 372, acks -1: a batch holding two records whose \
             header claims recordCount 5. The CRC covers the header too, so it is computed \
             over the wrong count and still matches.",
        )
        .response(
            "index 0, error_code 87 (INVALID_RECORD) from Apache Kafka 4.1.2 — a broker that \
             reports it as 2 (CORRUPT_MESSAGE) instead is accepted, because either way the \
             batch is rejected and nothing is appended",
        )
        .note(
            "A valid CRC is not a valid batch: this one only falls over when something tries \
             to read the five records it promises, so the record loop has to be bounds \
             checked as well. The same response shape carries the other rejections — magic \
             other than 2, and a batch over message.max.bytes, which is error 10 \
             (MESSAGE_TOO_LARGE).",
        ),
    ]
}
