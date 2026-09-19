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
use crate::proto::records::RecordBatch;
use crate::proto::ProtoError;
use crate::stages::transactions::{
    add_partition, await_stable_offset, end_txn, expect_ok, init_transactional,
    produce_transactional, txn_id, Producer, ADD_PARTITIONS_TO_TXN_KEY, ADD_PARTITIONS_TO_TXN_V3,
    END_TXN_KEY,
};
use crate::stages::{
    api_versions, error_label, expect_still_serving, fetch_request, proto_fail, require_api, Ctx,
    Stage, Test, FETCH_V16, NONE,
};
use kafka_protocol::messages::{AddPartitionsToTxnRequest, ProducerId};

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
             the producer id and epoch, and the Produce request names the transactional id too",
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
                "the aborted transaction is reported from the offset it began at",
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
            env.request(ADD_PARTITIONS_TO_TXN_V3, 471, &req)
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

/// One whole transaction: add the partition, write the values, commit or abort, and wait
/// for the control record to land. Returns the producer and the records' base offset.
async fn run_transaction(
    ctx: &Ctx,
    id: &str,
    topic: &TopicInfo,
    values: &[&str],
    commit: bool,
) -> Result<(Producer, i64), Failure> {
    let producer = init_transactional(ctx, id).await?;
    expect_ok(
        "AddPartitionsToTxn",
        add_partition(ctx, id, producer, &topic.name).await?,
    )?;
    let (code, base) = produce_transactional(ctx, id, &topic.name, producer, values).await?;
    expect_ok("the transactional Produce", code)?;
    expect_ok(
        if commit {
            "EndTxn (commit)"
        } else {
            "EndTxn (abort)"
        },
        end_txn(ctx, id, producer, commit).await?,
    )?;
    // The records, then the marker.
    await_stable_offset(ctx, topic, base + values.len() as i64 + 1).await?;
    Ok((producer, base))
}

/// One aborted transaction as a read_committed fetch reports it: `(producer_id, first_offset)`.
type Aborted = (i64, i64);

/// One data batch as a consumer sees it: whose it is, where it starts, what it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DataBatch {
    producer_id: i64,
    base_offset: i64,
    values: Vec<String>,
}

/// What one Fetch at an isolation level returned.
struct Fetched {
    batches: Vec<DataBatch>,
    high_watermark: i64,
    last_stable_offset: i64,
    aborted: Vec<Aborted>,
}

impl Fetched {
    /// Every record value, in log order, markers skipped.
    fn values(&self) -> Vec<String> {
        self.batches.iter().flat_map(|b| b.values.clone()).collect()
    }

    /// The consumer's half of read_committed: drop every batch that belongs to a producer
    /// whose aborted transaction began at or before the batch.
    ///
    /// This is exactly the rule a real consumer applies. The aborted list is keyed by
    /// producer id and first offset because one producer can have committed a transaction
    /// on this partition and then aborted the next; only the batches from the aborted one
    /// onwards go.
    fn committed_values(&self) -> Vec<String> {
        self.batches
            .iter()
            .filter(|b| {
                !self.aborted.iter().any(|&(producer_id, first_offset)| {
                    producer_id == b.producer_id && first_offset <= b.base_offset
                })
            })
            .flat_map(|b| b.values.clone())
            .collect()
    }
}

/// Fetch partition 0 from `offset` at an isolation level.
async fn fetch_at(
    ctx: &Ctx,
    topic: &TopicInfo,
    offset: i64,
    isolation: i8,
) -> Result<Fetched, Failure> {
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
    let batches = RecordBatch::decode_all(&bytes).map_err(|e| {
        Failure::proto(
            ProtoError::Decode(format!("record batches: {e:#}")),
            Some(&conn),
        )
    })?;
    // Control records (the commit and abort markers) carry the control bit and are not
    // application data, so they are skipped the way a consumer skips them.
    let batches = batches
        .into_iter()
        .filter(|b| !b.is_control())
        .map(|b| DataBatch {
            producer_id: b.producer_id,
            base_offset: b.base_offset,
            values: b
                .records
                .into_iter()
                .filter_map(|r| r.value.map(|v| String::from_utf8_lossy(&v).to_string()))
                .collect(),
        })
        .collect();
    let aborted = p
        .aborted_transactions
        .as_ref()
        .map(|list| {
            list.iter()
                .map(|a| (a.producer_id.0, a.first_offset))
                .collect()
        })
        .unwrap_or_default();
    Ok(Fetched {
        batches,
        high_watermark: p.high_watermark,
        last_stable_offset: p.last_stable_offset,
        aborted,
    })
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| v.to_string()).collect()
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

kafka_test!(advertises_the_apis, |ctx| {
    let mut conn = ctx.connect().await?;
    let versions = api_versions(&mut conn).await?;
    let (_, add) = require_api(
        &versions,
        ADD_PARTITIONS_TO_TXN_KEY,
        "AddPartitionsToTxn",
        &conn,
    )?;
    let (_, end) = require_api(&versions, END_TXN_KEY, "EndTxn", &conn)?;
    let mut c = Check::new("the two APIs a transaction is bracketed by", &conn);
    c.at_least("ApiVersions.AddPartitionsToTxn.max_version", 0i16, add);
    c.at_least("ApiVersions.EndTxn.max_version", 0i16, end);
    c.finish()
});

kafka_test!(add_a_partition, |ctx| {
    let topic = ctx.topic("txn")?.name.clone();
    let id = "kafkatest-add-id";
    let producer = init_transactional(ctx, id).await?;
    let code = add_partition(ctx, id, producer, &topic).await?;
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
    let topic = ctx.topic("txn")?.clone();
    let (_, base) = run_transaction(ctx, "kafkatest-commit-id", &topic, &["a", "b"], true).await?;
    let fetched = fetch_at(ctx, &topic, base, 1).await?;
    let conn = ctx.connect().await?;
    let mut c = Check::new("a committed transaction, read at read_committed", &conn);
    c.note(
        "The whole flow in one test: add the partition, write with the transactional bit \
         set, commit, and read back at the isolation level a real consumer uses.",
    );
    c.eq("the records", strings(&["a", "b"]), fetched.values());
    c.eq(
        "aborted_transactions",
        Vec::<Aborted>::new(),
        fetched.aborted,
    );
    c.finish()
});

kafka_test!(the_control_record, |ctx| {
    let topic = ctx.topic("txn")?.clone();
    let (_, base) = run_transaction(ctx, "kafkatest-marker-id", &topic, &["x"], true).await?;
    let fetched = fetch_at(ctx, &topic, base, 1).await?;
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
        fetched.high_watermark,
    );
    c.eq(
        "the last stable offset, now that nothing is open",
        fetched.high_watermark,
        fetched.last_stable_offset,
    );
    c.finish()
});

kafka_test!(abort_is_in_the_log, |ctx| {
    let topic = ctx.topic("txn")?.clone();
    let (_, base) = run_transaction(ctx, "kafkatest-abort-id", &topic, &["doomed"], false).await?;
    let fetched = fetch_at(ctx, &topic, base, 0).await?;
    let conn = ctx.connect().await?;
    let mut c = Check::new("an aborted transaction, read at read_uncommitted", &conn);
    c.note(
        "Aborting does not delete anything. The records were appended when they were \
         produced and they are still there; what the abort wrote is a marker saying they \
         should be ignored. A consumer at isolation_level 0 has asked to see them anyway.",
    );
    c.eq("the records", strings(&["doomed"]), fetched.values());
    c.eq(
        "the high watermark, relative to where the transaction began",
        base + 2,
        fetched.high_watermark,
    );
    c.finish()
});

kafka_test!(abort_is_filtered, |ctx| {
    let topic = ctx.topic("txn")?.clone();
    let (producer, base) =
        run_transaction(ctx, "kafkatest-abort-filter-id", &topic, &["doomed"], false).await?;
    let fetched = fetch_at(ctx, &topic, base, 1).await?;
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
        strings(&["doomed"]),
        fetched.values(),
    );
    c.that(
        "and names the producer that aborted",
        &format!("a list containing producer id {}", producer.id),
        fetched.aborted.iter().any(|&(pid, _)| pid == producer.id),
        format!("{:?}", fetched.aborted),
    );
    c.eq(
        "the batch carries that producer id, which is what the consumer matches on",
        vec![producer.id],
        fetched
            .batches
            .iter()
            .map(|b| b.producer_id)
            .collect::<Vec<_>>(),
    );
    // The consumer's half: drop the batches the list names.
    c.eq(
        "so a consumer applying the list keeps nothing",
        Vec::<String>::new(),
        fetched.committed_values(),
    );
    c.finish()
});

kafka_test!(aborted_transactions_are_listed, |ctx| {
    let topic = ctx.topic("txn")?.clone();
    let (producer, base) =
        run_transaction(ctx, "kafkatest-abort-list-id", &topic, &["doomed"], false).await?;
    let fetched = fetch_at(ctx, &topic, base, 1).await?;
    let conn = ctx.connect().await?;
    let mut c = Check::new(
        "what a read_committed fetch reports alongside the records",
        &conn,
    );
    c.note(
        "The broker tells the consumer which producer id had an aborted transaction and the \
         offset it began at, so a consumer reading a batch that spans the abort can drop \
         exactly the right records and keep the ones that producer committed earlier. A \
         broker that filters silently and reports nothing leaves a client with no way to do \
         the same.",
    );
    c.eq(
        "aborted_transactions",
        vec![(producer.id, base)],
        fetched.aborted,
    );
    c.finish()
});

kafka_test!(an_open_transaction_holds_the_lso, |ctx| {
    let topic = ctx.topic("txn")?.clone();
    let id = "kafkatest-open-id";
    let producer = init_transactional(ctx, id).await?;
    expect_ok(
        "AddPartitionsToTxn",
        add_partition(ctx, id, producer, &topic.name).await?,
    )?;
    let (code, base) = produce_transactional(ctx, id, &topic.name, producer, &["pending"]).await?;
    expect_ok("the transactional Produce", code)?;
    let fetched = fetch_at(ctx, &topic, base, 1).await?;
    // Leave nothing open behind us.
    end_txn(ctx, id, producer, true).await?;
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
        fetched.high_watermark > base,
        fetched.high_watermark,
    );
    c.that(
        "the last stable offset has not",
        &format!("no greater than {base}"),
        fetched.last_stable_offset <= base,
        fetched.last_stable_offset,
    );
    c.eq(
        "and nothing is returned",
        Vec::<String>::new(),
        fetched.values(),
    );
    c.finish()
});

kafka_test!(still_serving, |ctx| {
    let topic = ctx.topic("txn")?.name.clone();
    expect_still_serving(ctx, &topic).await
});
