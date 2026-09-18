//! Cluster-metadata record codecs (`__cluster_metadata-0`).
//!
//! `kafka-protocol` 0.18 generates the *client* APIs but not the controller's metadata
//! records (`TopicRecord`, `PartitionRecord`, `FeatureLevelRecord`), so those are encoded
//! here by hand. Each record's **value** is framed as
//!
//! ```text
//! frameVersion int8 = 1
//! type         uvarint   MetadataRecordType id (2 = topic, 3 = partition, 12 = feature level)
//! version      uvarint   record schema version
//! payload      the record in Kafka's flexible encoding (compact strings/arrays, tagged fields)
//! ```
//!
//! The schemas are those of Apache Kafka 4.1 (verified against a real
//! `__cluster_metadata-0` segment produced by the reference broker):
//!
//! ```text
//! TopicRecord        v0 {name: COMPACT_STRING, topic_id: UUID, _tagged_fields}
//! PartitionRecord    v1 {partition_id: INT32, topic_id: UUID, replicas/isr/removing/adding:
//!                        COMPACT_ARRAY(INT32), leader: INT32, leader_epoch: INT32,
//!                        partition_epoch: INT32, directories: COMPACT_ARRAY(UUID), _tagged_fields}
//! FeatureLevelRecord v0 {name: COMPACT_STRING, feature_level: INT16, _tagged_fields}
//! ```

use anyhow::{bail, Context, Result};
use uuid::Uuid;

use super::records::Cursor;

/// `MetadataRecordType` ids used by the fixtures.
pub mod record_type {
    /// `TopicRecord`
    pub const TOPIC: u32 = 2;
    /// `PartitionRecord`
    pub const PARTITION: u32 = 3;
    /// `FeatureLevelRecord`
    pub const FEATURE_LEVEL: u32 = 12;
}

/// The frame version every metadata record value starts with.
pub const FRAME_VERSION: u8 = 1;

/// Kafka's textual form of a topic id: URL-safe base64 of the 16 raw bytes, unpadded.
pub fn uuid_to_kafka_string(id: &Uuid) -> String {
    base64_url_nopad(id.as_bytes())
}

fn base64_url_nopad(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let chars = [
            ALPHABET[(n >> 18) as usize & 0x3f],
            ALPHABET[(n >> 12) as usize & 0x3f],
            ALPHABET[(n >> 6) as usize & 0x3f],
            ALPHABET[n as usize & 0x3f],
        ];
        let keep = match chunk.len() {
            1 => 2,
            2 => 3,
            _ => 4,
        };
        for &c in chars.iter().take(keep) {
            out.push(char::from(c));
        }
    }
    out
}

/// A byte-oriented writer for Kafka's flexible (compact) encoding.
#[derive(Default)]
pub struct Enc {
    /// The bytes written so far.
    pub buf: Vec<u8>,
}

impl Enc {
    /// A fresh, empty encoder.
    pub fn new() -> Self {
        Enc::default()
    }

    /// Append a single byte.
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }

    /// Append a big-endian i16.
    pub fn i16(&mut self, v: i16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    /// Append a big-endian i32.
    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    /// Append an unsigned LEB128 varint.
    pub fn uvarint(&mut self, mut v: u32) -> &mut Self {
        while v >= 0x80 {
            self.buf.push((v as u8) | 0x80);
            v >>= 7;
        }
        self.buf.push(v as u8);
        self
    }

    /// Append a compact (non-nullable) string: `uvarint(len + 1)` then the bytes.
    pub fn compact_string(&mut self, s: &str) -> &mut Self {
        self.uvarint(s.len() as u32 + 1);
        self.buf.extend_from_slice(s.as_bytes());
        self
    }

    /// Append a 16-byte UUID.
    pub fn uuid(&mut self, id: &Uuid) -> &mut Self {
        self.buf.extend_from_slice(id.as_bytes());
        self
    }

    /// Append a compact array of i32.
    pub fn compact_i32_array(&mut self, items: &[i32]) -> &mut Self {
        self.uvarint(items.len() as u32 + 1);
        for i in items {
            self.i32(*i);
        }
        self
    }

    /// Append a compact array of UUIDs.
    pub fn compact_uuid_array(&mut self, items: &[Uuid]) -> &mut Self {
        self.uvarint(items.len() as u32 + 1);
        for i in items {
            self.uuid(i);
        }
        self
    }

    /// Append an empty tagged-field section.
    pub fn empty_tagged_fields(&mut self) -> &mut Self {
        self.uvarint(0)
    }

    /// Finish, returning the bytes.
    pub fn done(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.buf)
    }
}

fn frame(enc: &mut Enc, type_id: u32, version: u32) {
    enc.u8(FRAME_VERSION);
    enc.uvarint(type_id);
    enc.uvarint(version);
}

/// A decoded/decodable `FeatureLevelRecord`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureLevelRecord {
    /// Feature name, e.g. `metadata.version`.
    pub name: String,
    /// The level the feature is set to.
    pub level: i16,
}

impl FeatureLevelRecord {
    /// Encode the record value (framing included).
    pub fn encode(&self) -> Vec<u8> {
        let mut e = Enc::new();
        frame(&mut e, record_type::FEATURE_LEVEL, 0);
        e.compact_string(&self.name);
        e.i16(self.level);
        e.empty_tagged_fields();
        e.done()
    }
}

/// A decoded/decodable `TopicRecord`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicRecord {
    /// Topic name.
    pub name: String,
    /// Topic id.
    pub id: Uuid,
}

impl TopicRecord {
    /// Encode the record value (framing included).
    pub fn encode(&self) -> Vec<u8> {
        let mut e = Enc::new();
        frame(&mut e, record_type::TOPIC, 0);
        e.compact_string(&self.name);
        e.uuid(&self.id);
        e.empty_tagged_fields();
        e.done()
    }
}

/// A decoded/decodable `PartitionRecord` (schema version 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionRecord {
    /// Partition index.
    pub partition_id: i32,
    /// The topic this partition belongs to.
    pub topic_id: Uuid,
    /// Replica broker ids.
    pub replicas: Vec<i32>,
    /// In-sync replica broker ids.
    pub isr: Vec<i32>,
    /// Leader broker id.
    pub leader: i32,
    /// Leader epoch.
    pub leader_epoch: i32,
    /// Partition epoch.
    pub partition_epoch: i32,
    /// Log directory ids, one per replica (schema v1+).
    pub directories: Vec<Uuid>,
}

impl PartitionRecord {
    /// A single-replica partition led by `broker`.
    pub fn single(partition_id: i32, topic_id: Uuid, broker: i32, dir: Uuid) -> Self {
        PartitionRecord {
            partition_id,
            topic_id,
            replicas: vec![broker],
            isr: vec![broker],
            leader: broker,
            leader_epoch: 0,
            partition_epoch: 0,
            directories: vec![dir],
        }
    }

    /// Encode the record value (framing included), schema version 1.
    pub fn encode(&self) -> Vec<u8> {
        let mut e = Enc::new();
        frame(&mut e, record_type::PARTITION, 1);
        e.i32(self.partition_id);
        e.uuid(&self.topic_id);
        e.compact_i32_array(&self.replicas);
        e.compact_i32_array(&self.isr);
        e.compact_i32_array(&[]); // removing_replicas
        e.compact_i32_array(&[]); // adding_replicas
        e.i32(self.leader);
        e.i32(self.leader_epoch);
        e.i32(self.partition_epoch);
        e.compact_uuid_array(&self.directories);
        e.empty_tagged_fields();
        e.done()
    }
}

/// Any metadata record the fixtures write, decoded back from a segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataRecord {
    /// A `FeatureLevelRecord`.
    FeatureLevel(FeatureLevelRecord),
    /// A `TopicRecord`.
    Topic(TopicRecord),
    /// A `PartitionRecord`.
    Partition(PartitionRecord),
    /// A record type the harness does not decode.
    Other {
        /// The `MetadataRecordType` id.
        type_id: u32,
        /// The schema version.
        version: u32,
    },
}

fn compact_string(c: &mut Cursor<'_>) -> Result<String> {
    let n = c.uvarlong()?;
    if n == 0 {
        bail!("compact string is null where a value is required");
    }
    let bytes = c.take((n - 1) as usize)?;
    String::from_utf8(bytes.to_vec()).context("compact string is not utf-8")
}

fn read_uuid(c: &mut Cursor<'_>) -> Result<Uuid> {
    let b = c.take(16)?;
    let mut a = [0u8; 16];
    a.copy_from_slice(b);
    Ok(Uuid::from_bytes(a))
}

fn compact_i32_array(c: &mut Cursor<'_>) -> Result<Vec<i32>> {
    let n = c.uvarlong()?;
    if n == 0 {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for _ in 0..(n - 1) {
        out.push(c.i32()?);
    }
    Ok(out)
}

fn compact_uuid_array(c: &mut Cursor<'_>) -> Result<Vec<Uuid>> {
    let n = c.uvarlong()?;
    if n == 0 {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for _ in 0..(n - 1) {
        out.push(read_uuid(c)?);
    }
    Ok(out)
}

/// Decode a metadata record value (framing included).
pub fn decode_record(value: &[u8]) -> Result<MetadataRecord> {
    let mut c = Cursor::new(value);
    let frame_version = c.i8().context("metadata record frame version")?;
    if frame_version != FRAME_VERSION as i8 {
        bail!("unexpected metadata frame version {frame_version}, want {FRAME_VERSION}");
    }
    let type_id = c.uvarlong().context("metadata record type")? as u32;
    let version = c.uvarlong().context("metadata record version")? as u32;
    match type_id {
        record_type::FEATURE_LEVEL => Ok(MetadataRecord::FeatureLevel(FeatureLevelRecord {
            name: compact_string(&mut c).context("feature_level.name")?,
            level: c.i16().context("feature_level.level")?,
        })),
        record_type::TOPIC => Ok(MetadataRecord::Topic(TopicRecord {
            name: compact_string(&mut c).context("topic.name")?,
            id: read_uuid(&mut c).context("topic.id")?,
        })),
        record_type::PARTITION => {
            let partition_id = c.i32().context("partition.partition_id")?;
            let topic_id = read_uuid(&mut c).context("partition.topic_id")?;
            let replicas = compact_i32_array(&mut c).context("partition.replicas")?;
            let isr = compact_i32_array(&mut c).context("partition.isr")?;
            let _removing = compact_i32_array(&mut c).context("partition.removing_replicas")?;
            let _adding = compact_i32_array(&mut c).context("partition.adding_replicas")?;
            let leader = c.i32().context("partition.leader")?;
            let leader_epoch = c.i32().context("partition.leader_epoch")?;
            let partition_epoch = c.i32().context("partition.partition_epoch")?;
            let directories = if version >= 1 {
                compact_uuid_array(&mut c).context("partition.directories")?
            } else {
                Vec::new()
            };
            Ok(MetadataRecord::Partition(PartitionRecord {
                partition_id,
                topic_id,
                replicas,
                isr,
                leader,
                leader_epoch,
                partition_epoch,
                directories,
            }))
        }
        _ => Ok(MetadataRecord::Other { type_id, version }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_level_matches_a_real_segment() {
        // Bytes lifted from a __cluster_metadata-0 segment written by Apache Kafka 4.1.2:
        // {"type":"FEATURE_LEVEL_RECORD","version":0,"data":{"name":"metadata.version",
        //  "featureLevel":27}}
        let expected: Vec<u8> = vec![
            0x01, 0x0c, 0x00, 0x11, b'm', b'e', b't', b'a', b'd', b'a', b't', b'a', b'.', b'v',
            b'e', b'r', b's', b'i', b'o', b'n', 0x00, 0x1b, 0x00,
        ];
        let got = FeatureLevelRecord {
            name: "metadata.version".to_string(),
            level: 27,
        }
        .encode();
        assert_eq!(got, expected);
        assert_eq!(got.len(), 23, "value size reported by kafka-dump-log.sh");
    }

    #[test]
    fn topic_record_matches_a_real_segment() {
        // {"type":"TOPIC_RECORD","version":0,"data":{"name":"foo",
        //  "topicId":"_uaYz7wTRr2Ogdc1lPtl8A"}}
        let id = Uuid::from_bytes([
            0xfe, 0xe6, 0x98, 0xcf, 0xbc, 0x13, 0x46, 0xbd, 0x8e, 0x81, 0xd7, 0x35, 0x94, 0xfb,
            0x65, 0xf0,
        ]);
        let expected: Vec<u8> = {
            let mut v = vec![0x01, 0x02, 0x00, 0x04, b'f', b'o', b'o'];
            v.extend_from_slice(id.as_bytes());
            v.push(0x00);
            v
        };
        let rec = TopicRecord {
            name: "foo".to_string(),
            id,
        };
        assert_eq!(rec.encode(), expected);
        assert_eq!(rec.encode().len(), 24, "value size from kafka-dump-log.sh");
        assert_eq!(uuid_to_kafka_string(&id), "_uaYz7wTRr2Ogdc1lPtl8A");
    }

    #[test]
    fn partition_record_round_trips() {
        let topic_id = Uuid::from_u128(0x1234_5678_9abc_def0_1122_3344_5566_7788);
        let dir = Uuid::from_u128(42);
        let rec = PartitionRecord::single(3, topic_id, 1, dir);
        let bytes = rec.encode();
        match decode_record(&bytes).expect("decode") {
            MetadataRecord::Partition(p) => assert_eq!(p, rec),
            other => panic!("wrong record kind: {other:?}"),
        }
        // v1 payload size: frame(3) + id(4) + uuid(16) + 4 compact arrays (2+4, 2+4, 1, 1)
        // + leader/epoch/partition_epoch(12) + directories(1 + 16) + tagged(1) = 65
        assert_eq!(
            bytes.len(),
            65,
            "matches the 65-byte PARTITION_RECORD Kafka writes"
        );
    }

    #[test]
    fn all_kinds_round_trip() {
        let f = FeatureLevelRecord {
            name: "metadata.version".into(),
            level: 27,
        };
        assert_eq!(
            decode_record(&f.encode()).expect("feature"),
            MetadataRecord::FeatureLevel(f)
        );
        let t = TopicRecord {
            name: "bar".into(),
            id: Uuid::from_u128(7),
        };
        assert_eq!(
            decode_record(&t.encode()).expect("topic"),
            MetadataRecord::Topic(t)
        );
    }

    #[test]
    fn base64_matches_kafka() {
        assert_eq!(uuid_to_kafka_string(&Uuid::nil()), "A".repeat(22));
        assert_eq!(
            uuid_to_kafka_string(&Uuid::from_u128(1)),
            "AAAAAAAAAAAAAAAAAAAAAQ"
        );
    }
}
