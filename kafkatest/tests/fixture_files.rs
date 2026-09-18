//! The `files` fixture strategy must produce bytes Kafka itself accepts.
//!
//! These tests round-trip the writer through our own reader and, when the reference
//! distribution is in `~/.cache/kafkatest`, through `kafka-dump-log.sh` from the tarball.
//! They skip (rather than fail) when the tarball has not been downloaded yet.

use kafkatest::broker::reference;
use kafkatest::fixtures::files::{materialize, read_back_topics};
use kafkatest::fixtures::{keyed, rec, FixtureSpec, TopicSpec};
use kafkatest::proto::records::RecordBatch;
use std::path::{Path, PathBuf};

fn spec() -> FixtureSpec {
    FixtureSpec::with(
        TopicSpec::new("orders", 2)
            .with_batch(0, vec![rec("first"), keyed("k", "second")])
            .with_batch(0, vec![rec("third")])
            .with_batch(1, vec![rec("other-partition")]),
    )
    .and(TopicSpec::new("empty", 1))
}

fn dist() -> Option<PathBuf> {
    let d = reference::dist_dir(reference::DEFAULT_VERSION);
    d.join("bin/kafka-dump-log.sh").is_file().then_some(d)
}

fn write_fixtures(dir: &Path) -> kafkatest::fixtures::FixtureHandle {
    let log_dir = dir.join("kraft-combined-logs");
    materialize(
        &spec(),
        "it-01",
        1234,
        &log_dir,
        1,
        "AAAAAAAAAAAAAAAAAAAAAQ",
    )
    .expect("the fixture writer must succeed")
}

#[test]
fn writer_round_trips_through_our_own_reader() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let handle = write_fixtures(tmp.path());
    let orders = handle.topic("orders").expect("orders");

    let p0 = handle.segment_bytes(&orders.name, 0).expect("partition 0");
    let batches = RecordBatch::decode_all(&p0).expect("partition 0 decodes");
    assert_eq!(batches.len(), 2, "one batch per declared batch");
    assert_eq!(batches[0].base_offset, 0);
    assert_eq!(batches[1].base_offset, 2, "offsets continue across batches");
    assert_eq!(batches[0].records[1].key.as_deref(), Some(&b"k"[..]));
    assert_eq!(batches[1].records[0].value.as_deref(), Some(&b"third"[..]));

    let p1 = handle.segment_bytes(&orders.name, 1).expect("partition 1");
    assert_eq!(RecordBatch::decode_all(&p1).expect("partition 1").len(), 1);

    let empty = handle.topic("empty").expect("empty");
    assert!(
        handle
            .segment_bytes(&empty.name, 0)
            .expect("empty segment")
            .is_empty(),
        "a topic with no records gets a zero-length segment"
    );
}

#[test]
fn metadata_log_declares_every_topic_and_partition() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let handle = write_fixtures(tmp.path());
    let log_dir = tmp.path().join("kraft-combined-logs");
    let topics = read_back_topics(&log_dir).expect("metadata log decodes");
    assert_eq!(topics.len(), 2);

    let orders = handle.topic("orders").expect("orders");
    let (id, partitions) = topics[&orders.name];
    assert_eq!(id, orders.id, "the id in the log is the id the test sees");
    assert_eq!(partitions, 2);

    assert!(log_dir.join("meta.properties").is_file());
    assert!(log_dir
        .join("__cluster_metadata-0/partition.metadata")
        .is_file());
}

#[test]
fn kafka_dump_log_accepts_the_partition_segments() {
    let Some(dist) = dist() else {
        eprintln!(
            "skipping: {} is not downloaded",
            reference::dist_dir(reference::DEFAULT_VERSION).display()
        );
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let handle = write_fixtures(tmp.path());
    let orders = handle.topic("orders").expect("orders");
    let segment = handle.segment_path(&orders.name, 0);

    let (stdout, stderr, ok) = reference::run_tool(
        &dist,
        "kafka-dump-log.sh",
        &[
            "--files".to_string(),
            segment.to_string_lossy().to_string(),
            "--deep-iteration".to_string(),
        ],
    )
    .expect("kafka-dump-log.sh must be runnable");
    assert!(ok, "kafka-dump-log.sh failed:\n{stdout}\n{stderr}");
    assert!(
        stdout.contains("isvalid: true"),
        "kafka-dump-log.sh did not validate the CRC:\n{stdout}"
    );
    assert!(
        stdout.contains("baseOffset: 0 lastOffset: 1 count: 2"),
        "the first batch is not what we wrote:\n{stdout}"
    );
    assert!(
        stdout.contains("baseOffset: 2 lastOffset: 2 count: 1"),
        "the second batch is not what we wrote:\n{stdout}"
    );
    assert!(stdout.contains("magic: 2"), "not a v2 batch:\n{stdout}");
}

#[test]
fn kafka_dump_log_decodes_the_cluster_metadata_log() {
    let Some(dist) = dist() else {
        eprintln!("skipping: the reference distribution is not downloaded");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let handle = write_fixtures(tmp.path());
    let orders = handle.topic("orders").expect("orders");
    let segment = tmp
        .path()
        .join("kraft-combined-logs/__cluster_metadata-0/00000000000000000000.log");

    let (stdout, stderr, ok) = reference::run_tool(
        &dist,
        "kafka-dump-log.sh",
        &[
            "--cluster-metadata-decoder".to_string(),
            "--files".to_string(),
            segment.to_string_lossy().to_string(),
        ],
    )
    .expect("kafka-dump-log.sh must be runnable");
    assert!(ok, "kafka-dump-log.sh failed:\n{stdout}\n{stderr}");
    for want in [
        "FEATURE_LEVEL_RECORD",
        "TOPIC_RECORD",
        "PARTITION_RECORD",
        &format!("\"name\":\"{}\"", orders.name),
    ] {
        assert!(
            stdout.contains(want),
            "the metadata log does not mention {want}:\n{stdout}"
        );
    }
    assert!(
        stdout.contains("isvalid: true"),
        "the metadata batches did not validate:\n{stdout}"
    );
}
