//! Stage 47 — A transaction from `AddPartitionsToTxn` to the control record.
//!
//! Stage 46 got a producer id and an epoch from the coordinator. This is what a producer
//! does with them, and it is four steps rather than one.
//!
//! **Tell the coordinator which partitions are involved** (`AddPartitionsToTxn`), before
//! writing to any of them — the coordinator has to know where to write markers if the
//! transaction is later aborted, and a producer that skips this and writes anyway is
//! refused. **Produce**, with the batch's transactional bit set and the producer id and
//! epoch stamped in. **End the transaction** (`EndTxn`) with commit or abort, at which point
//! the coordinator writes a **control record** into every partition the transaction touched.
//! That marker is a real record occupying a real offset, which is why the offsets a
//! consumer sees have gaps in them and why "the offset went up by two when I wrote one
//! record" is a normal thing to observe.
//!
//! Then the reading half. A consumer at `isolation_level` 0 sees everything the moment it
//! is appended, aborted or not. At `isolation_level` 1 it sees nothing past the **last
//! stable offset** — the point beyond which some transaction is still open — and the broker
//! hands it the list of aborted transactions so it can drop those records. Exactly-once is
//! that pair working together: a producer that cannot write twice, and a consumer that does
//! not read what was never committed.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::fixtures::{FixtureSpec, TopicInfo, TopicSpec};
use crate::kafka_test;
use crate::proto::records::{RecordBatch, RecordItem};
use crate::stages::{
    api_versions, error_label, expect_still_serving, fetch_request, produce_request, proto_fail,
    require_api, Ctx, Stage, Test, FETCH_V16, NONE, PRODUCE_V11,
};
use kafka_protocol::messages::{
    AddPartitionsToTxnRequest, EndTxnRequest, InitProducerIdRequest, ProducerId, TransactionalId,
};
use kafka_protocol::protocol::StrBytes;

const INIT_PRODUCER_ID_V4: i16 = 4;
const ADD_PARTITIONS_KEY: i16 = 24;
const END_TXN_KEY: i16 = 26;
const TRANSACTION_TIMEOUT_MS: i32 = 60_000;
const RETRIABLE: &[i16] = &[7, 14, 15, 16, 51];

/// The batch attribute bit that says "this batch belongs to a transaction".
const TRANSACTIONAL_BIT: i16 = 1 << 4;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 47,
        slug: "transactional_writes",
        name: "A transaction, end to end",
        ext: true,
        hints: &[
            "`AddPartitionsToTxn` (24) comes before the first write to a partition: the \
             coordinator must know where to put markers if the transaction aborts",
            "A transactional batch sets bit 4 of the record batch attributes as well as carrying \
             the producer id and epoch",
            "`EndTxn` (26) with `committed` true or false makes the coordinator write a control \
             record into every partition the transaction touched — a real record at a real offset",
            "A consumer at isolation_level 1 reads nothing past the last stable offset and is \
             given the aborted transactions to filter; at level 0 it sees everything immediately",
        ],
        examples,
        tests: vec![
            Test::new(
                "ApiVersions advertises AddPartitionsToTxn and EndTxn",
                advertises_the_apis,
            )
            .ext(),
            Test::new("a partition can be added to a transaction", add_a_partition)
                .ext()
                .with_fixtures(fixtures),
            Test::new(
                "a committed transaction's records are readable",
                commit_is_visible,
            )
            .ext()
            .with_fixtures(fixtures),
            Test::new(
                "the commit wrote a control record after them",
                the_control_record,
            )
            .ext()
            .with_fixtures(fixtures),
            Test::new(
                "an aborted transaction's records are in the log at read_uncommitted",
                abort_is_in_the_log,
            )
            .ext()
            .with_fixtures(fixtures),
            Test::new(
                "and read_committed hands the consumer the list to drop them by",
                abort_is_filtered,
            )
            .ext()
            .with_fixtures(fixtures),
            Test::new(
                "read_committed reports the aborted transaction so a consumer can drop it",
                aborted_transactions_are_listed,
            )
            .ext()
            .with_fixtures(fixtures),
            Test::new(
                "an open transaction holds the last stable offset back",
                an_open_transaction_holds_the_lso,
            )
            .ext()
            .with_fixtures(fixtures),
            Test::new("the broker is still serving afterwards", still_serving)
                .ext()
                .with_fixtures(fixtures),
        ],
    }
}

fn fixtures() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("txn", 1))
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("AddPartitionsToTxn before the first write", |env| {
            let mut req = AddPartitionsToTxnRequest::default();
            req.v3_and_below_transactional_id = txn_id("kafkatest-example-txn");
            req.v3_and_below_producer_id = ProducerId(90_000);
            req.v3_and_below_producer_epoch = 0;
            env.request(3, 471, &req)
        })
        .request(
            "AddPartitionsToTxn (api_key 24) v3, correlation id 471: the transactional id, the \
         producer id and epoch the coordinator handed out, and the topics and partitions this \
         transaction is about to write to",
        )
        .response("One entry per partition, each with error_code 0")
        .note(
            "This is the step that makes an abort possible. The coordinator records the \
         partitions, and if the transaction ends in an abort it knows which logs need an \
         abort marker — without it there would be records in partitions nobody could tell a \
         consumer to ignore.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// The transactional flow
// ---------------------------------------------------------------------------------------

fn txn_id(s: &str) -> TransactionalId {
    TransactionalId(StrBytes::from_string(s.to_string()))
}

/// Find the coordinator for an id, retrying while `__transaction_state` comes up.
async fn find_coordinator(ctx: &Ctx, id: &str) -> Result<(), Failure> {
    use crate::stages::group_protocol::{
        find_coordinator_request, FIND_COORDINATOR_KEY, FIND_COORDINATOR_V4,
    };
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
        let code = resp
            .coordinators
            .first()
            .map(|c| c.error_code)
            .unwrap_or(-1);
        if !RETRIABLE.contains(&code) || tokio::time::Instant::now() >= deadline {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// Begin a producer session for a transactional id.
async fn begin_producer(ctx: &Ctx, id: &str) -> Result<(i64, i16), Failure> {
    find_coordinator(ctx, id).await?;
    let mut conn = ctx.connect().await?;
    let versions = api_versions(&mut conn).await?;
    let (_, max) = require_api(
        &versions,
        crate::stages::INIT_PRODUCER_ID_KEY,
        "InitProducerId",
        &conn,
    )?;
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
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        find_coordinator(ctx, id).await?;
        resp = conn
            .request(version, &req)
            .await
            .map_err(|e| proto_fail(e, &conn))?;
    }
    if resp.error_code != NONE {
        return Err(Failure::harness(format!(
            "InitProducerId for {id:?} answered {}",
            error_label(resp.error_code)
        )));
    }
    Ok((resp.producer_id.0, resp.producer_epoch))
}

/// `AddPartitionsToTxn` for one partition, returning the error code.
async fn add_partition(
    ctx: &Ctx,
    id: &str,
    pid: i64,
    epoch: i16,
    topic: &str,
) -> Result<i16, Failure> {
    use kafka_protocol::messages::add_partitions_to_txn_request::AddPartitionsToTxnTopic;
    let mut conn = ctx.connect().await?;
    let versions = api_versions(&mut conn).await?;
    let (_, max) = require_api(&versions, ADD_PARTITIONS_KEY, "AddPartitionsToTxn", &conn)?;
    let version = max.min(3);
    let mut t = AddPartitionsToTxnTopic::default();
    t.name = kafka_protocol::messages::TopicName(StrBytes::from_string(topic.to_string()));
    t.partitions = vec![0];
    let mut req = AddPartitionsToTxnRequest::default();
    req.v3_and_below_transactional_id = txn_id(id);
    req.v3_and_below_producer_id = ProducerId(pid);
    req.v3_and_below_producer_epoch = epoch;
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

/// Produce one transactional batch, returning `(error_code, base_offset)`.
async fn produce_transactional(
    ctx: &Ctx,
    id: &str,
    topic: &str,
    pid: i64,
    epoch: i16,
    values: &[&str],
) -> Result<(i16, i64), Failure> {
    let mut batch = RecordBatch::of(
        0,
        1_700_000_000_000,
        values.iter().map(|v| RecordItem::value(*v)).collect(),
    );
    batch.producer_id = pid;
    batch.producer_epoch = epoch;
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

/// `EndTxn` with commit or abort.
async fn end_txn(ctx: &Ctx, id: &str, pid: i64, epoch: i16, commit: bool) -> Result<i16, Failure> {
    let mut conn = ctx.connect().await?;
    let versions = api_versions(&mut conn).await?;
    let (_, max) = require_api(&versions, END_TXN_KEY, "EndTxn", &conn)?;
    let version = max.min(3);
    let mut req = EndTxnRequest::default();
    req.transactional_id = txn_id(id);
    req.producer_id = ProducerId(pid);
    req.producer_epoch = epoch;
    req.committed = commit;
    let resp = conn
        .request(version, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    Ok(resp.error_code)
}

/// One whole transaction: add the partition, write the values, commit or abort.
async fn run_transaction(
    ctx: &Ctx,
    id: &str,
    topic: &str,
    values: &[&str],
    commit: bool,
) -> Result<i64, Failure> {
    let (pid, epoch) = begin_producer(ctx, id).await?;
    let added = add_partition(ctx, id, pid, epoch, topic).await?;
    if added != NONE {
        return Err(Failure::harness(format!(
            "AddPartitionsToTxn answered {}",
            error_label(added)
        )));
    }
    let (code, base) = produce_transactional(ctx, id, topic, pid, epoch, values).await?;
    if code != NONE {
        return Err(Failure::harness(format!(
            "the transactional produce answered {}",
            error_label(code)
        )));
    }
    let ended = end_txn(ctx, id, pid, epoch, commit).await?;
    if ended != NONE {
        return Err(Failure::harness(format!(
            "EndTxn answered {}",
            error_label(ended)
        )));
    }
    Ok(base)
}

/// Fetch at an isolation level, returning `(records, high_watermark, last_stable_offset,
/// aborted producer ids)`.
async fn fetch_at(
    ctx: &Ctx,
    topic: &TopicInfo,
    offset: i64,
    isolation: i8,
) -> Result<(Vec<String>, i64, i64, Vec<i64>), Failure> {
    let mut conn = ctx.connect().await?;
    let mut req = fetch_request(&[(topic.id, 0, offset)], 1_000);
    req.isolation_level = isolation;
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let Some(p) = resp.responses.first().and_then(|t| t.partitions.first()) else {
        return Err(Failure::harness("the Fetch returned no partition entry"));
    };
    let bytes = p.records.clone().unwrap_or_default();
    // Control records (the commit and abort markers) carry the control bit and are not
    // application data, so they are skipped the way a consumer skips them.
    let values: Vec<String> = RecordBatch::decode_all(&bytes)
        .unwrap_or_default()
        .into_iter()
        .filter(|b| !b.is_control())
        .flat_map(|b| {
            b.records
                .into_iter()
                .filter_map(|r| r.value.map(|v| String::from_utf8_lossy(&v).to_string()))
                .collect::<Vec<_>>()
        })
        .collect();
    let aborted = p
        .aborted_transactions
        .as_ref()
        .map(|list| list.iter().map(|a| a.producer_id.0).collect::<Vec<_>>())
        .unwrap_or_default();
    Ok((values, p.high_watermark, p.last_stable_offset, aborted))
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

kafka_test!(advertises_the_apis, |ctx| {
    let mut conn = ctx.connect().await?;
    let versions = api_versions(&mut conn).await?;
    let (_, add) = require_api(&versions, ADD_PARTITIONS_KEY, "AddPartitionsToTxn", &conn)?;
    let (_, end) = require_api(&versions, END_TXN_KEY, "EndTxn", &conn)?;
    let mut c = Check::new("the two APIs a transaction is bracketed by", &conn);
    c.at_least("ApiVersions.AddPartitionsToTxn.max_version", 0i16, add);
    c.at_least("ApiVersions.EndTxn.max_version", 0i16, end);
    c.finish()
});

kafka_test!(add_a_partition, |ctx| {
    let topic = ctx.topic("txn")?.name.clone();
    let id = "kafkatest-add-id";
    let (pid, epoch) = begin_producer(ctx, id).await?;
    let code = add_partition(ctx, id, pid, epoch, &topic).await?;
    let conn = ctx.connect().await?;
    let mut c = Check::new("AddPartitionsToTxn for one partition", &conn);
    c.note(
        "The coordinator now knows this transaction touches this partition, which is the \
         only reason it can later write an abort marker there. Everything after this point \
         is recoverable; anything written before it would not be.",
    );
    c.that(
        "the partition's error_code",
        &error_label(NONE),
        code == NONE,
        error_label(code),
    );
    c.finish()
});

kafka_test!(commit_is_visible, |ctx| {
    let topic_info = ctx.topic("txn")?.clone();
    let base = run_transaction(
        ctx,
        "kafkatest-commit-id",
        &topic_info.name,
        &["a", "b"],
        true,
    )
    .await?;
    let (values, _, _, _) = fetch_at(ctx, &topic_info, base, 1).await?;
    let conn = ctx.connect().await?;
    let mut c = Check::new("a committed transaction, read at read_committed", &conn);
    c.note(
        "The whole flow in one test: add the partition, write with the transactional bit \
         set, commit, and read back at the isolation level a real consumer uses.",
    );
    c.eq(
        "the records",
        vec!["a".to_string(), "b".to_string()],
        values,
    );
    c.finish()
});

kafka_test!(the_control_record, |ctx| {
    let topic_info = ctx.topic("txn")?.clone();
    let base = run_transaction(ctx, "kafkatest-marker-id", &topic_info.name, &["x"], true).await?;
    let (_, high_watermark, _, _) = fetch_at(ctx, &topic_info, base, 1).await?;
    let conn = ctx.connect().await?;
    let mut c = Check::new("the offsets a one-record transaction occupies", &conn);
    c.note(
        "One record was written and the log advanced by two: the commit marker is a real \
         record at a real offset. This is why a consumer's offsets have gaps in them, and \
         why comparing 'records read' against 'offset moved' is a good way to confuse \
         yourself for an afternoon.",
    );
    c.eq(
        "the high watermark, relative to where the transaction began",
        base + 2,
        high_watermark,
    );
    c.finish()
});

kafka_test!(abort_is_in_the_log, |ctx| {
    let topic_info = ctx.topic("txn")?.clone();
    let base = run_transaction(
        ctx,
        "kafkatest-abort-id",
        &topic_info.name,
        &["doomed"],
        false,
    )
    .await?;
    let (values, _, _, _) = fetch_at(ctx, &topic_info, base, 0).await?;
    let conn = ctx.connect().await?;
    let mut c = Check::new("an aborted transaction, read at read_uncommitted", &conn);
    c.note(
        "Aborting does not delete anything. The records were appended when they were \
         produced and they are still there; what the abort wrote is a marker saying they \
         should be ignored. A consumer at isolation_level 0 has asked to see them anyway.",
    );
    c.eq("the records", vec!["doomed".to_string()], values);
    c.finish()
});

kafka_test!(abort_is_filtered, |ctx| {
    let topic_info = ctx.topic("txn")?.clone();
    let id = "kafkatest-abort-filter-id";
    let (pid, epoch) = begin_producer(ctx, id).await?;
    add_partition(ctx, id, pid, epoch, &topic_info.name).await?;
    let (_, base) =
        produce_transactional(ctx, id, &topic_info.name, pid, epoch, &["doomed"]).await?;
    end_txn(ctx, id, pid, epoch, false).await?;
    let (values, _, _, aborted) = fetch_at(ctx, &topic_info, base, 1).await?;
    let conn = ctx.connect().await?;
    let mut c = Check::new("who actually does the filtering at read_committed", &conn);
    c.note(
        "The records come back. read_committed does not mean the broker strips aborted data \
         out of the batches — it means the broker stops at the last stable offset and tells \
         the consumer which producers aborted, and the *consumer* drops those records. \
         Sending the bytes and the list is cheaper than rewriting batches on every fetch, \
         and it is why a hand-written consumer that ignores aborted_transactions reads data \
         that was never committed.",
    );
    c.eq(
        "the broker still returns the bytes",
        vec!["doomed".to_string()],
        values.clone(),
    );
    c.that(
        "and names the producer that aborted",
        &format!("a list containing producer id {pid}"),
        aborted.contains(&pid),
        format!("{aborted:?}"),
    );
    // What a consumer does with that list.
    let kept: Vec<String> = if aborted.contains(&pid) {
        Vec::new()
    } else {
        values.clone()
    };
    c.eq(
        "so a consumer that uses the list keeps nothing",
        Vec::<String>::new(),
        kept,
    );
    c.finish()
});

kafka_test!(aborted_transactions_are_listed, |ctx| {
    let topic_info = ctx.topic("txn")?.clone();
    let base = run_transaction(
        ctx,
        "kafkatest-abort-list-id",
        &topic_info.name,
        &["doomed"],
        false,
    )
    .await?;
    let (_, _, _, aborted) = fetch_at(ctx, &topic_info, base, 1).await?;
    let conn = ctx.connect().await?;
    let mut c = Check::new(
        "what a read_committed fetch reports alongside the records",
        &conn,
    );
    c.note(
        "The broker tells the consumer which producer ids had an aborted transaction open \
         and from which offset, so a consumer reading a batch that spans the abort can drop \
         exactly the right records. A broker that filters silently and reports nothing \
         leaves a client with no way to do the same.",
    );
    c.at_least("aborted_transactions", 1, aborted.len());
    c.finish()
});

kafka_test!(an_open_transaction_holds_the_lso, |ctx| {
    let topic_info = ctx.topic("txn")?.clone();
    let id = "kafkatest-open-id";
    let (pid, epoch) = begin_producer(ctx, id).await?;
    add_partition(ctx, id, pid, epoch, &topic_info.name).await?;
    let (_, base) =
        produce_transactional(ctx, id, &topic_info.name, pid, epoch, &["pending"]).await?;
    let (values, high_watermark, lso, _) = fetch_at(ctx, &topic_info, base, 1).await?;
    // Leave nothing open behind us.
    end_txn(ctx, id, pid, epoch, true).await?;
    let conn = ctx.connect().await?;
    let mut c = Check::new("a transaction that has written but not ended", &conn);
    c.note(
        "The record is in the log — the high watermark has moved past it — and the last \
         stable offset has not, because the broker does not yet know whether it will be \
         committed. A read_committed consumer stops at the LSO and sees nothing, which is \
         also why one stuck transaction stalls every such consumer on the partition.",
    );
    c.that(
        "the high watermark has moved past the record",
        &format!("greater than {base}"),
        high_watermark > base,
        high_watermark,
    );
    c.that(
        "the last stable offset has not",
        &format!("no greater than {base}"),
        lso <= base,
        lso,
    );
    c.eq("and nothing is returned", Vec::<String>::new(), values);
    c.finish()
});

kafka_test!(still_serving, |ctx| {
    let topic = ctx.topic("txn")?.name.clone();
    expect_still_serving(ctx, &topic).await
});
