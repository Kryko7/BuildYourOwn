//! Fixture strategy `files`: write Kafka's on-disk format before the broker starts.
//!
//! ```text
//! <log.dirs>/
//!   meta.properties
//!   __cluster_metadata-0/
//!     00000000000000000000.log      FeatureLevelRecord, then TopicRecord + PartitionRecords
//!     partition.metadata
//!   <topic>-<partition>/
//!     00000000000000000000.log      RecordBatch v2, CRC-32C, one batch per declared batch
//!     partition.metadata
//!     leader-epoch-checkpoint
//! ```
//!
//! This is the layout the "build your own Kafka" convention hands your broker,
//! and the reason the `files` strategy implies a restart before every test: the broker
//! reads all of it at startup.

use super::{
    cluster_metadata_topic_id, derive_topic_id, unique_name, FixtureHandle, FixtureSpec, TopicInfo,
};
use crate::proto::meta::{uuid_to_kafka_string, FeatureLevelRecord, PartitionRecord, TopicRecord};
use crate::proto::records::{RecordBatch, RecordItem};
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::path::Path;
use uuid::Uuid;

/// `metadata.version` feature level written into the metadata log (Kafka 4.1-IV1).
pub const METADATA_VERSION_LEVEL: i16 = 27;

/// Remove and recreate a log directory, refusing to touch anything that looks dangerous.
pub fn reset_log_dir(log_dir: &Path) -> Result<()> {
    let components = log_dir.components().count();
    if components < 3 {
        bail!(
            "refusing to wipe {} : a log directory must be at least three path components deep",
            log_dir.display()
        );
    }
    if let Some(home) = std::env::var_os("HOME") {
        if log_dir == Path::new(&home) {
            bail!("refusing to wipe the home directory");
        }
    }
    if log_dir.exists() {
        std::fs::remove_dir_all(log_dir)
            .with_context(|| format!("cannot clear {}", log_dir.display()))?;
    }
    std::fs::create_dir_all(log_dir)
        .with_context(|| format!("cannot create {}", log_dir.display()))?;
    Ok(())
}

/// Write every fixture of `spec` into `log_dir`, returning the identities that were used.
pub fn materialize(
    spec: &FixtureSpec,
    salt: &str,
    seed: u64,
    log_dir: &Path,
    broker_id: i32,
    cluster_id: &str,
) -> Result<FixtureHandle> {
    reset_log_dir(log_dir)?;
    let directory_id = derive_topic_id(seed, "directory");
    write_meta_properties(log_dir, broker_id, cluster_id, &directory_id)?;

    let mut handle = FixtureHandle {
        log_dir: log_dir.to_path_buf(),
        broker_id,
        ..Default::default()
    };

    // Batch 0 of the metadata log: the feature level, as a real bootstrap does.
    let mut batches = vec![RecordBatch::of(
        0,
        now_ms(),
        vec![RecordItem::value(
            FeatureLevelRecord {
                name: "metadata.version".to_string(),
                level: METADATA_VERSION_LEVEL,
            }
            .encode(),
        )],
    )];
    let mut next_offset = batches[0].next_offset();

    for topic in &spec.topics {
        let name = unique_name(&topic.key, salt);
        let id = derive_topic_id(seed, &name);
        let mut items = vec![RecordItem::value(
            TopicRecord {
                name: name.clone(),
                id,
            }
            .encode(),
        )];
        for p in 0..topic.partitions {
            items.push(RecordItem::value(
                PartitionRecord::single(p, id, broker_id, directory_id).encode(),
            ));
        }
        let batch = RecordBatch::of(next_offset, now_ms(), items);
        next_offset = batch.next_offset();
        batches.push(batch);

        write_topic_partitions(log_dir, topic, &name, id, broker_id)?;
        handle.insert(TopicInfo {
            key: topic.key.clone(),
            name,
            id,
            partitions: topic.partitions,
            batches: topic.batches.clone(),
        });
    }

    let meta_dir = log_dir.join("__cluster_metadata-0");
    std::fs::create_dir_all(&meta_dir)?;
    let mut segment = Vec::new();
    for b in &batches {
        segment.extend_from_slice(&b.encode());
    }
    std::fs::write(meta_dir.join("00000000000000000000.log"), &segment).with_context(|| {
        format!(
            "cannot write the metadata segment in {}",
            meta_dir.display()
        )
    })?;
    write_partition_metadata(&meta_dir, &cluster_metadata_topic_id())?;
    write_leader_epoch(&meta_dir, broker_id)?;
    Ok(handle)
}

fn write_topic_partitions(
    log_dir: &Path,
    topic: &super::TopicSpec,
    name: &str,
    id: Uuid,
    broker_id: i32,
) -> Result<()> {
    for p in 0..topic.partitions {
        let dir = log_dir.join(format!("{name}-{p}"));
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("cannot create {}", dir.display()))?;
        let mut segment = Vec::new();
        let mut base = 0i64;
        for (index, batch) in topic.batches.get(&p).into_iter().flatten().enumerate() {
            let items: Vec<RecordItem> = batch
                .iter()
                .map(|r| RecordItem {
                    key: r.key.clone(),
                    value: Some(r.value.clone()),
                    ..Default::default()
                })
                .collect();
            // A batch the spec declared compressed is written compressed, codec bits and
            // all: a broker serves the stored bytes untouched, so the fixture must hold
            // exactly what the test expects to read back.
            let b = RecordBatch::of(base, now_ms(), items).with_compression(topic.codec(p, index));
            base = b.next_offset();
            segment.extend_from_slice(&b.encode());
        }
        std::fs::write(dir.join("00000000000000000000.log"), &segment)
            .with_context(|| format!("cannot write the segment in {}", dir.display()))?;
        write_partition_metadata(&dir, &id)?;
        write_leader_epoch(&dir, broker_id)?;
    }
    Ok(())
}

fn write_partition_metadata(dir: &Path, id: &Uuid) -> Result<()> {
    std::fs::write(
        dir.join("partition.metadata"),
        format!("version: 0\ntopic_id: {}", uuid_to_kafka_string(id)),
    )
    .with_context(|| format!("cannot write partition.metadata in {}", dir.display()))?;
    Ok(())
}

fn write_leader_epoch(dir: &Path, broker_id: i32) -> Result<()> {
    let _ = broker_id;
    std::fs::write(dir.join("leader-epoch-checkpoint"), "0\n1\n0 0\n")
        .with_context(|| format!("cannot write leader-epoch-checkpoint in {}", dir.display()))?;
    Ok(())
}

fn write_meta_properties(
    log_dir: &Path,
    broker_id: i32,
    cluster_id: &str,
    directory_id: &Uuid,
) -> Result<()> {
    let text = format!(
        "#\n#kafkatest\nnode.id={broker_id}\ndirectory.id={dir}\nversion=1\ncluster.id={cluster}\n",
        dir = uuid_to_kafka_string(directory_id),
        cluster = cluster_id,
    );
    std::fs::write(log_dir.join("meta.properties"), text)
        .with_context(|| format!("cannot write meta.properties in {}", log_dir.display()))?;
    Ok(())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Read the metadata log back and list the `(topic name, id, partition count)` it declares.
///
/// Used by `tests/fixture_files.rs` to prove the writer round-trips through our own reader.
pub fn read_back_topics(log_dir: &Path) -> Result<BTreeMap<String, (Uuid, usize)>> {
    use crate::proto::meta::{decode_record, MetadataRecord};
    let segment = std::fs::read(log_dir.join("__cluster_metadata-0/00000000000000000000.log"))
        .context("cannot read the metadata segment")?;
    let batches = RecordBatch::decode_all(&segment)?;
    let mut by_id: BTreeMap<Uuid, (String, usize)> = BTreeMap::new();
    for b in &batches {
        for r in &b.records {
            let Some(value) = &r.value else { continue };
            match decode_record(value)? {
                MetadataRecord::Topic(t) => {
                    by_id.entry(t.id).or_insert((t.name, 0));
                }
                MetadataRecord::Partition(p) => {
                    by_id.entry(p.topic_id).or_insert((String::new(), 0)).1 += 1;
                }
                _ => {}
            }
        }
    }
    Ok(by_id
        .into_iter()
        .map(|(id, (name, parts))| (name, (id, parts)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{rec, TopicSpec};

    #[test]
    fn writes_a_readable_tree() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log_dir = dir.path().join("kraft-combined-logs");
        let spec = FixtureSpec::with(
            TopicSpec::new("t1", 2)
                .with_batch(0, vec![rec("hello"), rec("world")])
                .with_batch(0, vec![rec("again")]),
        )
        .and(TopicSpec::new("t2", 1));
        let h = materialize(&spec, "s01-1", 42, &log_dir, 1, "abcdefghijklmnopqrstuv")
            .expect("materialize");

        let t1 = h.topic("t1").expect("t1");
        assert_eq!(t1.partitions, 2);
        assert_eq!(t1.high_watermark(0), 3);
        assert!(log_dir.join("meta.properties").is_file());
        assert!(log_dir
            .join(format!("{}-1", t1.name))
            .join("partition.metadata")
            .is_file());

        let topics = read_back_topics(&log_dir).expect("read back");
        assert_eq!(topics.len(), 2);
        let (id, parts) = topics[&t1.name];
        assert_eq!(id, t1.id);
        assert_eq!(parts, 2);

        let bytes = h.segment_bytes(&t1.name, 0).expect("segment");
        let batches = RecordBatch::decode_all(&bytes).expect("decode segment");
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].base_offset, 0);
        assert_eq!(batches[1].base_offset, 2);
        assert_eq!(batches[0].records[1].value.as_deref(), Some(&b"world"[..]));
        assert!(h.segment_bytes(&t1.name, 1).expect("p1").is_empty());
    }

    #[test]
    fn refuses_to_wipe_a_shallow_directory() {
        assert!(reset_log_dir(Path::new("/tmp")).is_err());
        assert!(reset_log_dir(Path::new("/")).is_err());
    }

    #[test]
    fn partition_metadata_uses_kafkas_base64() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_partition_metadata(dir.path(), &cluster_metadata_topic_id()).expect("write");
        let text = std::fs::read_to_string(dir.path().join("partition.metadata")).expect("read");
        assert_eq!(text, "version: 0\ntopic_id: AAAAAAAAAAAAAAAAAAAAAQ");
    }
}
