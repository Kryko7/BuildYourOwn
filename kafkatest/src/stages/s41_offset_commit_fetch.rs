//! Stage 41 — OffsetCommit (8) v8 and OffsetFetch (9) v8.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::fixtures::{rec, FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::stages::group_protocol::{
    await_coordinator, fetched_offset, offset_commit, offset_commit_request, offset_fetch,
    offset_fetch_group_error, offset_fetch_request, stable_member, FetchedOffset,
    GROUP_ID_NOT_FOUND, ILLEGAL_GENERATION, OFFSET_COMMIT_V8, OFFSET_FETCH_V8,
};
use crate::stages::{Stage, Test, NONE, UNKNOWN_TOPIC_OR_PARTITION};
use kafka_protocol::messages::OffsetCommitResponse;

/// The offset every "simple commit" test writes.
const COMMITTED: i64 = 5;
/// The metadata string committed with it.
const METADATA: &str = "kafkatest-position";

fn one_topic() -> FixtureSpec {
    FixtureSpec::with(
        TopicSpec::new("t1", 2)
            .with_batch(0, vec![rec("a"), rec("b"), rec("c"), rec("d"), rec("e")]),
    )
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 41,
        slug: "offset_commit_fetch",
        name: "OffsetCommit (8) and OffsetFetch (9)",
        ext: true,
        hints: &[
            "A commit with generation_id -1 and an empty member_id is a standalone commit: \
             no group membership is needed, and the answer is error 0 per partition",
            "Committed offsets live in __consumer_offsets keyed by (group, topic, \
             partition); the metadata string is stored with the offset and comes back \
             unchanged",
            "OffsetFetch for a group or a partition that never committed is offset -1, \
             metadata \"\" and error 0 — absence is not an error",
            "A commit from a member with an out-of-date generation is 22 \
             ILLEGAL_GENERATION; a commit for a topic that does not exist is 3",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("a standalone commit is accepted", commit_accepted)
                .with_fixtures(one_topic)
                .ext(),
            Test::new("the committed offset and metadata come back", fetch_back)
                .with_fixtures(one_topic)
                .ext(),
            Test::new("a partition that never committed is -1", never_committed)
                .with_fixtures(one_topic)
                .ext(),
            Test::new(
                "an unknown group fetches -1 without an error",
                unknown_group,
            )
            .with_fixtures(one_topic)
            .ext(),
            Test::new("committing to an unknown topic is error 3", unknown_topic)
                .with_fixtures(one_topic)
                .ext(),
            Test::new("a stale generation is error 22", stale_generation)
                .with_fixtures(one_topic)
                .ext(),
            Test::new("the last commit wins", last_commit_wins)
                .with_fixtures(one_topic)
                .ext(),
            Test::new(
                "OffsetFetch with no topic list returns everything",
                fetch_all,
            )
            .with_fixtures(one_topic)
            .ext(),
        ],
    }
}

fn commit_error(resp: &OffsetCommitResponse, topic: &str, partition: i32) -> i16 {
    resp.topics
        .iter()
        .find(|t| t.name.as_str() == topic)
        .and_then(|t| {
            t.partitions
                .iter()
                .find(|p| p.partition_index == partition)
                .map(|p| p.error_code)
        })
        .unwrap_or(-1)
}

fn missing() -> FetchedOffset {
    FetchedOffset {
        offset: i64::MIN,
        metadata: "<no entry in the response>".to_string(),
        error_code: -1,
    }
}

kafka_test!(commit_accepted, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let group = ctx.unique("g41-commit");
    await_coordinator(ctx, &group).await?;
    let mut conn = ctx.connect().await?;
    let resp = offset_commit(
        &mut conn,
        &group,
        -1,
        "",
        &[(t.name.as_str(), 0, COMMITTED, Some(METADATA))],
    )
    .await?;
    let mut c = Check::new(
        format!("a standalone OffsetCommit of offset {COMMITTED} for '{group}'"),
        &conn,
    );
    c.eq("response.topics.len", 1usize, resp.topics.len());
    c.eq(
        "response.topics[0].name",
        Some(t.name.clone()),
        resp.topics.first().map(|x| x.name.to_string()),
    );
    c.eq(
        "response.topics[0].partitions[0].error_code",
        NONE,
        commit_error(&resp, &t.name, 0),
    );
    c.finish()
});

kafka_test!(fetch_back, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let group = ctx.unique("g41-fetch-back");
    await_coordinator(ctx, &group).await?;
    let mut conn = ctx.connect().await?;
    offset_commit(
        &mut conn,
        &group,
        -1,
        "",
        &[(t.name.as_str(), 0, COMMITTED, Some(METADATA))],
    )
    .await?;
    let resp = offset_fetch(&mut conn, &group, Some(&[(t.name.as_str(), vec![0])])).await?;
    let got = fetched_offset(&resp, &t.name, 0).unwrap_or_else(missing);
    let mut c = Check::new(format!("OffsetFetch for '{group}' after committing"), &conn);
    c.eq(
        "response.groups[0].error_code",
        NONE,
        offset_fetch_group_error(&resp),
    );
    c.eq(
        "response.groups[0].topics[0].partitions[0].committed_offset",
        COMMITTED,
        got.offset,
    );
    c.eq(
        "response.groups[0].topics[0].partitions[0].metadata",
        METADATA.to_string(),
        got.metadata.clone(),
    );
    c.eq(
        "response.groups[0].topics[0].partitions[0].error_code",
        NONE,
        got.error_code,
    );
    c.finish()
});

kafka_test!(never_committed, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let group = ctx.unique("g41-partial");
    await_coordinator(ctx, &group).await?;
    let mut conn = ctx.connect().await?;
    offset_commit(
        &mut conn,
        &group,
        -1,
        "",
        &[(t.name.as_str(), 0, COMMITTED, None)],
    )
    .await?;
    // Partition 0 was committed, partition 1 never was.
    let resp = offset_fetch(&mut conn, &group, Some(&[(t.name.as_str(), vec![0, 1])])).await?;
    let zero = fetched_offset(&resp, &t.name, 0).unwrap_or_else(missing);
    let one = fetched_offset(&resp, &t.name, 1).unwrap_or_else(missing);
    let mut c = Check::new(
        format!(
            "OffsetFetch for a partition of '{}' that never committed",
            t.name
        ),
        &conn,
    );
    c.eq("partition 0 committed_offset", COMMITTED, zero.offset);
    c.eq("partition 1 committed_offset", -1i64, one.offset);
    c.eq("partition 1 metadata", String::new(), one.metadata.clone());
    c.eq("partition 1 error_code", NONE, one.error_code);
    c.finish()
});

kafka_test!(unknown_group, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let warm = ctx.unique("g41-warmup");
    await_coordinator(ctx, &warm).await?;
    let group = ctx.unique("g41-never-existed");
    let mut conn = ctx.connect().await?;
    let resp = offset_fetch(&mut conn, &group, Some(&[(t.name.as_str(), vec![0])])).await?;
    let got = fetched_offset(&resp, &t.name, 0).unwrap_or_else(missing);
    let group_error = offset_fetch_group_error(&resp);
    let mut c = Check::new(
        format!("OffsetFetch for the group '{group}', which never committed anything"),
        &conn,
    );
    c.eq(
        "response.groups[0].topics[0].partitions[0].committed_offset",
        -1i64,
        got.offset,
    );
    // Apache Kafka 4.1.2 answers 0 with a -1 offset rather than inventing a "no such
    // group" error; a broker that prefers 69 GROUP_ID_NOT_FOUND at the group level is
    // accepted too, as long as the offset it reports is still -1.
    c.that(
        "response.groups[0].error_code",
        "0, or 69 GROUP_ID_NOT_FOUND",
        group_error == NONE || group_error == GROUP_ID_NOT_FOUND,
        group_error,
    );
    c.finish()
});

kafka_test!(unknown_topic, |ctx| {
    let group = ctx.unique("g41-unknown-topic");
    await_coordinator(ctx, &group).await?;
    let missing_topic = ctx.unique("no-such-topic-41");
    let mut conn = ctx.connect().await?;
    let resp = offset_commit(
        &mut conn,
        &group,
        -1,
        "",
        &[(missing_topic.as_str(), 0, COMMITTED, None)],
    )
    .await?;
    let mut c = Check::new(
        format!("OffsetCommit for the unknown topic '{missing_topic}'"),
        &conn,
    );
    c.eq(
        "response.topics[0].partitions[0].error_code",
        UNKNOWN_TOPIC_OR_PARTITION,
        commit_error(&resp, &missing_topic, 0),
    );
    c.finish()
});

kafka_test!(stale_generation, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let group = ctx.unique("g41-stale");
    let mut member = stable_member(ctx, &group, &t.name).await?;
    let stale = member.generation_id - 1;
    let resp = offset_commit(
        &mut member.conn,
        &group,
        stale,
        &member.member_id,
        &[(t.name.as_str(), 0, COMMITTED, None)],
    )
    .await?;
    let mut c = Check::new(
        format!(
            "an OffsetCommit from generation {stale} while '{group}' is at {}",
            member.generation_id
        ),
        &member.conn,
    );
    c.eq(
        "response.topics[0].partitions[0].error_code",
        ILLEGAL_GENERATION,
        commit_error(&resp, &t.name, 0),
    );
    c.finish()
});

kafka_test!(last_commit_wins, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let group = ctx.unique("g41-overwrite");
    await_coordinator(ctx, &group).await?;
    let mut conn = ctx.connect().await?;
    offset_commit(
        &mut conn,
        &group,
        -1,
        "",
        &[(t.name.as_str(), 0, 2, Some("first"))],
    )
    .await?;
    offset_commit(
        &mut conn,
        &group,
        -1,
        "",
        &[(t.name.as_str(), 0, 4, Some("second"))],
    )
    .await?;
    let resp = offset_fetch(&mut conn, &group, Some(&[(t.name.as_str(), vec![0])])).await?;
    let got = fetched_offset(&resp, &t.name, 0).unwrap_or_else(missing);
    let mut c = Check::new(format!("the offset of '{group}' after two commits"), &conn);
    c.eq("committed_offset", 4i64, got.offset);
    c.eq("metadata", "second".to_string(), got.metadata.clone());
    c.finish()
});

kafka_test!(fetch_all, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let group = ctx.unique("g41-fetch-all");
    await_coordinator(ctx, &group).await?;
    let mut conn = ctx.connect().await?;
    offset_commit(
        &mut conn,
        &group,
        -1,
        "",
        &[
            (t.name.as_str(), 0, 1, Some("p0")),
            (t.name.as_str(), 1, 3, Some("p1")),
        ],
    )
    .await?;
    // topics = null asks for every partition the group has ever committed.
    let resp = offset_fetch(&mut conn, &group, None).await?;
    let zero = fetched_offset(&resp, &t.name, 0).unwrap_or_else(missing);
    let one = fetched_offset(&resp, &t.name, 1).unwrap_or_else(missing);
    let mut c = Check::new(format!("OffsetFetch(topics=null) for '{group}'"), &conn);
    c.eq(
        "response.groups[0].topics.len",
        1usize,
        resp.groups.first().map(|g| g.topics.len()).unwrap_or(0),
    );
    c.eq("partition 0 committed_offset", 1i64, zero.offset);
    c.eq("partition 1 committed_offset", 3i64, one.offset);
    c.eq(
        "partition 1 metadata",
        "p1".to_string(),
        one.metadata.clone(),
    );
    c.finish()
});

/// Worked examples: what the broker sees, and what a correct broker answers.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("Fetching an offset that was never committed", |env| {
            let name = env.name("t1")?;
            env.request(
                OFFSET_FETCH_V8,
                411,
                &offset_fetch_request(&env.group, Some(&[(name.as_str(), vec![0])])),
            )
        })
        .with_fixtures(one_topic)
        .request(
            "OffsetFetch v8 asking one group, 'kafkatest-group', for partition 0 of the \
             fixture topic; nothing has ever committed an offset for that pair",
        )
        .response(
            "groups[0].error_code 0 and one partition entry with committed_offset -1, \
             committed_leader_epoch -1, metadata \"\" and error_code 0: absence is reported as \
             -1, not as an error",
        )
        .note(
            "-1 is what tells a consumer there is no stored position, so it falls back to \
             auto.offset.reset and asks ListOffsets for the earliest or latest offset \
             instead. A broker that answers 0 here silently makes every new consumer re-read \
             the log from the start, and one that answers an error makes it refuse to start \
             at all.",
        ),
        ExampleSpec::wire(
            "A commit from a member the coordinator does not know",
            |env| {
                let name = env.name("t1")?;
                env.request(
                    OFFSET_COMMIT_V8,
                    412,
                    &offset_commit_request(
                        &env.group,
                        5,
                        "kafkatest-ghost-member",
                        &[(name.as_str(), 0, COMMITTED, Some(METADATA))],
                    ),
                )
            },
        )
        .with_fixtures(one_topic)
        .request(
            "OffsetCommit v8 for 'kafkatest-group' claiming generation 5 as the member \
             'kafkatest-ghost-member', committing offset 5 with metadata \
             'kafkatest-position' on partition 0 of the fixture topic",
        )
        .response(
            "The coordinator refuses: the partition entry carries an error code instead of 0 \
             — 25 UNKNOWN_MEMBER_ID when the group exists and holds no such member, 22 \
             ILLEGAL_GENERATION when there is no such group at all — and nothing is written \
             to __consumer_offsets either way",
        )
        .note(
            "Only one shape of commit needs no membership: generation_id -1 with an empty \
             member_id, the standalone commit an admin tool or an assign()-based consumer \
             sends. As soon as a generation or a member id is supplied it has to match the \
             group, or a consumer that was fenced out during a rebalance would keep writing \
             positions over the member that replaced it.",
        ),
    ]
}
