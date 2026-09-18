//! Stage 17 — CreateTopics (19) v7.
//!
//! Creation is a controller operation: the broker writes a `TopicRecord` plus one
//! `PartitionRecord` per partition and answers with the id it minted. Every topic in the
//! request succeeds or fails on its own — a duplicate or an illegal name is an `error_code`
//! in that topic's entry, never a failure of the whole request.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::fixtures::{FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::proto::{Conn, Decoded};
use crate::stages::{
    describe_request, proto_fail, require_api, topic_name, Ctx, Stage, Test, DESCRIBE_V0, NONE,
    UNKNOWN_TOPIC_OR_PARTITION,
};
use kafka_protocol::messages::create_topics_request::CreatableTopic;
use kafka_protocol::messages::metadata_request::MetadataRequestTopic;
use kafka_protocol::messages::{CreateTopicsRequest, CreateTopicsResponse, MetadataRequest};
use std::time::Duration;
use uuid::Uuid;

/// `CreateTopics`
const CREATE_TOPICS_KEY: i16 = 19;
/// The `CreateTopics` version this stage speaks.
const CREATE_TOPICS_V7: i16 = 7;
/// The `Metadata` version used to prove a topic really exists.
const METADATA_V12: i16 = 12;
/// `TOPIC_ALREADY_EXISTS`
const TOPIC_ALREADY_EXISTS: i16 = 36;
/// `INVALID_TOPIC_EXCEPTION`
const INVALID_TOPIC_EXCEPTION: i16 = 17;
/// `INVALID_PARTITIONS`
const INVALID_PARTITIONS: i16 = 37;
/// How long a test waits for the controller to publish a new topic.
const SETTLE: Duration = Duration::from_secs(4);

/// One topic that already exists, so a second CreateTopics for it must fail.
fn one_topic() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 1))
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 17,
        slug: "create_topics",
        name: "CreateTopics (19) v7",
        ext: true,
        hints: &[
            "Mint a fresh topic id, write a TopicRecord and one PartitionRecord per partition, \
             then answer with that id, num_partitions and replication_factor",
            "A duplicate name is error 36 TOPIC_ALREADY_EXISTS in that topic's entry only",
            "An illegal name (empty, '.', '..', >249 chars, anything outside [a-zA-Z0-9._-]) is \
             error 17, and num_partitions=0 is error 37 INVALID_PARTITIONS",
            "validate_only=true runs every check and changes nothing",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("ApiVersions advertises CreateTopics(19) v7", advertised).ext(),
            Test::new("a fresh topic is created and gets an id", creates).ext(),
            Test::new(
                "the created topic shows up in Metadata and Describe",
                visible,
            )
            .ext(),
            Test::new("a duplicate name is error 36", duplicate).ext(),
            Test::new("illegal names are error 17", illegal_names).ext(),
            Test::new("zero partitions is error 37", bad_partition_counts).ext(),
            Test::new("validate_only=true creates nothing", validate_only).ext(),
            Test::new("two topics can be created in one request", two_topics).ext(),
        ],
    }
}

/// A `CreateTopics` v7 request for `(name, partitions)` pairs.
fn create_request(topics: &[(&str, i32)], validate_only: bool) -> CreateTopicsRequest {
    let mut r = CreateTopicsRequest::default();
    r.timeout_ms = 5_000;
    r.validate_only = validate_only;
    r.topics = topics
        .iter()
        .map(|(name, partitions)| {
            let mut t = CreatableTopic::default();
            t.name = topic_name(name);
            t.num_partitions = *partitions;
            t.replication_factor = 1;
            t
        })
        .collect();
    r
}

async fn create(
    conn: &mut Conn,
    topics: &[(&str, i32)],
    validate_only: bool,
) -> Result<Decoded<CreateTopicsResponse>, Failure> {
    let req = create_request(topics, validate_only);
    let result = conn.request(CREATE_TOPICS_V7, &req).await;
    result.map_err(|e| proto_fail(e, conn))
}

/// The result entry for `name`, if the broker returned one.
fn result_for<'a>(
    resp: &'a CreateTopicsResponse,
    name: &str,
) -> Option<&'a kafka_protocol::messages::create_topics_response::CreatableTopicResult> {
    resp.topics.iter().find(|t| t.name.0.as_str() == name)
}

/// The error code for `name`, or `-1` when the broker did not mention it at all.
fn error_for(resp: &CreateTopicsResponse, name: &str) -> i16 {
    result_for(resp, name).map(|t| t.error_code).unwrap_or(-1)
}

/// A v12 `Metadata` request for one topic name.
fn metadata_request(name: &str) -> MetadataRequest {
    let mut r = MetadataRequest::default();
    r.allow_auto_topic_creation = false;
    let mut t = MetadataRequestTopic::default();
    t.name = Some(topic_name(name));
    r.topics = Some(vec![t]);
    r
}

/// Poll `Metadata` until `name` exists with `partitions` partitions; creation is asynchronous.
async fn wait_visible(ctx: &Ctx, name: &str, partitions: usize) -> Result<Uuid, Failure> {
    let mut conn = ctx.connect().await?;
    let req = metadata_request(name);
    let deadline = tokio::time::Instant::now() + SETTLE;
    let mut last = "the broker never answered Metadata".to_string();
    while tokio::time::Instant::now() < deadline {
        match conn.request(METADATA_V12, &req).await {
            Ok(resp) => match resp.topics.first() {
                Some(t) if t.error_code == NONE && t.partitions.len() == partitions => {
                    return Ok(t.topic_id)
                }
                Some(t) if t.error_code == NONE => {
                    last = format!("it has {} partitions, not {partitions}", t.partitions.len())
                }
                Some(t) => last = format!("Metadata answers error {}", t.error_code),
                None => last = "Metadata returned no topic entry".to_string(),
            },
            Err(e) => last = e.to_string(),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(Failure::harness(format!(
        "'{name}' was never published by the controller within {SETTLE:?}: {last}"
    )))
}

kafka_test!(advertised, |ctx| {
    let mut conn = ctx.connect().await?;
    let resp = crate::stages::api_versions(&mut conn).await?;
    let (min, max) = require_api(&resp, CREATE_TOPICS_KEY, "CreateTopics", &conn)?;
    let mut c = Check::new("the advertised CreateTopics version range", &conn);
    c.at_most("response.api_keys[19].min_version", CREATE_TOPICS_V7, min);
    c.at_least("response.api_keys[19].max_version", CREATE_TOPICS_V7, max);
    c.finish()
});

kafka_test!(creates, |ctx| {
    let name = ctx.unique("s17-fresh");
    let mut conn = ctx.connect().await?;
    let resp = create(&mut conn, &[(&name, 3)], false).await?;
    let mut c = Check::new(format!("CreateTopics for the fresh name '{name}'"), &conn);
    c.eq("response.topics.len", 1usize, resp.topics.len());
    c.eq(
        "response.topics[0].error_code",
        NONE,
        error_for(&resp, &name),
    );
    if let Some(got) = result_for(&resp, &name) {
        c.note("v7 echoes the partition count and the id the controller minted");
        c.ne("response.topics[0].topic_id", Uuid::nil(), got.topic_id);
        c.eq(
            "response.topics[0].num_partitions",
            3i32,
            got.num_partitions,
        );
        c.eq(
            "response.topics[0].replication_factor",
            1i16,
            got.replication_factor,
        );
        c.eq(
            "response.topics[0].error_message",
            None,
            got.error_message.as_ref().map(|m| m.to_string()),
        );
    }
    c.eq("response.trailing_bytes", 0usize, resp.trailing);
    c.finish()
});

kafka_test!(visible, |ctx| {
    let name = ctx.unique("s17-visible");
    let mut conn = ctx.connect().await?;
    let created = create(&mut conn, &[(&name, 2)], false).await?;
    let mut c = Check::new(
        format!("that '{name}' really exists after CreateTopics"),
        &conn,
    );
    c.eq(
        "response.topics[0].error_code",
        NONE,
        error_for(&created, &name),
    );
    let announced = result_for(&created, &name).map(|t| t.topic_id);
    c.finish()?;

    let published = wait_visible(ctx, &name, 2).await?;
    let mut conn = ctx.connect().await?;
    let req = describe_request(&[&name], 100);
    let described = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new(
        format!("DescribeTopicPartitions for the new topic '{name}'"),
        &conn,
    );
    c.note("the id CreateTopics announced is the id every later API must use");
    c.eq(
        "Metadata: response.topics[0].topic_id",
        announced,
        Some(published),
    );
    c.eq(
        "response.topics[0].error_code",
        NONE,
        described.topics.first().map(|t| t.error_code).unwrap_or(-1),
    );
    c.eq(
        "response.topics[0].topic_id",
        published,
        described
            .topics
            .first()
            .map(|t| t.topic_id)
            .unwrap_or_default(),
    );
    c.eq(
        "response.topics[0].partitions.len",
        2usize,
        described
            .topics
            .first()
            .map(|t| t.partitions.len())
            .unwrap_or(0),
    );
    c.finish()
});

kafka_test!(duplicate, |ctx| {
    let name = ctx.unique("s17-duplicate");
    let mut conn = ctx.connect().await?;
    let first = create(&mut conn, &[(&name, 1)], false).await?;
    let mut c = Check::new(format!("creating '{name}' twice"), &conn);
    c.eq(
        "the first CreateTopics: response.topics[0].error_code",
        NONE,
        error_for(&first, &name),
    );
    c.finish()?;
    wait_visible(ctx, &name, 1).await?;

    let mut conn = ctx.connect().await?;
    let second = create(&mut conn, &[(&name, 1)], false).await?;
    let mut c = Check::new(format!("the second CreateTopics for '{name}'"), &conn);
    c.note("the request itself is fine; only this topic's entry carries the error");
    c.eq(
        "response.topics[0].error_code",
        TOPIC_ALREADY_EXISTS,
        error_for(&second, &name),
    );
    if let Some(got) = result_for(&second, &name) {
        c.eq(
            "response.topics[0].name",
            name.clone(),
            got.name.0.to_string(),
        );
    }
    c.finish()
});

kafka_test!(illegal_names, |ctx| {
    let cases: [(&str, &str); 3] = [
        ("", "an empty name"),
        ("..", "the reserved name '..'"),
        ("not a topic!", "a name with a space and a '!'"),
    ];
    let mut conn = ctx.connect().await?;
    let mut c = Check::new("CreateTopics for illegal topic names", &conn);
    c.note("the broker rejects the name before it writes anything to the metadata log");
    for (name, what) in cases {
        let resp = create(&mut conn, &[(name, 1)], false).await?;
        c.bytes(&conn);
        c.eq(
            &format!("{what}: response.topics[0].error_code"),
            INVALID_TOPIC_EXCEPTION,
            error_for(&resp, name),
        );
    }
    c.finish()
});

kafka_test!(bad_partition_counts, |ctx| {
    let zero = ctx.unique("s17-zero-partitions");
    let defaulted = ctx.unique("s17-default-partitions");
    let mut conn = ctx.connect().await?;
    let resp = create(&mut conn, &[(&zero, 0)], false).await?;
    let mut c = Check::new("CreateTopics with num_partitions = 0", &conn);
    c.eq(
        "response.topics[0].error_code",
        INVALID_PARTITIONS,
        error_for(&resp, &zero),
    );
    c.finish()?;

    let resp = create(&mut conn, &[(&defaulted, -1)], false).await?;
    let got = error_for(&resp, &defaulted);
    let partitions = result_for(&resp, &defaulted)
        .map(|t| t.num_partitions)
        .unwrap_or(0);
    let mut c = Check::new("CreateTopics with num_partitions = -1", &conn);
    c.note(
        "-1 means 'use the broker default'; Apache Kafka has num.partitions=1 and accepts it, \
         a broker with no default must answer 37 INVALID_PARTITIONS",
    );
    c.that(
        "response.topics[0].error_code",
        "either 0 with a positive num_partitions, or 37 INVALID_PARTITIONS",
        (got == NONE && partitions >= 1) || got == INVALID_PARTITIONS,
        (got, partitions),
    );
    c.finish()
});

kafka_test!(validate_only, |ctx| {
    let name = ctx.unique("s17-validate-only");
    let mut conn = ctx.connect().await?;
    let resp = create(&mut conn, &[(&name, 2)], true).await?;
    let mut c = Check::new(
        format!("CreateTopics with validate_only=true for '{name}'"),
        &conn,
    );
    c.eq(
        "response.topics[0].error_code",
        NONE,
        error_for(&resp, &name),
    );
    c.finish()?;

    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut conn = ctx.connect().await?;
    let req = describe_request(&[&name], 100);
    let described = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new(format!("that validate_only left '{name}' uncreated"), &conn);
    c.note("validate_only runs every check and writes nothing to the metadata log");
    c.eq(
        "response.topics[0].error_code",
        UNKNOWN_TOPIC_OR_PARTITION,
        described.topics.first().map(|t| t.error_code).unwrap_or(-1),
    );
    c.finish()
});

kafka_test!(two_topics, |ctx| {
    let a = ctx.unique("s17-pair-a");
    let b = ctx.unique("s17-pair-b");
    let mut conn = ctx.connect().await?;
    let resp = create(&mut conn, &[(&a, 1), (&b, 2)], false).await?;
    let mut c = Check::new("two topics created in one request", &conn);
    c.eq("response.topics.len", 2usize, resp.topics.len());
    c.eq(
        &format!("response.topics[name={a}].error_code"),
        NONE,
        error_for(&resp, &a),
    );
    c.eq(
        &format!("response.topics[name={b}].error_code"),
        NONE,
        error_for(&resp, &b),
    );
    c.eq(
        &format!("response.topics[name={b}].num_partitions"),
        2i32,
        result_for(&resp, &b).map(|t| t.num_partitions).unwrap_or(0),
    );
    let ids: Vec<Uuid> = resp.topics.iter().map(|t| t.topic_id).collect();
    c.note("each topic gets its own freshly minted id");
    c.that(
        "response.topics[*].topic_id",
        "two different, non-zero ids",
        ids.len() == 2 && ids[0] != ids[1] && !ids.contains(&Uuid::nil()),
        ids,
    );
    c.finish()?;

    let ida = wait_visible(ctx, &a, 1).await?;
    let idb = wait_visible(ctx, &b, 2).await?;
    let mut c = Check::detached("that both new topics reached Metadata");
    c.that(
        "the published topic ids",
        "to match the ids CreateTopics announced",
        resp.topics
            .iter()
            .all(|t| t.topic_id == ida || t.topic_id == idb),
        (ida, idb),
    );
    c.finish()
});

/// Worked examples: a name that is free, and a name that is taken.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("Creating a fresh topic", |env| {
            env.request(
                CREATE_TOPICS_V7,
                171,
                &create_request(&[("kafkatest-new", 2)], false),
            )
        })
        .request(
            "CreateTopics v7, correlation id 171, one topic 'kafkatest-new' with \
             num_partitions 2, replication_factor 1, no assignments and no configs, \
             timeout_ms 5000, validate_only false",
        )
        .response(
            "throttle_time_ms, then one topics entry: error_code 0 (NONE), the name echoed, a \
             freshly minted topic_id (a different uuid every time), num_partitions 2, \
             replication_factor 1, error_message null, and the effective configs the broker \
             applied",
        )
        .note(
            "The controller writes a TopicRecord plus one PartitionRecord per partition before \
             it answers, and the id in this response is the id every later Fetch, Metadata and \
             Describe must use. Publication is asynchronous, so a Describe sent immediately \
             afterwards may still answer error 3 for a moment.",
        ),
        ExampleSpec::wire("A name that is already taken", |env| {
            let name = env.name("t1")?;
            env.request(CREATE_TOPICS_V7, 172, &create_request(&[(&name, 1)], false))
        })
        .with_fixtures(one_topic)
        .request(
            "CreateTopics v7, correlation id 172, one topic whose name already exists, \
             num_partitions 1, replication_factor 1, validate_only false",
        )
        .response(
            "One topics entry with error_code 36 (TOPIC_ALREADY_EXISTS), the name echoed, an \
             error_message saying the topic already exists and an all-zero topic_id. The \
             request itself succeeded: there is no request-level error field to set",
        )
        .note(
            "Every topic in a CreateTopics request succeeds or fails on its own, so a \
             duplicate in a two-topic request must not stop the other one. Check the name \
             before you write anything, and leave the existing topic's id and partition count \
             untouched.",
        ),
    ]
}
