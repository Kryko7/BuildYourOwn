//! Stage 36 — the idempotent producer: InitProducerId (22), sequences and duplicates.

use crate::assert::{Check, Failure};
use crate::examples::{ExampleSpec, Wire};
use crate::fixtures::{FixtureSpec, TopicInfo, TopicSpec};
use crate::kafka_test;
use crate::proto::records::{RecordBatch, RecordItem};
use crate::proto::Conn;
use crate::stages::{
    api_versions, error_is_one_of, error_label, expect_still_serving, fetch_request,
    produce_request, proto_fail, require_api, Ctx, Stage, Test, DUPLICATE_SEQUENCE_NUMBER,
    FETCH_V16, INIT_PRODUCER_ID_KEY, INVALID_PRODUCER_EPOCH, NONE, OUT_OF_ORDER_SEQUENCE_NUMBER,
    PRODUCE_V11,
};
use kafka_protocol::messages::InitProducerIdRequest;
use kafka_protocol::protocol::Message;

/// The version of InitProducerId the plan pins; the suite negotiates down from there.
const INIT_PRODUCER_ID_V4: i16 = 4;

/// The producer id the worked examples stamp into their batches.
///
/// A real producer uses whatever InitProducerId handed it; the examples pick a fixed value
/// so the bytes below never change from capture to capture.
const EXAMPLE_PRODUCER_ID: i64 = 90_000;

fn empty_topic() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 1))
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 36,
        slug: "idempotent_producer",
        name: "Idempotent producer: InitProducerId (22) and sequences",
        ext: true,
        hints: &[
            "InitProducerId with a null transactional_id hands out a fresh producer id \
             (>= 0) and epoch (>= 0); the producer stamps both into every batch header",
            "Keep the last sequence number per (producer id, partition): the next batch \
             must start at last + 1, and its last sequence is baseSequence + \
             lastOffsetDelta",
            "A batch that repeats a sequence already appended is a retry — return the \
             offset it got the first time (real Kafka) or error 46 \
             DUPLICATE_SEQUENCE_NUMBER, and append nothing",
            "A gap is error 45 OUT_OF_ORDER_SEQUENCE_NUMBER; an older epoch is error 47 \
             INVALID_PRODUCER_EPOCH (45 is accepted too if you check the sequence first) \
             — either way the log is left untouched",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("ApiVersions advertises InitProducerId(22)", advertised_key).ext(),
            Test::new("InitProducerId hands out a producer id and an epoch", init).ext(),
            Test::new(
                "a batch stamped with the producer id and sequence 0 is accepted",
                first_batch,
            )
            .with_fixtures(empty_topic)
            .ext(),
            Test::new("re-sending the same batch is deduplicated", duplicate)
                .with_fixtures(empty_topic)
                .ext(),
            Test::new(
                "a gap in the sequence is error 45 OUT_OF_ORDER_SEQUENCE_NUMBER",
                sequence_gap,
            )
            .with_fixtures(empty_topic)
            .ext(),
            Test::new("an older producer epoch is rejected", stale_epoch)
                .with_fixtures(empty_topic)
                .ext(),
            Test::new(
                "the broker keeps serving after the rejected batches",
                still_serving,
            )
            .with_fixtures(empty_topic)
            .ext(),
        ],
    }
}

/// The InitProducerId version to speak: what the broker offers, capped by what the suite
/// can encode, and never below the v4 the plan pins.
async fn negotiate(conn: &mut Conn) -> Result<i16, Failure> {
    let versions = api_versions(conn).await?;
    let (min, max) = require_api(&versions, INIT_PRODUCER_ID_KEY, "InitProducerId", conn)?;
    let ours = InitProducerIdRequest::VERSIONS.max;
    let chosen = max.min(ours);
    if chosen < min {
        let mut c = Check::new("the InitProducerId versions the broker offers", conn);
        c.that(
            "response.api_keys[api_key=22]",
            &format!("a range overlapping v0-v{ours}"),
            false,
            format!("v{min}-v{max}"),
        );
        return Err(c
            .finish()
            .err()
            .unwrap_or_else(|| Failure::harness("no usable InitProducerId version")));
    }
    Ok(chosen)
}

/// Error codes a real producer retries InitProducerId on rather than giving up.
///
/// Right after a KRaft broker starts, the first request has to wait for the transaction
/// coordinator to load and for a producer id block to arrive from the controller, and
/// answers 14 COORDINATOR_LOAD_IN_PROGRESS or 15 COORDINATOR_NOT_AVAILABLE until it has.
const RETRIABLE: &[i16] = &[7, 14, 15, 51];

/// Ask for a producer id, checking the answer looks like one.
async fn init_producer_id(ctx: &Ctx) -> Result<(i64, i16), Failure> {
    let mut conn = ctx.connect().await?;
    let version = negotiate(&mut conn).await?;
    let mut req = InitProducerIdRequest::default();
    // A null transactional id is what a plain idempotent producer sends; the timeout is
    // only meaningful for transactions.
    req.transactional_id = None;
    req.transaction_timeout_ms = -1;
    let mut resp = conn
        .request(version, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let deadline = tokio::time::Instant::now() + ctx.timeout;
    while RETRIABLE.contains(&resp.error_code) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        resp = conn
            .request(version, &req)
            .await
            .map_err(|e| proto_fail(e, &conn))?;
    }
    let mut c = Check::new(format!("InitProducerId v{version}"), &conn);
    c.note(
        "a retriable answer (7, 14, 15, 51) was retried until the per-request timeout; \
         this is the code the broker settled on",
    );
    c.that(
        "response.error_code",
        &error_label(NONE),
        resp.error_code == NONE,
        error_label(resp.error_code),
    );
    c.at_least("response.producer_id", 0i64, resp.producer_id.0);
    c.at_least("response.producer_epoch", 0i16, resp.producer_epoch);
    c.finish()?;
    Ok((resp.producer_id.0, resp.producer_epoch))
}

/// A batch stamped as coming from an idempotent producer.
fn idempotent_batch(pid: i64, epoch: i16, base_sequence: i32, values: &[&str]) -> RecordBatch {
    let mut b = RecordBatch::of(
        0,
        1_700_000_000_000,
        values.iter().map(|v| RecordItem::value(*v)).collect(),
    );
    b.producer_id = pid;
    b.producer_epoch = epoch;
    b.base_sequence = base_sequence;
    b
}

/// Produce one batch and return `(error_code, base_offset)` without asserting on either.
async fn produce(ctx: &Ctx, name: &str, batch: &RecordBatch) -> Result<(i16, i64), Failure> {
    let mut conn = ctx.connect().await?;
    let req = produce_request(&[(name.to_string(), 0, batch.encode())], -1);
    let resp = conn
        .request(PRODUCE_V11, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new(
        format!(
            "producing a batch with producer_id {}, epoch {}, base_sequence {}",
            batch.producer_id, batch.producer_epoch, batch.base_sequence
        ),
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
    c.finish()?;
    Ok((p.error_code, p.base_offset))
}

/// The partition's high watermark right now, so a test can prove nothing was appended.
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

kafka_test!(advertised_key, |ctx| {
    let mut conn = ctx.connect().await?;
    let versions = api_versions(&mut conn).await?;
    let (min, max) = require_api(&versions, INIT_PRODUCER_ID_KEY, "InitProducerId", &conn)?;
    let mut c = Check::new("the advertised InitProducerId(22) range", &conn);
    c.at_most("response.api_keys[api_key=22].min_version", 0i16, min);
    c.at_least(
        "response.api_keys[api_key=22].max_version",
        INIT_PRODUCER_ID_V4,
        max,
    );
    c.finish()
});

kafka_test!(init, |ctx| {
    let (pid, epoch) = init_producer_id(ctx).await?;
    // Two calls without a transactional id must both work; they may or may not hand out
    // the same id, so only the shape is checked.
    let (pid2, epoch2) = init_producer_id(ctx).await?;
    let mut c = Check::detached("two InitProducerId calls without a transactional id");
    c.at_least("response[0].producer_id", 0i64, pid);
    c.at_least("response[0].producer_epoch", 0i16, epoch);
    c.at_least("response[1].producer_id", 0i64, pid2);
    c.at_least("response[1].producer_epoch", 0i16, epoch2);
    c.finish()
});

kafka_test!(first_batch, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let (pid, epoch) = init_producer_id(ctx).await?;
    let batch = idempotent_batch(pid, epoch, 0, &["a", "b"]);
    let (error, base) = produce(ctx, &t.name, &batch).await?;
    let mut c = Check::detached("the first batch of a new idempotent producer");
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
    c.finish()?;
    let hw = high_watermark(ctx, &t).await?;
    let mut c = Check::detached("the log after one idempotent batch");
    c.eq(
        "response.responses[0].partitions[0].high_watermark",
        2i64,
        hw,
    );
    c.finish()
});

kafka_test!(duplicate, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let (pid, epoch) = init_producer_id(ctx).await?;
    let batch = idempotent_batch(pid, epoch, 0, &["a", "b"]);
    let (error, base) = produce(ctx, &t.name, &batch).await?;
    let mut c = Check::detached("the first send of an idempotent batch");
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
    c.finish()?;

    // Exactly the same bytes again: what a producer sends when an ack was lost.
    let (again, again_base) = produce(ctx, &t.name, &batch).await?;
    let mut c = Check::detached("the same idempotent batch sent a second time");
    c.note(
        "Apache Kafka 4.1.2 recognises the retry and answers 0 NONE with the offset the \
         batch got the first time; a broker that answers 46 DUPLICATE_SEQUENCE_NUMBER \
         instead is accepted too, because either way the records must be stored once",
    );
    if error_is_one_of(
        &mut c,
        "response.responses[0].partition_responses[0].error_code",
        &[NONE, DUPLICATE_SEQUENCE_NUMBER],
        again,
    ) && again == NONE
    {
        c.eq(
            "response.responses[0].partition_responses[0].base_offset (the retry)",
            base,
            again_base,
        );
    }
    c.finish()?;

    let hw = high_watermark(ctx, &t).await?;
    let mut c = Check::detached("the log after the duplicate");
    c.note("the duplicate must not be appended a second time");
    c.eq(
        "response.responses[0].partitions[0].high_watermark",
        2i64,
        hw,
    );
    c.finish()
});

kafka_test!(sequence_gap, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let (pid, epoch) = init_producer_id(ctx).await?;
    let (error, _) = produce(ctx, &t.name, &idempotent_batch(pid, epoch, 0, &["a", "b"])).await?;
    let mut c = Check::detached("the first idempotent batch (sequences 0-1)");
    c.that(
        "response.responses[0].partition_responses[0].error_code",
        &error_label(NONE),
        error == NONE,
        error_label(error),
    );
    c.finish()?;

    // The next batch should start at sequence 2; 7 leaves a hole.
    let (gap, _) = produce(ctx, &t.name, &idempotent_batch(pid, epoch, 7, &["c"])).await?;
    let mut c = Check::detached("a batch whose base_sequence skips ahead");
    c.note("sequences 0-1 are stored, so the next batch must start at 2, not 7");
    error_is_one_of(
        &mut c,
        "response.responses[0].partition_responses[0].error_code",
        &[OUT_OF_ORDER_SEQUENCE_NUMBER],
        gap,
    );
    c.finish()?;

    let hw = high_watermark(ctx, &t).await?;
    let mut c = Check::detached("the log after the rejected batch");
    c.eq(
        "response.responses[0].partitions[0].high_watermark",
        2i64,
        hw,
    );
    c.finish()
});

kafka_test!(stale_epoch, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let (pid, epoch) = init_producer_id(ctx).await?;
    let (error, _) = produce(ctx, &t.name, &idempotent_batch(pid, epoch, 0, &["a"])).await?;
    let mut c = Check::detached("the first idempotent batch");
    c.that(
        "response.responses[0].partition_responses[0].error_code",
        &error_label(NONE),
        error == NONE,
        error_label(error),
    );
    c.finish()?;

    // One epoch behind: a zombie producer that missed a fencing.
    let stale = idempotent_batch(pid, epoch - 1, 1, &["b"]);
    let (rejected, _) = produce(ctx, &t.name, &stale).await?;
    let mut c = Check::detached(format!(
        "a batch stamped with producer epoch {} while the broker holds {epoch}",
        epoch - 1
    ));
    c.note(
        "Apache Kafka 4.1.2 fences the older epoch with 47 INVALID_PRODUCER_EPOCH; a \
         broker that checks the sequence before the epoch answers 45 \
         OUT_OF_ORDER_SEQUENCE_NUMBER, which is accepted too because the batch is \
         rejected either way",
    );
    error_is_one_of(
        &mut c,
        "response.responses[0].partition_responses[0].error_code",
        &[INVALID_PRODUCER_EPOCH, OUT_OF_ORDER_SEQUENCE_NUMBER],
        rejected,
    );
    c.finish()?;

    let hw = high_watermark(ctx, &t).await?;
    let mut c = Check::detached("the log after the fenced batch");
    c.eq(
        "response.responses[0].partitions[0].high_watermark",
        1i64,
        hw,
    );
    c.finish()
});

kafka_test!(still_serving, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let (pid, epoch) = init_producer_id(ctx).await?;
    produce(ctx, &t.name, &idempotent_batch(pid, epoch, 0, &["a"])).await?;
    produce(ctx, &t.name, &idempotent_batch(pid, epoch, 99, &["gap"])).await?;
    produce(
        ctx,
        &t.name,
        &idempotent_batch(pid, epoch - 1, 1, &["stale"]),
    )
    .await?;
    expect_still_serving(ctx, "three rejected idempotent batches").await?;

    // And a correct batch is still accepted afterwards.
    let (error, base) = produce(ctx, &t.name, &idempotent_batch(pid, epoch, 1, &["b"])).await?;
    let mut c = Check::detached("a valid batch after the rejected ones");
    c.that(
        "response.responses[0].partition_responses[0].error_code",
        &error_label(NONE),
        error == NONE,
        error_label(error),
    );
    c.eq(
        "response.responses[0].partition_responses[0].base_offset",
        1i64,
        base,
    );
    c.finish()
});

/// Worked examples: asking for a producer id, and what the batch header then carries.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("InitProducerId with a null transactional id", |env| {
            let mut req = InitProducerIdRequest::default();
            req.transactional_id = None;
            req.transaction_timeout_ms = -1;
            req.producer_id = kafka_protocol::messages::ProducerId(-1);
            req.producer_epoch = -1;
            env.request(INIT_PRODUCER_ID_V4, 361, &req)
        })
        .request(
            "InitProducerId (api_key 22) v4, correlation id 361: transactional_id null, \
             transaction_timeout_ms -1, producer_id -1 and producer_epoch -1 — what a plain \
             idempotent producer sends when it has no id yet",
        )
        .response(
            "throttle_time_ms 0, error_code 0 (NONE), a producer_id the broker allocated \
             (a fresh int64, different on every boot) and producer_epoch 0",
        )
        .note(
            "Right after a KRaft broker starts this can answer 14 \
             (COORDINATOR_LOAD_IN_PROGRESS) or 15 (COORDINATOR_NOT_AVAILABLE) until the \
             coordinator has loaded; a real producer retries. Hand out ids from a block you \
             own, never a counter that restarts at 0 with the process, or two producers end \
             up sharing an id.",
        ),
        ExampleSpec::wire("A stamped batch, and then a gap in the sequence", |env| {
            let name = env.name("t1")?;
            let first = idempotent_batch(EXAMPLE_PRODUCER_ID, 0, 0, &["a", "b"]);
            let gap = idempotent_batch(EXAMPLE_PRODUCER_ID, 0, 7, &["c"]);
            Ok(Wire::Frames(vec![
                env.payload(
                    PRODUCE_V11,
                    362,
                    &produce_request(&[(name.clone(), 0, first.encode())], -1),
                )?,
                env.payload(
                    PRODUCE_V11,
                    363,
                    &produce_request(&[(name, 0, gap.encode())], -1),
                )?,
            ]))
        })
        .with_fixtures(empty_topic)
        .request(
            "Two Produce v11 frames to the same empty partition. Both batches carry \
             producerId 90000 and producerEpoch 0 in the v2 batch header (the int64 and \
             int16 that follow maxTimestamp, with baseSequence as the int32 after them). The \
             first, correlation id 362, has baseSequence 0 and two records; the second, \
             correlation id 363, has baseSequence 7 and one record.",
        )
        .response(
            "Frame 362 answers error_code 0 (NONE) with base_offset 0 — sequences 0 and 1 \
             are now stored for that producer. Frame 363 answers error_code 45 \
             (OUT_OF_ORDER_SEQUENCE_NUMBER), because the next batch had to start at \
             sequence 2, and nothing is appended: the high watermark stays 2.",
        )
        .note(
            "Keep the last sequence per (producerId, partition); the next batch must start \
             at last + 1 and its last sequence is baseSequence + lastOffsetDelta. A batch \
             repeating a sequence already stored is a retry after a lost ack — answer the \
             offset it got the first time and append nothing; an older producerEpoch is 47 \
             (INVALID_PRODUCER_EPOCH).",
        ),
    ]
}
