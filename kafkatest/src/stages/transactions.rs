//! The transactional producer's requests, shared by stages 46 and 47.
//!
//! Every step of a transaction is a coordinator operation: the coordinator is a broker
//! chosen by hashing the transactional id, it may not exist yet on a freshly formatted
//! cluster, and it may answer NOT_COORDINATOR while `__transaction_state` moves. Real
//! clients retry all of that, and so does this module, which is why each helper here
//! re-runs `FindCoordinator` rather than caching an answer.

use crate::assert::{Check, Failure};
use crate::fixtures::TopicInfo;
use crate::proto::records::{RecordBatch, RecordItem};
use crate::stages::group_protocol::{
    find_coordinator_request, FIND_COORDINATOR_KEY, FIND_COORDINATOR_V4,
};
use crate::stages::{
    api_versions, error_label, fetch_request, produce_request, proto_fail, require_api, Ctx,
    FETCH_V16, INIT_PRODUCER_ID_KEY, NONE, PRODUCE_V11,
};
use kafka_protocol::messages::add_partitions_to_txn_request::AddPartitionsToTxnTopic;
use kafka_protocol::messages::{
    AddPartitionsToTxnRequest, EndTxnRequest, InitProducerIdRequest, ProducerId, TopicName,
    TransactionalId,
};
use kafka_protocol::protocol::StrBytes;
use std::time::Duration;

/// `AddPartitionsToTxn`
pub const ADD_PARTITIONS_TO_TXN_KEY: i16 = 24;
/// `EndTxn`
pub const END_TXN_KEY: i16 = 26;
/// The version of InitProducerId these stages ask for.
pub const INIT_PRODUCER_ID_V4: i16 = 4;
/// The version of AddPartitionsToTxn these stages ask for: the last one addressed to a
/// single transaction, before v4 made the request a batch for several producers.
pub const ADD_PARTITIONS_TO_TXN_V3: i16 = 3;
/// The version of EndTxn these stages ask for.
pub const END_TXN_V3: i16 = 3;
/// How long a transaction may stay open before the coordinator aborts it for us.
pub const TRANSACTION_TIMEOUT_MS: i32 = 60_000;
/// The record batch attribute bit that says "this batch belongs to a transaction".
pub const TRANSACTIONAL_BIT: i16 = 1 << 4;

/// Answers that mean "ask again", not "you are wrong".
///
/// 7 REQUEST_TIMED_OUT, 14 COORDINATOR_LOAD_IN_PROGRESS and 15 COORDINATOR_NOT_AVAILABLE are
/// the coordinator still coming up; 16 NOT_COORDINATOR is the broker saying it is not the
/// coordinator for this id *yet* — a real client answers that by re-running FindCoordinator
/// and retrying, which is what this module does too; 51 CONCURRENT_TRANSACTIONS is the
/// previous transaction under this id still being completed.
pub const RETRIABLE: &[i16] = &[7, 14, 15, 16, 51];

/// How long to wait between retries of a coordinator operation.
const RETRY_EVERY: Duration = Duration::from_millis(200);

/// A `TransactionalId` from a `&str`.
pub fn txn_id(s: &str) -> TransactionalId {
    TransactionalId(StrBytes::from_string(s.to_string()))
}

/// What `FindCoordinator` answered for a transactional id.
pub struct Coordinator {
    /// The error code of the one coordinators entry (-1 when there was none).
    pub error_code: i16,
    /// Its node id (-1 when there was none).
    pub node_id: i32,
    /// Its host (empty when there was none).
    pub host: String,
}

/// Find the transaction coordinator for an id, retrying while the broker brings the
/// internal `__transaction_state` topic into existence.
///
/// The retry is not politeness: a broker that has never seen a transaction has no such
/// topic, creates it on the first lookup, and answers COORDINATOR_NOT_AVAILABLE until its
/// partitions have leaders. Every real client retries this, and a suite that does not would
/// be testing the broker's start-up timing rather than its protocol.
pub async fn find_coordinator(ctx: &Ctx, id: &str) -> Result<Coordinator, Failure> {
    let mut conn = ctx.connect().await?;
    let versions = api_versions(&mut conn).await?;
    let (_, max) = require_api(&versions, FIND_COORDINATOR_KEY, "FindCoordinator", &conn)?;
    let version = max.min(FIND_COORDINATOR_V4);
    let req = find_coordinator_request(&[id], 1);
    let deadline = tokio::time::Instant::now() + ctx.timeout;
    loop {
        let resp = conn
            .request(version, &req)
            .await
            .map_err(|e| proto_fail(e, &conn))?;
        let entry = resp.coordinators.first();
        let error_code = entry.map(|c| c.error_code).unwrap_or(-1);
        if !RETRIABLE.contains(&error_code) || tokio::time::Instant::now() >= deadline {
            return Ok(Coordinator {
                error_code,
                node_id: entry.map(|c| c.node_id.0).unwrap_or(-1),
                host: entry.map(|c| c.host.to_string()).unwrap_or_default(),
            });
        }
        tokio::time::sleep(RETRY_EVERY).await;
    }
}

/// The producer id and epoch the coordinator hands a transactional id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Producer {
    pub id: i64,
    pub epoch: i16,
}

/// `InitProducerId` for a transactional id, retrying while the coordinator loads, and
/// asserting the answer is a usable producer.
pub async fn init_transactional(ctx: &Ctx, id: &str) -> Result<Producer, Failure> {
    // Establish that the coordinator exists before asking it for anything, exactly as a
    // client does: InitProducerId for a transactional id is a request to one specific
    // broker, and asking any other gets NOT_COORDINATOR.
    find_coordinator(ctx, id).await?;
    let mut conn = ctx.connect().await?;
    let versions = api_versions(&mut conn).await?;
    let (_, max) = require_api(&versions, INIT_PRODUCER_ID_KEY, "InitProducerId", &conn)?;
    let version = max.min(INIT_PRODUCER_ID_V4);
    let mut req = InitProducerIdRequest::default();
    req.transactional_id = Some(txn_id(id));
    req.transaction_timeout_ms = TRANSACTION_TIMEOUT_MS;
    let mut resp = conn
        .request(version, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let deadline = tokio::time::Instant::now() + ctx.timeout;
    while RETRIABLE.contains(&resp.error_code) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(RETRY_EVERY).await;
        // Re-find the coordinator each time round, which is what makes NOT_COORDINATOR a
        // retriable answer rather than a fatal one.
        find_coordinator(ctx, id).await?;
        resp = conn
            .request(version, &req)
            .await
            .map_err(|e| proto_fail(e, &conn))?;
    }
    let mut c = Check::new(format!("InitProducerId v{version} for {id:?}"), &conn);
    c.note(
        "a transactional id makes this a coordinator operation: the answer is state the \
         broker keeps under that id, not a number minted for this connection",
    );
    c.that(
        "response.error_code",
        &error_label(NONE),
        resp.error_code == NONE,
        error_label(resp.error_code),
    );
    c.at_least("response.producer_id", 0i64, resp.producer_id.0);
    c.at_least("response.producer_epoch", 0i16, resp.producer_epoch);
    c.finish()?;
    Ok(Producer {
        id: resp.producer_id.0,
        epoch: resp.producer_epoch,
    })
}

/// `InitProducerId` presenting an existing producer id and epoch, as a client does when it
/// resumes a session rather than starting one. Returns the error code without asserting.
pub async fn init_resuming(ctx: &Ctx, id: &str, producer: Producer) -> Result<i16, Failure> {
    find_coordinator(ctx, id).await?;
    let mut conn = ctx.connect().await?;
    let versions = api_versions(&mut conn).await?;
    let (_, max) = require_api(&versions, INIT_PRODUCER_ID_KEY, "InitProducerId", &conn)?;
    let version = max.min(INIT_PRODUCER_ID_V4);
    let mut req = InitProducerIdRequest::default();
    req.transactional_id = Some(txn_id(id));
    req.transaction_timeout_ms = TRANSACTION_TIMEOUT_MS;
    req.producer_id = ProducerId(producer.id);
    req.producer_epoch = producer.epoch;
    let resp = conn
        .request(version, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    Ok(resp.error_code)
}

/// `AddPartitionsToTxn` for partition 0 of one topic, returning the partition's error code.
pub async fn add_partition(
    ctx: &Ctx,
    id: &str,
    producer: Producer,
    topic: &str,
) -> Result<i16, Failure> {
    let mut conn = ctx.connect().await?;
    let versions = api_versions(&mut conn).await?;
    let (_, max) = require_api(
        &versions,
        ADD_PARTITIONS_TO_TXN_KEY,
        "AddPartitionsToTxn",
        &conn,
    )?;
    let version = max.min(ADD_PARTITIONS_TO_TXN_V3);
    let mut t = AddPartitionsToTxnTopic::default();
    t.name = TopicName(StrBytes::from_string(topic.to_string()));
    t.partitions = vec![0];
    let mut req = AddPartitionsToTxnRequest::default();
    req.v3_and_below_transactional_id = txn_id(id);
    req.v3_and_below_producer_id = ProducerId(producer.id);
    req.v3_and_below_producer_epoch = producer.epoch;
    req.v3_and_below_topics = vec![t];
    let resp = conn
        .request(version, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    Ok(resp
        .results_by_topic_v3_and_below
        .first()
        .and_then(|t| {
            t.results_by_partition
                .first()
                .map(|p| p.partition_error_code)
        })
        .unwrap_or(-1))
}

/// Produce one transactional batch to partition 0, returning `(error_code, base_offset)`.
pub async fn produce_transactional(
    ctx: &Ctx,
    id: &str,
    topic: &str,
    producer: Producer,
    values: &[&str],
) -> Result<(i16, i64), Failure> {
    let mut batch = RecordBatch::of(
        0,
        1_700_000_000_000,
        values.iter().map(|v| RecordItem::value(*v)).collect(),
    );
    batch.producer_id = producer.id;
    batch.producer_epoch = producer.epoch;
    batch.base_sequence = 0;
    batch.attributes |= TRANSACTIONAL_BIT;
    let mut conn = ctx.connect().await?;
    let mut req = produce_request(&[(topic.to_string(), 0, batch.encode())], -1);
    // A Produce carrying transactional batches must name the transaction on the request as
    // well as in the batch header: without it the broker sees a batch claiming to be
    // transactional from a producer that has declared no transaction, and answers
    // INVALID_TXN_STATE (53).
    req.transactional_id = Some(txn_id(id));
    let resp = conn
        .request(PRODUCE_V11, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let p = resp
        .responses
        .first()
        .and_then(|t| t.partition_responses.first());
    Ok((
        p.map(|p| p.error_code).unwrap_or(-1),
        p.map(|p| p.base_offset).unwrap_or(-1),
    ))
}

/// `EndTxn` with commit or abort, returning the error code.
pub async fn end_txn(
    ctx: &Ctx,
    id: &str,
    producer: Producer,
    commit: bool,
) -> Result<i16, Failure> {
    let mut conn = ctx.connect().await?;
    let versions = api_versions(&mut conn).await?;
    let (_, max) = require_api(&versions, END_TXN_KEY, "EndTxn", &conn)?;
    let version = max.min(END_TXN_V3);
    let mut req = EndTxnRequest::default();
    req.transactional_id = txn_id(id);
    req.producer_id = ProducerId(producer.id);
    req.producer_epoch = producer.epoch;
    req.committed = commit;
    let resp = conn
        .request(version, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    Ok(resp.error_code)
}

/// Fail the test unless a step of the flow answered NONE.
///
/// A refused step is the broker under test getting the protocol wrong, so it is reported
/// as an assertion against that step and not as the harness failing to set the test up.
pub fn expect_ok(step: &str, code: i16) -> Result<(), Failure> {
    let mut c = Check::detached(format!("{step}, a step this test depends on"));
    c.that(
        "error_code",
        &error_label(NONE),
        code == NONE,
        error_label(code),
    );
    c.finish()
}

/// Wait until partition 0's last stable offset reaches `want`.
///
/// `EndTxn` answers once the coordinator has decided the outcome; the control records land
/// in the partitions afterwards, so a read_committed fetch straight after the response can
/// still see the transaction as open. Waiting on the LSO is what a consumer effectively
/// does, and it turns "the marker was never written" into a failure that says so.
pub async fn await_stable_offset(ctx: &Ctx, topic: &TopicInfo, want: i64) -> Result<(), Failure> {
    let deadline = tokio::time::Instant::now() + ctx.timeout;
    let mut last = -1i64;
    while tokio::time::Instant::now() < deadline {
        let mut conn = ctx.connect().await?;
        let mut req = fetch_request(&[(topic.id, 0, 0)], 100);
        req.isolation_level = 1;
        if let Ok(resp) = conn.request(FETCH_V16, &req).await {
            if let Some(p) = resp.responses.first().and_then(|t| t.partitions.first()) {
                last = p.last_stable_offset;
                if last >= want {
                    return Ok(());
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mut c = Check::detached(format!(
        "that the transaction's control record reached {}-0",
        topic.name
    ));
    c.note(
        "EndTxn was answered, but the last stable offset never moved past the transaction's \
         records: the coordinator did not write its commit or abort marker into the \
         partition, so a read_committed consumer would wait forever",
    );
    c.that(
        "last_stable_offset",
        &format!("at least {want}"),
        false,
        last,
    );
    c.finish()
}
