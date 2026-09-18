//! Stage 18 — DeleteTopics (20) v6 and the describe that follows.
//!
//! v6 deletes by name *or* by id: each entry of the request is a `{name, topic_id}` struct
//! and exactly one of the two is set. Deletion is keyed by id internally, so the name is
//! resolved first — which is why deleting an unknown name is error 3 while deleting an
//! unknown id is error 100, and why re-creating a deleted name mints a brand new id.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::fixtures::{FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::proto::{Conn, Decoded};
use crate::stages::{
    describe_request, proto_fail, require_api, topic_name, Ctx, Stage, Test, DESCRIBE_V0, NONE,
    UNKNOWN_TOPIC_ID, UNKNOWN_TOPIC_OR_PARTITION,
};
use kafka_protocol::messages::create_topics_request::CreatableTopic;
use kafka_protocol::messages::delete_topics_request::DeleteTopicState;
use kafka_protocol::messages::metadata_request::MetadataRequestTopic;
use kafka_protocol::messages::{
    CreateTopicsRequest, DeleteTopicsRequest, DeleteTopicsResponse, MetadataRequest,
};
use std::time::Duration;
use uuid::Uuid;

/// `DeleteTopics`
const DELETE_TOPICS_KEY: i16 = 20;
/// The `DeleteTopics` version this stage speaks.
const DELETE_TOPICS_V6: i16 = 6;
/// The `CreateTopics` version used to put a topic back.
const CREATE_TOPICS_V7: i16 = 7;
/// The `Metadata` version used to look a topic up by id.
const METADATA_V12: i16 = 12;
/// How long a test waits for the controller to publish a deletion or a creation.
const SETTLE: Duration = Duration::from_secs(4);

fn one_topic() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("doomed", 2))
}

fn two_topics() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("doomed", 2)).and(TopicSpec::new("spared", 1))
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 18,
        slug: "delete_topics",
        name: "DeleteTopics (20) v6 and the describe that follows",
        ext: true,
        hints: &[
            "v6 takes {name, topic_id} structs: resolve the name to an id, then write a \
             RemoveTopicRecord keyed by that id",
            "Deleting an unknown name is error 3, deleting an unknown id is error 100 — the \
             request as a whole still succeeds",
            "After the delete, DescribeTopicPartitions for the name must answer error 3 again \
             and the partition directories must be gone",
            "Re-creating the same name mints a NEW topic id; never reuse the old one",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("ApiVersions advertises DeleteTopics(20) v6", advertised).ext(),
            Test::new("deleting by name makes the topic unknown again", by_name)
                .with_fixtures(one_topic)
                .ext(),
            Test::new("deleting by id makes the id unknown again", by_id)
                .with_fixtures(one_topic)
                .ext(),
            Test::new("deleting an unknown name is error 3", unknown_name).ext(),
            Test::new("deleting an unknown id is error 100", unknown_id).ext(),
            Test::new("one delete leaves the other topics alone", leaves_others)
                .with_fixtures(two_topics)
                .ext(),
            Test::new("re-creating a deleted name gets a new topic id", recreate)
                .with_fixtures(one_topic)
                .ext(),
        ],
    }
}

/// A `DeleteTopics` v6 request: `Some(name)` deletes by name, `None` by id.
fn delete_request(topics: &[(Option<&str>, Uuid)]) -> DeleteTopicsRequest {
    let mut r = DeleteTopicsRequest::default();
    r.timeout_ms = 5_000;
    r.topics = topics
        .iter()
        .map(|(name, id)| {
            let mut t = DeleteTopicState::default();
            t.name = name.map(topic_name);
            t.topic_id = *id;
            t
        })
        .collect();
    r
}

async fn delete(
    conn: &mut Conn,
    topics: &[(Option<&str>, Uuid)],
) -> Result<Decoded<DeleteTopicsResponse>, Failure> {
    let req = delete_request(topics);
    let result = conn.request(DELETE_TOPICS_V6, &req).await;
    result.map_err(|e| proto_fail(e, conn))
}

/// A v12 `Metadata` request for one topic name.
fn metadata_by_name(name: &str) -> MetadataRequest {
    let mut r = MetadataRequest::default();
    r.allow_auto_topic_creation = false;
    let mut t = MetadataRequestTopic::default();
    t.name = Some(topic_name(name));
    r.topics = Some(vec![t]);
    r
}

/// A v12 `Metadata` request for one topic id — v12 is the first version that allows it.
fn metadata_by_id(id: Uuid) -> MetadataRequest {
    let mut r = MetadataRequest::default();
    r.allow_auto_topic_creation = false;
    let mut t = MetadataRequestTopic::default();
    t.topic_id = id;
    t.name = None;
    r.topics = Some(vec![t]);
    r
}

/// Poll `DescribeTopicPartitions` until `name` is unknown again.
async fn wait_gone(ctx: &Ctx, name: &str) -> Result<(), Failure> {
    let mut conn = ctx.connect().await?;
    let req = describe_request(&[name], 100);
    let deadline = tokio::time::Instant::now() + SETTLE;
    let mut last = "the broker never answered DescribeTopicPartitions".to_string();
    while tokio::time::Instant::now() < deadline {
        match conn.request(DESCRIBE_V0, &req).await {
            Ok(resp) => match resp.topics.first().map(|t| t.error_code) {
                Some(UNKNOWN_TOPIC_OR_PARTITION) => return Ok(()),
                Some(code) => last = format!("it still answers error {code}"),
                None => last = "the response carried no topic entry".to_string(),
            },
            Err(e) => last = e.to_string(),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(Failure::harness(format!(
        "'{name}' was still described {SETTLE:?} after DeleteTopics returned 0: {last}"
    )))
}

/// Poll `Metadata` until `name` exists again, returning the id it now has.
async fn wait_visible(ctx: &Ctx, name: &str) -> Result<Uuid, Failure> {
    let mut conn = ctx.connect().await?;
    let req = metadata_by_name(name);
    let deadline = tokio::time::Instant::now() + SETTLE;
    let mut last = "the broker never answered Metadata".to_string();
    while tokio::time::Instant::now() < deadline {
        match conn.request(METADATA_V12, &req).await {
            Ok(resp) => match resp.topics.first() {
                Some(t) if t.error_code == NONE && !t.partitions.is_empty() => {
                    return Ok(t.topic_id)
                }
                Some(t) => last = format!("Metadata answers error {}", t.error_code),
                None => last = "Metadata returned no topic entry".to_string(),
            },
            Err(e) => last = e.to_string(),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(Failure::harness(format!(
        "'{name}' never came back after being re-created within {SETTLE:?}: {last}"
    )))
}

/// The response entry for `name`, matched on the echoed name.
fn by_name_entry<'a>(
    resp: &'a DeleteTopicsResponse,
    name: &str,
) -> Option<&'a kafka_protocol::messages::delete_topics_response::DeletableTopicResult> {
    resp.responses
        .iter()
        .find(|t| t.name.as_ref().map(|n| n.0.as_str()) == Some(name))
}

kafka_test!(advertised, |ctx| {
    let mut conn = ctx.connect().await?;
    let resp = crate::stages::api_versions(&mut conn).await?;
    let (min, max) = require_api(&resp, DELETE_TOPICS_KEY, "DeleteTopics", &conn)?;
    let mut c = Check::new("the advertised DeleteTopics version range", &conn);
    c.note("v6 is the first version that can delete by topic id");
    c.at_most("response.api_keys[20].min_version", DELETE_TOPICS_V6, min);
    c.at_least("response.api_keys[20].max_version", DELETE_TOPICS_V6, max);
    c.finish()
});

kafka_test!(by_name, |ctx| {
    let t = ctx.topic("doomed")?.clone();
    let mut conn = ctx.connect().await?;
    let resp = delete(&mut conn, &[(Some(&t.name), Uuid::nil())]).await?;
    let mut c = Check::new(format!("DeleteTopics by name for '{}'", t.name), &conn);
    c.eq("response.responses.len", 1usize, resp.responses.len());
    match by_name_entry(&resp, &t.name) {
        None => {
            let echoed: Vec<String> = resp
                .responses
                .iter()
                .map(|r| r.name.as_ref().map(|n| n.0.to_string()).unwrap_or_default())
                .collect();
            c.that(
                "response.responses[*].name",
                &format!("to echo '{}'", t.name),
                false,
                echoed,
            );
        }
        Some(got) => {
            c.eq("response.responses[0].error_code", NONE, got.error_code);
            c.note("the broker resolved the name, so it can echo the id it deleted");
            c.eq("response.responses[0].topic_id", t.id, got.topic_id);
        }
    }
    c.eq("response.trailing_bytes", 0usize, resp.trailing);
    c.finish()?;

    wait_gone(ctx, &t.name).await?;
    let mut conn = ctx.connect().await?;
    let req = describe_request(&[&t.name], 100);
    let described = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new(
        format!("DescribeTopicPartitions after '{}' was deleted", t.name),
        &conn,
    );
    c.eq(
        "response.topics[0].error_code",
        UNKNOWN_TOPIC_OR_PARTITION,
        described.topics.first().map(|t| t.error_code).unwrap_or(-1),
    );
    c.eq(
        "response.topics[0].topic_id",
        Uuid::nil(),
        described.topics.first().map(|t| t.topic_id).unwrap_or(t.id),
    );
    c.finish()
});

kafka_test!(by_id, |ctx| {
    let t = ctx.topic("doomed")?.clone();
    let mut conn = ctx.connect().await?;
    let resp = delete(&mut conn, &[(None, t.id)]).await?;
    let mut c = Check::new(
        format!("DeleteTopics by id for '{}' ({})", t.name, t.id),
        &conn,
    );
    c.eq("response.responses.len", 1usize, resp.responses.len());
    if let Some(got) = resp.responses.first() {
        c.eq("response.responses[0].error_code", NONE, got.error_code);
        c.eq("response.responses[0].topic_id", t.id, got.topic_id);
        c.note("deleting by id, the broker fills the name back in from its own metadata");
        c.eq(
            "response.responses[0].name",
            Some(t.name.clone()),
            got.name.as_ref().map(|n| n.0.to_string()),
        );
    }
    c.finish()?;

    wait_gone(ctx, &t.name).await?;
    let mut conn = ctx.connect().await?;
    let req = metadata_by_id(t.id);
    let meta = conn
        .request(METADATA_V12, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new(
        format!("Metadata by id after '{}' was deleted", t.name),
        &conn,
    );
    c.note("a name that never existed is error 3; an id that no longer exists is error 100");
    c.eq(
        "response.topics[0].error_code",
        UNKNOWN_TOPIC_ID,
        meta.topics.first().map(|t| t.error_code).unwrap_or(-1),
    );
    c.finish()
});

kafka_test!(unknown_name, |ctx| {
    let missing = ctx.unique("s18-never-existed");
    let mut conn = ctx.connect().await?;
    let resp = delete(&mut conn, &[(Some(&missing), Uuid::nil())]).await?;
    let mut c = Check::new(
        format!("DeleteTopics for the unknown name '{missing}'"),
        &conn,
    );
    c.note("an unknown topic is an entry-level error, not a failure of the whole request");
    c.eq("response.responses.len", 1usize, resp.responses.len());
    c.eq(
        "response.responses[0].error_code",
        UNKNOWN_TOPIC_OR_PARTITION,
        resp.responses.first().map(|t| t.error_code).unwrap_or(-1),
    );
    c.eq("response.trailing_bytes", 0usize, resp.trailing);
    c.finish()
});

kafka_test!(unknown_id, |ctx| {
    let id = Uuid::from_u128(0x5318_0000_4000_8000_dead_beef_cafe_0001);
    let mut conn = ctx.connect().await?;
    let resp = delete(&mut conn, &[(None, id)]).await?;
    let mut c = Check::new(format!("DeleteTopics for the unknown id {id}"), &conn);
    c.eq("response.responses.len", 1usize, resp.responses.len());
    c.eq(
        "response.responses[0].error_code",
        UNKNOWN_TOPIC_ID,
        resp.responses.first().map(|t| t.error_code).unwrap_or(-1),
    );
    c.note("nothing was resolved, so there is no name to echo back");
    c.eq(
        "response.responses[0].topic_id",
        id,
        resp.responses
            .first()
            .map(|t| t.topic_id)
            .unwrap_or_default(),
    );
    c.finish()
});

kafka_test!(leaves_others, |ctx| {
    let doomed = ctx.topic("doomed")?.clone();
    let spared = ctx.topic("spared")?.clone();
    let mut conn = ctx.connect().await?;
    let resp = delete(&mut conn, &[(Some(&doomed.name), Uuid::nil())]).await?;
    let mut c = Check::new(format!("deleting only '{}'", doomed.name), &conn);
    c.eq(
        "response.responses[0].error_code",
        NONE,
        resp.responses.first().map(|t| t.error_code).unwrap_or(-1),
    );
    c.finish()?;

    wait_gone(ctx, &doomed.name).await?;
    let mut conn = ctx.connect().await?;
    let mut names = [doomed.name.as_str(), spared.name.as_str()];
    names.sort_unstable();
    let req = describe_request(&names, 100);
    let described = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the two topics after one of them was deleted", &conn);
    for got in &described.topics {
        let name = got
            .name
            .as_ref()
            .map(|n| n.0.to_string())
            .unwrap_or_default();
        if name == doomed.name {
            c.eq(
                &format!("response.topics[name={name}].error_code"),
                UNKNOWN_TOPIC_OR_PARTITION,
                got.error_code,
            );
        } else if name == spared.name {
            c.eq(
                &format!("response.topics[name={name}].error_code"),
                NONE,
                got.error_code,
            );
            c.eq(
                &format!("response.topics[name={name}].topic_id"),
                spared.id,
                got.topic_id,
            );
            c.eq(
                &format!("response.topics[name={name}].partitions.len"),
                1usize,
                got.partitions.len(),
            );
        }
    }
    c.finish()
});

kafka_test!(recreate, |ctx| {
    let t = ctx.topic("doomed")?.clone();
    let mut conn = ctx.connect().await?;
    let deleted = delete(&mut conn, &[(Some(&t.name), Uuid::nil())]).await?;
    let mut c = Check::new(
        format!("deleting '{}' before re-creating it", t.name),
        &conn,
    );
    c.eq(
        "response.responses[0].error_code",
        NONE,
        deleted
            .responses
            .first()
            .map(|r| r.error_code)
            .unwrap_or(-1),
    );
    c.finish()?;
    wait_gone(ctx, &t.name).await?;

    let mut req = CreateTopicsRequest::default();
    req.timeout_ms = 5_000;
    let mut spec = CreatableTopic::default();
    spec.name = topic_name(&t.name);
    spec.num_partitions = 1;
    spec.replication_factor = 1;
    req.topics = vec![spec];
    let mut conn = ctx.connect().await?;
    let created = conn
        .request(CREATE_TOPICS_V7, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let minted = created.topics.first().map(|t| t.topic_id);
    let mut c = Check::new(
        format!("re-creating '{}' under the same name", t.name),
        &conn,
    );
    c.eq(
        "response.topics[0].error_code",
        NONE,
        created.topics.first().map(|t| t.error_code).unwrap_or(-1),
    );
    c.note("the old id is retired with the RemoveTopicRecord and must never come back");
    c.ne("response.topics[0].topic_id", Some(t.id), minted);
    c.ne("response.topics[0].topic_id", Some(Uuid::nil()), minted);
    c.finish()?;

    let published = wait_visible(ctx, &t.name).await?;
    let mut c = Check::detached(format!("the id '{}' has after being re-created", t.name));
    c.eq(
        "Metadata: response.topics[0].topic_id",
        minted,
        Some(published),
    );
    c.ne("Metadata: response.topics[0].topic_id", t.id, published);
    c.finish()
});

/// Worked examples: deleting a topic, and deleting a name that was never there.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("Deleting a topic by name", |env| {
            let name = env.name("doomed")?;
            env.request(
                DELETE_TOPICS_V6,
                181,
                &delete_request(&[(Some(name.as_str()), Uuid::nil())]),
            )
        })
        .with_fixtures(one_topic)
        .request(
            "DeleteTopics v6, correlation id 181, one {name = the fixture topic, topic_id = \
             all zeroes} entry, timeout_ms 5000",
        )
        .response(
            "throttle_time_ms, then one responses entry: the name echoed, topic_id filled in \
             with the topic's real id — the broker resolved the name before deleting — \
             error_code 0 (NONE) and error_message null",
        )
        .note(
            "Deletion is keyed by id: resolve the name, write a RemoveTopicRecord for that id, \
             then answer. Once the record is published, DescribeTopicPartitions for the same \
             name is error 3 again with an all-zero topic_id, and re-creating the name mints a \
             brand new id.",
        ),
        ExampleSpec::wire("Deleting a name that does not exist", |env| {
            env.request(
                DELETE_TOPICS_V6,
                182,
                &delete_request(&[(Some("kafkatest-never-existed"), Uuid::nil())]),
            )
        })
        .request(
            "DeleteTopics v6, correlation id 182, one {name = 'kafkatest-never-existed', \
             topic_id = all zeroes} entry",
        )
        .response(
            "One responses entry with the name echoed, error_code 3 \
             (UNKNOWN_TOPIC_OR_PARTITION) and an all-zero topic_id; the request as a whole \
             still succeeds and the connection stays open",
        )
        .note(
            "Error 3 is what a name that cannot be resolved gets. Deleting by an id that no \
             longer exists is error 100 (UNKNOWN_TOPIC_ID) instead, and there the id is echoed \
             back unchanged, because the id is the thing that was looked up.",
        ),
    ]
}
