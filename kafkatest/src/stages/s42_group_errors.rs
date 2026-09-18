//! Stage 42 — the error codes a group coordinator owes a confused client.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::fixtures::{FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::stages::group_protocol::{
    assignment_v0, await_coordinator, heartbeat, heartbeat_request, join, leave, leave_request,
    rebalance_with_second_member, stable_member, sync, GROUP_ID_NOT_FOUND, HEARTBEAT_V4,
    ILLEGAL_GENERATION, LEAVE_GROUP_V5, NOT_COORDINATOR, REBALANCE_IN_PROGRESS, UNKNOWN_MEMBER_ID,
};
use crate::stages::{Stage, Test, NONE};

/// A member id no coordinator ever handed out.
const GHOST: &str = "kafkatest-ghost-member";

fn one_topic() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 1))
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 42,
        slug: "group_errors",
        name: "Group errors: UNKNOWN_MEMBER_ID, REBALANCE_IN_PROGRESS, ILLEGAL_GENERATION",
        ext: true,
        hints: &[
            "Check the member id first: one the group does not hold is 25 \
             UNKNOWN_MEMBER_ID, for Heartbeat, SyncGroup, JoinGroup and LeaveGroup alike",
            "Then check the generation: a known member sending a stale generation gets 22 \
             ILLEGAL_GENERATION, never 25",
            "While the group is rebalancing every Heartbeat is 27 REBALANCE_IN_PROGRESS — \
             that is how a consumer learns it has to re-join",
            "LeaveGroup reports per member: the top-level error stays 0 and the bad member \
             carries its own 25",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new(
                "a heartbeat from an unknown member is 25",
                heartbeat_unknown_member,
            )
            .with_fixtures(one_topic)
            .ext(),
            Test::new(
                "SyncGroup from an unknown member is 25",
                sync_unknown_member,
            )
            .with_fixtures(one_topic)
            .ext(),
            Test::new(
                "SyncGroup with the wrong generation is 22",
                sync_wrong_generation,
            )
            .with_fixtures(one_topic)
            .ext(),
            Test::new(
                "a heartbeat with the wrong generation is 22",
                heartbeat_wrong_generation,
            )
            .with_fixtures(one_topic)
            .ext(),
            Test::new(
                "a heartbeat during a rebalance is 27",
                heartbeat_rebalancing,
            )
            .with_fixtures(one_topic)
            .ext(),
            Test::new(
                "LeaveGroup of an unknown member is 25 for that member",
                leave_unknown,
            )
            .with_fixtures(one_topic)
            .ext(),
            Test::new(
                "JoinGroup with a member id the group never issued is 25",
                join_unknown,
            )
            .with_fixtures(one_topic)
            .ext(),
            Test::new(
                "a heartbeat for a group that does not exist is refused",
                heartbeat_no_group,
            )
            .with_fixtures(one_topic)
            .ext(),
        ],
    }
}

kafka_test!(heartbeat_unknown_member, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let group = ctx.unique("g42-hb-unknown");
    let member = stable_member(ctx, &group, &t.name).await?;
    let mut conn = ctx.connect().await?;
    let resp = heartbeat(&mut conn, &group, member.generation_id, GHOST).await?;
    let mut c = Check::new(
        format!("a heartbeat for '{GHOST}', which '{group}' has never seen"),
        &conn,
    );
    c.eq("response.error_code", UNKNOWN_MEMBER_ID, resp.error_code);
    c.finish()
});

kafka_test!(sync_unknown_member, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let group = ctx.unique("g42-sync-unknown");
    let member = stable_member(ctx, &group, &t.name).await?;
    let mut conn = ctx.connect().await?;
    let resp = sync(&mut conn, &group, member.generation_id, GHOST, &[]).await?;
    let mut c = Check::new(
        format!("a SyncGroup for '{GHOST}', which '{group}' has never seen"),
        &conn,
    );
    c.eq("response.error_code", UNKNOWN_MEMBER_ID, resp.error_code);
    c.finish()
});

kafka_test!(sync_wrong_generation, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let group = ctx.unique("g42-sync-generation");
    let mut member = stable_member(ctx, &group, &t.name).await?;
    let stale = member.generation_id - 1;
    let assignment = assignment_v0(&[(t.name.as_str(), &[0])]);
    let resp = sync(
        &mut member.conn,
        &group,
        stale,
        &member.member_id,
        &[(member.member_id.as_str(), assignment)],
    )
    .await?;
    let mut c = Check::new(
        format!(
            "a SyncGroup claiming generation {stale} while '{group}' is at {}",
            member.generation_id
        ),
        &member.conn,
    );
    c.eq("response.error_code", ILLEGAL_GENERATION, resp.error_code);
    c.finish()
});

kafka_test!(heartbeat_wrong_generation, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let group = ctx.unique("g42-hb-generation");
    let mut member = stable_member(ctx, &group, &t.name).await?;
    let stale = member.generation_id - 1;
    let resp = heartbeat(&mut member.conn, &group, stale, &member.member_id).await?;
    let mut c = Check::new(
        format!(
            "a heartbeat claiming generation {stale} while '{group}' is at {}",
            member.generation_id
        ),
        &member.conn,
    );
    c.eq("response.error_code", ILLEGAL_GENERATION, resp.error_code);
    c.finish()
});

kafka_test!(heartbeat_rebalancing, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let group = ctx.unique("g42-rebalancing");
    let r = rebalance_with_second_member(ctx, &group, &t.name).await?;
    let mut c = Check::new(
        format!("the first member's heartbeat while '{group}' is rebalancing"),
        &r.first.conn,
    );
    c.eq(
        "response.error_code",
        REBALANCE_IN_PROGRESS,
        r.heartbeat_error,
    );
    c.finish()
});

kafka_test!(leave_unknown, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let group = ctx.unique("g42-leave-unknown");
    stable_member(ctx, &group, &t.name).await?;
    let mut conn = ctx.connect().await?;
    let resp = leave(&mut conn, &group, GHOST).await?;
    let mut c = Check::new(
        format!("LeaveGroup for '{GHOST}', which '{group}' has never seen"),
        &conn,
    );
    c.eq("response.error_code", NONE, resp.error_code);
    c.eq("response.members.len", 1usize, resp.members.len());
    c.eq(
        "response.members[0].error_code",
        UNKNOWN_MEMBER_ID,
        resp.members.first().map(|m| m.error_code).unwrap_or(-1),
    );
    c.finish()
});

kafka_test!(join_unknown, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let group = ctx.unique("g42-join-unknown");
    await_coordinator(ctx, &group).await?;
    let mut conn = ctx.connect().await?;
    let resp = join(&mut conn, &group, GHOST, &[t.name.as_str()]).await?;
    let mut c = Check::new(
        format!("a JoinGroup for '{GHOST}' into the brand-new group '{group}'"),
        &conn,
    );
    c.eq("response.error_code", UNKNOWN_MEMBER_ID, resp.error_code);
    c.finish()
});

kafka_test!(heartbeat_no_group, |ctx| {
    let warm = ctx.unique("g42-warmup");
    await_coordinator(ctx, &warm).await?;
    let group = ctx.unique("g42-no-such-group");
    let mut conn = ctx.connect().await?;
    let resp = heartbeat(&mut conn, &group, 1, GHOST).await?;
    let code = resp.error_code;
    let mut c = Check::new(
        format!("a heartbeat for the group '{group}', which does not exist"),
        &conn,
    );
    // Apache Kafka 4.1.2 treats "no such group" as "you are not a member of it" and
    // answers 25 — verified against the reference broker. Brokers that distinguish the two
    // cases answer 69 GROUP_ID_NOT_FOUND, and one that has moved the group answers 16
    // NOT_COORDINATOR; all three are refusals a client can act on, so all three pass.
    c.that(
        "response.error_code",
        "25 UNKNOWN_MEMBER_ID, 69 GROUP_ID_NOT_FOUND or 16 NOT_COORDINATOR",
        code == UNKNOWN_MEMBER_ID || code == GROUP_ID_NOT_FOUND || code == NOT_COORDINATOR,
        code,
    );
    c.finish()
});

/// Worked examples: what the broker sees, and what a correct broker answers.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("A heartbeat from a member nobody knows", |env| {
            env.request(HEARTBEAT_V4, 421, &heartbeat_request(&env.group, 1, GHOST))
        })
        .request(
            "Heartbeat v4 for 'kafkatest-group', generation_id 1, from the member \
             'kafkatest-ghost-member', which no coordinator ever issued",
        )
        .response(
            "throttle_time_ms 0 and error_code 25 (UNKNOWN_MEMBER_ID) — the same answer \
             whether the group is stable, rebalancing, or has never existed at all",
        )
        .note(
            "Check the member id before the generation and before the group's state: a \
             client that gets 25 knows it has been fenced out and must re-join from scratch \
             with an empty member id, while 22 ILLEGAL_GENERATION or 27 \
             REBALANCE_IN_PROGRESS would send it down a different recovery path. The wrong \
             code here is worse than none: it makes a consumer retry forever.",
        ),
        ExampleSpec::wire("LeaveGroup for a member that never joined", |env| {
            env.request(LEAVE_GROUP_V5, 422, &leave_request(&env.group, GHOST))
        })
        .request(
            "LeaveGroup v5 for 'kafkatest-group' with a members array of one identity: \
             member_id 'kafkatest-ghost-member', no group_instance_id, reason 'kafkatest'",
        )
        .response(
            "Top-level error_code 0, and one members entry naming the member id it was asked \
             about with error_code 25 (UNKNOWN_MEMBER_ID) in it",
        )
        .note(
            "From v3 on LeaveGroup is a batch: the top level says whether the request was \
             understood, and each member carries its own outcome. A broker that reports 25 at \
             the top level breaks a client leaving several members at once, because the one \
             member that really did leave is then indistinguishable from the one that never \
             existed.",
        ),
    ]
}
