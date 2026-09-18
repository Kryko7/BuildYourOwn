//! Stage 16 — Metadata (3) v12.
//!
//! v12 is the flexible version every modern client bootstraps with: compact strings, tagged
//! fields, and a `topic_id` next to every topic name. The response has to describe the
//! cluster (brokers, controller, cluster id) as well as the topics, and an unknown topic is
//! an error *inside* its own entry, never a request-level failure.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::fixtures::{FixtureSpec, TopicSpec};
use crate::proto::{Conn, Decoded};
use crate::stages::{
    describe_request, proto_fail, topic_name, Stage, Test, DESCRIBE_V0, NONE,
    UNKNOWN_TOPIC_OR_PARTITION,
};
use crate::{fixtures::TopicInfo, kafka_test};
use kafka_protocol::messages::metadata_request::MetadataRequestTopic;
use kafka_protocol::messages::{MetadataRequest, MetadataResponse};
use uuid::Uuid;

/// The `Metadata` version this stage speaks.
const METADATA_V12: i16 = 12;

fn one_topic() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("m1", 3))
}

fn two_topics() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("m1", 3)).and(TopicSpec::new("m2", 1))
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 16,
        slug: "metadata_v12",
        name: "Metadata (3) v12: brokers, controller and topic ids",
        ext: true,
        hints: &[
            "v12 is flexible: compact arrays and strings, a tagged-field byte after every \
             struct, and a nullable cluster_id",
            "brokers[] must advertise a host and port a client can really connect to, and \
             controller_id must be one of those node ids",
            "A null topics array means 'every topic'; an empty array means 'no topics'",
            "An unknown topic is error 3 inside its own entry with a zero topic_id, and \
             allow_auto_topic_creation=false must never create it",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("the brokers array advertises a reachable listener", brokers)
                .with_fixtures(one_topic)
                .ext(),
            Test::new("controller_id names one of the brokers", controller)
                .with_fixtures(one_topic)
                .ext(),
            Test::new("cluster_id is a non-empty string", cluster_id)
                .with_fixtures(one_topic)
                .ext(),
            Test::new(
                "a known topic comes back with its id and partitions",
                known_topic,
            )
            .with_fixtures(one_topic)
            .ext(),
            Test::new(
                "an unknown topic is error 3 in its own entry",
                unknown_topic,
            )
            .with_fixtures(one_topic)
            .ext(),
            Test::new(
                "allow_auto_topic_creation=false creates nothing",
                no_auto_create,
            )
            .with_fixtures(one_topic)
            .ext(),
            Test::new("a null topics array lists every topic", null_topics)
                .with_fixtures(two_topics)
                .ext(),
            Test::new(
                "the v12 response decodes with no trailing bytes",
                decodes_cleanly,
            )
            .with_fixtures(two_topics)
            .ext(),
        ],
    }
}

/// A v12 `Metadata` request: `None` asks for every topic, `Some(names)` for those names.
fn metadata_request(names: Option<&[&str]>) -> MetadataRequest {
    let mut r = MetadataRequest::default();
    r.allow_auto_topic_creation = false;
    r.include_cluster_authorized_operations = false;
    r.include_topic_authorized_operations = false;
    r.topics = names.map(|ns| {
        ns.iter()
            .map(|n| {
                let mut t = MetadataRequestTopic::default();
                t.name = Some(topic_name(n));
                t
            })
            .collect()
    });
    r
}

async fn metadata(
    conn: &mut Conn,
    names: Option<&[&str]>,
) -> Result<Decoded<MetadataResponse>, Failure> {
    let req = metadata_request(names);
    let result = conn.request(METADATA_V12, &req).await;
    result.map_err(|e| proto_fail(e, conn))
}

/// The entry for `name`, or a failure listing what did come back.
fn entry<'a>(
    resp: &'a MetadataResponse,
    name: &str,
) -> Option<&'a kafka_protocol::messages::metadata_response::MetadataResponseTopic> {
    resp.topics
        .iter()
        .find(|t| t.name.as_ref().map(|n| n.0.as_str()) == Some(name))
}

/// The names the response carries, for failure messages.
fn names_of(resp: &MetadataResponse) -> Vec<String> {
    resp.topics
        .iter()
        .map(|t| t.name.as_ref().map(|n| n.0.to_string()).unwrap_or_default())
        .collect()
}

kafka_test!(brokers, |ctx| {
    let t = ctx.topic("m1")?.clone();
    let port = ctx.addr.port();
    let mut conn = ctx.connect().await?;
    let resp = metadata(&mut conn, Some(&[&t.name])).await?;
    let mut c = Check::new("the brokers array of a v12 Metadata response", &conn);
    c.eq("response.error_code", NONE, resp.error_code);
    c.at_least("response.brokers.len", 1usize, resp.brokers.len());
    for (i, b) in resp.brokers.iter().enumerate() {
        c.at_least(&format!("response.brokers[{i}].node_id"), 0i32, b.node_id.0);
        c.that(
            &format!("response.brokers[{i}].host"),
            "a non-empty host name",
            !b.host.is_empty(),
            b.host.to_string(),
        );
        c.at_least(&format!("response.brokers[{i}].port"), 1i32, b.port);
    }
    c.note("the advertised listener is what a client reconnects to, so it must be this one");
    let advertised: Vec<i32> = resp.brokers.iter().map(|b| b.port).collect();
    c.that(
        "response.brokers[*].port",
        &format!("to include the port we connected to ({port})"),
        advertised.contains(&i32::from(port)),
        advertised,
    );
    c.finish()
});

kafka_test!(controller, |ctx| {
    let t = ctx.topic("m1")?.clone();
    let mut conn = ctx.connect().await?;
    let resp = metadata(&mut conn, Some(&[&t.name])).await?;
    let ids: Vec<i32> = resp.brokers.iter().map(|b| b.node_id.0).collect();
    let mut c = Check::new("controller_id", &conn);
    c.note("in KRaft the broker answers with an alive broker that can serve the metadata log");
    c.that(
        "response.controller_id",
        &format!("one of the advertised node ids {ids:?}"),
        ids.contains(&resp.controller_id.0),
        resp.controller_id.0,
    );
    c.finish()
});

kafka_test!(cluster_id, |ctx| {
    let t = ctx.topic("m1")?.clone();
    let mut conn = ctx.connect().await?;
    let resp = metadata(&mut conn, Some(&[&t.name])).await?;
    let got = resp.cluster_id.as_ref().map(|s| s.to_string());
    let mut c = Check::new("cluster_id", &conn);
    c.note("cluster_id is a nullable compact string; it is the id `kafka-storage.sh format` wrote");
    c.that(
        "response.cluster_id",
        "a non-empty string",
        got.as_ref().is_some_and(|s| !s.is_empty()),
        got,
    );
    c.finish()
});

kafka_test!(known_topic, |ctx| {
    let t: TopicInfo = ctx.topic("m1")?.clone();
    let mut conn = ctx.connect().await?;
    let resp = metadata(&mut conn, Some(&[&t.name])).await?;
    let mut c = Check::new(format!("the Metadata entry for '{}'", t.name), &conn);
    let Some(got) = entry(&resp, &t.name) else {
        c.that(
            "response.topics[*].name",
            &format!("to contain '{}'", t.name),
            false,
            names_of(&resp),
        );
        return c.finish();
    };
    c.eq("response.topics[0].error_code", NONE, got.error_code);
    c.note("the id is the one CreateTopics handed out; it is never derived from the name");
    c.eq("response.topics[0].topic_id", t.id, got.topic_id);
    c.eq("response.topics[0].is_internal", false, got.is_internal);
    let mut indices: Vec<i32> = got.partitions.iter().map(|p| p.partition_index).collect();
    indices.sort_unstable();
    c.note(
        "Metadata makes no promise about partition order (Apache Kafka answers them in hash \
         order), so the indices are sorted before they are compared",
    );
    c.eq(
        "sorted response.topics[0].partitions[*].partition_index",
        vec![0i32, 1, 2],
        indices,
    );
    for p in &got.partitions {
        let path = format!("response.topics[0].partitions[{}]", p.partition_index);
        c.eq(&format!("{path}.error_code"), NONE, p.error_code);
        c.at_least(&format!("{path}.leader_id"), 0i32, p.leader_id.0);
        let replicas: Vec<i32> = p.replica_nodes.iter().map(|b| b.0).collect();
        let isr: Vec<i32> = p.isr_nodes.iter().map(|b| b.0).collect();
        c.that(
            &format!("{path}.replica_nodes"),
            &format!("to contain the leader ({})", p.leader_id.0),
            replicas.contains(&p.leader_id.0),
            replicas,
        );
        c.that(
            &format!("{path}.isr_nodes"),
            &format!("to contain the leader ({})", p.leader_id.0),
            isr.contains(&p.leader_id.0),
            isr,
        );
    }
    c.finish()
});

kafka_test!(unknown_topic, |ctx| {
    let known = ctx.topic("m1")?.clone();
    let missing = ctx.unique("s16-never-created");
    let mut conn = ctx.connect().await?;
    let resp = metadata(&mut conn, Some(&[&known.name, &missing])).await?;
    let mut c = Check::new(
        "a known and an unknown topic in one Metadata request",
        &conn,
    );
    c.note("an unknown topic never fails the request: the error lives in its own entry");
    c.eq("response.error_code", NONE, resp.error_code);
    c.eq("response.topics.len", 2usize, resp.topics.len());
    match entry(&resp, &missing) {
        None => {
            c.that(
                "response.topics[*].name",
                &format!("to contain the unknown name '{missing}'"),
                false,
                names_of(&resp),
            );
        }
        Some(got) => {
            c.eq(
                &format!("response.topics[name={missing}].error_code"),
                UNKNOWN_TOPIC_OR_PARTITION,
                got.error_code,
            );
            c.eq(
                &format!("response.topics[name={missing}].topic_id"),
                Uuid::nil(),
                got.topic_id,
            );
            c.eq(
                &format!("response.topics[name={missing}].partitions.len"),
                0usize,
                got.partitions.len(),
            );
        }
    }
    if let Some(got) = entry(&resp, &known.name) {
        c.eq(
            &format!("response.topics[name={}].error_code", known.name),
            NONE,
            got.error_code,
        );
    }
    c.finish()
});

kafka_test!(no_auto_create, |ctx| {
    let missing = ctx.unique("s16-no-auto-create");
    let mut conn = ctx.connect().await?;
    let first = metadata(&mut conn, Some(&[&missing])).await?;
    let mut c = Check::new("Metadata with allow_auto_topic_creation=false", &conn);
    c.eq(
        "response.topics[0].error_code",
        UNKNOWN_TOPIC_OR_PARTITION,
        first.topics.first().map(|t| t.error_code).unwrap_or(NONE),
    );

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let second = metadata(&mut conn, Some(&[&missing])).await?;
    c.bytes(&conn);
    c.note("asking about a topic must never bring it into existence");
    c.eq(
        "a second Metadata: response.topics[0].error_code",
        UNKNOWN_TOPIC_OR_PARTITION,
        second.topics.first().map(|t| t.error_code).unwrap_or(NONE),
    );

    let req = describe_request(&[&missing], 100);
    let described = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    c.bytes(&conn);
    c.eq(
        "DescribeTopicPartitions: response.topics[0].error_code",
        UNKNOWN_TOPIC_OR_PARTITION,
        described
            .topics
            .first()
            .map(|t| t.error_code)
            .unwrap_or(NONE),
    );
    c.finish()
});

kafka_test!(null_topics, |ctx| {
    let a = ctx.topic("m1")?.clone();
    let b = ctx.topic("m2")?.clone();
    let mut conn = ctx.connect().await?;
    let resp = metadata(&mut conn, None).await?;
    let mut c = Check::new("Metadata with a null topics array", &conn);
    c.note("a null (0xff) topics array asks for every topic the broker knows");
    for want in [&a, &b] {
        match entry(&resp, &want.name) {
            None => {
                c.that(
                    "response.topics[*].name",
                    &format!("to contain '{}'", want.name),
                    false,
                    names_of(&resp),
                );
            }
            Some(got) => {
                c.eq(
                    &format!("response.topics[name={}].error_code", want.name),
                    NONE,
                    got.error_code,
                );
                c.eq(
                    &format!("response.topics[name={}].topic_id", want.name),
                    want.id,
                    got.topic_id,
                );
                c.eq(
                    &format!("response.topics[name={}].partitions.len", want.name),
                    want.partitions as usize,
                    got.partitions.len(),
                );
            }
        }
    }
    c.finish()
});

kafka_test!(decodes_cleanly, |ctx| {
    let a = ctx.topic("m1")?.clone();
    let b = ctx.topic("m2")?.clone();
    let mut conn = ctx.connect().await?;
    let decoded = metadata(&mut conn, Some(&[&a.name, &b.name])).await?;
    let mut c = Check::new("a two-topic v12 Metadata body", &conn);
    c.note("every broker, partition and topic struct ends in its own tagged-field byte");
    c.eq("response.trailing_bytes", 0usize, decoded.trailing);
    c.finish()
});

/// Worked examples: the bootstrap request every real client sends first.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("Metadata v12 for one named topic", |env| {
            let name = env.name("m1")?;
            env.request(METADATA_V12, 161, &metadata_request(Some(&[&name])))
        })
        .with_fixtures(one_topic)
        .request(
            "Metadata v12, correlation id 161, topics [the fixture topic], \
             allow_auto_topic_creation false, no authorized-operations flags",
        )
        .response(
            "The cluster first: throttle_time_ms, a brokers array whose single entry has a \
             node_id, the advertised host and the port this connection came in on, then \
             cluster_id (a non-null compact string) and controller_id naming one of those \
             brokers. Then one topics entry: error_code 0, the name, its real topic_id, \
             is_internal false, and three partitions each with leader_id, leader_epoch, \
             replica_nodes and isr_nodes",
        )
        .note(
            "v12 is flexible — compact strings and arrays, a tagged-field byte after every \
             struct — and it is the first Metadata version that carries topic_id beside the \
             name. brokers[] is what a client reconnects to, so it must advertise a listener \
             that really accepts connections, not 0.0.0.0.",
        ),
        ExampleSpec::wire("A null topics array asks for every topic", |env| {
            env.request(METADATA_V12, 162, &metadata_request(None))
        })
        .with_fixtures(two_topics)
        .request(
            "Metadata v12, correlation id 162, the topics array encoded as null (the single \
             byte 0x00), allow_auto_topic_creation false",
        )
        .response(
            "The same cluster section, then one entry per topic that exists on the broker — \
             both fixture topics among them, each with its own id, partitions and error_code 0",
        )
        .note(
            "Null and empty are two different requests: 0x00 is a null array and means 'every \
             topic', while 0x01 is an empty compact array and means 'no topics, just tell me \
             about the cluster'. Reading the length byte as a count rather than count + 1 \
             confuses the two.",
        ),
    ]
}
