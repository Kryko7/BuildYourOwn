//! Stage 40 — JoinGroup (11) / SyncGroup (14) / Heartbeat (12) / LeaveGroup (13).

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::fixtures::{FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::stages::group_protocol::{
    assignment_v0, await_coordinator, decode_assignment_v0, heartbeat, join, join_new,
    join_request, leave, rebalance_with_second_member, stable_member, subscription_v0, sync,
    sync_request, JOIN_GROUP_V9, MEMBER_ID_REQUIRED, PROTOCOL_NAME, PROTOCOL_TYPE,
    REBALANCE_IN_PROGRESS, SYNC_GROUP_V5,
};
use crate::stages::{Stage, Test, NONE};

fn one_topic() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 2))
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 40,
        slug: "consumer_group",
        name: "JoinGroup, SyncGroup, Heartbeat, LeaveGroup",
        ext: true,
        hints: &[
            "A join with an empty member_id gets 79 MEMBER_ID_REQUIRED plus a freshly minted \
             member id (KIP-394); the client joins again with that id",
            "The first member to join is the leader: its response carries members[] with \
             every subscription, and only the leader sends assignments in SyncGroup",
            "SyncGroup hands each member back the bytes the leader addressed to it, \
             unchanged; the group is Stable only after the leader has synced",
            "A second member makes the group rebalance: the existing member's Heartbeat \
             turns into 27 REBALANCE_IN_PROGRESS and it must re-join to reach generation 2",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new(
                "a join with an empty member id gets a member id",
                first_join,
            )
            .with_fixtures(one_topic)
            .ext(),
            Test::new(
                "the first member becomes the leader of generation 1",
                becomes_leader,
            )
            .with_fixtures(one_topic)
            .ext(),
            Test::new(
                "the leader's join lists the member and its subscription",
                lists_members,
            )
            .with_fixtures(one_topic)
            .ext(),
            Test::new("SyncGroup echoes the leader's assignment back", sync_echoes)
                .with_fixtures(one_topic)
                .ext(),
            Test::new("a heartbeat in a stable group is accepted", heartbeat_ok)
                .with_fixtures(one_topic)
                .ext(),
            Test::new("LeaveGroup answers 0 for the member it removed", leave_ok)
                .with_fixtures(one_topic)
                .ext(),
            Test::new(
                "a second member forces a rebalance",
                second_member_rebalances,
            )
            .with_fixtures(one_topic)
            .ext(),
            Test::new("generation 2 assigns both members", generation_two)
                .with_fixtures(one_topic)
                .ext(),
        ],
    }
}

kafka_test!(first_join, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let group = ctx.unique("g40-first-join");
    await_coordinator(ctx, &group).await?;
    let mut conn = ctx.connect().await?;
    let resp = join(&mut conn, &group, "", &[t.name.as_str()]).await?;
    let mut c = Check::new(
        format!("the first JoinGroup for '{group}', with an empty member id"),
        &conn,
    );
    // Apache Kafka 4.1.2 implements KIP-394 and answers 79 MEMBER_ID_REQUIRED with a member
    // id to join again with; a from-scratch broker may skip KIP-394 and settle the
    // first join with error 0. Both are accepted, and both have to hand back a member id.
    c.that(
        "response.error_code",
        "79 MEMBER_ID_REQUIRED, or 0 if the broker settles the first join",
        resp.error_code == MEMBER_ID_REQUIRED || resp.error_code == NONE,
        resp.error_code,
    );
    c.that(
        "response.member_id",
        "a non-empty member id the client can join again with",
        !resp.member_id.is_empty(),
        resp.member_id.to_string(),
    );
    c.finish()
});

kafka_test!(becomes_leader, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let group = ctx.unique("g40-leader");
    await_coordinator(ctx, &group).await?;
    let mut conn = ctx.connect().await?;
    let outcome = join_new(&mut conn, &group, &[t.name.as_str()]).await?;
    let settled = &outcome.settled;
    let mut c = Check::new(format!("the settled JoinGroup for '{group}'"), &conn);
    c.eq("response.error_code", NONE, settled.error_code);
    c.eq("response.generation_id", 1i32, settled.generation_id);
    c.eq(
        "response.leader",
        outcome.member_id.clone(),
        settled.leader.to_string(),
    );
    c.eq(
        "response.member_id",
        outcome.member_id.clone(),
        settled.member_id.to_string(),
    );
    c.eq(
        "response.protocol_name",
        Some(PROTOCOL_NAME.to_string()),
        settled.protocol_name.as_ref().map(|p| p.to_string()),
    );
    c.eq(
        "response.protocol_type",
        Some(PROTOCOL_TYPE.to_string()),
        settled.protocol_type.as_ref().map(|p| p.to_string()),
    );
    c.finish()
});

kafka_test!(lists_members, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let group = ctx.unique("g40-members");
    await_coordinator(ctx, &group).await?;
    let mut conn = ctx.connect().await?;
    let outcome = join_new(&mut conn, &group, &[t.name.as_str()]).await?;
    let settled = &outcome.settled;
    let mut c = Check::new(
        format!("the members[] of the leader's join in '{group}'"),
        &conn,
    );
    c.eq("response.members.len", 1usize, settled.members.len());
    c.eq(
        "response.members[0].member_id",
        Some(outcome.member_id.clone()),
        settled.members.first().map(|m| m.member_id.to_string()),
    );
    // The leader gets each member's subscription back byte for byte, which is how it can
    // compute an assignment without the broker understanding the assignor.
    let sent = subscription_v0(&[t.name.as_str()]);
    c.bytes_eq(
        "response.members[0].metadata",
        &sent,
        settled
            .members
            .first()
            .map(|m| m.metadata.as_ref())
            .unwrap_or_default(),
    );
    c.finish()
});

kafka_test!(sync_echoes, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let group = ctx.unique("g40-sync");
    await_coordinator(ctx, &group).await?;
    let mut conn = ctx.connect().await?;
    let outcome = join_new(&mut conn, &group, &[t.name.as_str()]).await?;
    let assignment = assignment_v0(&[(t.name.as_str(), &[0, 1])]);
    let resp = sync(
        &mut conn,
        &group,
        outcome.settled.generation_id,
        &outcome.member_id,
        &[(outcome.member_id.as_str(), assignment.clone())],
    )
    .await?;
    let mut c = Check::new(format!("the leader's SyncGroup in '{group}'"), &conn);
    c.eq("response.error_code", NONE, resp.error_code);
    c.bytes_eq("response.assignment", &assignment, &resp.assignment);
    let decoded = decode_assignment_v0(&resp.assignment);
    c.eq(
        "response.assignment (decoded)",
        Some(vec![(t.name.clone(), vec![0i32, 1])]),
        decoded.map(|a| a.assigned),
    );
    c.eq(
        "response.protocol_name",
        Some(PROTOCOL_NAME.to_string()),
        resp.protocol_name.as_ref().map(|p| p.to_string()),
    );
    c.finish()
});

kafka_test!(heartbeat_ok, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let group = ctx.unique("g40-heartbeat");
    let mut member = stable_member(ctx, &group, &t.name).await?;
    let resp = heartbeat(
        &mut member.conn,
        &group,
        member.generation_id,
        &member.member_id,
    )
    .await?;
    let mut c = Check::new(
        format!("a heartbeat from the only member of '{group}'"),
        &member.conn,
    );
    c.eq("response.error_code", NONE, resp.error_code);
    c.eq("response.throttle_time_ms", 0i32, resp.throttle_time_ms);
    c.finish()
});

kafka_test!(leave_ok, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let group = ctx.unique("g40-leave");
    let mut member = stable_member(ctx, &group, &t.name).await?;
    let resp = leave(&mut member.conn, &group, &member.member_id).await?;
    let mut c = Check::new(
        format!("LeaveGroup for the only member of '{group}'"),
        &member.conn,
    );
    c.eq("response.error_code", NONE, resp.error_code);
    c.eq("response.members.len", 1usize, resp.members.len());
    c.eq(
        "response.members[0].member_id",
        Some(member.member_id.clone()),
        resp.members.first().map(|m| m.member_id.to_string()),
    );
    c.eq(
        "response.members[0].error_code",
        NONE,
        resp.members.first().map(|m| m.error_code).unwrap_or(-1),
    );
    c.finish()
});

kafka_test!(second_member_rebalances, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let group = ctx.unique("g40-rebalance");
    let r = rebalance_with_second_member(ctx, &group, &t.name).await?;
    let mut c = Check::new(
        format!("the first member's heartbeat while '{group}' rebalances"),
        &r.first.conn,
    );
    c.eq(
        "heartbeat.response.error_code",
        REBALANCE_IN_PROGRESS,
        r.heartbeat_error,
    );
    c.eq("rejoin.response.error_code", NONE, r.rejoin.error_code);
    c.observe("second_member.answered_79_first", r.member_id_required);
    c.finish()
});

kafka_test!(generation_two, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let group = ctx.unique("g40-generation-two");
    let mut r = rebalance_with_second_member(ctx, &group, &t.name).await?;
    let mut c = Check::new(format!("the second generation of '{group}'"), &r.first.conn);
    c.eq(
        "rejoin.response.generation_id",
        2i32,
        r.rejoin.generation_id,
    );
    c.eq(
        "second_join.response.generation_id",
        2i32,
        r.second_join.generation_id,
    );
    c.eq(
        "second_join.response.error_code",
        NONE,
        r.second_join.error_code,
    );
    // The leader's re-join is the one that carries the whole membership.
    c.eq(
        "rejoin.response.members.len",
        2usize,
        r.rejoin.members.len(),
    );
    let mut ids: Vec<String> = r
        .rejoin
        .members
        .iter()
        .map(|m| m.member_id.to_string())
        .collect();
    ids.sort();
    let mut want = vec![r.first.member_id.clone(), r.second_member_id.clone()];
    want.sort();
    c.eq("rejoin.response.members[*].member_id", want, ids);
    c.eq(
        "rejoin.response.leader",
        r.first.member_id.clone(),
        r.rejoin.leader.to_string(),
    );
    c.finish()?;

    // The leader splits the topic between the two members, and SyncGroup has to route each
    // member's share to it — the follower never sees the other member's bytes.
    let for_first = assignment_v0(&[(t.name.as_str(), &[0])]);
    let for_second = assignment_v0(&[(t.name.as_str(), &[1])]);
    let leader_sync = sync(
        &mut r.first.conn,
        &group,
        r.first.generation_id,
        &r.first.member_id,
        &[
            (r.first.member_id.as_str(), for_first.clone()),
            (r.second_member_id.as_str(), for_second.clone()),
        ],
    )
    .await?;
    let mut c = Check::new("the leader's SyncGroup for generation 2", &r.first.conn);
    c.eq("response.error_code", NONE, leader_sync.error_code);
    c.bytes_eq("response.assignment", &for_first, &leader_sync.assignment);
    c.finish()?;

    let follower_sync = sync(
        &mut r.second,
        &group,
        r.second_join.generation_id,
        &r.second_member_id,
        &[],
    )
    .await?;
    let mut c = Check::new("the follower's SyncGroup for generation 2", &r.second);
    c.eq("response.error_code", NONE, follower_sync.error_code);
    c.bytes_eq(
        "response.assignment",
        &for_second,
        &follower_sync.assignment,
    );
    c.eq(
        "response.assignment (decoded)",
        Some(vec![(t.name.clone(), vec![1i32])]),
        decode_assignment_v0(&follower_sync.assignment).map(|a| a.assigned),
    );
    c.finish()
});

/// Worked examples: what the broker sees, and what a correct broker answers.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("The first join, with an empty member id", |env| {
            let name = env.name("t1")?;
            env.request(
                JOIN_GROUP_V9,
                401,
                &join_request(&env.group, "", &[name.as_str()]),
            )
        })
        .with_fixtures(one_topic)
        .request(
            "JoinGroup v9 for 'kafkatest-group' with an empty member_id, session_timeout_ms \
             10000, rebalance_timeout_ms 10000, protocol_type 'consumer' and one protocol \
             named 'range' whose metadata is a ConsumerProtocolSubscription v0 naming the \
             fixture topic",
        )
        .response(
            "error_code 79 (MEMBER_ID_REQUIRED), generation_id -1, an empty members array — \
             and a member_id the coordinator has just minted (Kafka builds it from the client \
             id and a fresh UUID). The client must send that id back in a second, otherwise \
             identical, JoinGroup, which is the one that really joins",
        )
        .note(
            "This is KIP-394. The first join is refused on purpose: it costs the coordinator \
             nothing to mint an id, and it means a client that disappears between its two \
             joins cannot hold a rebalance open. Do not treat 79 as a failure — it carries \
             the id you need, and only the second join gets generation 1 and the leader's \
             members list.",
        ),
        ExampleSpec::wire("SyncGroup from a member that never joined", |env| {
            env.request(
                SYNC_GROUP_V5,
                402,
                &sync_request(&env.group, 1, "kafkatest-ghost-member", &[]),
            )
        })
        .request(
            "SyncGroup v5 for 'kafkatest-group' claiming generation 1 as the member \
             'kafkatest-ghost-member', which no coordinator ever handed out, with an empty \
             assignments array (what a follower sends)",
        )
        .response(
            "error_code 25 (UNKNOWN_MEMBER_ID), throttle_time_ms 0 and an empty assignment; \
             nothing about the group changes",
        )
        .note(
            "Membership is checked before the generation, so this is 25 and not 22 \
             ILLEGAL_GENERATION whatever generation the request claims — and the same 25 \
             comes back whether the group is stable, rebalancing or has never existed. In the \
             happy path SyncGroup hands each member back exactly the bytes the leader \
             addressed to it, unchanged; the group only reaches Stable once the leader has \
             synced.",
        ),
    ]
}
