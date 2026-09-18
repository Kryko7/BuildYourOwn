//! Stage 14 — Several topics in one DescribeTopicPartitions request.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::fixtures::{FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::stages::{
    describe_request, proto_fail, Stage, Test, DESCRIBE_V0, NONE, UNKNOWN_TOPIC_OR_PARTITION,
};

fn three_topics() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("alpha", 1))
        .and(TopicSpec::new("bravo", 2))
        .and(TopicSpec::new("charlie", 1))
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 14,
        slug: "describe_multiple_topics",
        name: "DescribeTopicPartitions: multiple topics",
        ext: false,
        hints: &[
            "The request's topics array is a compact array of {name, tagged fields}",
            "Answer one entry per requested topic, in order of topic name",
            "Unknown names sit next to known ones: each entry carries its own error_code",
            "Keep the per-topic tagged-field byte after topic_authorized_operations",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("all three topics are described", all_three).with_fixtures(three_topics),
            Test::new("the response is ordered by topic name", ordered_by_name)
                .with_fixtures(three_topics),
            Test::new(
                "an unsorted request still comes back sorted",
                unsorted_request,
            )
            .with_fixtures(three_topics)
            .ext(),
            Test::new("each topic keeps its own partition count", partition_counts)
                .with_fixtures(three_topics),
            Test::new("each topic keeps its own id", distinct_ids).with_fixtures(three_topics),
            Test::new("known and unknown topics can be mixed", mixed).with_fixtures(three_topics),
            Test::new(
                "the response decodes with no trailing bytes",
                decodes_cleanly,
            )
            .with_fixtures(three_topics),
        ],
    }
}

fn sorted_names(ctx: &crate::stages::Ctx) -> Result<Vec<String>, crate::assert::Failure> {
    let mut names = vec![
        ctx.topic("alpha")?.name.clone(),
        ctx.topic("bravo")?.name.clone(),
        ctx.topic("charlie")?.name.clone(),
    ];
    names.sort();
    Ok(names)
}

kafka_test!(all_three, |ctx| {
    let names = sorted_names(ctx)?;
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut conn = ctx.connect().await?;
    let req = describe_request(&refs, 100);
    let resp = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("three topics in one request", &conn);
    c.eq("response.topics.len", 3usize, resp.topics.len());
    for (i, t) in resp.topics.iter().enumerate() {
        c.eq(
            &format!("response.topics[{i}].error_code"),
            NONE,
            t.error_code,
        );
    }
    c.finish()
});

kafka_test!(ordered_by_name, |ctx| {
    let names = sorted_names(ctx)?;
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut conn = ctx.connect().await?;
    let req = describe_request(&refs, 100);
    let resp = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let got: Vec<String> = resp
        .topics
        .iter()
        .map(|t| t.name.as_ref().map(|n| n.0.to_string()).unwrap_or_default())
        .collect();
    let mut c = Check::new("the order of the topic entries", &conn);
    c.eq("response.topics[*].name", names, got);
    c.finish()
});

kafka_test!(unsorted_request, |ctx| {
    let names = sorted_names(ctx)?;
    let reversed: Vec<&str> = names.iter().rev().map(String::as_str).collect();
    let mut conn = ctx.connect().await?;
    let req = describe_request(&reversed, 100);
    let resp = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let got: Vec<String> = resp
        .topics
        .iter()
        .map(|t| t.name.as_ref().map(|n| n.0.to_string()).unwrap_or_default())
        .collect();
    let mut c = Check::new("a request whose topics are in reverse order", &conn);
    c.note("DescribeTopicPartitions walks topics in name order so the cursor can page them");
    c.eq("response.topics[*].name", names, got);
    c.finish()
});

kafka_test!(partition_counts, |ctx| {
    let alpha = ctx.topic("alpha")?.clone();
    let bravo = ctx.topic("bravo")?.clone();
    let charlie = ctx.topic("charlie")?.clone();
    let names = sorted_names(ctx)?;
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut conn = ctx.connect().await?;
    let req = describe_request(&refs, 100);
    let resp = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("per-topic partition counts", &conn);
    for t in &resp.topics {
        let name = t.name.as_ref().map(|n| n.0.to_string()).unwrap_or_default();
        let want = if name == alpha.name {
            1usize
        } else if name == bravo.name {
            2
        } else if name == charlie.name {
            1
        } else {
            continue;
        };
        c.eq(
            &format!("response.topics[name={name}].partitions.len"),
            want,
            t.partitions.len(),
        );
    }
    c.finish()
});

kafka_test!(distinct_ids, |ctx| {
    let alpha = ctx.topic("alpha")?.clone();
    let bravo = ctx.topic("bravo")?.clone();
    let names = sorted_names(ctx)?;
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut conn = ctx.connect().await?;
    let req = describe_request(&refs, 100);
    let resp = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("that each topic keeps its own id", &conn);
    let mut ids = Vec::new();
    for t in &resp.topics {
        let name = t.name.as_ref().map(|n| n.0.to_string()).unwrap_or_default();
        if name == alpha.name {
            c.eq(
                &format!("response.topics[name={name}].topic_id"),
                alpha.id,
                t.topic_id,
            );
        }
        if name == bravo.name {
            c.eq(
                &format!("response.topics[name={name}].topic_id"),
                bravo.id,
                t.topic_id,
            );
        }
        ids.push(t.topic_id);
    }
    let before = ids.len();
    ids.sort();
    ids.dedup();
    c.eq("response.topics[*].topic_id (unique)", before, ids.len());
    c.finish()
});

kafka_test!(mixed, |ctx| {
    let mut names = sorted_names(ctx)?;
    let unknown = ctx.unique("zzz-missing");
    names.push(unknown.clone());
    names.sort();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut conn = ctx.connect().await?;
    let req = describe_request(&refs, 100);
    let resp = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("three known topics and one unknown", &conn);
    c.eq("response.topics.len", 4usize, resp.topics.len());
    for t in &resp.topics {
        let name = t.name.as_ref().map(|n| n.0.to_string()).unwrap_or_default();
        let want = if name == unknown {
            UNKNOWN_TOPIC_OR_PARTITION
        } else {
            NONE
        };
        c.eq(
            &format!("response.topics[name={name}].error_code"),
            want,
            t.error_code,
        );
    }
    c.finish()
});

kafka_test!(decodes_cleanly, |ctx| {
    let names = sorted_names(ctx)?;
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut conn = ctx.connect().await?;
    let req = describe_request(&refs, 100);
    let decoded = conn
        .request(DESCRIBE_V0, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("a three-topic response body", &conn);
    c.eq("response.trailing_bytes", 0usize, decoded.trailing);
    c.finish()
});

/// Worked examples: several names in one request, and the order they come back in.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("Three topics, requested out of order", |env| {
            let a = env.name("alpha")?;
            let b = env.name("bravo")?;
            let c = env.name("charlie")?;
            env.request(DESCRIBE_V0, 141, &describe_request(&[&c, &b, &a], 100))
        })
        .with_fixtures(three_topics)
        .request(
            "DescribeTopicPartitions v0, correlation id 141, three fixture topic names in \
             reverse alphabetical order (charlie, bravo, alpha), response_partition_limit 100",
        )
        .response(
            "Three entries in alphabetical order — alpha, then bravo, then charlie — each with \
             error_code 0, its own topic id, and 1, 2 and 1 partitions respectively; \
             next_cursor is null",
        )
        .note(
            "The response order is the broker's, not the request's: DescribeTopicPartitions \
             walks topics in name order so that next_cursor can resume a page. A client \
             matches entries by the echoed name, never by position.",
        ),
        ExampleSpec::wire("Known topics next to a missing one", |env| {
            let a = env.name("alpha")?;
            let b = env.name("bravo")?;
            let c = env.name("charlie")?;
            env.request(
                DESCRIBE_V0,
                142,
                &describe_request(&[&a, &b, &c, "zzz-missing"], 100),
            )
        })
        .with_fixtures(three_topics)
        .request(
            "DescribeTopicPartitions v0, correlation id 142, the three fixture topics plus \
             'zzz-missing', which no one created",
        )
        .response(
            "Four entries in name order: the three fixtures with error_code 0, their real ids \
             and their partitions, then 'zzz-missing' with error_code 3 \
             (UNKNOWN_TOPIC_OR_PARTITION), an all-zero topic_id and an empty partitions array",
        )
        .note(
            "One unknown name spoils nothing else. The request-level fields — throttle_time_ms \
             and next_cursor — look exactly as they would if every name had been found, and \
             the topics array still has one entry per requested name.",
        ),
    ]
}
