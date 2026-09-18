//! Stage 12 — DescribeTopicPartitions for a topic with one partition.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::fixtures::{FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::stages::{describe_request, proto_fail, Stage, Test, DESCRIBE_V0, NONE};
use uuid::Uuid;

fn one_partition() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 1))
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 12,
        slug: "describe_single_partition",
        name: "DescribeTopicPartitions: a single-partition topic",
        ext: false,
        hints: &[
            "Read __cluster_metadata-0/00000000000000000000.log at startup and index \
             TopicRecord (type 2) and PartitionRecord (type 3) by name",
            "The topic id comes from the TopicRecord, not from the topic name",
            "Each partition entry carries error_code, partition_index, leader_id, leader_epoch, \
             replica_nodes, isr_nodes and three more arrays",
            "error_code is 0 for a topic that exists, even when the partition is empty",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("the topic is found with error code 0", found).with_fixtures(one_partition),
            Test::new("the topic id matches the metadata log", topic_id)
                .with_fixtures(one_partition),
            Test::new("exactly one partition is returned", one_partition_returned)
                .with_fixtures(one_partition),
            Test::new("the partition index is 0", partition_index).with_fixtures(one_partition),
            Test::new("the partition has a leader", leader).with_fixtures(one_partition),
            Test::new("replicas and ISR contain the leader", replicas).with_fixtures(one_partition),
            Test::new("next_cursor is null when everything fits", cursor_null)
                .with_fixtures(one_partition),
            Test::new(
                "an unknown topic asked alongside it still returns error 3",
                mixed,
            )
            .with_fixtures(one_partition),
        ],
    }
}

kafka_test!(found, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = describe_request(&[&t.name], 100);
    let resp = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new(format!("DescribeTopicPartitions for '{}'", t.name), &conn);
    c.eq("response.topics.len", 1usize, resp.topics.len());
    if let Some(got) = resp.topics.first() {
        c.eq("response.topics[0].error_code", NONE, got.error_code);
        c.eq(
            "response.topics[0].name",
            Some(t.name.clone()),
            got.name.as_ref().map(|n| n.0.to_string()),
        );
    }
    c.finish()
});

kafka_test!(topic_id, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = describe_request(&[&t.name], 100);
    let resp = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the topic id from the metadata log", &conn);
    if let Some(got) = resp.topics.first() {
        c.ne("response.topics[0].topic_id", Uuid::nil(), got.topic_id);
        c.eq("response.topics[0].topic_id", t.id, got.topic_id);
    }
    c.finish()
});

kafka_test!(one_partition_returned, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = describe_request(&[&t.name], 100);
    let resp = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the partition count", &conn);
    c.eq(
        "response.topics[0].partitions.len",
        1usize,
        resp.topics.first().map(|t| t.partitions.len()).unwrap_or(0),
    );
    c.finish()
});

kafka_test!(partition_index, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = describe_request(&[&t.name], 100);
    let resp = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the single partition entry", &conn);
    if let Some(p) = resp.topics.first().and_then(|t| t.partitions.first()) {
        c.eq(
            "response.topics[0].partitions[0].partition_index",
            0i32,
            p.partition_index,
        );
        c.eq(
            "response.topics[0].partitions[0].error_code",
            NONE,
            p.error_code,
        );
    } else {
        c.that(
            "response.topics[0].partitions[0]",
            "a partition entry",
            false,
            "none",
        );
    }
    c.finish()
});

kafka_test!(leader, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = describe_request(&[&t.name], 100);
    let resp = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the partition leader", &conn);
    if let Some(p) = resp.topics.first().and_then(|t| t.partitions.first()) {
        c.at_least(
            "response.topics[0].partitions[0].leader_id",
            0i32,
            p.leader_id.0,
        );
        c.at_least(
            "response.topics[0].partitions[0].leader_epoch",
            0i32,
            p.leader_epoch,
        );
    }
    c.finish()
});

kafka_test!(replicas, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = describe_request(&[&t.name], 100);
    let resp = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the replica and ISR arrays", &conn);
    if let Some(p) = resp.topics.first().and_then(|t| t.partitions.first()) {
        let leader = p.leader_id.0;
        let replicas: Vec<i32> = p.replica_nodes.iter().map(|b| b.0).collect();
        let isr: Vec<i32> = p.isr_nodes.iter().map(|b| b.0).collect();
        c.that(
            "response.topics[0].partitions[0].replica_nodes",
            &format!("to contain the leader ({leader})"),
            replicas.contains(&leader),
            replicas.clone(),
        );
        c.that(
            "response.topics[0].partitions[0].isr_nodes",
            &format!("to contain the leader ({leader})"),
            isr.contains(&leader),
            isr,
        );
    }
    c.finish()
});

kafka_test!(cursor_null, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = describe_request(&[&t.name], 100);
    let resp = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("next_cursor with a partition limit of 100", &conn);
    c.that(
        "response.next_cursor",
        "null",
        resp.next_cursor.is_none(),
        resp.next_cursor.as_ref().map(|c| c.partition_index),
    );
    c.finish()
});

kafka_test!(mixed, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let unknown = ctx.unique("zzz-unknown");
    let mut names = [t.name.as_str(), unknown.as_str()];
    names.sort_unstable();
    let mut conn = ctx.connect().await?;
    let req = describe_request(&names, 100);
    let resp = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("a known and an unknown topic in one request", &conn);
    c.eq("response.topics.len", 2usize, resp.topics.len());
    for got in &resp.topics {
        let name = got
            .name
            .as_ref()
            .map(|n| n.0.to_string())
            .unwrap_or_default();
        let want = if name == t.name {
            NONE
        } else {
            crate::stages::UNKNOWN_TOPIC_OR_PARTITION
        };
        c.eq(
            &format!("response.topics[name={name}].error_code"),
            want,
            got.error_code,
        );
    }
    c.finish()
});

/// Worked examples: a topic that really exists, and one that does not, side by side.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("A topic with one partition", |env| {
            let name = env.name("t1")?;
            env.request(DESCRIBE_V0, 121, &describe_request(&[&name], 100))
        })
        .with_fixtures(one_partition)
        .request(
            "DescribeTopicPartitions v0, correlation id 121, the fixture topic's real name, \
             response_partition_limit 100",
        )
        .response(
            "One topics entry with error_code 0 (NONE), the name echoed and the topic's real \
             id from its TopicRecord, then one partition: error_code 0, partition_index 0, \
             leader_id the broker's node id, leader_epoch 0, replica_nodes and isr_nodes both \
             holding just that node, and nothing in eligible_leader_replicas, last_known_elr \
             or offline_replicas; next_cursor is null",
        )
        .note(
            "The id is the one the metadata log minted, not anything derived from the name — \
             a client that later fetches by id will use exactly these 16 bytes. error_code is \
             0 even though the partition holds no records.",
        ),
        ExampleSpec::wire("A known and an unknown topic together", |env| {
            let name = env.name("t1")?;
            env.request(
                DESCRIBE_V0,
                122,
                &describe_request(&["zzz-unknown", &name], 100),
            )
        })
        .with_fixtures(one_partition)
        .request(
            "DescribeTopicPartitions v0, correlation id 122, two names: 'zzz-unknown' first, \
             then the fixture topic",
        )
        .response(
            "Two entries in topic-name order — the fixture topic first with error_code 0, its \
             real id and its single partition, then 'zzz-unknown' with error_code 3 \
             (UNKNOWN_TOPIC_OR_PARTITION), an all-zero id and no partitions",
        )
        .note(
            "Each entry carries its own error_code, and the order is the broker's, not the \
             request's: topics are walked in name order so that a cursor can page through \
             them. Match entries by the echoed name.",
        ),
    ]
}
