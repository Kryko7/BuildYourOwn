//! Stage 33 — the `acks` field of Produce: 0, 1, -1 and everything else.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::fixtures::{FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::proto::records::{RecordBatch, RecordItem};
use crate::proto::ProtoError;
use crate::stages::{
    api_versions_request, await_high_watermark, error_is_one_of, error_label, expect_still_serving,
    produce_request, proto_fail, Ctx, Stage, Test, API_VERSIONS_V4, INVALID_REQUIRED_ACKS, NONE,
    PRODUCE_V11,
};
use std::time::Duration;

/// How long an `acks=0` produce must stay unanswered before we believe the broker.
const QUIET: Duration = Duration::from_millis(500);

fn two_partitions() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 2))
}

fn batch(values: &[&str]) -> Vec<u8> {
    RecordBatch::of(
        0,
        1_700_000_000_000,
        values.iter().map(|v| RecordItem::value(*v)).collect(),
    )
    .encode()
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 33,
        slug: "produce_acks",
        name: "acks 0, 1 and -1",
        ext: true,
        hints: &[
            "acks=0 means no response at all: append the records and write nothing back, \
             but keep reading the connection — the next request still needs an answer",
            "acks=1 answers once the leader's own log has the batch, acks=-1 once every \
             in-sync replica does; with one broker both answer the same base_offset",
            "Any other value (2, 3, -2, ...) is error 21 INVALID_REQUIRED_ACKS, one entry \
             per partition, and the request is not appended",
            "Check acks before you append, not after: an invalid request must not change \
             the log",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("acks=1 answers with the base offset", acks_one)
                .with_fixtures(two_partitions)
                .ext(),
            Test::new("acks=-1 answers with the base offset", acks_all)
                .with_fixtures(two_partitions)
                .ext(),
            Test::new("acks=0 sends no response at all", acks_zero_is_silent)
                .with_fixtures(two_partitions)
                .ext(),
            Test::new(
                "the next request on an acks=0 connection is answered normally",
                acks_zero_then_api_versions,
            )
            .with_fixtures(two_partitions)
            .ext(),
            Test::new("acks=0 still appends the records", acks_zero_appends)
                .with_fixtures(two_partitions)
                .ext(),
            Test::new("acks=2 is error 21 INVALID_REQUIRED_ACKS", invalid_acks)
                .with_fixtures(two_partitions)
                .ext(),
            Test::new(
                "an invalid acks value appends nothing and the broker keeps serving",
                invalid_acks_is_harmless,
            )
            .with_fixtures(two_partitions)
            .ext(),
        ],
    }
}

/// Produce one batch with the given `acks` and return `(error_code, base_offset)`.
async fn produce_with_acks(
    ctx: &Ctx,
    name: &str,
    partition: i32,
    acks: i16,
    values: &[&str],
) -> Result<(i16, i64), Failure> {
    let mut conn = ctx.connect().await?;
    let req = produce_request(&[(name.to_string(), partition, batch(values))], acks);
    let resp = conn
        .request(PRODUCE_V11, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new(format!("a Produce with acks={acks}"), &conn);
    let Some(p) = resp
        .responses
        .first()
        .and_then(|t| t.partition_responses.first())
    else {
        c.that(
            "response.responses[0].partition_responses[0]",
            "one entry for the partition that was written",
            false,
            "no entry at all",
        );
        return Err(c
            .finish()
            .err()
            .unwrap_or_else(|| Failure::harness("no partition entry")));
    };
    c.eq(
        "response.responses[0].partition_responses[0].index",
        partition,
        p.index,
    );
    c.finish()?;
    Ok((p.error_code, p.base_offset))
}

kafka_test!(acks_one, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let (error, base) = produce_with_acks(ctx, &t.name, 0, 1, &["a", "b"]).await?;
    let mut c = Check::detached("the response to a Produce with acks=1");
    c.that(
        "response.responses[0].partition_responses[0].error_code",
        &error_label(NONE),
        error == NONE,
        error_label(error),
    );
    c.eq(
        "response.responses[0].partition_responses[0].base_offset",
        0i64,
        base,
    );
    c.finish()?;
    await_high_watermark(ctx, &t, 0, 2).await
});

kafka_test!(acks_all, |ctx| {
    let t = ctx.topic("t1")?.clone();
    // A different partition, so this test does not depend on the acks=1 one.
    let (error, base) = produce_with_acks(ctx, &t.name, 1, -1, &["a", "b", "c"]).await?;
    let mut c = Check::detached("the response to a Produce with acks=-1");
    c.that(
        "response.responses[0].partition_responses[0].error_code",
        &error_label(NONE),
        error == NONE,
        error_label(error),
    );
    c.eq(
        "response.responses[0].partition_responses[0].base_offset",
        0i64,
        base,
    );
    c.finish()?;
    await_high_watermark(ctx, &t, 1, 3).await
});

kafka_test!(acks_zero_is_silent, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = produce_request(&[(t.name.clone(), 0, batch(&["fire", "and", "forget"]))], 0);
    let id = conn.next_correlation_id();
    conn.send_request(PRODUCE_V11, id, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    match conn.read_silence(QUIET).await {
        Ok(seen) if seen.is_empty() => Ok(()),
        Ok(seen) => {
            let mut c = Check::new("the silence after a Produce with acks=0", &conn);
            c.note(format!(
                "correlation id {id} was sent with acks=0; a broker that answers it puts \
                 the connection permanently out of step with its client"
            ));
            c.that(
                "connection.bytes_after_acks_0_produce",
                &format!("nothing at all within {} ms", QUIET.as_millis()),
                false,
                format!("{} bytes", seen.len()),
            );
            c.finish()
        }
        Err(ProtoError::Closed) => {
            let mut c = Check::new("the connection after a Produce with acks=0", &conn);
            c.that(
                "connection.state",
                "still open",
                false,
                "closed by the broker",
            );
            c.finish()
        }
        Err(e) => Err(proto_fail(e, &conn)),
    }
});

kafka_test!(acks_zero_then_api_versions, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = produce_request(&[(t.name.clone(), 0, batch(&["silent"]))], 0);
    let produce_id = conn.next_correlation_id();
    conn.send_request(PRODUCE_V11, produce_id, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    // No read in between: whatever comes back next must belong to the ApiVersions below.
    let next_id = conn.next_correlation_id();
    let resp = conn
        .request_with_id(API_VERSIONS_V4, next_id, &api_versions_request())
        .await
        .map_err(|e| {
            proto_fail(e, &conn).note(format!(
                "the acks=0 Produce used correlation id {produce_id}; if the broker answered \
                 it, this ApiVersions reads that response instead of its own"
            ))
        })?;
    let mut c = Check::new(
        "the request that follows an acks=0 Produce on the same connection",
        &conn,
    );
    c.eq("response.correlation_id", next_id, resp.correlation_id);
    c.eq("response.error_code", NONE, resp.error_code);
    c.finish()
});

kafka_test!(acks_zero_appends, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = produce_request(&[(t.name.clone(), 1, batch(&["x", "y"]))], 0);
    let id = conn.next_correlation_id();
    conn.send_request(PRODUCE_V11, id, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    // acks=0 is "do not tell me", not "do not do it".
    await_high_watermark(ctx, &t, 1, 2).await
});

kafka_test!(invalid_acks, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let (error, _) = produce_with_acks(ctx, &t.name, 0, 2, &["nope"]).await?;
    let mut c = Check::detached("the response to a Produce with acks=2");
    c.that(
        "response.responses[0].partition_responses[0].error_code",
        &error_label(INVALID_REQUIRED_ACKS),
        error == INVALID_REQUIRED_ACKS,
        error_label(error),
    );
    c.finish()
});

kafka_test!(invalid_acks_is_harmless, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let (rejected, _) = produce_with_acks(ctx, &t.name, 0, 3, &["nope", "nope"]).await?;
    let mut c = Check::detached("a Produce with acks=3");
    error_is_one_of(
        &mut c,
        "response.responses[0].partition_responses[0].error_code",
        &[INVALID_REQUIRED_ACKS],
        rejected,
    );
    c.finish()?;
    // The rejected batch must not be in the log, so a good one still starts at offset 0.
    let (error, base) = produce_with_acks(ctx, &t.name, 0, -1, &["real"]).await?;
    let mut c = Check::detached("a valid Produce after one with an invalid acks value");
    c.that(
        "response.responses[0].partition_responses[0].error_code",
        &error_label(NONE),
        error == NONE,
        error_label(error),
    );
    c.eq(
        "response.responses[0].partition_responses[0].base_offset",
        0i64,
        base,
    );
    c.finish()?;
    expect_still_serving(ctx, "a Produce with an invalid acks value").await
});

/// Worked examples: the same batch sent with acks 0, -1 and 2.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("acks=0: fire and forget", |env| {
            env.request(
                PRODUCE_V11,
                331,
                &produce_request(
                    &[(env.name("t1")?, 0, batch(&["fire", "and", "forget"]))],
                    0,
                ),
            )
        })
        .with_fixtures(two_partitions)
        .expect_silence()
        .request(
            "Produce v11, correlation id 331, acks 0: three records to partition 0 of the \
             fixture topic",
        )
        .response(
            "Nothing at all. The records are appended and the connection stays open, but no \
             response frame is ever written — correlation id 331 is never answered.",
        )
        .note(
            "acks=0 means 'do not tell me', not 'do not do it'. Append the batch, write no \
             bytes back, and keep reading: the next request on this connection still needs \
             its answer. A broker that replies here puts the client permanently one \
             response ahead for the life of the connection.",
        ),
        ExampleSpec::wire("acks=-1: the same batch, answered", |env| {
            env.request(
                PRODUCE_V11,
                332,
                &produce_request(
                    &[(env.name("t1")?, 0, batch(&["fire", "and", "forget"]))],
                    -1,
                ),
            )
        })
        .with_fixtures(two_partitions)
        .request(
            "The identical request with acks -1 instead of 0, correlation id 332 — the only \
             byte that differs in the body is the acks int16",
        )
        .response(
            "A normal Produce v11 response: index 0, error_code 0 (NONE), base_offset 0, \
             log_append_time_ms -1, log_start_offset 0, throttle_time_ms 0",
        )
        .note(
            "acks=1 answers once the leader's own log holds the batch, acks=-1 once every \
             in-sync replica does. On a single-broker setup the leader is the whole ISR, so \
             both answer identically — but the acks value still has to be read, because 0 \
             changes what goes back on the wire.",
        ),
        ExampleSpec::wire("acks=2: a value that does not exist", |env| {
            env.request(
                PRODUCE_V11,
                333,
                &produce_request(&[(env.name("t1")?, 0, batch(&["nope"]))], 2),
            )
        })
        .with_fixtures(two_partitions)
        .request("Produce v11, correlation id 333, acks 2 — one record to partition 0")
        .response(
            "One partition_responses entry: index 0, error_code 21 (INVALID_REQUIRED_ACKS), \
             base_offset -1. Nothing is appended: the partition's high watermark stays 0.",
        )
        .note(
            "acks is only ever 0, 1 or -1. Validate it before you touch the log, not after \
             — a rejected request must leave the segment file exactly as it was.",
        ),
    ]
}
