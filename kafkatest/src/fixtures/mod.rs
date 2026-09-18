//! Topics and records a test needs, and the two ways of putting them in place.
//!
//! A test declares *what* it needs ([`FixtureSpec`]) using logical keys (`"t1"`), and gets
//! back a [`FixtureHandle`] with the real topic names, topic ids and offsets. Tests never
//! hardcode an id: the `files` strategy invents them, the `api` strategy discovers them
//! from `Metadata`, and both look the same from the test's side.

pub mod api;
pub mod files;

use crate::assert::Failure;
use std::collections::BTreeMap;
use std::path::PathBuf;
use uuid::Uuid;

/// One record in a fixture.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecordSpec {
    /// Record key, `None` for a null key.
    pub key: Option<Vec<u8>>,
    /// Record value.
    pub value: Vec<u8>,
}

/// A record with a value and no key.
pub fn rec(value: impl Into<Vec<u8>>) -> RecordSpec {
    RecordSpec {
        key: None,
        value: value.into(),
    }
}

/// A record with both a key and a value.
pub fn keyed(key: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>) -> RecordSpec {
    RecordSpec {
        key: Some(key.into()),
        value: value.into(),
    }
}

/// A topic a test wants to exist before it runs.
#[derive(Debug, Clone)]
pub struct TopicSpec {
    /// Logical key the test uses to look the topic up.
    pub key: String,
    /// Number of partitions.
    pub partitions: i32,
    /// partition index → the batches to write, in order.
    pub batches: BTreeMap<i32, Vec<Vec<RecordSpec>>>,
    /// `(partition, batch index)` → the compression codec that batch must be stored with.
    /// Absent means codec 0, "stored as it is".
    pub batch_codecs: BTreeMap<(i32, usize), i16>,
}

impl TopicSpec {
    /// An empty topic with `partitions` partitions.
    pub fn new(key: &str, partitions: i32) -> TopicSpec {
        TopicSpec {
            key: key.to_string(),
            partitions,
            batches: BTreeMap::new(),
            batch_codecs: BTreeMap::new(),
        }
    }

    /// Append one batch of records to a partition.
    pub fn with_batch(mut self, partition: i32, records: Vec<RecordSpec>) -> TopicSpec {
        self.batches.entry(partition).or_default().push(records);
        self
    }

    /// Append one batch that must be stored compressed.
    ///
    /// `codec` is the value of attribute bits 0-2: 1 gzip, 2 snappy, 3 lz4, 4 zstd. Both
    /// fixture strategies honour it — `files` writes a compressed batch into the segment,
    /// `api` produces one, which a broker with `compression.type=producer` stores as it is.
    pub fn with_compressed_batch(
        mut self,
        partition: i32,
        codec: i16,
        records: Vec<RecordSpec>,
    ) -> TopicSpec {
        let entry = self.batches.entry(partition).or_default();
        entry.push(records);
        let index = entry.len() - 1;
        self.batch_codecs.insert((partition, index), codec);
        self
    }

    /// The compression codec declared for one batch; 0 when it was not declared.
    pub fn codec(&self, partition: i32, index: usize) -> i16 {
        self.batch_codecs
            .get(&(partition, index))
            .copied()
            .unwrap_or(0)
    }

    /// Append one single-record batch per value.
    pub fn with_values(mut self, partition: i32, values: &[&str]) -> TopicSpec {
        for v in values {
            self.batches
                .entry(partition)
                .or_default()
                .push(vec![rec(*v)]);
        }
        self
    }
}

/// The complete set of fixtures for one test.
#[derive(Debug, Clone, Default)]
pub struct FixtureSpec {
    /// Topics to create.
    pub topics: Vec<TopicSpec>,
}

impl FixtureSpec {
    /// No fixtures at all.
    pub fn none() -> FixtureSpec {
        FixtureSpec::default()
    }

    /// Start a spec with one topic.
    pub fn with(topic: TopicSpec) -> FixtureSpec {
        FixtureSpec {
            topics: vec![topic],
        }
    }

    /// Add another topic.
    pub fn and(mut self, topic: TopicSpec) -> FixtureSpec {
        self.topics.push(topic);
        self
    }

    /// True when there is nothing to create.
    pub fn is_empty(&self) -> bool {
        self.topics.is_empty()
    }
}

/// A topic that now exists, with the identity the test must use.
#[derive(Debug, Clone)]
pub struct TopicInfo {
    /// The logical key from the spec.
    pub key: String,
    /// The real topic name (unique per test run).
    pub name: String,
    /// The real topic id.
    pub id: Uuid,
    /// Number of partitions.
    pub partitions: i32,
    /// partition index → batches of records, exactly as written.
    pub batches: BTreeMap<i32, Vec<Vec<RecordSpec>>>,
}

impl TopicInfo {
    /// Every record of a partition, flattened, in offset order.
    pub fn records(&self, partition: i32) -> Vec<RecordSpec> {
        self.batches
            .get(&partition)
            .map(|b| b.iter().flatten().cloned().collect())
            .unwrap_or_default()
    }

    /// The high watermark a correct broker reports for a partition.
    pub fn high_watermark(&self, partition: i32) -> i64 {
        self.records(partition).len() as i64
    }
}

/// The result of materializing a [`FixtureSpec`].
#[derive(Debug, Clone, Default)]
pub struct FixtureHandle {
    /// Topics by logical key; use [`FixtureHandle::topic`] rather than reaching in here.
    pub topics: BTreeMap<String, TopicInfo>,
    /// The broker's log directory, so tests can read the segments back off disk.
    pub log_dir: PathBuf,
    /// The broker id that leads every fixture partition.
    pub broker_id: i32,
}

impl FixtureHandle {
    /// Look a topic up by its logical key.
    pub fn topic(&self, key: &str) -> Result<&TopicInfo, Failure> {
        self.topics.get(key).ok_or_else(|| {
            Failure::harness(format!(
                "the test asked for fixture topic '{key}', which it never declared"
            ))
        })
    }

    /// Every topic, in key order.
    pub fn all(&self) -> impl Iterator<Item = &TopicInfo> {
        self.topics.values()
    }

    /// Path of the first log segment of a partition.
    pub fn segment_path(&self, topic: &str, partition: i32) -> PathBuf {
        self.log_dir
            .join(format!("{topic}-{partition}"))
            .join("00000000000000000000.log")
    }

    /// The raw bytes of a partition's first segment, as the broker has them on disk.
    pub fn segment_bytes(&self, topic: &str, partition: i32) -> Result<Vec<u8>, Failure> {
        let p = self.segment_path(topic, partition);
        std::fs::read(&p).map_err(|e| {
            Failure::harness(format!("cannot read the log segment {}: {e}", p.display()))
        })
    }

    /// Record a materialized topic.
    pub fn insert(&mut self, info: TopicInfo) {
        self.topics.insert(info.key.clone(), info);
    }
}

/// Derive a stable, unique topic name for a test.
pub fn unique_name(key: &str, salt: &str) -> String {
    let cleaned: String = key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("{cleaned}-{salt}")
}

/// Derive a deterministic, non-reserved topic id from the seed and the topic name.
pub fn derive_topic_id(seed: u64, name: &str) -> Uuid {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(seed.to_be_bytes());
    h.update(name.as_bytes());
    let digest = h.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // Shape it like a v4 UUID, which is what Kafka's Uuid.randomUuid() produces, and keep
    // it away from the reserved zero/metadata ids.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    if bytes.iter().all(|b| *b == 0) {
        bytes[15] = 2;
    }
    Uuid::from_bytes(bytes)
}

/// The id Kafka reserves for the `__cluster_metadata` topic.
pub fn cluster_metadata_topic_id() -> Uuid {
    Uuid::from_u128(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_specs_collect_batches_per_partition() {
        let t = TopicSpec::new("t1", 2)
            .with_batch(0, vec![rec("a"), rec("b")])
            .with_batch(0, vec![rec("c")])
            .with_values(1, &["x", "y"]);
        assert_eq!(t.batches[&0].len(), 2);
        assert_eq!(t.batches[&1].len(), 2);
        assert_eq!(t.batches[&1][0], vec![rec("x")]);
    }

    #[test]
    fn compressed_batches_remember_their_codec() {
        let t = TopicSpec::new("t1", 1)
            .with_batch(0, vec![rec("plain")])
            .with_compressed_batch(0, 4, vec![rec("zstd")]);
        assert_eq!(t.batches[&0].len(), 2);
        assert_eq!(t.codec(0, 0), 0, "an ordinary batch is uncompressed");
        assert_eq!(t.codec(0, 1), 4);
        assert_eq!(t.codec(1, 0), 0, "an undeclared partition is uncompressed");
    }

    #[test]
    fn topic_info_flattens_offsets_and_watermarks() {
        let mut batches = BTreeMap::new();
        batches.insert(0, vec![vec![rec("a"), rec("b")], vec![rec("c")]]);
        let info = TopicInfo {
            key: "t1".into(),
            name: "t1-abc".into(),
            id: Uuid::from_u128(9),
            partitions: 1,
            batches,
        };
        assert_eq!(info.records(0).len(), 3);
        assert_eq!(info.high_watermark(0), 3);
        assert_eq!(info.high_watermark(1), 0);
    }

    #[test]
    fn derived_ids_are_stable_and_distinct() {
        let a = derive_topic_id(7, "foo");
        assert_eq!(a, derive_topic_id(7, "foo"));
        assert_ne!(a, derive_topic_id(8, "foo"));
        assert_ne!(a, derive_topic_id(7, "bar"));
        assert_ne!(a, Uuid::nil());
        assert_ne!(a, cluster_metadata_topic_id());
        assert_eq!(a.get_version_num(), 4);
    }

    #[test]
    fn missing_fixtures_fail_loudly() {
        let h = FixtureHandle::default();
        let err = h.topic("nope").expect_err("must fail");
        assert!(format!("{:?}", err.messages).contains("never declared"));
    }

    #[test]
    fn names_are_valid_kafka_topic_names() {
        let n = unique_name("t1", "s1-04-2");
        assert!(n
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_'));
    }
}
