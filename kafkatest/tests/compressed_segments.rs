//! A compressed fixture segment must be a segment Kafka itself can read.
//!
//! Stage 27 claims that gzip, snappy, lz4 and zstd batches written by the `files` strategy
//! carry the framing Kafka expects. The proof is `kafka-dump-log.sh --deep-iteration` from
//! the reference tarball: it verifies the CRC-32C, decompresses each payload and prints the
//! records. These tests skip (rather than fail) when the tarball is not downloaded yet.

use kafkatest::broker::reference;
use kafkatest::fixtures::files::materialize;
use kafkatest::fixtures::{rec, FixtureSpec, TopicSpec};
use kafkatest::proto::records::{codec_name, RecordBatch, GZIP, LZ4, SNAPPY, ZSTD};
use std::path::PathBuf;

const CODECS: &[i16] = &[GZIP, SNAPPY, LZ4, ZSTD];

/// One two-record batch per codec, in the order of `CODECS`.
fn spec() -> FixtureSpec {
    let mut t = TopicSpec::new("packed", 1);
    for codec in CODECS {
        let n = codec_name(*codec);
        t = t.with_compressed_batch(
            0,
            *codec,
            vec![rec(format!("{n}-0")), rec(format!("{n}-1"))],
        );
    }
    FixtureSpec::with(t)
}

fn dist() -> Option<PathBuf> {
    let d = reference::dist_dir(reference::DEFAULT_VERSION);
    d.join("bin/kafka-dump-log.sh").is_file().then_some(d)
}

fn write_fixtures(dir: &std::path::Path) -> kafkatest::fixtures::FixtureHandle {
    materialize(
        &spec(),
        "it-27",
        4321,
        &dir.join("kraft-combined-logs"),
        1,
        "AAAAAAAAAAAAAAAAAAAAAQ",
    )
    .expect("the fixture writer must succeed")
}

#[test]
fn compressed_batches_round_trip_through_the_segment() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let handle = write_fixtures(tmp.path());
    let topic = handle.topic("packed").expect("packed");
    let segment = handle.segment_bytes(&topic.name, 0).expect("segment");
    let batches = RecordBatch::decode_all(&segment).expect("the segment decodes");

    assert_eq!(batches.len(), CODECS.len(), "one batch per codec");
    for (batch, codec) in batches.iter().zip(CODECS) {
        let n = codec_name(*codec);
        assert_eq!(batch.compression(), *codec, "{n} codec bits");
        assert_eq!(batch.records.len(), 2, "{n} record count");
        assert_eq!(
            batch.records[0].value.as_deref(),
            Some(format!("{n}-0").as_bytes()),
            "{n} first value"
        );
    }
    let bases: Vec<i64> = batches.iter().map(|b| b.base_offset).collect();
    assert_eq!(bases, vec![0, 2, 4, 6], "offsets continue across codecs");
}

#[test]
fn kafka_dump_log_reads_every_compressed_codec() {
    let Some(dist) = dist() else {
        eprintln!(
            "skipping: {} is not downloaded",
            reference::dist_dir(reference::DEFAULT_VERSION).display()
        );
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let handle = write_fixtures(tmp.path());
    let topic = handle.topic("packed").expect("packed");
    let segment = handle.segment_path(&topic.name, 0);

    let (stdout, stderr, ok) = reference::run_tool(
        &dist,
        "kafka-dump-log.sh",
        &[
            "--files".to_string(),
            segment.to_string_lossy().to_string(),
            "--deep-iteration".to_string(),
            "--print-data-log".to_string(),
        ],
    )
    .expect("kafka-dump-log.sh must be runnable");
    assert!(ok, "kafka-dump-log.sh failed:\n{stdout}\n{stderr}");

    // Kafka reports the codec it found in the attribute bits, batch by batch.
    for codec in CODECS {
        let n = codec_name(*codec);
        assert!(
            stdout.contains(&format!("compresscodec: {n}")),
            "kafka-dump-log.sh did not see a {n} batch:\n{stdout}"
        );
        // --print-data-log decompresses the payload and prints the record values.
        assert!(
            stdout.contains(&format!("payload: {n}-1")),
            "kafka-dump-log.sh could not decompress the {n} batch:\n{stdout}"
        );
    }
    assert!(
        !stdout.contains("isvalid: false"),
        "a compressed batch failed its CRC check:\n{stdout}"
    );
    assert!(
        stdout.contains("baseOffset: 6 lastOffset: 7 count: 2"),
        "the last compressed batch is not what we wrote:\n{stdout}"
    );
}
