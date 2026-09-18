//! Stage 28 — Long poll: `max_wait_ms` and `min_bytes`.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::fixtures::{rec, FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::proto::records::{RecordBatch, RecordItem};
use crate::stages::{
    api_versions, produce_request, proto_fail, Ctx, Stage, Test, FETCH_V16, NONE, PRODUCE_V11,
};
use kafka_protocol::messages::fetch_request::{FetchPartition, FetchTopic};
use kafka_protocol::messages::{BrokerId, FetchRequest};
use std::time::{Duration, Instant};
use uuid::Uuid;

/// How long the polls in this stage ask the broker to hold the request.
const WAIT_MS: i32 = 1_500;
/// A response must not arrive more than this early — otherwise nothing was held.
const EARLY_SLACK_MS: u128 = 100;
/// A loaded machine may add this much on top of `max_wait_ms`.
const LATE_SLACK_MS: u128 = 1_500;
/// An "immediately" answer has this long to come back, even on a busy CI box.
const IMMEDIATE_MS: u128 = 800;

/// An empty partition to wait on, and a partition that already holds three records.
fn empty_and_full() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 1))
        .and(TopicSpec::new("t2", 1).with_batch(0, vec![rec("r0"), rec("r1"), rec("r2")]))
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 28,
        slug: "long_poll",
        name: "Long poll: max_wait_ms and min_bytes",
        ext: true,
        hints: &[
            "Park a fetch that cannot be satisfied instead of answering it: hold it until \
             min_bytes of records exist or max_wait_ms has passed, then answer with what \
             there is (possibly nothing) and error 0",
            "An append must complete the parked fetch straight away — poll or notify, but \
             do not make the consumer wait out the full max_wait_ms for data already there",
            "max_wait_ms 0, or min_bytes 0, means answer now; a parked fetch must never \
             block the other connections or the accept loop",
            "The tests allow the answer to be up to 100 ms early and 1500 ms late, so a \
             sleep-based implementation is fine, but busy-waiting a whole core is not",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("an empty partition holds the fetch for max_wait_ms", holds)
                .with_fixtures(empty_and_full)
                .ext(),
            Test::new("max_wait_ms 0 answers immediately", no_wait)
                .with_fixtures(empty_and_full)
                .ext(),
            Test::new(
                "min_bytes 0 answers immediately with nothing",
                min_bytes_zero,
            )
            .with_fixtures(empty_and_full)
            .ext(),
            Test::new("data already in the log answers immediately", data_now)
                .with_fixtures(empty_and_full)
                .ext(),
            Test::new(
                "a produce during the wait ends the poll early",
                wakes_on_produce,
            )
            .with_fixtures(empty_and_full)
            .ext(),
            Test::new(
                "min_bytes larger than the log waits for the timeout",
                min_bytes_waits,
            )
            .with_fixtures(empty_and_full)
            .ext(),
            Test::new(
                "a parked fetch does not block another connection",
                still_serving,
            )
            .with_fixtures(empty_and_full)
            .ext(),
        ],
    }
}

/// A `Fetch` v16 request with explicit long-poll parameters.
fn poll_request(
    topic: Uuid,
    partition: i32,
    offset: i64,
    max_wait_ms: i32,
    min_bytes: i32,
) -> FetchRequest {
    let mut p = FetchPartition::default();
    p.partition = partition;
    p.fetch_offset = offset;
    p.current_leader_epoch = -1;
    p.last_fetched_epoch = -1;
    p.log_start_offset = -1;
    p.partition_max_bytes = 1024 * 1024;
    let mut t = FetchTopic::default();
    t.topic_id = topic;
    t.partitions = vec![p];
    let mut r = FetchRequest::default();
    r.max_wait_ms = max_wait_ms;
    r.min_bytes = min_bytes;
    r.max_bytes = 10 * 1024 * 1024;
    r.session_id = 0;
    r.session_epoch = 0;
    r.replica_id = BrokerId(-1);
    r.topics = vec![t];
    r
}

/// What a poll produced: how long it took, the partition error and the records.
struct Poll {
    elapsed: Duration,
    error_code: i16,
    records: Vec<u8>,
}

/// Run one long poll and time it.
async fn poll(
    ctx: &Ctx,
    key: &str,
    offset: i64,
    max_wait_ms: i32,
    min_bytes: i32,
) -> Result<Poll, Failure> {
    let t = ctx.topic(key)?.clone();
    let mut conn = ctx.connect().await?;
    let req = poll_request(t.id, 0, offset, max_wait_ms, min_bytes);
    let started = Instant::now();
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let elapsed = started.elapsed();
    let mut c = Check::new(
        format!(
            "a fetch of '{}' with max_wait_ms {max_wait_ms}, min_bytes {min_bytes}",
            t.name
        ),
        &conn,
    );
    c.eq("response.error_code", NONE, resp.error_code);
    c.finish()?;
    let p = resp.responses.first().and_then(|r| r.partitions.first());
    Ok(Poll {
        elapsed,
        error_code: p.map(|p| p.error_code).unwrap_or(-1),
        records: p
            .and_then(|p| p.records.clone())
            .map(|b| b.to_vec())
            .unwrap_or_default(),
    })
}

/// Note the bounds a timing check used, so a failure explains itself.
fn timing_note(c: &mut Check, low: u128, high: u128, elapsed: Duration) {
    c.note(format!(
        "expected the response between {low} ms and {high} ms, it took {} ms",
        elapsed.as_millis()
    ));
}

kafka_test!(holds, |ctx| {
    let got = poll(ctx, "t1", 0, WAIT_MS, 1).await?;
    let low = WAIT_MS as u128 - EARLY_SLACK_MS;
    let high = WAIT_MS as u128 + LATE_SLACK_MS;
    let ms = got.elapsed.as_millis();
    let mut c = Check::detached("a fetch of an empty partition with max_wait_ms 1500");
    timing_note(&mut c, low, high, got.elapsed);
    c.that(
        "response.elapsed_ms",
        &format!("at least {low} (the request must be held)"),
        ms >= low,
        ms,
    );
    c.that(
        "response.elapsed_ms",
        &format!("at most {high} (max_wait_ms plus slack)"),
        ms <= high,
        ms,
    );
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
    c.finish()
});

kafka_test!(no_wait, |ctx| {
    let got = poll(ctx, "t1", 0, 0, 1).await?;
    let ms = got.elapsed.as_millis();
    let mut c = Check::detached("a fetch of an empty partition with max_wait_ms 0");
    timing_note(&mut c, 0, IMMEDIATE_MS, got.elapsed);
    c.that(
        "response.elapsed_ms",
        &format!("at most {IMMEDIATE_MS} (nothing to wait for)"),
        ms <= IMMEDIATE_MS,
        ms,
    );
    c.eq(
        "response.responses[0].partitions[0].records.len",
        0usize,
        got.records.len(),
    );
    c.finish()
});

kafka_test!(min_bytes_zero, |ctx| {
    let got = poll(ctx, "t1", 0, WAIT_MS, 0).await?;
    let ms = got.elapsed.as_millis();
    let mut c = Check::detached("a fetch with min_bytes 0 on an empty partition");
    c.note("min_bytes 0 is satisfied by an empty response, so max_wait_ms never applies");
    timing_note(&mut c, 0, IMMEDIATE_MS, got.elapsed);
    c.that(
        "response.elapsed_ms",
        &format!("at most {IMMEDIATE_MS} (min_bytes 0 is already satisfied)"),
        ms <= IMMEDIATE_MS,
        ms,
    );
    c.eq(
        "response.responses[0].partitions[0].error_code",
        NONE,
        got.error_code,
    );
    c.finish()
});

kafka_test!(data_now, |ctx| {
    let got = poll(ctx, "t2", 0, WAIT_MS, 1).await?;
    let ms = got.elapsed.as_millis();
    let mut c = Check::detached("a fetch of a partition that already holds three records");
    timing_note(&mut c, 0, IMMEDIATE_MS, got.elapsed);
    c.that(
        "response.elapsed_ms",
        &format!("at most {IMMEDIATE_MS} (min_bytes is already satisfied)"),
        ms <= IMMEDIATE_MS,
        ms,
    );
    c.at_least(
        "response.responses[0].partitions[0].records.len",
        1usize,
        got.records.len(),
    );
    c.finish()
});

kafka_test!(min_bytes_waits, |ctx| {
    // Three short records are nowhere near a megabyte, so the fetch has to wait it out
    // and then answer with what it has.
    let got = poll(ctx, "t2", 0, WAIT_MS, 1024 * 1024).await?;
    let low = WAIT_MS as u128 - EARLY_SLACK_MS;
    let high = WAIT_MS as u128 + LATE_SLACK_MS;
    let ms = got.elapsed.as_millis();
    let mut c = Check::detached("a fetch whose min_bytes is larger than the whole partition");
    timing_note(&mut c, low, high, got.elapsed);
    c.that(
        "response.elapsed_ms",
        &format!("at least {low} (min_bytes is not satisfied)"),
        ms >= low,
        ms,
    );
    c.that(
        "response.elapsed_ms",
        &format!("at most {high} (max_wait_ms plus slack)"),
        ms <= high,
        ms,
    );
    c.note("when the timer expires the broker sends what it has, however little that is");
    c.at_least(
        "response.responses[0].partitions[0].records.len",
        1usize,
        got.records.len(),
    );
    c.finish()
});

kafka_test!(wakes_on_produce, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let here: &Ctx = ctx;
    let wait_ms = 3_000;
    let delay = Duration::from_millis(300);

    let waiting = poll(here, "t1", 0, wait_ms, 1);
    let producing = async {
        tokio::time::sleep(delay).await;
        let mut conn = here.connect().await?;
        let batch = RecordBatch::of(0, 0, vec![RecordItem::value("woken")]);
        let req = produce_request(&[(t.name.clone(), 0, batch.encode())], -1);
        let resp = conn
            .request(PRODUCE_V11, &req)
            .await
            .map_err(|e| proto_fail(e, &conn))?;
        let mut c = Check::new("the produce that must wake the parked fetch", &conn);
        c.eq(
            "response.responses[0].partition_responses[0].error_code",
            NONE,
            resp.responses
                .first()
                .and_then(|t| t.partition_responses.first())
                .map(|p| p.error_code)
                .unwrap_or(-1),
        );
        c.finish()
    };
    let (polled, produced) = tokio::join!(waiting, producing);
    produced?;
    let got = polled?;

    let ms = got.elapsed.as_millis();
    let high = wait_ms as u128 - 500;
    let mut c = Check::detached("a fetch woken by a produce on another connection");
    c.note(format!(
        "the record was produced after {} ms of a {wait_ms} ms poll",
        delay.as_millis()
    ));
    timing_note(&mut c, 0, high, got.elapsed);
    c.that(
        "response.elapsed_ms",
        &format!("well under {wait_ms} (the append must complete the fetch)"),
        ms <= high,
        ms,
    );
    let batches = RecordBatch::decode_all(&got.records)
        .map_err(|e| Failure::harness(format!("the woken fetch's records do not decode: {e:#}")))?;
    let values: Vec<String> = batches
        .iter()
        .flat_map(|b| b.records.iter())
        .map(|r| String::from_utf8_lossy(r.value.as_deref().unwrap_or_default()).to_string())
        .collect();
    c.eq("records[*].value", vec!["woken".to_string()], values);
    c.finish()
});

kafka_test!(still_serving, |ctx| {
    let here: &Ctx = ctx;
    let waiting = poll(here, "t1", 0, WAIT_MS, 1);
    let other = async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let started = Instant::now();
        let mut conn = here.connect().await?;
        let resp = api_versions(&mut conn).await?;
        let elapsed = started.elapsed();
        let mut c = Check::new("a second connection while a fetch is parked", &conn);
        c.eq("response.error_code", NONE, resp.error_code);
        timing_note(&mut c, 0, IMMEDIATE_MS, elapsed);
        c.that(
            "response.elapsed_ms",
            &format!("at most {IMMEDIATE_MS} (the parked fetch must not block the loop)"),
            elapsed.as_millis() <= IMMEDIATE_MS,
            elapsed.as_millis(),
        );
        c.finish()
    };
    let (polled, served) = tokio::join!(waiting, other);
    served?;
    polled?;
    Ok(())
});

/// Worked examples: a poll that has to wait, and a poll that does not.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("A poll on an empty partition", |env| {
            env.request(FETCH_V16, 281, &poll_request(env.id("t1")?, 0, 0, 1_000, 1))
        })
        .with_fixtures(empty_and_full)
        .request(
            "Fetch v16, correlation id 281: partition 0 of an empty topic from offset 0, \
             max_wait_ms 1000, min_bytes 1",
        )
        .response(
            "Nothing for about a second, then throttle_time_ms 0, top-level error_code 0, and \
             the partition with error_code 0, high_watermark 0 and an empty records field",
        )
        .note(
            "Park the request instead of answering it: min_bytes 1 cannot be met, so the broker \
             owes the client either a record or the timeout. Answering at once turns a \
             consumer's poll loop into a busy loop, and blocking a whole thread on the sleep \
             starves every other connection.",
        ),
        ExampleSpec::wire("The same poll where records already exist", |env| {
            env.request(FETCH_V16, 282, &poll_request(env.id("t2")?, 0, 0, 1_000, 1))
        })
        .with_fixtures(empty_and_full)
        .request(
            "The same request, correlation id 282, against a partition that already holds three \
             records",
        )
        .response(
            "Immediately, in milliseconds: error_code 0, high_watermark 3, and the batch \
             carrying 'r0', 'r1' and 'r2'",
        )
        .note(
            "min_bytes is tested before the wait begins, and again every time the log grows — a \
             produce that lands while the fetch is parked must complete it there and then, not \
             leave the consumer to wait out max_wait_ms for data that is already on disk.",
        ),
    ]
}
