//! Shared building blocks for stage tests: API keys, error codes, request builders and
//! raw-frame helpers.

use crate::assert::{Check, Failure, FailureKind};
use crate::fixtures::TopicInfo;
use crate::proto::Conn;
use crate::stages::Ctx;
use bytes::Bytes;
use kafka_protocol::messages::describe_topic_partitions_request::TopicRequest;
use kafka_protocol::messages::fetch_request::{FetchPartition, FetchTopic};
use kafka_protocol::messages::produce_request::{PartitionProduceData, TopicProduceData};
use kafka_protocol::messages::{
    ApiVersionsRequest, DescribeTopicPartitionsRequest, FetchRequest, ProduceRequest, TopicName,
};
use kafka_protocol::protocol::StrBytes;
use std::time::Duration;
use uuid::Uuid;

/// `ApiVersions`
pub const API_VERSIONS_KEY: i16 = 18;
/// `Produce`
pub const PRODUCE_KEY: i16 = 0;
/// `Fetch`
pub const FETCH_KEY: i16 = 1;
/// `Metadata`
pub const METADATA_KEY: i16 = 3;
/// `DescribeTopicPartitions`
pub const DESCRIBE_TOPIC_PARTITIONS_KEY: i16 = 75;
/// `InitProducerId`
pub const INIT_PRODUCER_ID_KEY: i16 = 22;

/// No error.
pub const NONE: i16 = 0;
/// `UNKNOWN_TOPIC_OR_PARTITION`
pub const UNKNOWN_TOPIC_OR_PARTITION: i16 = 3;
/// `UNSUPPORTED_VERSION`
pub const UNSUPPORTED_VERSION: i16 = 35;
/// `UNKNOWN_TOPIC_ID`
pub const UNKNOWN_TOPIC_ID: i16 = 100;
/// `CORRUPT_MESSAGE`
pub const CORRUPT_MESSAGE: i16 = 2;
/// `MESSAGE_TOO_LARGE`
pub const MESSAGE_TOO_LARGE: i16 = 10;
/// `INVALID_REQUIRED_ACKS`
pub const INVALID_REQUIRED_ACKS: i16 = 21;
/// `OUT_OF_ORDER_SEQUENCE_NUMBER`
pub const OUT_OF_ORDER_SEQUENCE_NUMBER: i16 = 45;
/// `DUPLICATE_SEQUENCE_NUMBER`
pub const DUPLICATE_SEQUENCE_NUMBER: i16 = 46;
/// `INVALID_PRODUCER_EPOCH`
pub const INVALID_PRODUCER_EPOCH: i16 = 47;
/// `INVALID_RECORD`
pub const INVALID_RECORD: i16 = 87;

/// The `ApiVersions` version the suite speaks.
pub const API_VERSIONS_V4: i16 = 4;
/// The `DescribeTopicPartitions` version the suite speaks.
pub const DESCRIBE_V0: i16 = 0;
/// The `Fetch` version the suite speaks.
pub const FETCH_V16: i16 = 16;
/// The `Produce` version the suite speaks.
pub const PRODUCE_V11: i16 = 11;

/// A `TopicName` from a `&str`.
pub fn topic_name(s: &str) -> TopicName {
    TopicName(StrBytes::from_string(s.to_string()))
}

/// An `ApiVersions` request that names this client, as a real client does.
pub fn api_versions_request() -> ApiVersionsRequest {
    let mut r = ApiVersionsRequest::default();
    r.client_software_name = StrBytes::from_static_str("kafkatest");
    r.client_software_version = StrBytes::from_static_str("0.1.0");
    r
}

/// A `DescribeTopicPartitions` v0 request for the given topic names.
pub fn describe_request(
    names: &[&str],
    response_partition_limit: i32,
) -> DescribeTopicPartitionsRequest {
    let mut r = DescribeTopicPartitionsRequest::default();
    r.response_partition_limit = response_partition_limit;
    r.topics = names
        .iter()
        .map(|n| {
            let mut t = TopicRequest::default();
            t.name = topic_name(n);
            t
        })
        .collect();
    r
}

/// A `Fetch` v16 request for `(topic id, partition, fetch offset)` triples.
pub fn fetch_request(parts: &[(Uuid, i32, i64)], max_wait_ms: i32) -> FetchRequest {
    let mut r = FetchRequest::default();
    r.max_wait_ms = max_wait_ms;
    r.min_bytes = 1;
    r.max_bytes = 10 * 1024 * 1024;
    r.session_id = 0;
    r.session_epoch = 0;
    r.replica_id = kafka_protocol::messages::BrokerId(-1);
    let mut by_topic: Vec<(Uuid, Vec<FetchPartition>)> = Vec::new();
    for (id, partition, offset) in parts {
        let mut p = FetchPartition::default();
        p.partition = *partition;
        p.fetch_offset = *offset;
        p.current_leader_epoch = -1;
        p.last_fetched_epoch = -1;
        p.log_start_offset = -1;
        p.partition_max_bytes = 1024 * 1024;
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

/// A `Produce` v11 request carrying one already-encoded record batch per entry.
pub fn produce_request(batches: &[(String, i32, Vec<u8>)], acks: i16) -> ProduceRequest {
    let mut r = ProduceRequest::default();
    r.acks = acks;
    r.timeout_ms = 5_000;
    let mut by_topic: Vec<(String, Vec<PartitionProduceData>)> = Vec::new();
    for (name, partition, bytes) in batches {
        let mut p = PartitionProduceData::default();
        p.index = *partition;
        p.records = Some(Bytes::from(bytes.clone()));
        match by_topic.iter_mut().find(|(t, _)| t == name) {
            Some((_, ps)) => ps.push(p),
            None => by_topic.push((name.clone(), vec![p])),
        }
    }
    r.topic_data = by_topic
        .into_iter()
        .map(|(name, partition_data)| {
            let mut t = TopicProduceData::default();
            t.name = topic_name(&name);
            t.partition_data = partition_data;
            t
        })
        .collect();
    r
}

/// Build a request frame payload by hand (header v2, the flexible one).
///
/// Used where `kafka-protocol` cannot express what the test wants to send: unknown header
/// tagged fields, deliberately wrong versions, truncated bodies.
pub fn raw_flexible_frame(
    api_key: i16,
    api_version: i16,
    correlation_id: i32,
    client_id: Option<&str>,
    header_tags: &[(u32, Vec<u8>)],
    body: &[u8],
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&api_key.to_be_bytes());
    out.extend_from_slice(&api_version.to_be_bytes());
    out.extend_from_slice(&correlation_id.to_be_bytes());
    match client_id {
        None => out.extend_from_slice(&(-1i16).to_be_bytes()),
        Some(s) => {
            out.extend_from_slice(&(s.len() as i16).to_be_bytes());
            out.extend_from_slice(s.as_bytes());
        }
    }
    put_uvarint(&mut out, header_tags.len() as u32);
    for (tag, value) in header_tags {
        put_uvarint(&mut out, *tag);
        put_uvarint(&mut out, value.len() as u32);
        out.extend_from_slice(value);
    }
    out.extend_from_slice(body);
    out
}

/// The v4 `ApiVersions` request body: two compact strings and an empty tagged section.
pub fn api_versions_body_v4() -> Vec<u8> {
    let mut body = Vec::new();
    for s in ["kafkatest", "0.1.0"] {
        put_uvarint(&mut body, s.len() as u32 + 1);
        body.extend_from_slice(s.as_bytes());
    }
    put_uvarint(&mut body, 0);
    body
}

/// Append an unsigned LEB128 varint.
pub fn put_uvarint(out: &mut Vec<u8>, mut v: u32) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// Big-endian i32 at `at`, or `None` when the buffer is too short.
pub fn be_i32(bytes: &[u8], at: usize) -> Option<i32> {
    let s = bytes.get(at..at + 4)?;
    Some(i32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

/// Big-endian i16 at `at`, or `None` when the buffer is too short.
pub fn be_i16(bytes: &[u8], at: usize) -> Option<i16> {
    let s = bytes.get(at..at + 2)?;
    Some(i16::from_be_bytes([s[0], s[1]]))
}

/// Send a hand-built frame and read the response payload.
pub async fn roundtrip_raw(conn: &mut Conn, payload: &[u8]) -> Result<Vec<u8>, Failure> {
    conn.send_frame(payload)
        .await
        .map_err(|e| Failure::proto(e, Some(conn)))?;
    conn.read_frame()
        .await
        .map_err(|e| Failure::proto(e, Some(conn)))
}

/// Read a correlation id out of a raw response, failing with a hex dump if it is short.
pub fn correlation_id_of(resp: &[u8], conn: &Conn) -> Result<i32, Failure> {
    be_i32(resp, 0).ok_or_else(|| {
        let mut f = Failure::new(
            FailureKind::Protocol,
            format!(
                "the response is {} bytes long; a response header starts with a 4-byte \
                 correlation id",
                resp.len()
            ),
        );
        f.request = conn.last_request.clone();
        f.response = Some(resp.to_vec());
        f
    })
}

/// The Kafka name of an error code, for failure messages that read like the protocol guide.
pub fn error_name(code: i16) -> &'static str {
    match code {
        -1 => "UNKNOWN_SERVER_ERROR",
        0 => "NONE",
        1 => "OFFSET_OUT_OF_RANGE",
        2 => "CORRUPT_MESSAGE",
        3 => "UNKNOWN_TOPIC_OR_PARTITION",
        5 => "LEADER_NOT_AVAILABLE",
        6 => "NOT_LEADER_OR_FOLLOWER",
        7 => "REQUEST_TIMED_OUT",
        10 => "MESSAGE_TOO_LARGE",
        14 => "COORDINATOR_LOAD_IN_PROGRESS",
        15 => "COORDINATOR_NOT_AVAILABLE",
        16 => "NOT_COORDINATOR",
        18 => "INVALID_TOPIC_EXCEPTION",
        21 => "INVALID_REQUIRED_ACKS",
        22 => "ILLEGAL_GENERATION",
        23 => "INCONSISTENT_GROUP_PROTOCOL",
        25 => "UNKNOWN_MEMBER_ID",
        27 => "REBALANCE_IN_PROGRESS",
        35 => "UNSUPPORTED_VERSION",
        36 => "TOPIC_ALREADY_EXISTS",
        37 => "INVALID_PARTITIONS",
        38 => "INVALID_REPLICATION_FACTOR",
        41 => "NOT_CONTROLLER",
        42 => "INVALID_REQUEST",
        43 => "UNSUPPORTED_FOR_MESSAGE_FORMAT",
        45 => "OUT_OF_ORDER_SEQUENCE_NUMBER",
        46 => "DUPLICATE_SEQUENCE_NUMBER",
        47 => "INVALID_PRODUCER_EPOCH",
        79 => "MEMBER_ID_REQUIRED",
        87 => "INVALID_RECORD",
        100 => "UNKNOWN_TOPIC_ID",
        _ => "an error code this suite does not name",
    }
}

/// `"45 OUT_OF_ORDER_SEQUENCE_NUMBER"` — how every error code is written in a report.
pub fn error_label(code: i16) -> String {
    format!("{code} {}", error_name(code))
}

/// Assert an error code is one of several acceptable values, naming every one of them.
///
/// Kafka's exact choice varies by version, and a from-scratch broker may reasonably
/// pick the other code of a pair; where that is true the stage's hints say so too.
pub fn error_is_one_of(c: &mut Check, path: &str, allowed: &[i16], actual: i16) -> bool {
    let ok = allowed.contains(&actual);
    let wanted = allowed
        .iter()
        .map(|c| error_label(*c))
        .collect::<Vec<_>>()
        .join(" or ");
    c.that(path, &wanted, ok, error_label(actual));
    ok
}

/// Prove the broker is still serving: open a new connection and run `ApiVersions` on it.
pub async fn expect_still_serving(ctx: &Ctx, after: &str) -> Result<(), Failure> {
    let mut conn = ctx.connect().await.map_err(|f| {
        f.note(format!(
            "the broker stopped accepting connections after {after}"
        ))
    })?;
    let resp = super::api_versions(&mut conn).await.map_err(|f| {
        f.note(format!(
            "the broker stopped answering ApiVersions after {after}"
        ))
    })?;
    let mut c = Check::new(format!("the broker still serves after {after}"), &conn);
    c.eq("response.error_code", NONE, resp.error_code);
    c.finish()
}

/// The version range the broker advertises for `key`, or a failure naming the missing key.
pub fn require_api(
    resp: &kafka_protocol::messages::ApiVersionsResponse,
    key: i16,
    name: &str,
    conn: &Conn,
) -> Result<(i16, i16), Failure> {
    super::advertised(resp, key).ok_or_else(|| {
        let mut c = Check::new(format!("that ApiVersions advertises {name}({key})"), conn);
        let listed: Vec<i16> = resp.api_keys.iter().map(|k| k.api_key).collect();
        c.that(
            &format!("response.api_keys[api_key={key}]"),
            &format!("an entry for {name}({key})"),
            false,
            listed,
        );
        c.finish()
            .err()
            .unwrap_or_else(|| Failure::harness(format!("{name}({key}) is not advertised")))
    })
}

/// Wait until a partition's high watermark reaches `want`, so tests never race an append.
pub async fn await_high_watermark(
    ctx: &Ctx,
    topic: &TopicInfo,
    partition: i32,
    want: i64,
) -> Result<(), Failure> {
    let deadline = tokio::time::Instant::now() + ctx.timeout;
    let mut last = -1i64;
    while tokio::time::Instant::now() < deadline {
        let mut conn = ctx.connect().await?;
        let req = fetch_request(&[(topic.id, partition, 0)], 100);
        if let Ok(resp) = conn.request(FETCH_V16, &req).await {
            if let Some(p) = resp.responses.first().and_then(|t| t.partitions.first()) {
                last = p.high_watermark;
                if p.high_watermark >= want {
                    return Ok(());
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(Failure::harness(format!(
        "{}-{partition} never reached high watermark {want} (last saw {last})",
        topic.name
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_frames_start_with_the_header_fields() {
        let f = raw_flexible_frame(18, 4, 99, Some("abc"), &[], &api_versions_body_v4());
        assert_eq!(be_i16(&f, 0), Some(18));
        assert_eq!(be_i16(&f, 2), Some(4));
        assert_eq!(be_i32(&f, 4), Some(99));
        assert_eq!(be_i16(&f, 8), Some(3));
        assert_eq!(&f[10..13], b"abc");
        assert_eq!(f[13], 0, "empty header tagged fields");
    }

    #[test]
    fn raw_frames_can_carry_header_tagged_fields() {
        let f = raw_flexible_frame(18, 4, 1, None, &[(99, vec![1, 2, 3])], &[]);
        assert_eq!(be_i16(&f, 8), Some(-1), "null client id");
        assert_eq!(f[10], 1, "one tagged field");
        assert_eq!(f[11], 99, "tag");
        assert_eq!(f[12], 3, "length");
        assert_eq!(&f[13..16], &[1, 2, 3]);
    }

    #[test]
    fn api_versions_body_is_two_compact_strings() {
        let b = api_versions_body_v4();
        assert_eq!(b[0], 10, "compact length of 'kafkatest'");
        assert_eq!(&b[1..10], b"kafkatest");
        assert_eq!(b[10], 6, "compact length of '0.1.0'");
        assert_eq!(*b.last().unwrap_or(&9), 0, "empty tagged fields");
    }

    #[test]
    fn fetch_requests_group_partitions_by_topic() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let r = fetch_request(&[(a, 0, 0), (a, 1, 5), (b, 0, 0)], 500);
        assert_eq!(r.topics.len(), 2);
        assert_eq!(r.topics[0].partitions.len(), 2);
        assert_eq!(r.topics[0].partitions[1].fetch_offset, 5);
        assert_eq!(r.topics[1].topic_id, b);
    }

    #[test]
    fn produce_requests_group_partitions_by_topic() {
        let r = produce_request(
            &[
                ("t".to_string(), 0, vec![1]),
                ("t".to_string(), 1, vec![2]),
                ("u".to_string(), 0, vec![3]),
            ],
            -1,
        );
        assert_eq!(r.topic_data.len(), 2);
        assert_eq!(r.topic_data[0].partition_data.len(), 2);
        assert_eq!(r.acks, -1);
    }

    #[test]
    fn be_readers_are_bounds_checked() {
        assert_eq!(be_i32(&[0, 0, 0, 7], 0), Some(7));
        assert_eq!(be_i32(&[0, 0, 0], 0), None);
        assert_eq!(be_i16(&[0, 35], 0), Some(35));
        assert_eq!(be_i16(&[0], 0), None);
    }
}
