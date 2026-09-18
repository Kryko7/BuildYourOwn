//! Stage 26 — `max_bytes` and `partition_max_bytes` truncation.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::fixtures::{FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::proto::records::RecordBatch;
use crate::stages::{proto_fail, Ctx, Stage, Test, FETCH_V16, NONE};
use kafka_protocol::messages::fetch_request::{FetchPartition, FetchTopic};
use kafka_protocol::messages::{BrokerId, FetchRequest, FetchResponse};
use uuid::Uuid;

/// Values of partition 0's six batches; the lengths differ so the limits are not multiples.
const P0: &[&str] = &[
    "b0",
    "b1-xxxx",
    "b2-xxxxxxxxxx",
    "b3-xxxxxxxxxxxxxxxxxxxx",
    "b4-x",
    "b5-xxxxxxx",
];
/// Values of partition 1's three batches.
const P1: &[&str] = &["q0-yyyyyy", "q1", "q2-yyyyyyyyyyyy"];

/// A partition with six one-record batches and a second with three.
fn many_batches() -> FixtureSpec {
    FixtureSpec::with(
        TopicSpec::new("t1", 2)
            .with_values(0, P0)
            .with_values(1, P1),
    )
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 26,
        slug: "fetch_max_bytes",
        name: "max_bytes and partition_max_bytes truncation",
        ext: true,
        hints: &[
            "Fill a partition with whole batches until the next one would pass \
             partition_max_bytes, then stop — never send half a batch you chose to send",
            "The first batch is exempt: a batch larger than the limit still goes out in \
             full, or a consumer whose limit is too small never makes progress",
            "The top-level max_bytes is the budget for the whole response; subtract what \
             each partition used and give the rest to the next one",
            "Only the very first partition of the response gets the 'at least one batch' \
             exemption; once the budget is gone the others come back empty",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new(
                "a limit below the first batch still returns it",
                at_least_one,
            )
            .with_fixtures(many_batches)
            .ext(),
            Test::new("a limit of exactly one batch returns one batch", one_batch)
                .with_fixtures(many_batches)
                .ext(),
            Test::new(
                "a limit of exactly three batches returns three",
                three_batches,
            )
            .with_fixtures(many_batches)
            .ext(),
            Test::new("no batch beyond the limit is added", stops_at_the_limit)
                .with_fixtures(many_batches)
                .ext(),
            Test::new("a large limit returns every batch", everything)
                .with_fixtures(many_batches)
                .ext(),
            Test::new("the top-level max_bytes caps the whole response", top_level)
                .with_fixtures(many_batches)
                .ext(),
            Test::new("each partition carries its own limit", per_partition)
                .with_fixtures(many_batches)
                .ext(),
        ],
    }
}

/// A `Fetch` v16 request with an explicit budget: `(topic, partition, offset, max bytes)`.
fn limited_fetch(parts: &[(Uuid, i32, i64, i32)], max_bytes: i32) -> FetchRequest {
    let mut r = FetchRequest::default();
    r.max_wait_ms = 500;
    r.min_bytes = 1;
    r.max_bytes = max_bytes;
    r.session_id = 0;
    r.session_epoch = 0;
    r.replica_id = BrokerId(-1);
    let mut by_topic: Vec<(Uuid, Vec<FetchPartition>)> = Vec::new();
    for (id, partition, offset, partition_max_bytes) in parts {
        let mut p = FetchPartition::default();
        p.partition = *partition;
        p.fetch_offset = *offset;
        p.current_leader_epoch = -1;
        p.last_fetched_epoch = -1;
        p.log_start_offset = -1;
        p.partition_max_bytes = *partition_max_bytes;
        match by_topic.iter_mut().find(|(t, _)| t == id) {
            Some((_, ps)) => ps.push(p),
            None => by_topic.push((*id, vec![p])),
        }
    }
    r.topics = by_topic
        .into_iter()
        .map(|(id, partitions)| {
            let mut t = FetchTopic::default();
            t.topic_id = id;
            t.partitions = partitions;
            t
        })
        .collect();
    r
}

/// Every batch of a buffer that arrived complete; a truncated tail is ignored.
///
/// A broker is free to cut the byte stream mid-batch once it has sent everything the limit
/// allows, so the interesting number is how many *whole* batches the client can use.
fn complete_batches(bytes: &[u8]) -> Vec<RecordBatch> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 12 <= bytes.len() {
        match RecordBatch::decode(&bytes[at..]) {
            Ok((b, used)) if used > 0 => {
                at += used;
                out.push(b);
            }
            _ => break,
        }
    }
    out
}

/// The size in bytes of every batch of a partition, read off the broker's own segment.
fn segment_batch_sizes(ctx: &Ctx, topic: &str, partition: i32) -> Result<Vec<usize>, Failure> {
    let bytes = ctx.fixtures.segment_bytes(topic, partition)?;
    let mut sizes = Vec::new();
    let mut at = 0usize;
    while at + 12 <= bytes.len() {
        let (_, used) = RecordBatch::decode(&bytes[at..]).map_err(|e| {
            Failure::harness(format!(
                "the fixture segment of {topic}-{partition} does not decode at byte {at}: {e:#}"
            ))
        })?;
        sizes.push(used);
        at += used;
    }
    Ok(sizes)
}

/// Run a fetch and hand back the decoded body after checking the top-level error code.
async fn fetch(
    ctx: &Ctx,
    parts: &[(Uuid, i32, i64, i32)],
    max_bytes: i32,
) -> Result<FetchResponse, Failure> {
    let mut conn = ctx.connect().await?;
    let req = limited_fetch(parts, max_bytes);
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new(
        format!("a fetch with max_bytes {max_bytes} and per-partition limits"),
        &conn,
    );
    c.eq("response.error_code", NONE, resp.error_code);
    c.finish()?;
    Ok(resp.body)
}

/// The records of one partition of a response.
fn records_of(resp: &FetchResponse, topic: Uuid, partition: i32) -> Vec<u8> {
    resp.responses
        .iter()
        .find(|t| t.topic_id == topic)
        .and_then(|t| t.partitions.iter().find(|p| p.partition_index == partition))
        .and_then(|p| p.records.clone())
        .map(|b| b.to_vec())
        .unwrap_or_default()
}

kafka_test!(at_least_one, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let sizes = segment_batch_sizes(ctx, &t.name, 0)?;
    let resp = fetch(ctx, &[(t.id, 0, 0, 1)], 10 * 1024 * 1024).await?;
    let records = records_of(&resp, t.id, 0);
    let batches = complete_batches(&records);
    let mut c = Check::detached("a partition_max_bytes of 1, far below the first batch");
    c.note(format!(
        "the first batch of the fixture is {} bytes on disk",
        sizes.first().copied().unwrap_or(0)
    ));
    c.note("without this exemption a consumer with a small limit can never advance");
    c.at_least("records.complete_batches.len", 1usize, batches.len());
    c.eq(
        "records.batches[0].base_offset",
        0i64,
        batches.first().map(|b| b.base_offset).unwrap_or(-1),
    );
    c.finish()
});

kafka_test!(one_batch, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let sizes = segment_batch_sizes(ctx, &t.name, 0)?;
    let limit = sizes.first().copied().unwrap_or(0) as i32;
    let resp = fetch(ctx, &[(t.id, 0, 0, limit)], 10 * 1024 * 1024).await?;
    let records = records_of(&resp, t.id, 0);
    let batches = complete_batches(&records);
    let mut c = Check::detached("a partition_max_bytes of exactly one batch");
    c.note(format!(
        "partition_max_bytes = {limit}, the first batch to the byte"
    ));
    c.eq("records.complete_batches.len", 1usize, batches.len());
    c.eq("records.len", limit as usize, records.len());
    c.finish()
});

kafka_test!(three_batches, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let sizes = segment_batch_sizes(ctx, &t.name, 0)?;
    let limit: usize = sizes.iter().take(3).sum();
    let resp = fetch(ctx, &[(t.id, 0, 0, limit as i32)], 10 * 1024 * 1024).await?;
    let records = records_of(&resp, t.id, 0);
    let batches = complete_batches(&records);
    let mut c = Check::detached("a partition_max_bytes of exactly three batches");
    c.note(format!("the six batches are {sizes:?} bytes on disk"));
    c.eq("records.complete_batches.len", 3usize, batches.len());
    c.eq("records.len", limit, records.len());
    c.eq(
        "records.batches[*].base_offset",
        vec![0i64, 1, 2],
        batches.iter().map(|b| b.base_offset).collect::<Vec<i64>>(),
    );
    c.finish()
});

kafka_test!(stops_at_the_limit, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let sizes = segment_batch_sizes(ctx, &t.name, 0)?;
    let three: usize = sizes.iter().take(3).sum();
    // One byte short of the fourth batch: it must not be counted as delivered.
    let limit = three + sizes.get(3).copied().unwrap_or(1) - 1;
    let resp = fetch(ctx, &[(t.id, 0, 0, limit as i32)], 10 * 1024 * 1024).await?;
    let records = records_of(&resp, t.id, 0);
    let batches = complete_batches(&records);
    let mut c = Check::detached("a partition_max_bytes one byte short of a fourth batch");
    c.note(format!(
        "three batches are {three} bytes, the fourth needs {} more",
        sizes.get(3).copied().unwrap_or(0)
    ));
    c.eq("records.complete_batches.len", 3usize, batches.len());
    c.at_most("records.len", limit, records.len());
    c.finish()
});

kafka_test!(everything, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let sizes = segment_batch_sizes(ctx, &t.name, 0)?;
    let resp = fetch(ctx, &[(t.id, 0, 0, 1024 * 1024)], 10 * 1024 * 1024).await?;
    let records = records_of(&resp, t.id, 0);
    let batches = complete_batches(&records);
    let mut c = Check::detached("a partition_max_bytes larger than the whole partition");
    c.eq("records.complete_batches.len", P0.len(), batches.len());
    c.eq("records.len", sizes.iter().sum::<usize>(), records.len());
    c.finish()
});

kafka_test!(top_level, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let sizes = segment_batch_sizes(ctx, &t.name, 0)?;
    // Both partitions asked for with room for everything, but a top-level budget of 1 byte.
    let resp = fetch(
        ctx,
        &[(t.id, 0, 0, 1024 * 1024), (t.id, 1, 0, 1024 * 1024)],
        1,
    )
    .await?;
    let p0 = complete_batches(&records_of(&resp, t.id, 0));
    let p1 = complete_batches(&records_of(&resp, t.id, 1));
    let mut c = Check::detached("a top-level max_bytes of 1 across two partitions");
    c.note("the first partition keeps the 'at least one batch' exemption, the rest do not");
    c.note(format!(
        "partition 0's first batch is {} bytes",
        sizes.first().copied().unwrap_or(0)
    ));
    c.eq("partition 0 complete batches", 1usize, p0.len());
    c.eq("partition 1 complete batches", 0usize, p1.len());
    c.finish()
});

kafka_test!(per_partition, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let sizes = segment_batch_sizes(ctx, &t.name, 0)?;
    let tight = sizes.first().copied().unwrap_or(0) as i32;
    let resp = fetch(
        ctx,
        &[(t.id, 0, 0, tight), (t.id, 1, 0, 1024 * 1024)],
        10 * 1024 * 1024,
    )
    .await?;
    let p0 = complete_batches(&records_of(&resp, t.id, 0));
    let p1 = complete_batches(&records_of(&resp, t.id, 1));
    let mut c = Check::detached("two partitions with different partition_max_bytes");
    c.note(format!(
        "partition 0 is limited to {tight} bytes, partition 1 to a megabyte"
    ));
    c.eq("partition 0 complete batches", 1usize, p0.len());
    c.eq("partition 1 complete batches", P1.len(), p1.len());
    c.finish()
});

/// Worked examples: the smallest limit there is, and a budget spent before the second
/// partition.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("partition_max_bytes 1", |env| {
            env.request(
                FETCH_V16,
                261,
                &limited_fetch(&[(env.id("t1")?, 0, 0, 1)], 10 * 1024 * 1024),
            )
        })
        .with_fixtures(many_batches)
        .request(
            "Fetch v16, correlation id 261: partition 0 from offset 0 with partition_max_bytes \
             1 and a top-level max_bytes of 10485760. The partition holds six one-record \
             batches, every one of them far larger than a byte",
        )
        .response(
            "error_code 0, high_watermark 6, and a records field holding the first batch \
             complete — base_offset 0, its record 'b0' — and nothing after it",
        )
        .note(
            "The first batch is exempt from the limit, and it goes out whole or not at all. \
             Without that rule a consumer whose buffer is smaller than the batch it is sitting \
             on can never advance, and one oversized batch stalls the partition for ever.",
        ),
        ExampleSpec::wire("A top-level max_bytes of 1 across two partitions", |env| {
            let id = env.id("t1")?;
            env.request(
                FETCH_V16,
                262,
                &limited_fetch(&[(id, 0, 0, 1024 * 1024), (id, 1, 0, 1024 * 1024)], 1),
            )
        })
        .with_fixtures(many_batches)
        .request(
            "Fetch v16, correlation id 262: partitions 0 and 1, each allowed a megabyte of its \
             own, but a top-level max_bytes of 1 for the whole response",
        )
        .response(
            "error_code 0 on both partitions: partition 0 carries its first batch — the \
             exemption is spent there — and partition 1 comes back with an empty records field",
        )
        .note(
            "max_bytes is a budget for the response as a whole: subtract what each partition \
             used and hand the remainder to the next one. Once it is gone the remaining \
             partitions are answered with error 0 and no records — they are never dropped from \
             the response.",
        ),
    ]
}
