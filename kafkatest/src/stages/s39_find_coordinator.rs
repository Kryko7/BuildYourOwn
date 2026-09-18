//! Stage 39 — FindCoordinator (10) v4 for a consumer group.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::kafka_test;
use crate::proto::Conn;
use crate::stages::group_protocol::{
    await_coordinator, coordinator_of, find_coordinator, find_coordinator_request,
    COORDINATOR_NOT_AVAILABLE, FIND_COORDINATOR_KEY, FIND_COORDINATOR_V4, KEY_TYPE_GROUP,
    KEY_TYPE_TRANSACTION,
};
use crate::stages::{api_versions, proto_fail, require_api, Ctx, Stage, Test, NONE};
use kafka_protocol::messages::MetadataRequest;

/// The `Metadata` version used to cross-check the coordinator's identity.
const METADATA_V12: i16 = 12;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 39,
        slug: "find_coordinator",
        name: "FindCoordinator (10) for a consumer group",
        ext: true,
        hints: &[
            "v4 takes coordinator_keys[] and answers coordinators[], one entry per key, each \
             with its own error_code — the top-level error_code stays 0",
            "The coordinator of a group is the broker leading \
             __consumer_offsets-(hash(group) % partitions); with one broker that is always \
             this broker, but node_id, host and port must be the ones Metadata advertises",
            "Kafka creates __consumer_offsets lazily, so the first lookup may answer 15 \
             COORDINATOR_NOT_AVAILABLE for a few hundred ms; a client retries",
            "key_type 0 is a group, 1 is a transactional id; an unimplemented key type is an \
             error code, never a dropped connection",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("ApiVersions advertises FindCoordinator(10) v4", advertised).ext(),
            Test::new("a group key resolves to a coordinator", resolves).ext(),
            Test::new(
                "the coordinator is the broker Metadata advertises",
                matches_metadata,
            )
            .ext(),
            Test::new("the coordinator address answers ApiVersions", address_works).ext(),
            Test::new("two keys are answered in one response", two_keys).ext(),
            Test::new("a group that was never used still resolves", unused_group).ext(),
            Test::new("the v4 top-level error code stays 0", top_level_error).ext(),
            Test::new("a transactional key type is answered", transaction_key).ext(),
        ],
    }
}

/// Every broker `Metadata` v12 knows about, as `(node_id, host, port)`.
async fn brokers(ctx: &Ctx) -> Result<Vec<(i32, String, i32)>, Failure> {
    let mut conn = ctx.connect().await?;
    let mut req = MetadataRequest::default();
    req.topics = Some(Vec::new());
    req.allow_auto_topic_creation = false;
    let resp = conn
        .request(METADATA_V12, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    Ok(resp
        .brokers
        .iter()
        .map(|b| (b.node_id.0, b.host.to_string(), b.port))
        .collect())
}

kafka_test!(advertised, |ctx| {
    let mut conn = ctx.connect().await?;
    let resp = api_versions(&mut conn).await?;
    let (min, max) = require_api(&resp, FIND_COORDINATOR_KEY, "FindCoordinator", &conn)?;
    let mut c = Check::new("the advertised FindCoordinator version range", &conn);
    c.at_most(
        "response.api_keys[FindCoordinator].min_version",
        FIND_COORDINATOR_V4,
        min,
    );
    c.at_least(
        "response.api_keys[FindCoordinator].max_version",
        FIND_COORDINATOR_V4,
        max,
    );
    c.finish()
});

kafka_test!(resolves, |ctx| {
    let group = ctx.unique("g39-resolves");
    let info = await_coordinator(ctx, &group).await?;
    let mut c = Check::detached(format!("the coordinator of group '{group}'"));
    c.eq(
        "response.coordinators[0].key",
        group.clone(),
        info.key.clone(),
    );
    c.eq("response.coordinators[0].error_code", NONE, info.error_code);
    c.at_least("response.coordinators[0].node_id", 0i32, info.node_id);
    c.at_least("response.coordinators[0].port", 1i32, info.port);
    c.that(
        "response.coordinators[0].host",
        "a non-empty host name",
        !info.host.is_empty(),
        info.host.clone(),
    );
    c.finish()
});

kafka_test!(matches_metadata, |ctx| {
    let group = ctx.unique("g39-metadata");
    let info = await_coordinator(ctx, &group).await?;
    let listed = brokers(ctx).await?;
    let mut c = Check::detached("the coordinator against the brokers Metadata lists");
    c.observe("metadata.brokers", listed.clone());
    c.that(
        "response.coordinators[0].(node_id, host, port)",
        "a broker that Metadata also lists",
        listed.iter().any(|(id, host, port)| {
            *id == info.node_id && *host == info.host && *port == info.port
        }),
        (info.node_id, info.host.clone(), info.port),
    );
    c.finish()
});

kafka_test!(address_works, |ctx| {
    let group = ctx.unique("g39-address");
    let info = await_coordinator(ctx, &group).await?;
    let target = format!("{}:{}", info.host, info.port);
    let addr = tokio::net::lookup_host(&target)
        .await
        .map_err(|e| {
            Failure::harness(format!(
                "the coordinator address '{target}' does not resolve: {e}"
            ))
        })?
        .next()
        .ok_or_else(|| Failure::harness(format!("'{target}' resolved to no address")))?;
    let mut conn = Conn::connect(addr, ctx.timeout).await.map_err(|e| {
        Failure::proto(e, None).note(format!(
            "connecting to the coordinator address '{target}' the broker returned"
        ))
    })?;
    let resp = api_versions(&mut conn).await?;
    let mut c = Check::new(
        format!("ApiVersions on the coordinator address '{target}'"),
        &conn,
    );
    c.eq("response.error_code", NONE, resp.error_code);
    c.at_least("response.api_keys.len", 1usize, resp.api_keys.len());
    c.finish()
});

kafka_test!(two_keys, |ctx| {
    let a = ctx.unique("g39-two-a");
    let b = ctx.unique("g39-two-b");
    await_coordinator(ctx, &a).await?;
    let mut conn = ctx.connect().await?;
    let resp = find_coordinator(&mut conn, &[a.as_str(), b.as_str()], KEY_TYPE_GROUP).await?;
    let mut c = Check::new("FindCoordinator with two coordinator_keys", &conn);
    c.eq("response.coordinators.len", 2usize, resp.coordinators.len());
    c.eq(
        "response.coordinators[*].key",
        vec![a.clone(), b.clone()],
        resp.coordinators
            .iter()
            .map(|c| c.key.to_string())
            .collect::<Vec<String>>(),
    );
    c.eq(
        "response.coordinators[*].error_code",
        vec![NONE, NONE],
        resp.coordinators
            .iter()
            .map(|c| c.error_code)
            .collect::<Vec<i16>>(),
    );
    c.finish()
});

kafka_test!(unused_group, |ctx| {
    let known = ctx.unique("g39-warmup");
    await_coordinator(ctx, &known).await?;
    // A group id nothing has ever committed to or joined still has a coordinator: the
    // lookup is pure arithmetic over __consumer_offsets, not a lookup of group state.
    let fresh = ctx.unique("g39-never-used");
    let mut conn = ctx.connect().await?;
    let resp = find_coordinator(&mut conn, &[fresh.as_str()], KEY_TYPE_GROUP).await?;
    let info = coordinator_of(&resp, &fresh);
    let mut c = Check::new(
        format!("FindCoordinator for the unused group '{fresh}'"),
        &conn,
    );
    c.eq(
        "response.coordinators[0].error_code",
        NONE,
        info.as_ref().map(|i| i.error_code).unwrap_or(-1),
    );
    c.finish()
});

kafka_test!(top_level_error, |ctx| {
    let group = ctx.unique("g39-toplevel");
    await_coordinator(ctx, &group).await?;
    let mut conn = ctx.connect().await?;
    let resp = find_coordinator(&mut conn, &[group.as_str()], KEY_TYPE_GROUP).await?;
    let mut c = Check::new("the top level of a FindCoordinator v4 response", &conn);
    c.eq("response.error_code", NONE, resp.error_code);
    c.eq("response.throttle_time_ms", 0i32, resp.throttle_time_ms);
    c.finish()
});

kafka_test!(transaction_key, |ctx| {
    let group = ctx.unique("g39-txn-warmup");
    await_coordinator(ctx, &group).await?;
    let txn = ctx.unique("txn39");
    let mut conn = ctx.connect().await?;
    let resp = find_coordinator(&mut conn, &[txn.as_str()], KEY_TYPE_TRANSACTION).await?;
    let info = coordinator_of(&resp, &txn);
    let code = info.as_ref().map(|i| i.error_code).unwrap_or(-1);
    let mut c = Check::new(
        format!("FindCoordinator(key_type=1) for the transactional id '{txn}'"),
        &conn,
    );
    // Kafka creates __transaction_state lazily too, so the honest answers are 0 (the topic
    // is there) and 15 COORDINATOR_NOT_AVAILABLE (it is not, yet). Apache Kafka 4.1.2
    // answers 15 here, because nothing in this suite has ever started a transaction. A
    // broker that does not do transactions at all is expected to say 15 rather than drop
    // the request, so both are accepted and only a third answer fails.
    c.eq("response.coordinators.len", 1usize, resp.coordinators.len());
    c.that(
        "response.coordinators[0].error_code",
        "0 (a coordinator) or 15 COORDINATOR_NOT_AVAILABLE",
        code == NONE || code == COORDINATOR_NOT_AVAILABLE,
        code,
    );
    c.finish()
});

/// Worked examples: what the broker sees, and what a correct broker answers.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("Where does this group live", |env| {
            env.request(
                FIND_COORDINATOR_V4,
                391,
                &find_coordinator_request(&[env.group.as_str()], KEY_TYPE_GROUP),
            )
        })
        .request(
            "FindCoordinator v4, key_type 0 (a consumer group), with a single entry in \
             coordinator_keys: the group id 'kafkatest-group'",
        )
        .response(
            "Top-level error_code 0 and throttle_time_ms 0, then one coordinators entry whose \
             key is the group id it was asked about, error_code 0 (NONE), error_message null, \
             and the node_id, host and port of the broker that owns \
             __consumer_offsets-(hash(group) % partitions) — the same address Metadata \
             advertises for that node",
        )
        .note(
            "Answer the host and port a client can actually reach, not the socket this \
             request arrived on: the client opens a fresh connection to whatever you return \
             and sends every group request there. Apache Kafka creates __consumer_offsets \
             lazily, so the very first lookup after a cold start can be 15 \
             COORDINATOR_NOT_AVAILABLE for a few hundred milliseconds; a client retries.",
        ),
        ExampleSpec::wire("Two keys, one of them never used", |env| {
            env.request(
                FIND_COORDINATOR_V4,
                392,
                &find_coordinator_request(
                    &[env.group.as_str(), "kafkatest-never-used"],
                    KEY_TYPE_GROUP,
                ),
            )
        })
        .request(
            "The same request with two coordinator_keys: 'kafkatest-group' and \
             'kafkatest-never-used', a group id nothing has ever joined or committed to",
        )
        .response(
            "coordinators has two entries, in the order the keys were asked, each carrying \
             its own key and error_code 0; on a single-broker cluster both name the same \
             node_id, host and port. The unused group is answered exactly like the used one",
        )
        .note(
            "Resolving a coordinator is arithmetic over the group id, not a lookup of group \
             state: a group that does not exist yet still has the broker that would own it. \
             v4 is also a batch API — every key gets its own error_code and the top-level \
             error_code stays 0, which is what lets a client resolve several groups in one \
             round trip.",
        ),
    ]
}
