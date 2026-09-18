//! Stage 13 — DescribeTopicPartitions for a topic with several partitions.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::fixtures::{FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::stages::{describe_request, proto_fail, Stage, Test, DESCRIBE_V0, NONE};

fn three_partitions() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 3))
}

fn two_topics_many_partitions() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 3)).and(TopicSpec::new("t2", 2))
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 13,
        slug: "describe_multiple_partitions",
        name: "DescribeTopicPartitions: multiple partitions",
        ext: false,
        hints: &[
            "One PartitionRecord per partition: collect them all before answering",
            "Partitions come back sorted by partition_index, not in metadata-log order",
            "replica_nodes, isr_nodes, eligible_leader_replicas, last_known_elr and \
             offline_replicas are five separate compact arrays",
            "A topic with N partitions still has exactly one topic entry in the response",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("all three partitions are returned", all_three)
                .with_fixtures(three_partitions),
            Test::new("partitions are ordered by index", ordered).with_fixtures(three_partitions),
            Test::new("every partition has error code 0", all_ok).with_fixtures(three_partitions),
            Test::new("every partition has a leader and an ISR", leaders)
                .with_fixtures(three_partitions),
            Test::new(
                "the partition arrays decode with no trailing bytes",
                decodes_cleanly,
            )
            .with_fixtures(three_partitions),
            Test::new(
                "two topics with different partition counts are both right",
                two_topics,
            )
            .with_fixtures(two_topics_many_partitions),
        ],
    }
}

kafka_test!(all_three, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = describe_request(&[&t.name], 100);
    let resp = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new(format!("the partitions of '{}'", t.name), &conn);
    c.eq("response.topics.len", 1usize, resp.topics.len());
    c.eq(
        "response.topics[0].partitions.len",
        3usize,
        resp.topics.first().map(|t| t.partitions.len()).unwrap_or(0),
    );
    c.finish()
});

kafka_test!(ordered, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = describe_request(&[&t.name], 100);
    let resp = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let indices: Vec<i32> = resp
        .topics
        .first()
        .map(|t| t.partitions.iter().map(|p| p.partition_index).collect())
        .unwrap_or_default();
    let mut c = Check::new("the order of the partition entries", &conn);
    c.eq(
        "response.topics[0].partitions[*].partition_index",
        vec![0i32, 1, 2],
        indices,
    );
    c.finish()
});

kafka_test!(all_ok, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = describe_request(&[&t.name], 100);
    let resp = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the error code of every partition", &conn);
    if let Some(topic) = resp.topics.first() {
        c.eq("response.topics[0].error_code", NONE, topic.error_code);
        for p in &topic.partitions {
            c.eq(
                &format!(
                    "response.topics[0].partitions[{}].error_code",
                    p.partition_index
                ),
                NONE,
                p.error_code,
            );
        }
    }
    c.finish()
});

kafka_test!(leaders, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = describe_request(&[&t.name], 100);
    let resp = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("leaders and ISR of every partition", &conn);
    if let Some(topic) = resp.topics.first() {
        for p in &topic.partitions {
            let path = format!("response.topics[0].partitions[{}]", p.partition_index);
            c.at_least(&format!("{path}.leader_id"), 0i32, p.leader_id.0);
            let isr: Vec<i32> = p.isr_nodes.iter().map(|b| b.0).collect();
            c.that(
                &format!("{path}.isr_nodes"),
                &format!("to contain the leader ({})", p.leader_id.0),
                isr.contains(&p.leader_id.0),
                isr,
            );
        }
    }
    c.finish()
});

kafka_test!(decodes_cleanly, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = describe_request(&[&t.name], 100);
    let decoded = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("the multi-partition response body", &conn);
    c.note("a missing per-partition tagged-field byte shows up here as leftover bytes");
    c.eq("response.trailing_bytes", 0usize, decoded.trailing);
    c.finish()
});

kafka_test!(two_topics, |ctx| {
    let t1 = ctx.topic("t1")?.clone();
    let t2 = ctx.topic("t2")?.clone();
    let mut names = [t1.name.as_str(), t2.name.as_str()];
    names.sort_unstable();
    let mut conn = ctx.connect().await?;
    let req = describe_request(&names, 100);
    let resp = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("two topics with 3 and 2 partitions", &conn);
    c.eq("response.topics.len", 2usize, resp.topics.len());
    for got in &resp.topics {
        let name = got
            .name
            .as_ref()
            .map(|n| n.0.to_string())
            .unwrap_or_default();
        let want = if name == t1.name { 3usize } else { 2usize };
        c.eq(
            &format!("response.topics[name={name}].partitions.len"),
            want,
            got.partitions.len(),
        );
    }
    c.finish()
});

/// Worked examples: one topic entry, several partition entries.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("Three partitions, in index order", |env| {
            let name = env.name("t1")?;
            env.request(DESCRIBE_V0, 131, &describe_request(&[&name], 100))
        })
        .with_fixtures(three_partitions)
        .request(
            "DescribeTopicPartitions v0, correlation id 131, the fixture topic (3 partitions), \
             response_partition_limit 100",
        )
        .response(
            "One topics entry with error_code 0 and its real topic id, holding a compact array \
             of three partitions with partition_index 0, 1 and 2 in that order; each has \
             error_code 0, a leader_id, a leader_epoch, replica_nodes and isr_nodes naming the \
             one broker, and nothing in the three remaining arrays. next_cursor is null, since \
             all three fitted under the limit",
        )
        .note(
            "The metadata log stores one PartitionRecord per partition and not necessarily in \
             index order, so sort before you write. Five compact arrays follow each \
             leader_epoch (replicas, ISR, eligible leader replicas, last known ELR, offline \
             replicas) — a forgotten empty one is the usual cause of a body that will not \
             decode.",
        ),
        ExampleSpec::wire("Two topics, different partition counts", |env| {
            let t1 = env.name("t1")?;
            let t2 = env.name("t2")?;
            env.request(DESCRIBE_V0, 132, &describe_request(&[&t1, &t2], 100))
        })
        .with_fixtures(two_topics_many_partitions)
        .request(
            "DescribeTopicPartitions v0, correlation id 132, two fixture topics — the first \
             with 3 partitions, the second with 2 — response_partition_limit 100",
        )
        .response(
            "Two topics entries in name order, both error_code 0: the first carrying \
             partitions 0, 1, 2 and the second partitions 0, 1, each partition with its own \
             leader and ISR; next_cursor is null",
        )
        .note(
            "The partitions array belongs to its topic entry, so two topics mean two arrays \
             and not one flat list. However many partitions a topic has, it still gets exactly \
             one topic entry.",
        ),
    ]
}
