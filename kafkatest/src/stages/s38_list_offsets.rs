//! Stage 38 — ListOffsets (2) v8: earliest, latest and by timestamp.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::fixtures::{FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::proto::records::{RecordBatch, RecordItem};
use crate::stages::group_protocol::LIST_OFFSETS_V8;
use crate::stages::{
    await_high_watermark, produce_request, proto_fail, topic_name, Ctx, Stage, Test, NONE,
    PRODUCE_V11, UNKNOWN_TOPIC_OR_PARTITION,
};
use kafka_protocol::messages::list_offsets_request::{ListOffsetsPartition, ListOffsetsTopic};
use kafka_protocol::messages::list_offsets_response::ListOffsetsPartitionResponse;
use kafka_protocol::messages::{BrokerId, ListOffsetsRequest, ListOffsetsResponse};

/// `timestamp` asking for the first offset of the log.
const EARLIEST: i64 = -2;
/// `timestamp` asking for the offset one past the last record.
const LATEST: i64 = -1;
/// Timestamp of the first fixture batch; the rest are `STEP` apart.
const T0: i64 = 1_700_000_000_000;
/// Distance between the fixture batches, in milliseconds.
const STEP: i64 = 10_000;
/// Records the fixture writes into partition 0 of `t1`.
const TOTAL: i64 = 5;

fn one_partition() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 1))
}

fn two_partitions() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 2))
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 38,
        slug: "list_offsets",
        name: "ListOffsets (2): earliest, latest, by timestamp",
        ext: true,
        hints: &[
            "timestamp -2 means the log start offset, -1 means the high watermark; neither \
             reads a record",
            "Any other timestamp returns the first offset whose record timestamp is >= it, \
             and that record's timestamp; past the end it is offset -1, timestamp -1",
            "Answer per partition: error_code, timestamp, offset, leader_epoch — earliest \
             and latest report timestamp -1",
            "An unknown topic or partition is error 3 in that partition's entry, not a \
             top-level failure",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("the earliest offset of the log is 0", earliest)
                .with_fixtures(one_partition)
                .ext(),
            Test::new("the latest offset is the high watermark", latest)
                .with_fixtures(one_partition)
                .ext(),
            Test::new(
                "a timestamp finds the first record at or after it",
                by_timestamp,
            )
            .with_fixtures(one_partition)
            .ext(),
            Test::new(
                "a timestamp between two batches rounds forward",
                between_batches,
            )
            .with_fixtures(one_partition)
            .ext(),
            Test::new("a timestamp after every record is offset -1", past_the_end)
                .with_fixtures(one_partition)
                .ext(),
            Test::new("an unknown topic is error 3", unknown_topic)
                .with_fixtures(one_partition)
                .ext(),
            Test::new(
                "read_committed sees the same latest offset",
                isolation_level,
            )
            .with_fixtures(one_partition)
            .ext(),
            Test::new("each partition is answered on its own", per_partition)
                .with_fixtures(two_partitions)
                .ext(),
        ],
    }
}

/// A `ListOffsets` v8 request for `(topic, partition, timestamp)` triples.
fn list_offsets_request(entries: &[(&str, i32, i64)], isolation_level: i8) -> ListOffsetsRequest {
    let mut by_topic: Vec<(String, Vec<ListOffsetsPartition>)> = Vec::new();
    for (topic, partition, timestamp) in entries {
        let mut p = ListOffsetsPartition::default();
        p.partition_index = *partition;
        p.current_leader_epoch = -1;
        p.timestamp = *timestamp;
        match by_topic.iter_mut().find(|(t, _)| t == topic) {
            Some((_, ps)) => ps.push(p),
            None => by_topic.push(((*topic).to_string(), vec![p])),
        }
    }
    let mut r = ListOffsetsRequest::default();
    r.replica_id = BrokerId(-1);
    r.isolation_level = isolation_level;
    r.topics = by_topic
        .into_iter()
        .map(|(name, partitions)| {
            let mut t = ListOffsetsTopic::default();
            t.name = topic_name(&name);
            t.partitions = partitions;
            t
        })
        .collect();
    r
}

fn partition_of(
    resp: &ListOffsetsResponse,
    topic: &str,
    partition: i32,
) -> Option<ListOffsetsPartitionResponse> {
    resp.topics
        .iter()
        .find(|t| t.name.as_str() == topic)?
        .partitions
        .iter()
        .find(|p| p.partition_index == partition)
        .cloned()
}

/// Write three batches with known, increasing timestamps: 2 records at `T0`, 1 at
/// `T0 + STEP`, 2 at `T0 + 2 * STEP`.
async fn seed_timestamps(ctx: &Ctx, name: &str, partition: i32) -> Result<(), Failure> {
    let plan: [(i64, &[&str]); 3] = [
        (T0, &["a0", "a1"]),
        (T0 + STEP, &["b0"]),
        (T0 + 2 * STEP, &["c0", "c1"]),
    ];
    for (timestamp, values) in plan {
        let bytes = RecordBatch::of(
            0,
            timestamp,
            values.iter().map(|v| RecordItem::value(*v)).collect(),
        )
        .encode();
        let mut conn = ctx.connect().await?;
        let req = produce_request(&[(name.to_string(), partition, bytes)], -1);
        let resp = conn
            .request(PRODUCE_V11, &req)
            .await
            .map_err(|e| proto_fail(e, &conn))?;
        let mut c = Check::new(
            format!("seeding {name}-{partition} with a batch stamped {timestamp}"),
            &conn,
        );
        c.eq(
            "response.responses[0].partition_responses[0].error_code",
            NONE,
            resp.responses
                .first()
                .and_then(|t| t.partition_responses.first())
                .map(|p| p.error_code)
                .unwrap_or(-1),
        );
        c.finish()?;
    }
    Ok(())
}

/// Seed `t1` and ask one `ListOffsets` question about it.
async fn ask(
    ctx: &Ctx,
    timestamp: i64,
    isolation_level: i8,
) -> Result<ListOffsetsPartitionResponse, Failure> {
    let t = ctx.topic("t1")?.clone();
    seed_timestamps(ctx, &t.name, 0).await?;
    await_high_watermark(ctx, &t, 0, TOTAL).await?;
    let mut conn = ctx.connect().await?;
    let req = list_offsets_request(&[(t.name.as_str(), 0, timestamp)], isolation_level);
    let resp = conn
        .request(LIST_OFFSETS_V8, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let Some(p) = partition_of(&resp, &t.name, 0) else {
        let mut c = Check::new(
            format!("ListOffsets(timestamp={timestamp}) for '{}'", t.name),
            &conn,
        );
        c.that(
            "response.topics[name=t1].partitions[0]",
            "an entry for partition 0",
            false,
            resp.topics
                .iter()
                .map(|t| t.name.to_string())
                .collect::<Vec<String>>(),
        );
        return Err(c
            .finish()
            .err()
            .unwrap_or_else(|| Failure::harness("no partition entry")));
    };
    let mut c = Check::new(
        format!("ListOffsets(timestamp={timestamp}) for '{}'", t.name),
        &conn,
    );
    c.eq(
        "response.topics[0].partitions[0].error_code",
        NONE,
        p.error_code,
    );
    c.finish()?;
    Ok(p)
}

kafka_test!(earliest, |ctx| {
    let p = ask(ctx, EARLIEST, 0).await?;
    let mut c = Check::detached("the earliest offset of a log that starts at 0");
    c.eq("response.topics[0].partitions[0].offset", 0i64, p.offset);
    c.eq(
        "response.topics[0].partitions[0].timestamp",
        -1i64,
        p.timestamp,
    );
    c.finish()
});

kafka_test!(latest, |ctx| {
    let p = ask(ctx, LATEST, 0).await?;
    let mut c = Check::detached("the latest offset of a log holding five records");
    c.eq("response.topics[0].partitions[0].offset", TOTAL, p.offset);
    c.eq(
        "response.topics[0].partitions[0].timestamp",
        -1i64,
        p.timestamp,
    );
    c.at_least(
        "response.topics[0].partitions[0].leader_epoch",
        0i32,
        p.leader_epoch,
    );
    c.finish()
});

kafka_test!(by_timestamp, |ctx| {
    // The second batch is stamped T0 + STEP and starts at offset 2.
    let p = ask(ctx, T0 + STEP, 0).await?;
    let mut c = Check::detached("the offset of the first record stamped T0 + 10s");
    c.eq("response.topics[0].partitions[0].offset", 2i64, p.offset);
    c.eq(
        "response.topics[0].partitions[0].timestamp",
        T0 + STEP,
        p.timestamp,
    );
    c.finish()
});

kafka_test!(between_batches, |ctx| {
    // Nothing is stamped T0 + 5s, so the answer is the next record after it.
    let p = ask(ctx, T0 + STEP / 2, 0).await?;
    let mut c = Check::detached("a timestamp that falls between two batches");
    c.eq("response.topics[0].partitions[0].offset", 2i64, p.offset);
    c.eq(
        "response.topics[0].partitions[0].timestamp",
        T0 + STEP,
        p.timestamp,
    );
    c.finish()
});

kafka_test!(past_the_end, |ctx| {
    let p = ask(ctx, T0 + 10 * STEP, 0).await?;
    let mut c = Check::detached("a timestamp later than every record in the log");
    c.eq("response.topics[0].partitions[0].offset", -1i64, p.offset);
    c.eq(
        "response.topics[0].partitions[0].timestamp",
        -1i64,
        p.timestamp,
    );
    c.finish()
});

kafka_test!(unknown_topic, |ctx| {
    let missing = ctx.unique("no-such-topic-38");
    let mut conn = ctx.connect().await?;
    let req = list_offsets_request(&[(missing.as_str(), 0, LATEST)], 0);
    let resp = conn
        .request(LIST_OFFSETS_V8, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let p = partition_of(&resp, &missing, 0);
    let mut c = Check::new(
        format!("ListOffsets for the unknown topic '{missing}'"),
        &conn,
    );
    c.eq(
        "response.topics[0].partitions[0].error_code",
        UNKNOWN_TOPIC_OR_PARTITION,
        p.as_ref().map(|p| p.error_code).unwrap_or(NONE),
    );
    c.eq(
        "response.topics[0].partitions[0].offset",
        -1i64,
        p.as_ref().map(|p| p.offset).unwrap_or(0),
    );
    c.finish()
});

kafka_test!(isolation_level, |ctx| {
    // Without transactions the last stable offset equals the high watermark, so
    // read_committed(1) and read_uncommitted(0) must agree.
    let p = ask(ctx, LATEST, 1).await?;
    let mut c = Check::detached("the latest offset under isolation_level=read_committed");
    c.eq("response.topics[0].partitions[0].offset", TOTAL, p.offset);
    c.finish()
});

kafka_test!(per_partition, |ctx| {
    let t = ctx.topic("t1")?.clone();
    seed_timestamps(ctx, &t.name, 0).await?;
    let bytes = RecordBatch::of(0, T0, vec![RecordItem::value("only")]).encode();
    let mut conn = ctx.connect().await?;
    let req = produce_request(&[(t.name.clone(), 1, bytes)], -1);
    conn.request(PRODUCE_V11, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    await_high_watermark(ctx, &t, 0, TOTAL).await?;
    await_high_watermark(ctx, &t, 1, 1).await?;

    let mut conn = ctx.connect().await?;
    let req = list_offsets_request(
        &[(t.name.as_str(), 0, LATEST), (t.name.as_str(), 1, LATEST)],
        0,
    );
    let resp = conn
        .request(LIST_OFFSETS_V8, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("two partitions of one topic in one ListOffsets", &conn);
    c.eq(
        "response.topics[0].partitions.len",
        2usize,
        resp.topics.first().map(|t| t.partitions.len()).unwrap_or(0),
    );
    c.eq(
        "response.topics[0].partitions[partition=0].offset",
        TOTAL,
        partition_of(&resp, &t.name, 0)
            .map(|p| p.offset)
            .unwrap_or(-2),
    );
    c.eq(
        "response.topics[0].partitions[partition=1].offset",
        1i64,
        partition_of(&resp, &t.name, 1)
            .map(|p| p.offset)
            .unwrap_or(-2),
    );
    c.finish()
});

/// A one-partition topic holding three records, so the offsets below are fixed.
fn three_records() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 1).with_values(0, &["a", "b", "c"]))
}

/// Worked examples: what the broker sees, and what a correct broker answers.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("Latest offset of a three-record log", |env| {
            let name = env.name("t1")?;
            env.request(
                LIST_OFFSETS_V8,
                381,
                &list_offsets_request(&[(name.as_str(), 0, LATEST)], 0),
            )
        })
        .with_fixtures(three_records)
        .request(
            "ListOffsets v8, replica_id -1, isolation_level 0 (read_uncommitted), asking the \
             fixture topic's partition 0 for timestamp -1 with current_leader_epoch -1",
        )
        .response(
            "One topic entry, one partition entry: error_code 0 (NONE), offset 3 — the high \
             watermark, one past the last of the three records — timestamp -1, and the \
             partition's current leader_epoch; throttle_time_ms 0",
        )
        .note(
            "-1 (LATEST) and -2 (EARLIEST) are answered out of the log's metadata: no record \
             is read, which is why the timestamp comes back -1 rather than the last record's \
             timestamp. The offset is the one the next append will get, not the offset of the \
             last record — an off-by-one here makes every consumer read one record twice or \
             stop one short.",
        ),
        ExampleSpec::wire("Earliest offset of the same log", |env| {
            let name = env.name("t1")?;
            env.request(
                LIST_OFFSETS_V8,
                382,
                &list_offsets_request(&[(name.as_str(), 0, EARLIEST)], 0),
            )
        })
        .with_fixtures(three_records)
        .request("The same request with timestamp -2 (EARLIEST) instead of -1")
        .response(
            "error_code 0 and offset 0, the log start offset, with timestamp -1 again: the \
             three records are all still there, so the log starts where it was created",
        )
        .note(
            "The earliest offset is the log start offset, not a constant 0: once retention \
             has deleted the first segments it moves forward, and that is the offset a \
             consumer with auto.offset.reset=earliest resumes from. A timestamp that is \
             neither -1 nor -2 is a real search — the first record stamped at or after it, \
             with that record's own timestamp, or offset -1 / timestamp -1 past the end.",
        ),
    ]
}
