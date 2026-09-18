//! The classic consumer-group protocol, shared by stages 39-42.
//!
//! Everything the group stages need that is not already in [`helpers`](super::helpers):
//! the API keys and error codes of the group APIs, request builders for JoinGroup(11),
//! SyncGroup(14), Heartbeat(12), LeaveGroup(13), OffsetCommit(8) and OffsetFetch(9), and a
//! hand-written `ConsumerProtocolSubscription` / `ConsumerProtocolAssignment` codec.
//!
//! The member metadata and the assignment are **opaque bytes** to the broker in the classic
//! protocol, but Kafka's own coordinator parses the subscription when it describes a group,
//! so the bytes have to be the real thing:
//!
//! ```text
//! ConsumerProtocolSubscription v0   version int16, topics [string], user_data bytes
//! ConsumerProtocolAssignment   v0   version int16, assigned_partitions [(string, [int32])],
//!                                   user_data bytes
//! ```
//!
//! Both are *non-flexible*: arrays are an int32 count, strings an int16 length, bytes an
//! int32 length, and -1 means null. The int16 version comes first, outside the struct —
//! that is `ConsumerProtocol.serializeSubscription` in Kafka's client.
//!
//! This suite deliberately speaks the **classic** protocol (`protocol_type = "consumer"`,
//! JoinGroup/SyncGroup) and not KIP-848's ConsumerGroupHeartbeat(68), because that is what
//! the classic track and every existing client library implement.

use crate::assert::{Check, Failure};
use crate::proto::Conn;
use crate::stages::{proto_fail, Ctx};
use bytes::Bytes;
use kafka_protocol::messages::offset_commit_request::{
    OffsetCommitRequestPartition, OffsetCommitRequestTopic,
};
use kafka_protocol::messages::offset_fetch_request::{
    OffsetFetchRequestGroup, OffsetFetchRequestTopics,
};
use kafka_protocol::messages::{
    FindCoordinatorRequest, FindCoordinatorResponse, GroupId, HeartbeatRequest, HeartbeatResponse,
    JoinGroupRequest, JoinGroupResponse, LeaveGroupRequest, LeaveGroupResponse,
    OffsetCommitRequest, OffsetCommitResponse, OffsetFetchRequest, OffsetFetchResponse,
    SyncGroupRequest, SyncGroupResponse,
};
use kafka_protocol::protocol::StrBytes;
use std::time::Duration;

// ---------------------------------------------------------------------------------------
// API keys and the versions this suite speaks.
// ---------------------------------------------------------------------------------------

/// `FindCoordinator`
pub const FIND_COORDINATOR_KEY: i16 = 10;

/// The `ListOffsets` version the suite speaks (flexible, with leader epochs).
pub const LIST_OFFSETS_V8: i16 = 8;
/// The `FindCoordinator` version the suite speaks (batched `coordinator_keys`).
pub const FIND_COORDINATOR_V4: i16 = 4;
/// The `JoinGroup` version the suite speaks.
pub const JOIN_GROUP_V9: i16 = 9;
/// The `Heartbeat` version the suite speaks.
pub const HEARTBEAT_V4: i16 = 4;
/// The `LeaveGroup` version the suite speaks (batched `members`).
pub const LEAVE_GROUP_V5: i16 = 5;
/// The `SyncGroup` version the suite speaks.
pub const SYNC_GROUP_V5: i16 = 5;
/// The `OffsetCommit` version the suite speaks.
pub const OFFSET_COMMIT_V8: i16 = 8;
/// The `OffsetFetch` version the suite speaks (batched `groups`).
pub const OFFSET_FETCH_V8: i16 = 8;

// ---------------------------------------------------------------------------------------
// Error codes of section E.
// ---------------------------------------------------------------------------------------

/// `COORDINATOR_NOT_AVAILABLE` — usually `__consumer_offsets` does not exist yet.
pub const COORDINATOR_NOT_AVAILABLE: i16 = 15;
/// `NOT_COORDINATOR`
pub const NOT_COORDINATOR: i16 = 16;
/// `ILLEGAL_GENERATION`
pub const ILLEGAL_GENERATION: i16 = 22;
/// `UNKNOWN_MEMBER_ID`
pub const UNKNOWN_MEMBER_ID: i16 = 25;
/// `REBALANCE_IN_PROGRESS`
pub const REBALANCE_IN_PROGRESS: i16 = 27;
/// `GROUP_ID_NOT_FOUND`
pub const GROUP_ID_NOT_FOUND: i16 = 69;
/// `MEMBER_ID_REQUIRED` — KIP-394, the answer to a join with an empty member id.
pub const MEMBER_ID_REQUIRED: i16 = 79;

/// `key_type` 0: the key is a consumer group id.
pub const KEY_TYPE_GROUP: i8 = 0;
/// `key_type` 1: the key is a transactional id.
pub const KEY_TYPE_TRANSACTION: i8 = 1;

/// The `protocol_type` every consumer group uses.
pub const PROTOCOL_TYPE: &str = "consumer";
/// The assignor name this suite offers; "range" is one of Kafka's own.
pub const PROTOCOL_NAME: &str = "range";
/// Session timeout used everywhere; inside Kafka's 6000..1800000 default window.
pub const SESSION_TIMEOUT_MS: i32 = 10_000;
/// Rebalance timeout used everywhere; short so a stuck rebalance fails the test fast.
pub const REBALANCE_TIMEOUT_MS: i32 = 10_000;

// ---------------------------------------------------------------------------------------
// Small conversions.
// ---------------------------------------------------------------------------------------

/// A `GroupId` from a `&str`.
pub fn group_id(s: &str) -> GroupId {
    GroupId(StrBytes::from_string(s.to_string()))
}

/// A `StrBytes` from a `&str`.
pub fn sb(s: &str) -> StrBytes {
    StrBytes::from_string(s.to_string())
}

// ---------------------------------------------------------------------------------------
// ConsumerProtocol codec.
// ---------------------------------------------------------------------------------------

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as i16).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
}

/// Encode a `ConsumerProtocolSubscription` v0 with its version prefix.
pub fn subscription_v0(topics: &[&str]) -> Bytes {
    let mut out = Vec::new();
    out.extend_from_slice(&0i16.to_be_bytes());
    out.extend_from_slice(&(topics.len() as i32).to_be_bytes());
    for t in topics {
        put_str(&mut out, t);
    }
    out.extend_from_slice(&(-1i32).to_be_bytes()); // null user_data
    Bytes::from(out)
}

/// Encode a `ConsumerProtocolAssignment` v0 with its version prefix.
pub fn assignment_v0(assigned: &[(&str, &[i32])]) -> Bytes {
    let mut out = Vec::new();
    out.extend_from_slice(&0i16.to_be_bytes());
    out.extend_from_slice(&(assigned.len() as i32).to_be_bytes());
    for (topic, partitions) in assigned {
        put_str(&mut out, topic);
        out.extend_from_slice(&(partitions.len() as i32).to_be_bytes());
        for p in partitions.iter() {
            out.extend_from_slice(&p.to_be_bytes());
        }
    }
    out.extend_from_slice(&(-1i32).to_be_bytes());
    Bytes::from(out)
}

/// A decoded `ConsumerProtocolAssignment`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    /// The int16 version prefix.
    pub version: i16,
    /// `(topic, partitions)` in the order they were encoded.
    pub assigned: Vec<(String, Vec<i32>)>,
}

struct Cursor<'a> {
    b: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn i16(&mut self) -> Option<i16> {
        let v = super::be_i16(self.b, self.at)?;
        self.at += 2;
        Some(v)
    }
    fn i32(&mut self) -> Option<i32> {
        let v = super::be_i32(self.b, self.at)?;
        self.at += 4;
        Some(v)
    }
    fn string(&mut self) -> Option<String> {
        let len = self.i16()?;
        if len < 0 {
            return Some(String::new());
        }
        let s = self.b.get(self.at..self.at + len as usize)?;
        self.at += len as usize;
        Some(String::from_utf8_lossy(s).to_string())
    }
}

/// Decode a `ConsumerProtocolAssignment` v0; `None` when the bytes are truncated.
pub fn decode_assignment_v0(bytes: &[u8]) -> Option<Assignment> {
    let mut c = Cursor { b: bytes, at: 0 };
    let version = c.i16()?;
    let count = c.i32()?;
    if !(0..=1024).contains(&count) {
        return None;
    }
    let mut assigned = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let topic = c.string()?;
        let n = c.i32()?;
        if !(0..=4096).contains(&n) {
            return None;
        }
        let mut partitions = Vec::with_capacity(n as usize);
        for _ in 0..n {
            partitions.push(c.i32()?);
        }
        assigned.push((topic, partitions));
    }
    // The trailing user_data is read but thrown away, so that a truncated buffer is
    // rejected instead of silently decoding as a complete assignment.
    let user_data = c.i32()?;
    if user_data >= 0 {
        c.b.get(c.at..c.at.checked_add(user_data as usize)?)?;
    }
    Some(Assignment { version, assigned })
}

// ---------------------------------------------------------------------------------------
// Request builders.
// ---------------------------------------------------------------------------------------

/// A `FindCoordinator` v4 request for one or more keys.
pub fn find_coordinator_request(keys: &[&str], key_type: i8) -> FindCoordinatorRequest {
    let mut r = FindCoordinatorRequest::default();
    r.key_type = key_type;
    r.coordinator_keys = keys.iter().map(|k| sb(k)).collect();
    r
}

/// A `JoinGroup` v9 request offering the single "range" protocol.
pub fn join_request(group: &str, member_id: &str, topics: &[&str]) -> JoinGroupRequest {
    let mut protocol =
        kafka_protocol::messages::join_group_request::JoinGroupRequestProtocol::default();
    protocol.name = sb(PROTOCOL_NAME);
    protocol.metadata = subscription_v0(topics);
    let mut r = JoinGroupRequest::default();
    r.group_id = group_id(group);
    r.session_timeout_ms = SESSION_TIMEOUT_MS;
    r.rebalance_timeout_ms = REBALANCE_TIMEOUT_MS;
    r.member_id = sb(member_id);
    r.protocol_type = sb(PROTOCOL_TYPE);
    r.protocols = vec![protocol];
    r
}

/// A `SyncGroup` v5 request; `assignments` is empty for a follower.
pub fn sync_request(
    group: &str,
    generation_id: i32,
    member_id: &str,
    assignments: &[(&str, Bytes)],
) -> SyncGroupRequest {
    let mut r = SyncGroupRequest::default();
    r.group_id = group_id(group);
    r.generation_id = generation_id;
    r.member_id = sb(member_id);
    r.protocol_type = Some(sb(PROTOCOL_TYPE));
    r.protocol_name = Some(sb(PROTOCOL_NAME));
    r.assignments = assignments
        .iter()
        .map(|(member, bytes)| {
            let mut a =
                kafka_protocol::messages::sync_group_request::SyncGroupRequestAssignment::default();
            a.member_id = sb(member);
            a.assignment = bytes.clone();
            a
        })
        .collect();
    r
}

/// A `Heartbeat` v4 request.
pub fn heartbeat_request(group: &str, generation_id: i32, member_id: &str) -> HeartbeatRequest {
    let mut r = HeartbeatRequest::default();
    r.group_id = group_id(group);
    r.generation_id = generation_id;
    r.member_id = sb(member_id);
    r
}

/// A `LeaveGroup` v5 request for one member.
pub fn leave_request(group: &str, member_id: &str) -> LeaveGroupRequest {
    let mut identity = kafka_protocol::messages::leave_group_request::MemberIdentity::default();
    identity.member_id = sb(member_id);
    identity.reason = Some(sb("kafkatest"));
    let mut r = LeaveGroupRequest::default();
    r.group_id = group_id(group);
    r.members = vec![identity];
    r
}

/// An `OffsetCommit` v8 request; `generation_id` -1 and an empty member id is a
/// standalone ("simple") commit that needs no group membership.
pub fn offset_commit_request(
    group: &str,
    generation_id: i32,
    member_id: &str,
    entries: &[(&str, i32, i64, Option<&str>)],
) -> OffsetCommitRequest {
    let mut by_topic: Vec<(String, Vec<OffsetCommitRequestPartition>)> = Vec::new();
    for (topic, partition, offset, metadata) in entries {
        let mut p = OffsetCommitRequestPartition::default();
        p.partition_index = *partition;
        p.committed_offset = *offset;
        p.committed_leader_epoch = -1;
        p.committed_metadata = metadata.map(sb);
        match by_topic.iter_mut().find(|(t, _)| t == topic) {
            Some((_, ps)) => ps.push(p),
            None => by_topic.push(((*topic).to_string(), vec![p])),
        }
    }
    let mut r = OffsetCommitRequest::default();
    r.group_id = group_id(group);
    r.generation_id_or_member_epoch = generation_id;
    r.member_id = sb(member_id);
    r.topics = by_topic
        .into_iter()
        .map(|(name, partitions)| {
            let mut t = OffsetCommitRequestTopic::default();
            t.name = super::topic_name(&name);
            t.partitions = partitions;
            t
        })
        .collect();
    r
}

/// An `OffsetFetch` v8 request for one group; `None` topics means "every topic".
pub fn offset_fetch_request(
    group: &str,
    topics: Option<&[(&str, Vec<i32>)]>,
) -> OffsetFetchRequest {
    let mut g = OffsetFetchRequestGroup::default();
    g.group_id = group_id(group);
    g.topics = topics.map(|ts| {
        ts.iter()
            .map(|(name, partitions)| {
                let mut t = OffsetFetchRequestTopics::default();
                t.name = super::topic_name(name);
                t.partition_indexes = partitions.clone();
                t
            })
            .collect()
    });
    let mut r = OffsetFetchRequest::default();
    r.groups = vec![g];
    r
}

// ---------------------------------------------------------------------------------------
// Round trips.
// ---------------------------------------------------------------------------------------

/// Send a `FindCoordinator` v4 request and decode the response.
pub async fn find_coordinator(
    conn: &mut Conn,
    keys: &[&str],
    key_type: i8,
) -> Result<FindCoordinatorResponse, Failure> {
    let req = find_coordinator_request(keys, key_type);
    let resp = conn
        .request(FIND_COORDINATOR_V4, &req)
        .await
        .map_err(|e| proto_fail(e, conn))?;
    Ok(resp.body)
}

/// What the broker said about one coordinator key.
#[derive(Debug, Clone)]
pub struct CoordinatorInfo {
    /// The key the broker echoed.
    pub key: String,
    /// The coordinator's broker id.
    pub node_id: i32,
    /// The coordinator's advertised host.
    pub host: String,
    /// The coordinator's advertised port.
    pub port: i32,
    /// The per-key error code.
    pub error_code: i16,
}

/// Pull the entry for `key` out of a `FindCoordinator` v4 response.
pub fn coordinator_of(resp: &FindCoordinatorResponse, key: &str) -> Option<CoordinatorInfo> {
    resp.coordinators
        .iter()
        .find(|c| c.key.as_str() == key)
        .map(|c| CoordinatorInfo {
            key: c.key.to_string(),
            node_id: c.node_id.0,
            host: c.host.to_string(),
            port: c.port,
            error_code: c.error_code,
        })
}

/// Resolve a group's coordinator, retrying while the broker creates `__consumer_offsets`.
///
/// Kafka creates the offsets topic lazily on the first `FindCoordinator`, and answers
/// `COORDINATOR_NOT_AVAILABLE` (15) for a few hundred milliseconds until it exists. Every
/// group test starts here so that later assertions never see that transient error.
pub async fn await_coordinator(ctx: &Ctx, group: &str) -> Result<CoordinatorInfo, Failure> {
    let deadline = tokio::time::Instant::now() + ctx.timeout;
    let mut last = None;
    while tokio::time::Instant::now() < deadline {
        let mut conn = ctx.connect().await?;
        let resp = find_coordinator(&mut conn, &[group], KEY_TYPE_GROUP).await?;
        if let Some(info) = coordinator_of(&resp, group) {
            if info.error_code == 0 {
                return Ok(info);
            }
            last = Some(info);
        } else if resp.error_code == 0 && resp.node_id.0 >= 0 {
            // A broker that answers v4 with the old top-level shape.
            return Ok(CoordinatorInfo {
                key: group.to_string(),
                node_id: resp.node_id.0,
                host: resp.host.to_string(),
                port: resp.port,
                error_code: resp.error_code,
            });
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(Failure::harness(format!(
        "no coordinator for group '{group}' within {} ms (last answer: {last:?})",
        ctx.timeout.as_millis()
    )))
}

/// Send a `JoinGroup` v9 request and decode the response.
pub async fn join(
    conn: &mut Conn,
    group: &str,
    member_id: &str,
    topics: &[&str],
) -> Result<JoinGroupResponse, Failure> {
    let req = join_request(group, member_id, topics);
    let resp = conn
        .request(JOIN_GROUP_V9, &req)
        .await
        .map_err(|e| proto_fail(e, conn))?;
    Ok(resp.body)
}

/// The result of bringing a brand-new member into a group.
#[derive(Debug, Clone)]
pub struct JoinOutcome {
    /// The member id the broker handed out.
    pub member_id: String,
    /// The join that settled the membership.
    pub settled: JoinGroupResponse,
}

/// Join with an empty member id and, if the broker asks for one (KIP-394, error 79),
/// join again with the id it handed back.
pub async fn join_new(
    conn: &mut Conn,
    group: &str,
    topics: &[&str],
) -> Result<JoinOutcome, Failure> {
    let first = join(conn, group, "", topics).await?;
    let member_id = first.member_id.to_string();
    if first.error_code != MEMBER_ID_REQUIRED {
        return Ok(JoinOutcome {
            member_id,
            settled: first,
        });
    }
    let settled = join(conn, group, &member_id, topics).await?;
    Ok(JoinOutcome { member_id, settled })
}

/// Write a `JoinGroup` v9 request without reading the answer.
///
/// The coordinator holds a join open until every member of the group has re-joined, so a
/// second member's join can only be completed after the first member re-joins. Tests drive
/// that by hand: `begin_join` on one connection, work on another, then [`finish_join`].
/// Returns the correlation id to finish with.
pub async fn begin_join(
    conn: &mut Conn,
    group: &str,
    member_id: &str,
    topics: &[&str],
) -> Result<i32, Failure> {
    let req = join_request(group, member_id, topics);
    let id = conn.next_correlation_id();
    conn.send_request(JOIN_GROUP_V9, id, &req)
        .await
        .map_err(|e| proto_fail(e, conn))?;
    Ok(id)
}

/// Read the answer to a join started with [`begin_join`].
pub async fn finish_join(
    conn: &mut Conn,
    correlation_id: i32,
) -> Result<JoinGroupResponse, Failure> {
    let raw = conn.read_frame().await.map_err(|e| proto_fail(e, conn))?;
    let decoded = crate::proto::decode_response::<JoinGroupRequest>(JOIN_GROUP_V9, &raw)
        .map_err(|e| proto_fail(e, conn))?;
    if decoded.correlation_id != correlation_id {
        return Err(Failure::harness(format!(
            "response.correlation_id: expected {correlation_id} for the pending JoinGroup, \
             got {}",
            decoded.correlation_id
        )));
    }
    Ok(decoded.body)
}

/// Heartbeat until the coordinator answers `want`, or give up after `within`.
///
/// A rebalance starts when the coordinator processes another member's join, which is not
/// synchronous with the request that triggered it; polling is what a real consumer does.
pub async fn await_heartbeat_code(
    conn: &mut Conn,
    group: &str,
    generation_id: i32,
    member_id: &str,
    want: i16,
    within: Duration,
) -> Result<HeartbeatResponse, Failure> {
    let deadline = tokio::time::Instant::now() + within;
    let mut last = heartbeat(conn, group, generation_id, member_id).await?;
    while last.error_code != want && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
        last = heartbeat(conn, group, generation_id, member_id).await?;
    }
    Ok(last)
}

/// Send a `SyncGroup` v5 request and decode the response.
pub async fn sync(
    conn: &mut Conn,
    group: &str,
    generation_id: i32,
    member_id: &str,
    assignments: &[(&str, Bytes)],
) -> Result<SyncGroupResponse, Failure> {
    let req = sync_request(group, generation_id, member_id, assignments);
    let resp = conn
        .request(SYNC_GROUP_V5, &req)
        .await
        .map_err(|e| proto_fail(e, conn))?;
    Ok(resp.body)
}

/// Send a `Heartbeat` v4 request and decode the response.
pub async fn heartbeat(
    conn: &mut Conn,
    group: &str,
    generation_id: i32,
    member_id: &str,
) -> Result<HeartbeatResponse, Failure> {
    let req = heartbeat_request(group, generation_id, member_id);
    let resp = conn
        .request(HEARTBEAT_V4, &req)
        .await
        .map_err(|e| proto_fail(e, conn))?;
    Ok(resp.body)
}

/// Send a `LeaveGroup` v5 request and decode the response.
pub async fn leave(
    conn: &mut Conn,
    group: &str,
    member_id: &str,
) -> Result<LeaveGroupResponse, Failure> {
    let req = leave_request(group, member_id);
    let resp = conn
        .request(LEAVE_GROUP_V5, &req)
        .await
        .map_err(|e| proto_fail(e, conn))?;
    Ok(resp.body)
}

/// Send an `OffsetCommit` v8 request and decode the response.
pub async fn offset_commit(
    conn: &mut Conn,
    group: &str,
    generation_id: i32,
    member_id: &str,
    entries: &[(&str, i32, i64, Option<&str>)],
) -> Result<OffsetCommitResponse, Failure> {
    let req = offset_commit_request(group, generation_id, member_id, entries);
    let resp = conn
        .request(OFFSET_COMMIT_V8, &req)
        .await
        .map_err(|e| proto_fail(e, conn))?;
    Ok(resp.body)
}

/// Send an `OffsetFetch` v8 request and decode the response.
pub async fn offset_fetch(
    conn: &mut Conn,
    group: &str,
    topics: Option<&[(&str, Vec<i32>)]>,
) -> Result<OffsetFetchResponse, Failure> {
    let req = offset_fetch_request(group, topics);
    let resp = conn
        .request(OFFSET_FETCH_V8, &req)
        .await
        .map_err(|e| proto_fail(e, conn))?;
    Ok(resp.body)
}

/// One committed offset as `OffsetFetch` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedOffset {
    /// The committed offset, -1 when nothing was ever committed.
    pub offset: i64,
    /// The metadata string that was committed with it.
    pub metadata: String,
    /// The per-partition error code.
    pub error_code: i16,
}

/// Pull one `(topic, partition)` out of an `OffsetFetch` v8 response.
pub fn fetched_offset(
    resp: &OffsetFetchResponse,
    topic: &str,
    partition: i32,
) -> Option<FetchedOffset> {
    resp.groups
        .first()?
        .topics
        .iter()
        .find(|t| t.name.as_str() == topic)?
        .partitions
        .iter()
        .find(|p| p.partition_index == partition)
        .map(|p| FetchedOffset {
            offset: p.committed_offset,
            metadata: p
                .metadata
                .as_ref()
                .map(|m| m.to_string())
                .unwrap_or_default(),
            error_code: p.error_code,
        })
}

/// The group-level error of an `OffsetFetch` v8 response.
pub fn offset_fetch_group_error(resp: &OffsetFetchResponse) -> i16 {
    resp.groups
        .first()
        .map(|g| g.error_code)
        .unwrap_or(resp.error_code)
}

/// A member that has joined and synced, so its group is stable.
pub struct StableMember {
    /// The connection the member was created on; keep using it for heartbeats.
    pub conn: Conn,
    /// The member id the coordinator handed out.
    pub member_id: String,
    /// The generation the member is in.
    pub generation_id: i32,
}

/// Join and sync a single member so the group is stable, failing loudly on any error.
pub async fn stable_member(ctx: &Ctx, group: &str, topic: &str) -> Result<StableMember, Failure> {
    await_coordinator(ctx, group).await?;
    let mut conn = ctx.connect().await?;
    let outcome = join_new(&mut conn, group, &[topic]).await?;
    let mut c = Check::new(format!("bringing one member into group '{group}'"), &conn);
    c.eq("join.response.error_code", 0i16, outcome.settled.error_code);
    c.finish()?;
    let generation_id = outcome.settled.generation_id;
    let assignment = assignment_v0(&[(topic, &[0])]);
    let synced = sync(
        &mut conn,
        group,
        generation_id,
        &outcome.member_id,
        &[(outcome.member_id.as_str(), assignment)],
    )
    .await?;
    let mut c = Check::new(format!("syncing the leader of group '{group}'"), &conn);
    c.eq("sync.response.error_code", 0i16, synced.error_code);
    c.finish()?;
    Ok(StableMember {
        conn,
        member_id: outcome.member_id,
        generation_id,
    })
}

/// A second member's join, which may or may not still be waiting for the rebalance.
pub struct SecondMember {
    /// The member id, empty when the broker has not handed one out yet.
    pub member_id: String,
    /// Correlation id of a join that is still open; `None` when it already answered.
    pub pending: Option<i32>,
    /// The join response, when the broker answered straight away.
    pub settled: Option<JoinGroupResponse>,
    /// True when the broker answered the empty-member-id join with 79 (KIP-394).
    pub member_id_required: bool,
}

/// Start a second member's join, coping with both join protocols.
///
/// Kafka answers a join with an empty member id immediately with 79 MEMBER_ID_REQUIRED and
/// a fresh member id (KIP-394); a simpler broker just holds the join open until the
/// rebalance finishes. Either way this leaves exactly one join in flight.
pub async fn begin_second_member(
    conn: &mut Conn,
    group: &str,
    topics: &[&str],
) -> Result<SecondMember, Failure> {
    let first = begin_join(conn, group, "", topics).await?;
    match conn.read_frame_within(Duration::from_millis(750)).await {
        Ok(raw) => {
            let decoded = crate::proto::decode_response::<JoinGroupRequest>(JOIN_GROUP_V9, &raw)
                .map_err(|e| proto_fail(e, conn))?;
            let body = decoded.body;
            let member_id = body.member_id.to_string();
            if body.error_code == MEMBER_ID_REQUIRED {
                let pending = begin_join(conn, group, &member_id, topics).await?;
                Ok(SecondMember {
                    member_id,
                    pending: Some(pending),
                    settled: None,
                    member_id_required: true,
                })
            } else {
                Ok(SecondMember {
                    member_id,
                    pending: None,
                    settled: Some(body),
                    member_id_required: false,
                })
            }
        }
        Err(crate::proto::ProtoError::Timeout(_)) => Ok(SecondMember {
            member_id: String::new(),
            pending: Some(first),
            settled: None,
            member_id_required: false,
        }),
        Err(e) => Err(proto_fail(e, conn)),
    }
}

/// A group that a second member has just rebalanced.
pub struct Rebalance {
    /// The first member; `generation_id` is the generation it re-joined into.
    pub first: StableMember,
    /// The second member's connection; keep it open to keep the member in the group.
    pub second: Conn,
    /// The second member's id.
    pub second_member_id: String,
    /// The error the first member's heartbeat returned while the rebalance was in flight.
    pub heartbeat_error: i16,
    /// The first member's re-join, which settled the new generation.
    pub rejoin: JoinGroupResponse,
    /// The second member's join.
    pub second_join: JoinGroupResponse,
    /// True when the broker asked the second member for a member id first (KIP-394).
    pub member_id_required: bool,
}

/// Make a stable one-member group, then bring a second member in and let the rebalance run.
pub async fn rebalance_with_second_member(
    ctx: &Ctx,
    group: &str,
    topic: &str,
) -> Result<Rebalance, Failure> {
    let mut first = stable_member(ctx, group, topic).await?;
    let mut second = ctx.connect().await?;
    let pending = begin_second_member(&mut second, group, &[topic]).await?;
    let hb = await_heartbeat_code(
        &mut first.conn,
        group,
        first.generation_id,
        &first.member_id,
        REBALANCE_IN_PROGRESS,
        Duration::from_millis(4_000),
    )
    .await?;
    let rejoin = join(&mut first.conn, group, &first.member_id, &[topic]).await?;
    first.generation_id = rejoin.generation_id;
    let second_join = match pending.pending {
        Some(id) => finish_join(&mut second, id).await?,
        None => pending.settled.clone().unwrap_or_default(),
    };
    let second_member_id = if pending.member_id.is_empty() {
        second_join.member_id.to_string()
    } else {
        pending.member_id.clone()
    };
    Ok(Rebalance {
        first,
        second,
        second_member_id,
        heartbeat_error: hb.error_code,
        rejoin,
        second_join,
        member_id_required: pending.member_id_required,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_subscription_is_a_versioned_string_array() {
        let b = subscription_v0(&["t1", "t2"]);
        assert_eq!(&b[0..2], &[0, 0], "version 0");
        assert_eq!(&b[2..6], &[0, 0, 0, 2], "two topics");
        assert_eq!(&b[6..8], &[0, 2], "length of 't1'");
        assert_eq!(&b[8..10], b"t1");
        assert_eq!(
            &b[b.len() - 4..],
            &[0xff, 0xff, 0xff, 0xff],
            "null user data"
        );
    }

    #[test]
    fn an_assignment_round_trips() {
        let b = assignment_v0(&[("t1", &[0, 1]), ("t2", &[3])]);
        let decoded = decode_assignment_v0(&b).expect("decodes");
        assert_eq!(decoded.version, 0);
        assert_eq!(
            decoded.assigned,
            vec![("t1".to_string(), vec![0, 1]), ("t2".to_string(), vec![3]),]
        );
    }

    #[test]
    fn a_truncated_assignment_decodes_to_none() {
        let b = assignment_v0(&[("t1", &[0, 1])]);
        assert!(decode_assignment_v0(&b[..b.len() - 3]).is_none());
        assert!(decode_assignment_v0(&[]).is_none());
    }

    #[test]
    fn an_empty_assignment_is_still_valid() {
        let b = assignment_v0(&[]);
        let decoded = decode_assignment_v0(&b).expect("decodes");
        assert!(decoded.assigned.is_empty());
    }

    #[test]
    fn commit_requests_group_partitions_by_topic() {
        let r = offset_commit_request(
            "g",
            -1,
            "",
            &[("t", 0, 5, Some("m")), ("t", 1, 6, None), ("u", 0, 7, None)],
        );
        assert_eq!(r.topics.len(), 2);
        assert_eq!(r.topics[0].partitions.len(), 2);
        assert_eq!(r.topics[0].partitions[0].committed_offset, 5);
        assert_eq!(r.generation_id_or_member_epoch, -1);
    }

    #[test]
    fn offset_fetch_requests_carry_one_group() {
        let r = offset_fetch_request("g", Some(&[("t", vec![0, 1])]));
        assert_eq!(r.groups.len(), 1);
        assert_eq!(
            r.groups[0]
                .topics
                .as_ref()
                .map(|t| t[0].partition_indexes.clone()),
            Some(vec![0, 1])
        );
        let all = offset_fetch_request("g", None);
        assert!(all.groups[0].topics.is_none(), "None means every topic");
    }
}
