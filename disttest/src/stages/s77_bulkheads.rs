//! Stage 77 — Bulkheads and load shedding.
//!
//! A ship is divided into compartments so that a hole in one does not sink the whole hull,
//! and a service is divided the same way: a fixed pool of concurrency per dependency, so
//! that the one dependency which has gone slow can only ever consume its own share. Without
//! the partition every caller competes for one pool of threads, the slow dependency holds
//! all of them, and requests that had nothing to do with it start timing out — which is how
//! a partial outage becomes a total one.
//!
//! The oracle is the counting. The tester keeps each pool's in-flight set itself and knows
//! before every call whether it must be admitted, so it can assert both the verdict and the
//! reason; the isolation test runs the identical traffic against separate pools and against
//! one shared pool and asserts the second starves where the first does not. The seeded run
//! at the end walks two hundred calls across three pools and asserts no pool ever exceeds
//! its limit and no pool's rejection count moves because of another pool's traffic.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;
use std::collections::{BTreeMap, BTreeSet};

/// Stage 77.
pub fn stage() -> Stage {
    Stage {
        number: 77,
        slug: "bulkheads",
        name: "Bulkheads and load shedding",
        ext: true,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `bulkhead`: one concurrency limit per pool, and `now` arrives as an argument",
            "A full pool rejects at once — an unbounded queue is how one slow dependency takes everything",
            "Pools are independent: saturating one must not change what another admits",
            "Shedding work whose deadline has passed is free capacity, and it is not a rejection",
        ],
        examples,
        tests: vec![
            Test::new("a pool admits up to its limit", a_pool_admits_up_to_its_limit),
            Test::new(
                "a full pool rejects instead of queueing",
                a_full_pool_rejects,
            ),
            Test::new(
                "finishing a call frees the slot at once",
                done_frees_a_slot,
            ),
            Test::new(
                "finishing a call that is not in flight is an error",
                done_for_a_stranger,
            ),
            Test::new(
                "one saturated pool does not change what another admits",
                pools_are_isolated,
            ),
            Test::new(
                "one shared pool lets the first caller starve the second",
                a_shared_pool_starves,
            ),
            Test::new(
                "a call to a pool that does not exist is refused, not invented",
                an_unknown_pool,
            ),
            Test::new(
                "shedding drops work past its deadline and frees the capacity",
                shedding_frees_capacity,
            ),
            Test::new(
                "two hundred seeded calls never exceed a limit",
                a_long_seeded_run,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("Two pools, one of them saturated", "bulkhead", || {
            lines(&[
                "init {\"a\":2,\"b\":2}",
                "call a x1 0",
                "call a x2 0",
                "call a x3 0",
                "call b y1 0",
                "stats",
            ])
        })
        .request("a pool per dependency, and a third call to the one that is already full")
        .response("a rejects its third call while b admits its first without noticing")
        .note(
            "The rejection is the feature. Queueing x3 instead would hold a caller, a \
             connection and a timeout's worth of latency on a dependency that has already \
             shown it cannot keep up, and it is the queue — not the slow dependency — that \
             eventually takes the whole service down.",
        ),
        prim_example("The same traffic with no bulkhead", "bulkhead", || {
            lines(&[
                "init-shared 2",
                "call shared x1 0",
                "call shared x2 0",
                "call shared y1 0",
                "stats",
            ])
        })
        .request("the identical four calls against one pool shared by both dependencies")
        .response("y1 is rejected, although nothing is wrong with the dependency it was for")
        .note(
            "Nothing here is broken and nothing is misconfigured. One dependency simply got \
             there first, and the caller that is refused is the one that would have \
             succeeded. This is the failure the whole pattern exists to prevent.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// The tester's own model of the pools. Nothing below consults the program.
// ---------------------------------------------------------------------------------------

/// A set of pools, each with a concurrency limit and an in-flight set.
struct Model {
    limits: BTreeMap<String, usize>,
    inflight: BTreeMap<String, BTreeSet<String>>,
    rejected: BTreeMap<String, u64>,
}

impl Model {
    fn new(limits: &[(&str, usize)]) -> Model {
        Model {
            limits: limits.iter().map(|(n, l)| ((*n).to_string(), *l)).collect(),
            inflight: limits
                .iter()
                .map(|(n, _)| ((*n).to_string(), BTreeSet::new()))
                .collect(),
            rejected: limits.iter().map(|(n, _)| ((*n).to_string(), 0)).collect(),
        }
    }

    fn count(&self, pool: &str) -> usize {
        self.inflight.get(pool).map(BTreeSet::len).unwrap_or(0)
    }

    /// Admit the call if there is room; returns whether it was admitted.
    fn call(&mut self, pool: &str, id: &str) -> bool {
        let Some(limit) = self.limits.get(pool).copied() else {
            return false;
        };
        if self.count(pool) >= limit {
            *self.rejected.entry(pool.to_string()).or_insert(0) += 1;
            return false;
        }
        self.inflight
            .entry(pool.to_string())
            .or_default()
            .insert(id.to_string());
        true
    }

    fn done(&mut self, pool: &str, id: &str) -> bool {
        self.inflight
            .get_mut(pool)
            .map(|s| s.remove(id))
            .unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

/// Two pools of two and three, which most tests in this stage start from.
const INIT: &str = "init {\"a\":2,\"b\":3}";

dist_test!(a_pool_admits_up_to_its_limit, |ctx| {
    let p = ctx.prim("bulkhead").await?;
    p.send(INIT).await?;
    let mut seen = Vec::new();
    for i in 1..=2 {
        let r = p.send(&format!("call a x{i} 0")).await?;
        seen.push((
            p.expect_bool(&r, "call", "admitted")?,
            p.expect_i64(&r, "call", "inflight")?,
            p.expect_str(&r, "call", "reason")?,
        ));
    }
    let mut c = Check::new("two calls into a pool of two");
    c.eq(
        "the (admitted, inflight, reason) of each call",
        vec![
            (true, 1, "admitted".to_string()),
            (true, 2, "admitted".to_string()),
        ],
        seen,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_full_pool_rejects, |ctx| {
    let p = ctx.prim("bulkhead").await?;
    p.send(INIT).await?;
    p.send("call a x1 0").await?;
    p.send("call a x2 0").await?;
    let third = p.send("call a x3 0").await?;
    let fourth = p.send("call a x4 0").await?;
    let s = p.send("stats").await?;
    let mut c = Check::new("a third call into a pool of two");
    // Refusing now is cheap; queueing would hold the caller for as long as the dependency
    // stays slow, which is exactly the resource the bulkhead is protecting.
    c.eq(
        "call.admitted",
        false,
        p.expect_bool(&third, "call a x3 0", "admitted")?,
    );
    c.eq(
        "call.reason",
        "pool is full".to_string(),
        p.expect_str(&third, "call", "reason")?,
    );
    c.eq(
        "call.inflight",
        2,
        p.expect_i64(&third, "call", "inflight")?,
    );
    c.eq(
        "the second rejection's count",
        2,
        p.expect_i64(&fourth, "call", "rejected")?,
    );
    c.eq(
        "stats.pools.a.rejected",
        2,
        s["pools"]["a"]["rejected"].as_i64().unwrap_or(-1),
    );
    c.eq(
        "stats.total_rejected",
        2,
        p.expect_i64(&s, "stats", "total_rejected")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(done_frees_a_slot, |ctx| {
    let p = ctx.prim("bulkhead").await?;
    p.send(INIT).await?;
    p.send("call a x1 0").await?;
    p.send("call a x2 0").await?;
    p.send("call a x3 0").await?;
    let finished = p.send("done a x1").await?;
    let now_admitted = p.send("call a x4 0").await?;
    let s = p.send("stats").await?;
    let mut c = Check::new("a slot freed and immediately taken by the next caller");
    c.eq(
        "done.ok",
        true,
        p.expect_bool(&finished, "done a x1", "ok")?,
    );
    c.eq(
        "done.inflight",
        1,
        p.expect_i64(&finished, "done a x1", "inflight")?,
    );
    c.eq(
        "the next call.admitted",
        true,
        p.expect_bool(&now_admitted, "call a x4 0", "admitted")?,
    );
    c.eq(
        "stats.pools.a.inflight",
        2,
        s["pools"]["a"]["inflight"].as_i64().unwrap_or(-1),
    );
    c.eq(
        "stats.pools.a.admitted",
        3,
        s["pools"]["a"]["admitted"].as_i64().unwrap_or(-1),
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(done_for_a_stranger, |ctx| {
    let p = ctx.prim("bulkhead").await?;
    p.send(INIT).await?;
    p.send("call a x1 0").await?;
    let never_started = p.send("done a x9").await?;
    let twice = p.send("done a x1").await?;
    let again = p.send("done a x1").await?;
    let mut c = Check::new("finishing calls that were never in flight");
    // A silent success here would let a double completion free somebody else's slot, and
    // the pool would quietly admit more than its limit for ever after.
    c.eq(
        "done(x9).ok",
        false,
        p.expect_bool(&never_started, "done a x9", "ok")?,
    );
    c.eq(
        "done(x1).ok",
        true,
        p.expect_bool(&twice, "done a x1", "ok")?,
    );
    c.eq(
        "the second done(x1).ok",
        false,
        p.expect_bool(&again, "done a x1", "ok")?,
    );
    c.eq(
        "the second done(x1).inflight",
        0,
        p.expect_i64(&again, "done", "inflight")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(pools_are_isolated, |ctx| {
    let p = ctx.prim("bulkhead").await?;
    p.send(INIT).await?;
    // Pool a is saturated and then hammered; nothing about that may reach pool b.
    for i in 1..=5 {
        p.send(&format!("call a x{i} 0")).await?;
    }
    let mut b = Vec::new();
    for i in 1..=3 {
        let r = p.send(&format!("call b y{i} 0")).await?;
        b.push(p.expect_bool(&r, "call", "admitted")?);
    }
    let s = p.send("stats").await?;
    let mut c = Check::new("a dependency that has gone slow, next to one that has not");
    c.eq("the three calls into b", vec![true, true, true], b);
    c.eq(
        "stats.pools.a.inflight",
        2,
        s["pools"]["a"]["inflight"].as_i64().unwrap_or(-1),
    );
    c.eq(
        "stats.pools.a.rejected",
        3,
        s["pools"]["a"]["rejected"].as_i64().unwrap_or(-1),
    );
    c.eq(
        "stats.pools.b.inflight",
        3,
        s["pools"]["b"]["inflight"].as_i64().unwrap_or(-1),
    );
    // b's rejection count must not have moved: that is the whole guarantee.
    c.eq(
        "stats.pools.b.rejected",
        0,
        s["pools"]["b"]["rejected"].as_i64().unwrap_or(-1),
    );
    c.note("compare with 'one shared pool lets the first caller starve the second'");
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_shared_pool_starves, |ctx| {
    let p = ctx.prim("bulkhead").await?;
    p.send("init-shared 2").await?;
    // The same traffic as the previous test, against one undivided pool.
    for i in 1..=5 {
        p.send(&format!("call shared x{i} 0")).await?;
    }
    let mut b = Vec::new();
    for i in 1..=3 {
        let r = p.send(&format!("call shared y{i} 0")).await?;
        b.push((
            p.expect_bool(&r, "call", "admitted")?,
            p.expect_str(&r, "call", "reason")?,
        ));
    }
    let s = p.send("stats").await?;
    let mut c = Check::new("the same traffic with the compartments taken out");
    // Nothing is wrong with the dependency the y calls were for. It simply arrived second.
    c.eq(
        "the three calls that would have gone to b",
        vec![
            (false, "pool is full".to_string()),
            (false, "pool is full".to_string()),
            (false, "pool is full".to_string()),
        ],
        b,
    );
    c.eq(
        "stats.pools.shared.rejected",
        6,
        s["pools"]["shared"]["rejected"].as_i64().unwrap_or(-1),
    );
    c.note("compare with 'one saturated pool does not change what another admits'");
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(an_unknown_pool, |ctx| {
    let p = ctx.prim("bulkhead").await?;
    p.send(INIT).await?;
    let r = p.send("call typo x1 0").await?;
    let s = p.send("stats").await?;
    let mut c = Check::new("a call naming a pool that was never configured");
    // Creating the pool on demand would give a misspelt dependency its own unbounded share
    // of the machine, which is the one thing the configuration was there to prevent.
    c.eq(
        "call.admitted",
        false,
        p.expect_bool(&r, "call typo x1 0", "admitted")?,
    );
    c.eq(
        "call.reason",
        "no such pool".to_string(),
        p.expect_str(&r, "call", "reason")?,
    );
    c.eq(
        "stats.pools.typo",
        &serde_json::Value::Null,
        &s["pools"]["typo"],
    );
    c.eq(
        "stats.total_rejected",
        0,
        p.expect_i64(&s, "stats", "total_rejected")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(shedding_frees_capacity, |ctx| {
    let p = ctx.prim("bulkhead").await?;
    p.send("init {\"a\":2}").await?;
    p.send("call a x1 0").await?;
    p.send("call a x2 0").await?;
    let full = p.send("call a x3 100").await?;
    // Both in-flight calls have been queued for a second against a deadline of half of one.
    // Whoever was waiting for them has given up, so finishing them would help nobody.
    let shed = p.send("shed 1000 500").await?;
    let after = p.send("call a x4 1000").await?;
    let s = p.send("stats").await?;
    let mut c = Check::new("two calls that outlived their deadline");
    c.eq(
        "the call before shedding",
        false,
        p.expect_bool(&full, "call a x3 100", "admitted")?,
    );
    c.eq(
        "shed.shed",
        vec!["x1".to_string(), "x2".to_string()],
        p.expect_strs(&shed, "shed 1000 500", "shed")?,
    );
    c.eq(
        "shed.inflight.a",
        0,
        shed["inflight"]["a"].as_i64().unwrap_or(-1),
    );
    c.eq(
        "the call after shedding",
        true,
        p.expect_bool(&after, "call a x4 1000", "admitted")?,
    );
    c.eq(
        "stats.pools.a.shed",
        2,
        s["pools"]["a"]["shed"].as_i64().unwrap_or(-1),
    );
    // Shedding is not a rejection: the work was admitted and then abandoned, and counting it
    // as a rejection would hide the fact that the pool was sized correctly all along.
    c.eq(
        "stats.pools.a.rejected",
        1,
        s["pools"]["a"]["rejected"].as_i64().unwrap_or(-1),
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_long_seeded_run, |ctx| {
    // The whole conversation is worked out here first — every call, whether it must be
    // admitted, and what each pool's in-flight count must then be — from the limits alone.
    // The program is asked the same questions afterwards and never consulted.
    let pools = [("a", 2usize), ("b", 3), ("c", 1)];
    let mut model = Model::new(&pools);
    let mut live: Vec<(String, String)> = Vec::new();
    let mut script: Vec<(String, bool, usize)> = Vec::new();
    for i in 0..200 {
        let (pool, _) = pools[ctx.rng.random_range(0..pools.len())];
        // Half the steps finish something, so the pools breathe rather than filling once.
        if !live.is_empty() && ctx.rng.random_bool(0.45) {
            let idx = ctx.rng.random_range(0..live.len());
            let (p_name, id) = live.remove(idx);
            let ok = model.done(&p_name, &id);
            script.push((format!("done {p_name} {id}"), ok, model.count(&p_name)));
        } else {
            let id = format!("r{i}");
            let admitted = model.call(pool, &id);
            if admitted {
                live.push((pool.to_string(), id.clone()));
            }
            script.push((format!("call {pool} {id} {i}"), admitted, model.count(pool)));
        }
    }
    let seed = ctx.seed;
    let p = ctx.prim("bulkhead").await?;
    p.send("init {\"a\":2,\"b\":3,\"c\":1}").await?;
    let mut actual = Vec::with_capacity(script.len());
    for (command, ..) in &script {
        let r = p.send(command).await?;
        let flag = if command.starts_with("done") {
            p.expect_bool(&r, command, "ok")?
        } else {
            p.expect_bool(&r, command, "admitted")?
        };
        actual.push((flag, p.expect_i64(&r, command, "inflight")?));
    }
    let stats = p.send("stats").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("two hundred seeded calls across three pools");
    c.note(format!("seed {seed}, {} steps", script.len()));
    for (i, ((command, want_flag, want_inflight), (got_flag, got_inflight))) in
        script.iter().zip(&actual).enumerate()
    {
        c.eq(
            &format!("step[{i}] {command} → verdict"),
            *want_flag,
            *got_flag,
        );
        c.eq(
            &format!("step[{i}] {command} → inflight"),
            *want_inflight as i64,
            *got_inflight,
        );
        c.that(
            &format!("step[{i}] {command} → the limit"),
            "an in-flight count that never exceeds the pool's limit",
            *got_inflight <= 3,
            *got_inflight,
        );
        if !c.ok() {
            break;
        }
    }
    for (name, limit) in pools {
        c.eq(
            &format!("stats.pools.{name}.rejected"),
            model.rejected.get(name).copied().unwrap_or(0) as i64,
            stats["pools"][name]["rejected"].as_i64().unwrap_or(-1),
        );
        c.that(
            &format!("stats.pools.{name}.inflight"),
            "at or below the pool's own limit",
            stats["pools"][name]["inflight"].as_i64().unwrap_or(-1) <= limit as i64,
            stats["pools"][name]["inflight"].clone(),
        );
    }
    c.block("transcript", transcript);
    c.finish()
});
