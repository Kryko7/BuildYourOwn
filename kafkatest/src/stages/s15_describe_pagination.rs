//! Stage 15 — `response_partition_limit` and the DescribeTopicPartitions cursor.
//!
//! The limit counts partitions, not topics, and it is global to the response: a request for
//! several topics fills the page in topic-name order and stops mid-topic when the budget runs
//! out. `next_cursor` then names the exact `(topic_name, partition_index)` the next request
//! must resume at, and a request that carries a cursor skips everything before it.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::fixtures::{FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::proto::{Conn, Decoded};
use crate::stages::{describe_request, proto_fail, topic_name, Stage, Test, DESCRIBE_V0, NONE};
use kafka_protocol::messages::describe_topic_partitions_request::Cursor;
use kafka_protocol::messages::DescribeTopicPartitionsResponse;

/// Six partitions: enough that a limit of 2 needs three pages.
fn six_partitions() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("big", 6))
}

/// Two topics whose unique names keep the fixture keys' alphabetical order.
fn two_topics() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("aaa", 3)).and(TopicSpec::new("bbb", 3))
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 15,
        slug: "describe_pagination",
        name: "DescribeTopicPartitions: partition limit and cursor",
        ext: true,
        hints: &[
            "response_partition_limit counts partitions across the whole response, not per topic",
            "When the budget runs out, next_cursor is {topic_name, partition_index} of the first \
             partition that did not fit; otherwise it is a null (0xff) compact struct",
            "A request carrying a cursor resumes at that topic and partition and skips every \
             topic sorted before it",
            "Walk topics in name order so paging is stable between requests",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new(
                "a limit below the partition count returns a partial page",
                partial_page,
            )
            .with_fixtures(six_partitions)
            .ext(),
            Test::new(
                "following the cursor yields every partition exactly once",
                follow_cursor,
            )
            .with_fixtures(six_partitions)
            .ext(),
            Test::new(
                "a limit larger than the total returns a null cursor",
                limit_above_total,
            )
            .with_fixtures(six_partitions)
            .ext(),
            Test::new(
                "a limit equal to the partition count ends the walk",
                limit_equals_total,
            )
            .with_fixtures(six_partitions)
            .ext(),
            Test::new(
                "a cursor in the middle of a topic resumes there",
                cursor_mid_topic,
            )
            .with_fixtures(six_partitions)
            .ext(),
            Test::new(
                "pagination crosses topic boundaries in name order",
                across_topics,
            )
            .with_fixtures(two_topics)
            .ext(),
            Test::new(
                "a paged response decodes with no trailing bytes",
                decodes_cleanly,
            )
            .with_fixtures(six_partitions)
            .ext(),
        ],
    }
}

/// One DescribeTopicPartitions v0 request with an optional cursor.
async fn describe_page(
    conn: &mut Conn,
    names: &[&str],
    limit: i32,
    cursor: Option<(&str, i32)>,
) -> Result<Decoded<DescribeTopicPartitionsResponse>, Failure> {
    let mut req = describe_request(names, limit);
    req.cursor = cursor.map(|(name, partition)| {
        let mut c = Cursor::default();
        c.topic_name = topic_name(name);
        c.partition_index = partition;
        c
    });
    let result = conn.request(DESCRIBE_V0, &req).await;
    result.map_err(|e| proto_fail(e, conn))
}

/// Every `(topic name, partition index)` pair a response carries, in wire order.
fn pairs(resp: &DescribeTopicPartitionsResponse) -> Vec<(String, i32)> {
    resp.topics
        .iter()
        .flat_map(|t| {
            let name = t.name.as_ref().map(|n| n.0.to_string()).unwrap_or_default();
            t.partitions
                .iter()
                .map(move |p| (name.clone(), p.partition_index))
        })
        .collect()
}

/// The cursor as a plain pair, so failures print something readable.
fn cursor_pair(resp: &DescribeTopicPartitionsResponse) -> Option<(String, i32)> {
    resp.next_cursor
        .as_ref()
        .map(|c| (c.topic_name.0.to_string(), c.partition_index))
}

kafka_test!(partial_page, |ctx| {
    let t = ctx.topic("big")?.clone();
    let mut conn = ctx.connect().await?;
    let resp = describe_page(&mut conn, &[&t.name], 2, None).await?;
    let mut c = Check::new(
        format!("a 2-partition page of '{}', which has 6 partitions", t.name),
        &conn,
    );
    c.eq("response.topics.len", 1usize, resp.topics.len());
    if let Some(topic) = resp.topics.first() {
        c.eq("response.topics[0].error_code", NONE, topic.error_code);
        let indices: Vec<i32> = topic.partitions.iter().map(|p| p.partition_index).collect();
        c.eq(
            "response.topics[0].partitions[*].partition_index",
            vec![0i32, 1],
            indices,
        );
    }
    c.note("the limit counts partitions, so the page stops after two of the six");
    c.eq(
        "response.next_cursor",
        Some((t.name.clone(), 2i32)),
        cursor_pair(&resp),
    );
    c.finish()
});

kafka_test!(follow_cursor, |ctx| {
    let t = ctx.topic("big")?.clone();
    let mut conn = ctx.connect().await?;
    let mut seen: Vec<(String, i32)> = Vec::new();
    let mut cursor: Option<(String, i32)> = None;
    let mut pages = 0usize;
    let mut overran = false;
    loop {
        let resp = describe_page(
            &mut conn,
            &[&t.name],
            2,
            cursor.as_ref().map(|(n, p)| (n.as_str(), *p)),
        )
        .await?;
        pages += 1;
        seen.extend(pairs(&resp));
        match cursor_pair(&resp) {
            Some(next) => cursor = Some(next),
            None => break,
        }
        if pages >= 8 {
            overran = true;
            break;
        }
    }
    let mut c = Check::new(
        format!("paging through '{}' two partitions at a time", t.name),
        &conn,
    );
    c.that(
        "the walk terminates",
        "next_cursor to become null within 8 pages",
        !overran,
        cursor.clone(),
    );
    let want: Vec<(String, i32)> = (0..6).map(|i| (t.name.clone(), i)).collect();
    c.eq("the partitions collected by paging", want, seen);
    c.note("a broker may spend a final empty page when the limit lands on a topic boundary");
    c.that(
        "the number of pages",
        "3 or 4 for 6 partitions at 2 per page",
        (3..=4).contains(&pages),
        pages,
    );
    c.finish()
});

kafka_test!(limit_above_total, |ctx| {
    let t = ctx.topic("big")?.clone();
    let mut conn = ctx.connect().await?;
    let resp = describe_page(&mut conn, &[&t.name], 1_000, None).await?;
    let mut c = Check::new("a limit far above the partition count", &conn);
    c.eq(
        "response.topics[0].partitions.len",
        6usize,
        resp.topics.first().map(|t| t.partitions.len()).unwrap_or(0),
    );
    c.note("nothing was left out, so there is nothing to resume at");
    c.eq("response.next_cursor", None, cursor_pair(&resp));
    c.finish()
});

kafka_test!(limit_equals_total, |ctx| {
    let t = ctx.topic("big")?.clone();
    let mut conn = ctx.connect().await?;
    let resp = describe_page(&mut conn, &[&t.name], 6, None).await?;
    let mut c = Check::new("a limit exactly equal to the partition count", &conn);
    let indices: Vec<i32> = resp
        .topics
        .first()
        .map(|t| t.partitions.iter().map(|p| p.partition_index).collect())
        .unwrap_or_default();
    c.eq(
        "response.topics[0].partitions[*].partition_index",
        vec![0i32, 1, 2, 3, 4, 5],
        indices,
    );
    c.eq("response.next_cursor", None, cursor_pair(&resp));
    c.finish()
});

kafka_test!(cursor_mid_topic, |ctx| {
    let t = ctx.topic("big")?.clone();
    let mut conn = ctx.connect().await?;
    let resp = describe_page(&mut conn, &[&t.name], 1_000, Some((&t.name, 3))).await?;
    let mut c = Check::new(
        format!("a request resuming at partition 3 of '{}'", t.name),
        &conn,
    );
    c.note("the cursor is inclusive: the named partition is the first one returned");
    let indices: Vec<i32> = resp
        .topics
        .first()
        .map(|t| t.partitions.iter().map(|p| p.partition_index).collect())
        .unwrap_or_default();
    c.eq(
        "response.topics[0].partitions[*].partition_index",
        vec![3i32, 4, 5],
        indices,
    );
    c.eq("response.next_cursor", None, cursor_pair(&resp));
    c.finish()
});

kafka_test!(across_topics, |ctx| {
    let a = ctx.topic("aaa")?.clone();
    let b = ctx.topic("bbb")?.clone();
    let mut conn = ctx.connect().await?;
    let resp = describe_page(&mut conn, &[&a.name, &b.name], 4, None).await?;
    let mut c = Check::new(
        format!("a 4-partition page over '{}' and '{}'", a.name, b.name),
        &conn,
    );
    c.note("the budget is global: three partitions of the first topic, then one of the second");
    let mut want: Vec<(String, i32)> = (0..3).map(|i| (a.name.clone(), i)).collect();
    want.push((b.name.clone(), 0));
    c.eq("response.topics[*].partitions[*]", want, pairs(&resp));
    c.eq(
        "response.next_cursor",
        Some((b.name.clone(), 1i32)),
        cursor_pair(&resp),
    );

    let rest = describe_page(&mut conn, &[&a.name, &b.name], 4, Some((&b.name, 1))).await?;
    c.bytes(&conn);
    c.note("the second page must not repeat the topic that was already finished");
    let want_rest: Vec<(String, i32)> = (1..3).map(|i| (b.name.clone(), i)).collect();
    c.eq(
        "page 2: response.topics[*].partitions[*]",
        want_rest,
        pairs(&rest),
    );
    c.eq("page 2: response.next_cursor", None, cursor_pair(&rest));
    c.finish()
});

kafka_test!(decodes_cleanly, |ctx| {
    let t = ctx.topic("big")?.clone();
    let mut conn = ctx.connect().await?;
    let decoded = describe_page(&mut conn, &[&t.name], 2, None).await?;
    let mut c = Check::new("a response whose cursor is present", &conn);
    c.note("a non-null cursor is a compact string plus an int32 plus its own tagged-field byte");
    c.eq("response.trailing_bytes", 0usize, decoded.trailing);
    c.finish()
});

/// Worked examples: a page that does not fit, and the request that resumes it.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("A partition limit of 1", |env| {
            let name = env.name("big")?;
            env.request(DESCRIBE_V0, 151, &describe_request(&[&name], 1))
        })
        .with_fixtures(six_partitions)
        .request(
            "DescribeTopicPartitions v0, correlation id 151, the fixture topic (6 partitions), \
             response_partition_limit 1, cursor null",
        )
        .response(
            "One topics entry with error_code 0 carrying exactly one partition \
             (partition_index 0), then a non-null next_cursor: the topic's name and \
             partition_index 1 — the first partition that did not fit",
        )
        .note(
            "response_partition_limit counts partitions across the whole response, not per \
             topic. When the budget runs out, next_cursor stops being the null byte 0xff and \
             becomes a struct: a compact string, an int32 and its own tagged-field byte.",
        ),
        ExampleSpec::wire("The follow-up request that carries the cursor", |env| {
            let name = env.name("big")?;
            let mut req = describe_request(&[&name], 1);
            let mut cursor = Cursor::default();
            cursor.topic_name = topic_name(&name);
            cursor.partition_index = 1;
            req.cursor = Some(cursor);
            env.request(DESCRIBE_V0, 152, &req)
        })
        .with_fixtures(six_partitions)
        .request(
            "The same request, correlation id 152, response_partition_limit 1, but with cursor \
             {topic_name = the fixture topic, partition_index 1} — what a client sends after \
             the page above",
        )
        .response(
            "partition_index 1 alone: the cursor is inclusive, so the partition it names is \
             the first one returned. next_cursor then points at partition_index 2",
        )
        .note(
            "Resuming means skipping every topic sorted before the cursor's name and every \
             partition below its index. The walk ends when next_cursor comes back null; a \
             broker may spend one final, empty page when the limit lands exactly on the end \
             of a topic.",
        ),
    ]
}
