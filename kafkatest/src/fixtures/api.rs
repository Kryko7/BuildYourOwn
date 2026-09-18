//! Fixture strategy `api`: build the same state through the wire protocol.
//!
//! `CreateTopics` v7 creates the topics, `Metadata` v12 discovers their ids and waits for a
//! leader, and `Produce` v11 writes one request per declared batch so that batch boundaries
//! survive into the log. Nothing about the result is assumed: the ids come back from the
//! broker and go into the [`FixtureHandle`] the test reads.

use super::{unique_name, FixtureHandle, FixtureSpec, TopicInfo};
use crate::assert::{Failure, FailureKind};
use crate::proto::records::{RecordBatch, RecordItem};
use crate::proto::Conn;
use bytes::Bytes;
use kafka_protocol::messages::create_topics_request::CreatableTopic;
use kafka_protocol::messages::fetch_request::{FetchPartition, FetchTopic};
use kafka_protocol::messages::metadata_request::MetadataRequestTopic;
use kafka_protocol::messages::produce_request::{PartitionProduceData, TopicProduceData};
use kafka_protocol::messages::{
    BrokerId, CreateTopicsRequest, FetchRequest, MetadataRequest, ProduceRequest, TopicName,
};
use kafka_protocol::protocol::StrBytes;
use std::path::Path;
use std::time::Duration;

/// `CreateTopics` version used to build fixtures.
pub const CREATE_TOPICS_VERSION: i16 = 7;
/// `Metadata` version used to discover ids.
pub const METADATA_VERSION: i16 = 12;
/// `Produce` version used to write fixture records.
pub const PRODUCE_VERSION: i16 = 11;
/// `Fetch` version used to check that a partition is servable.
pub const FETCH_VERSION: i16 = 16;

const TOPIC_ALREADY_EXISTS: i16 = 36;

fn topic_name(s: &str) -> TopicName {
    TopicName(StrBytes::from_string(s.to_string()))
}

/// Create every topic of `spec` and write its records, through the protocol.
pub async fn materialize(
    conn: &mut Conn,
    spec: &FixtureSpec,
    salt: &str,
    log_dir: &Path,
) -> Result<FixtureHandle, Failure> {
    let mut handle = FixtureHandle {
        log_dir: log_dir.to_path_buf(),
        broker_id: 1,
        ..Default::default()
    };
    if spec.is_empty() {
        return Ok(handle);
    }
    let names: Vec<(String, String, i32)> = spec
        .topics
        .iter()
        .map(|t| (t.key.clone(), unique_name(&t.key, salt), t.partitions))
        .collect();

    create_topics(conn, &names).await?;
    let discovered = wait_for_leaders(conn, &names).await?;
    wait_until_fetchable(conn, &names, &discovered).await?;

    for (topic, (_key, name, _parts)) in spec.topics.iter().zip(names.iter()) {
        let (id, broker_id) = discovered
            .iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, id, leader)| (*id, *leader))
            .ok_or_else(|| {
                Failure::harness(format!(
                    "Metadata did not return the topic '{name}' we created"
                ))
            })?;
        handle.broker_id = broker_id;
        for (partition, batches) in &topic.batches {
            for (index, batch) in batches.iter().enumerate() {
                let codec = topic.codec(*partition, index);
                produce_batch(conn, name, *partition, batch, codec).await?;
            }
        }
        handle.insert(TopicInfo {
            key: topic.key.clone(),
            name: name.clone(),
            id,
            partitions: topic.partitions,
            batches: topic.batches.clone(),
        });
    }
    Ok(handle)
}

async fn create_topics(conn: &mut Conn, names: &[(String, String, i32)]) -> Result<(), Failure> {
    let mut req = CreateTopicsRequest::default();
    req.timeout_ms = 15_000;
    req.topics = names
        .iter()
        .map(|(_, name, partitions)| {
            let mut t = CreatableTopic::default();
            t.name = topic_name(name);
            t.num_partitions = *partitions;
            t.replication_factor = 1;
            t
        })
        .collect();

    let mut last = String::new();
    for attempt in 0..20 {
        match conn.request(CREATE_TOPICS_VERSION, &req).await {
            Ok(resp) => {
                let mut retry = false;
                for t in &resp.topics {
                    if t.error_code == 0 || t.error_code == TOPIC_ALREADY_EXISTS {
                        continue;
                    }
                    last = format!(
                        "CreateTopics for '{}' returned error {} ({})",
                        t.name.0.as_str(),
                        t.error_code,
                        t.error_message
                            .as_ref()
                            .map(|m| m.to_string())
                            .unwrap_or_else(|| "no message".to_string())
                    );
                    retry = true;
                }
                if !retry {
                    return Ok(());
                }
            }
            Err(e) => last = e.to_string(),
        }
        tokio::time::sleep(Duration::from_millis(100 + attempt * 50)).await;
    }
    Err(Failure::harness(format!(
        "could not create the fixture topics: {last}"
    )))
}

type Discovered = Vec<(String, uuid::Uuid, i32)>;

async fn wait_for_leaders(
    conn: &mut Conn,
    names: &[(String, String, i32)],
) -> Result<Discovered, Failure> {
    let mut req = MetadataRequest::default();
    req.allow_auto_topic_creation = false;
    req.topics = Some(
        names
            .iter()
            .map(|(_, name, _)| {
                let mut t = MetadataRequestTopic::default();
                t.name = Some(topic_name(name));
                t
            })
            .collect(),
    );
    let mut last = String::new();
    for attempt in 0..40 {
        match conn.request(METADATA_VERSION, &req).await {
            Ok(resp) => {
                let mut out = Discovered::new();
                let mut ready = true;
                for (_, name, partitions) in names {
                    let Some(t) = resp
                        .topics
                        .iter()
                        .find(|t| t.name.as_ref().map(|n| n.0.as_str()) == Some(name.as_str()))
                    else {
                        ready = false;
                        last = format!("Metadata does not list '{name}' yet");
                        break;
                    };
                    if t.error_code != 0 {
                        ready = false;
                        last = format!("Metadata for '{name}' has error {}", t.error_code);
                        break;
                    }
                    if t.partitions.len() != *partitions as usize
                        || t.partitions.iter().any(|p| p.leader_id.0 < 0)
                    {
                        ready = false;
                        last = format!("'{name}' has no leader for every partition yet");
                        break;
                    }
                    let leader = t.partitions.first().map(|p| p.leader_id.0).unwrap_or(1);
                    out.push((name.clone(), t.topic_id, leader));
                }
                if ready {
                    return Ok(out);
                }
            }
            Err(e) => last = e.to_string(),
        }
        tokio::time::sleep(Duration::from_millis(50 + attempt * 25)).await;
    }
    Err(Failure::harness(format!(
        "the fixture topics never became available: {last}"
    )))
}

/// Wait until every partition answers a `Fetch` without `NOT_LEADER_OR_FOLLOWER`.
///
/// `Metadata` reports a leader as soon as the controller has committed the
/// `PartitionRecord`, but the broker's replica manager creates the local log a moment
/// later. Without this, the first test of a stage occasionally sees error 6 and a high
/// watermark of -1.
async fn wait_until_fetchable(
    conn: &mut Conn,
    names: &[(String, String, i32)],
    discovered: &Discovered,
) -> Result<(), Failure> {
    let mut req = FetchRequest::default();
    req.max_wait_ms = 100;
    req.min_bytes = 1;
    req.max_bytes = 1024 * 1024;
    req.replica_id = BrokerId(-1);
    req.topics = discovered
        .iter()
        .map(|(name, id, _)| {
            let partitions = names
                .iter()
                .find(|(_, n, _)| n == name)
                .map(|(_, _, p)| *p)
                .unwrap_or(1);
            let mut t = FetchTopic::default();
            t.topic_id = *id;
            t.partitions = (0..partitions)
                .map(|index| {
                    let mut p = FetchPartition::default();
                    p.partition = index;
                    p.fetch_offset = 0;
                    p.current_leader_epoch = -1;
                    p.last_fetched_epoch = -1;
                    p.log_start_offset = -1;
                    p.partition_max_bytes = 1024 * 1024;
                    p
                })
                .collect();
            t
        })
        .collect();

    let mut last = String::new();
    for attempt in 0..40 {
        match conn.request(FETCH_VERSION, &req).await {
            Ok(resp) => {
                let bad = resp
                    .responses
                    .iter()
                    .flat_map(|t| t.partitions.iter().map(move |p| (t.topic_id, p)))
                    .find(|(_, p)| p.error_code != 0);
                match bad {
                    None if resp.error_code == 0 => return Ok(()),
                    None => last = format!("Fetch returned top-level error {}", resp.error_code),
                    Some((id, p)) => {
                        last = format!(
                            "partition {} of {id} is not servable yet (error {})",
                            p.partition_index, p.error_code
                        )
                    }
                }
            }
            Err(e) => last = e.to_string(),
        }
        tokio::time::sleep(Duration::from_millis(50 + attempt * 10)).await;
    }
    Err(Failure::harness(format!(
        "the fixture partitions never became servable: {last}"
    )))
}

async fn produce_batch(
    conn: &mut Conn,
    name: &str,
    partition: i32,
    records: &[super::RecordSpec],
    codec: i16,
) -> Result<(), Failure> {
    let items: Vec<RecordItem> = records
        .iter()
        .map(|r| RecordItem {
            key: r.key.clone(),
            value: Some(r.value.clone()),
            ..Default::default()
        })
        .collect();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    // The base offset is rewritten by the broker on append; 0 is what a producer sends.
    // A broker running with the default `compression.type=producer` stores a compressed
    // batch exactly as the producer sent it, which is what makes stage 27 testable here.
    let batch = RecordBatch::of(0, timestamp, items).with_compression(codec);

    let mut partition_data = PartitionProduceData::default();
    partition_data.index = partition;
    partition_data.records = Some(Bytes::from(batch.encode()));
    let mut topic_data = TopicProduceData::default();
    topic_data.name = topic_name(name);
    topic_data.partition_data = vec![partition_data];
    let mut req = ProduceRequest::default();
    req.acks = -1;
    req.timeout_ms = 15_000;
    req.topic_data = vec![topic_data];

    let resp = conn
        .request(PRODUCE_VERSION, &req)
        .await
        .map_err(|e| Failure::proto(e, Some(conn)).note("while producing fixture records"))?;
    for t in &resp.responses {
        for p in &t.partition_responses {
            if p.error_code != 0 {
                return Err(Failure::new(
                    FailureKind::Harness,
                    format!(
                        "producing fixture records to {name}-{partition} returned error {}",
                        p.error_code
                    ),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_the_ones_the_plan_pins() {
        assert_eq!(CREATE_TOPICS_VERSION, 7);
        assert_eq!(METADATA_VERSION, 12);
        assert_eq!(PRODUCE_VERSION, 11);
    }

    #[test]
    fn topic_names_round_trip_through_kafka_protocol() {
        let n = topic_name("t1-abc");
        assert_eq!(n.0.as_str(), "t1-abc");
    }
}
