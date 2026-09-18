//! Stage 34 — produced records really are on disk, in Kafka's segment format.

use crate::assert::{Check, Failure};
use crate::examples::{ExampleSpec, Wire};
use crate::fixtures::{FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::proto::records::{RecordBatch, RecordItem, MAGIC_V2};
use crate::stages::{produce_request, proto_fail, Ctx, Stage, Test, NONE, PRODUCE_V11};
use std::path::{Path, PathBuf};

fn empty_topic() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 1))
}

fn batch(values: &[&str]) -> Vec<u8> {
    RecordBatch::of(
        0,
        1_700_000_000_000,
        values.iter().map(|v| RecordItem::value(*v)).collect(),
    )
    .encode()
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 34,
        slug: "produce_persisted",
        name: "Produced records are persisted on disk",
        ext: true,
        hints: &[
            "Append the batch to <log.dirs>/<topic>-<partition>/00000000000000000000.log \
             byte for byte; only baseOffset is rewritten, and the CRC-32C does not cover it",
            "A second Produce appends a second batch after the first — a segment is a \
             concatenation of batches, never a rewrite",
            "Create the empty 00000000000000000000.index and .timeindex next to the segment; \
             real clients and kafka-dump-log.sh expect them to be there",
            "kafka-dump-log.sh --deep-iteration over your segment is the ground truth",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new(
                "the segment file grows when a record is produced",
                segment_grows,
            )
            .with_fixtures(empty_topic)
            .ext(),
            Test::new(
                "the segment holds a v2 RecordBatch with a valid CRC-32C",
                segment_is_a_valid_batch,
            )
            .with_fixtures(empty_topic)
            .ext(),
            Test::new(
                "the batch on disk carries the base offset the response reported",
                base_offset_matches,
            )
            .with_fixtures(empty_topic)
            .ext(),
            Test::new(
                "the record values on disk are the ones produced",
                values_match,
            )
            .with_fixtures(empty_topic)
            .ext(),
            Test::new(
                "the record count in the header matches the records",
                record_count_matches,
            )
            .with_fixtures(empty_topic)
            .ext(),
            Test::new(
                "a second produce appends a second batch at the next offset",
                second_batch_appended,
            )
            .with_fixtures(empty_topic)
            .ext(),
            Test::new(
                "the partition has an .index and a .timeindex file",
                index_files_exist,
            )
            .with_fixtures(empty_topic)
            .ext(),
        ],
    }
}

/// Produce one batch with acks=-1 and return the base offset the broker reported.
async fn produce(ctx: &Ctx, name: &str, values: &[&str]) -> Result<i64, Failure> {
    let mut conn = ctx.connect().await?;
    let req = produce_request(&[(name.to_string(), 0, batch(values))], -1);
    let resp = conn
        .request(PRODUCE_V11, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new(format!("producing {} record(s)", values.len()), &conn);
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
    c.eq(
        "response.responses[0].partition_responses[0].error_code",
        NONE,
        p.error_code,
    );
    c.finish()?;
    Ok(p.base_offset)
}

/// The partition's segment bytes, with the path in the failure when it cannot be read.
fn segment(ctx: &Ctx, name: &str) -> Result<(PathBuf, Vec<u8>), Failure> {
    let path = ctx.fixtures.segment_path(name, 0);
    let bytes = ctx.fixtures.segment_bytes(name, 0).map_err(|f| {
        f.note(format!(
            "the produce succeeded, so {} must exist and hold the batch",
            path.display()
        ))
    })?;
    Ok((path, bytes))
}

/// Decode every batch of a segment, turning a decode error into a readable failure.
fn batches(path: &Path, bytes: &[u8]) -> Result<Vec<RecordBatch>, Failure> {
    RecordBatch::decode_all(bytes).map_err(|e| {
        let mut c = Check::detached(format!("the segment file {}", path.display()));
        c.that(
            "segment.batches",
            "a concatenation of valid v2 record batches (correct batchLength and CRC-32C)",
            false,
            format!("{e:#}"),
        );
        c.finish()
            .err()
            .unwrap_or_else(|| Failure::harness("segment does not decode"))
    })
}

kafka_test!(segment_grows, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let before = ctx
        .fixtures
        .segment_bytes(&t.name, 0)
        .map(|b| b.len())
        .unwrap_or(0);
    produce(ctx, &t.name, &["persist me"]).await?;
    let (path, bytes) = segment(ctx, &t.name)?;
    let mut c = Check::detached(format!("the segment file {}", path.display()));
    c.at_least("segment.len", before + 1, bytes.len());
    c.finish()
});

kafka_test!(segment_is_a_valid_batch, |ctx| {
    let t = ctx.topic("t1")?.clone();
    produce(ctx, &t.name, &["one", "two"]).await?;
    let (path, bytes) = segment(ctx, &t.name)?;
    // decode_all verifies batchLength and recomputes the CRC-32C of every batch.
    let all = batches(&path, &bytes)?;
    let mut c = Check::detached(format!("the batch written to {}", path.display()));
    c.eq("segment.batches.len", 1usize, all.len());
    let magic = bytes.get(16).map(|b| *b as i8).unwrap_or(-1);
    c.eq("segment.batches[0].magic", MAGIC_V2, magic);
    c.finish()
});

kafka_test!(base_offset_matches, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let base = produce(ctx, &t.name, &["a", "b", "c"]).await?;
    let (path, bytes) = segment(ctx, &t.name)?;
    let all = batches(&path, &bytes)?;
    let mut c = Check::detached(format!("the base offset in {}", path.display()));
    c.eq("response.base_offset", 0i64, base);
    c.eq(
        "segment.batches[0].base_offset",
        base,
        all.first().map(|b| b.base_offset).unwrap_or(-1),
    );
    c.eq(
        "segment.batches[0].last_offset_delta",
        2i32,
        all.first().map(|b| b.last_offset_delta).unwrap_or(-1),
    );
    c.finish()
});

kafka_test!(values_match, |ctx| {
    let t = ctx.topic("t1")?.clone();
    produce(ctx, &t.name, &["alpha", "beta", "gamma"]).await?;
    let (path, bytes) = segment(ctx, &t.name)?;
    let all = batches(&path, &bytes)?;
    let on_disk: Vec<String> = all
        .iter()
        .flat_map(|b| b.records.iter())
        .map(|r| String::from_utf8_lossy(r.value.as_deref().unwrap_or_default()).to_string())
        .collect();
    let mut c = Check::detached(format!("the records in {}", path.display()));
    c.eq(
        "segment.batches[*].records[*].value",
        vec!["alpha".to_string(), "beta".into(), "gamma".into()],
        on_disk,
    );
    c.finish()
});

kafka_test!(record_count_matches, |ctx| {
    let t = ctx.topic("t1")?.clone();
    produce(ctx, &t.name, &["r0", "r1", "r2", "r3"]).await?;
    let (path, bytes) = segment(ctx, &t.name)?;
    let all = batches(&path, &bytes)?;
    // decode_all reads exactly `recordCount` records, so a wrong count would already have
    // failed above; this pins the number down for the report.
    let mut c = Check::detached(format!("the record count in {}", path.display()));
    c.eq(
        "segment.batches[0].records.len",
        4usize,
        all.first().map(|b| b.records.len()).unwrap_or(0),
    );
    c.finish()
});

kafka_test!(second_batch_appended, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let first = produce(ctx, &t.name, &["first", "second"]).await?;
    let second = produce(ctx, &t.name, &["third"]).await?;
    let (path, bytes) = segment(ctx, &t.name)?;
    let all = batches(&path, &bytes)?;
    let mut c = Check::detached(format!("the two batches in {}", path.display()));
    c.eq("response[0].base_offset", 0i64, first);
    c.eq("response[1].base_offset", 2i64, second);
    c.eq("segment.batches.len", 2usize, all.len());
    c.eq(
        "segment.batches[*].base_offset",
        vec![first, second],
        all.iter().map(|b| b.base_offset).collect::<Vec<i64>>(),
    );
    c.eq(
        "segment.batches[*].records[*].value",
        vec!["first".to_string(), "second".into(), "third".into()],
        all.iter()
            .flat_map(|b| b.records.iter())
            .map(|r| String::from_utf8_lossy(r.value.as_deref().unwrap_or_default()).to_string())
            .collect::<Vec<String>>(),
    );
    c.finish()
});

kafka_test!(index_files_exist, |ctx| {
    let t = ctx.topic("t1")?.clone();
    produce(ctx, &t.name, &["indexed"]).await?;
    let log = ctx.fixtures.segment_path(&t.name, 0);
    let mut c = Check::detached(format!("the index files next to {}", log.display()));
    for extension in ["index", "timeindex"] {
        let path = log.with_extension(extension);
        c.that(
            &format!("<log.dirs>/{}-0/00000000000000000000.{extension}", t.name),
            "a file (its contents are not checked)",
            path.is_file(),
            format!("{} does not exist", path.display()),
        );
    }
    c.finish()
});

/// Worked examples: the bytes that end up in the segment file, once and then twice.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("The produce whose bytes land in the segment", |env| {
            env.request(
                PRODUCE_V11,
                341,
                &produce_request(&[(env.name("t1")?, 0, batch(&["on disk"]))], -1),
            )
        })
        .with_fixtures(empty_topic)
        .request(
            "Produce v11, correlation id 341, acks -1: one v2 RecordBatch — baseOffset 0, \
             magic 2, a CRC-32C over everything after the crc field, baseTimestamp \
             1700000000000, recordCount 1 — holding the record 'on disk'",
        )
        .response(
            "An ordinary success: index 0, error_code 0 (NONE), base_offset 0, \
             log_append_time_ms -1, log_start_offset 0, throttle_time_ms 0",
        )
        .note(
            "The interesting half of this example is not the response but the disk. After \
             it, <log.dirs>/<topic>-0/00000000000000000000.log must hold exactly the batch \
             bytes from the request, with only baseOffset rewritten — the CRC-32C does not \
             cover baseOffset or batchLength, so it must still verify. An empty \
             00000000000000000000.index and .timeindex belong next to it; \
             kafka-dump-log.sh --deep-iteration over the segment is the ground truth.",
        ),
        ExampleSpec::wire("A second produce appends a second batch", |env| {
            let name = env.name("t1")?;
            Ok(Wire::Frames(vec![
                env.payload(
                    PRODUCE_V11,
                    342,
                    &produce_request(&[(name.clone(), 0, batch(&["first"]))], -1),
                )?,
                env.payload(
                    PRODUCE_V11,
                    343,
                    &produce_request(&[(name, 0, batch(&["second"]))], -1),
                )?,
            ]))
        })
        .with_fixtures(empty_topic)
        .request(
            "Two Produce v11 frames, correlation ids 342 and 343, one record each to \
             partition 0 of the same topic",
        )
        .response(
            "Two responses: 342 with base_offset 0, 343 with base_offset 1, both error_code \
             0 (NONE)",
        )
        .note(
            "The segment now holds two complete v2 batches, one after the other, the second \
             stamped baseOffset 1. A log segment is a concatenation of batches — appended \
             to, never rewritten, and never merged into one batch.",
        ),
    ]
}
