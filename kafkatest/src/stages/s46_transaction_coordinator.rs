//! Stage 46 — The transaction coordinator: a transactional id, and the epoch that fences it.
//!
//! Stage 36 built the idempotent producer: a producer id, a sequence number per partition,
//! and a broker that silently drops a duplicate. That gets exactly-once *within one
//! producer session*. The moment the producer restarts it asks for a new producer id and
//! the old session's guarantees are gone — which is fine for a retry loop and useless for a
//! stream processor that must not process an input twice after a crash.
//!
//! A **transactional id** closes that. The application chooses it, it outlives the process,
//! and the broker keeps state against it in a transaction coordinator — a broker chosen by
//! hashing the id, found with `FindCoordinator` using key type 1 rather than the 0 that
//! finds a consumer group's coordinator.
//!
//! What the coordinator gives back is a producer id and an **epoch**, and asking twice for
//! the same transactional id bumps the epoch. That is the whole fencing mechanism: the new
//! instance takes the higher epoch, the old one — a process that is still running because
//! it was merely partitioned rather than dead — keeps the lower one, and every write it
//! attempts from then on is refused. Two instances of the same job cannot both be writing,
//! and neither of them needs to find out which one is the zombie.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::fixtures::{FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::stages::group_protocol::{
    find_coordinator_request, FIND_COORDINATOR_KEY, FIND_COORDINATOR_V4,
};
use crate::stages::transactions::{
    find_coordinator, init_resuming, init_transactional, txn_id, INIT_PRODUCER_ID_V4,
    TRANSACTION_TIMEOUT_MS,
};
use crate::stages::{
    api_versions, error_label, expect_still_serving, require_api, Stage, Test,
    INIT_PRODUCER_ID_KEY, INVALID_PRODUCER_EPOCH, NONE, PRODUCER_FENCED,
};
use kafka_protocol::messages::InitProducerIdRequest;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 46,
        slug: "transaction_coordinator",
        name: "The transaction coordinator and producer fencing",
        ext: true,
        hints: &[
            "`FindCoordinator` with `key_type` 1 asks for a *transaction* coordinator; the same \
             request with 0 asks for a consumer group's, and the two are different lookups",
            "`InitProducerId` with a transactional id is a different operation from the same \
             request with a null one: it is coordinator state that outlives the connection",
            "Asking twice for the same transactional id must return the same producer id with a \
             higher epoch — that bump is the entire fencing mechanism",
            "Fencing happens at the coordinator: a superseded instance resuming its session is \
             refused with INVALID_PRODUCER_EPOCH (47) or PRODUCER_FENCED (90), while the live \
             instance presenting the epoch it holds is let through",
        ],
        examples,
        tests: vec![
            Test::new(
                "ApiVersions advertises InitProducerId and FindCoordinator",
                advertises_the_apis,
            )
            .ext(),
            Test::new(
                "FindCoordinator with key_type 1 answers for a transactional id",
                finds_a_transaction_coordinator,
            )
            .ext(),
            Test::new(
                "the same transactional id maps to the same coordinator every time",
                the_mapping_is_stable,
            )
            .ext(),
            Test::new(
                "InitProducerId with a transactional id hands out a producer id",
                init_with_a_transactional_id,
            )
            .ext(),
            Test::new(
                "asking again for the same id bumps the epoch",
                asking_again_bumps_the_epoch,
            )
            .ext(),
            Test::new("and keeps the same producer id", the_producer_id_is_stable).ext(),
            Test::new(
                "the superseded instance is fenced when it resumes",
                the_old_epoch_is_fenced,
            )
            .ext(),
            Test::new("and the live one is not", the_current_epoch_is_accepted).ext(),
            Test::new(
                "a different transactional id is a different producer",
                different_ids_are_different_producers,
            )
            .ext(),
            Test::new("the broker is still serving afterwards", still_serving)
                .ext()
                .with_fixtures(fixtures),
        ],
    }
}

fn fixtures() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("txn", 1))
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("FindCoordinator for a transactional id", |env| {
            let req = find_coordinator_request(&["kafkatest-example-txn"], 1);
            env.request(FIND_COORDINATOR_V4, 461, &req)
        })
        .request(
            "FindCoordinator (api_key 10) v4, correlation id 461: key_type 1 and one \
             coordinator_key, the transactional id — the same request a consumer group sends \
             with key_type 0",
        )
        .response(
            "One coordinators entry: the key echoed, a node_id, host and port, and \
             error_code 0 — or 15 (COORDINATOR_NOT_AVAILABLE) while the broker is still \
             creating __transaction_state",
        )
        .note(
            "The coordinator is chosen by hashing the transactional id onto a partition of \
             the internal __transaction_state topic, so every producer using this id reaches \
             the same broker with no coordination between them. A broker that has never run \
             a transaction has no such topic and creates it here, which is why the first \
             lookup of a fresh cluster is the one that needs retrying.",
        ),
        ExampleSpec::wire("InitProducerId with a transactional id", |env| {
            let mut req = InitProducerIdRequest::default();
            req.transactional_id = Some(txn_id("kafkatest-example-txn"));
            req.transaction_timeout_ms = TRANSACTION_TIMEOUT_MS;
            req.producer_id = kafka_protocol::messages::ProducerId(-1);
            req.producer_epoch = -1;
            env.request(INIT_PRODUCER_ID_V4, 462, &req)
        })
        .request(
            "InitProducerId (api_key 22) v4, correlation id 462: the same request stage 36 \
             sends, with a transactional_id in place of the null and a real \
             transaction_timeout_ms",
        )
        .response(
            "error_code 0, a producer_id the coordinator keeps under that transactional id, \
             and a producer_epoch that is one higher than whatever the previous session held",
        )
        .note(
            "Those two fields are the difference between idempotence and exactly-once. The \
             producer id survives a restart because the coordinator remembers it; the epoch \
             goes up because the coordinator has just decided this caller is the current \
             owner of the id, and everything holding a lower epoch is now fenced.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

kafka_test!(advertises_the_apis, |ctx| {
    let mut conn = ctx.connect().await?;
    let versions = api_versions(&mut conn).await?;
    let (_, init) = require_api(&versions, INIT_PRODUCER_ID_KEY, "InitProducerId", &conn)?;
    let (_, find) = require_api(&versions, FIND_COORDINATOR_KEY, "FindCoordinator", &conn)?;
    let mut c = Check::new("the two APIs a transactional producer starts with", &conn);
    c.note(
        "Both already exist for other reasons — FindCoordinator for consumer groups, \
         InitProducerId for the idempotent producer — and transactions reuse them with \
         different arguments rather than adding new ones.",
    );
    c.at_least("ApiVersions.InitProducerId.max_version", 0i16, init);
    c.at_least("ApiVersions.FindCoordinator.max_version", 0i16, find);
    c.finish()
});

kafka_test!(finds_a_transaction_coordinator, |ctx| {
    let found = find_coordinator(ctx, "kafkatest-txn-id").await?;
    let conn = ctx.connect().await?;
    let mut c = Check::new("FindCoordinator with key_type 1", &conn);
    c.note(
        "key_type 1 means the key is a transactional id. The coordinator is a broker chosen \
         by hashing the id onto a partition of the internal __transaction_state topic, which \
         is why the answer is stable and why it can move when that topic's leadership does.",
    );
    c.note(
        "A broker that has never run a transaction answers COORDINATOR_NOT_AVAILABLE (15) \
         while it creates __transaction_state; that is retriable and was retried here.",
    );
    c.that(
        "response.coordinators[0].error_code",
        &error_label(NONE),
        found.error_code == NONE,
        error_label(found.error_code),
    );
    c.at_least("response.coordinators[0].node_id", 0i32, found.node_id);
    c.that(
        "response.coordinators[0].host",
        "a non-empty host",
        !found.host.is_empty(),
        found.host.clone(),
    );
    c.finish()
});

kafka_test!(the_mapping_is_stable, |ctx| {
    let mut nodes = Vec::new();
    for _ in 0..3 {
        nodes.push(find_coordinator(ctx, "kafkatest-stable-id").await?.node_id);
    }
    let conn = ctx.connect().await?;
    let mut c = Check::new("the same id, asked three times", &conn);
    c.note(
        "The mapping is a hash, not a load-balancing decision, so every producer with this \
         transactional id reaches the same coordinator without any coordination between them.",
    );
    c.at_least("the coordinator node id", 0i32, nodes[0]);
    c.eq("the three answers", vec![nodes[0]; 3], nodes.clone());
    c.finish()
});

kafka_test!(init_with_a_transactional_id, |ctx| {
    let producer = init_transactional(ctx, "kafkatest-init-id").await?;
    let conn = ctx.connect().await?;
    let mut c = Check::new("what the coordinator handed back", &conn);
    c.note(
        "The producer id is the same kind of number the idempotent producer gets; what has \
         changed is that the coordinator remembers which transactional id it belongs to, so \
         the next process to use that id inherits it.",
    );
    c.at_least("producer_id", 0i64, producer.id);
    c.at_least("producer_epoch", 0i16, producer.epoch);
    c.finish()
});

kafka_test!(asking_again_bumps_the_epoch, |ctx| {
    let id = "kafkatest-fence-id";
    let first = init_transactional(ctx, id).await?.epoch;
    let second = init_transactional(ctx, id).await?.epoch;
    let conn = ctx.connect().await?;
    let mut c = Check::new("two InitProducerId calls for one transactional id", &conn);
    c.note(
        "This is the fencing mechanism in one number. The second caller is a new instance of \
         the same job — after a restart, or after a scheduler decided the first one was gone \
         — and it takes an epoch the first can no longer match.",
    );
    c.that(
        "the second epoch",
        &format!("greater than the first ({first})"),
        second > first,
        second,
    );
    c.finish()
});

kafka_test!(the_producer_id_is_stable, |ctx| {
    let id = "kafkatest-stable-pid";
    let first_pid = init_transactional(ctx, id).await?.id;
    let second_pid = init_transactional(ctx, id).await?.id;
    let conn = ctx.connect().await?;
    let mut c = Check::new("the producer id across two sessions", &conn);
    c.note(
        "The identity belongs to the transactional id and persists; only the epoch moves. \
         That is what lets the coordinator find the previous session's open transaction and \
         abort it before the new session starts writing.",
    );
    c.eq("producer_id", first_pid, second_pid);
    c.finish()
});

kafka_test!(the_old_epoch_is_fenced, |ctx| {
    let id = "kafkatest-zombie-id";
    let zombie = init_transactional(ctx, id).await?;
    // A second instance starts under the same id and takes a higher epoch. The first is now
    // a zombie, and resuming its session is where it finds out.
    let live = init_transactional(ctx, id).await?;
    let code = init_resuming(ctx, id, zombie).await?;
    let conn = ctx.connect().await?;
    let mut c = Check::new("the superseded instance, presenting its epoch", &conn);
    c.note(
        "The zombie is not misbehaving and does not know anything is wrong: it holds a \
         producer id and an epoch the coordinator gave it. Only the coordinator can tell, \
         because only the coordinator saw the second InitProducerId — and it refuses.",
    );
    c.that(
        &format!(
            "resuming with epoch {} (current is {})",
            zombie.epoch, live.epoch
        ),
        "a fencing error: INVALID_PRODUCER_EPOCH (47) or PRODUCER_FENCED (90)",
        code == INVALID_PRODUCER_EPOCH || code == PRODUCER_FENCED,
        error_label(code),
    );
    c.finish()
});

kafka_test!(the_current_epoch_is_accepted, |ctx| {
    let id = "kafkatest-resume-id";
    let producer = init_transactional(ctx, id).await?;
    let code = init_resuming(ctx, id, producer).await?;
    let conn = ctx.connect().await?;
    let mut c = Check::new("the live instance, presenting its epoch", &conn);
    c.note(
        "The other half of the same test, and what stops \"refuse everything\" from passing: \
         a producer resuming with the epoch it actually holds has to be allowed to carry on, \
         or nothing could recover from a dropped connection.",
    );
    c.that(
        &format!("resuming with the current epoch {}", producer.epoch),
        "accepted",
        code == NONE,
        error_label(code),
    );
    c.finish()
});

kafka_test!(different_ids_are_different_producers, |ctx| {
    let pid_a = init_transactional(ctx, "kafkatest-id-a").await?.id;
    let pid_b = init_transactional(ctx, "kafkatest-id-b").await?.id;
    let conn = ctx.connect().await?;
    let mut c = Check::new("two transactional ids", &conn);
    c.note(
        "Two instances of two different jobs, fencing each other not at all. The epoch bump \
         is scoped to one transactional id, which is why choosing that id well — stable per \
         logical task, distinct between tasks — is the thing an application has to get right.",
    );
    c.that(
        "the two producer ids",
        "different",
        pid_a != pid_b,
        format!("both {pid_a}"),
    );
    c.finish()
});

kafka_test!(still_serving, |ctx| {
    let topic = ctx.topic("txn")?.name.clone();
    expect_still_serving(ctx, &topic).await
});
