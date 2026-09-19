//! Stage 76 — Hedged requests.
//!
//! In a service of any size the slowest response is never the typical one, and it is usually
//! not even a sick machine — it is a garbage collection, a cold cache, a queue that happened
//! to be busy. Waiting out a request that has already taken longer than almost all of them
//! is therefore a bad bet, and the cheap fix is to stop waiting: send a second copy to
//! another replica and take whichever answers first. It moves the tail in dramatically and
//! it is not free, because every hedge is a request the cluster did not need to serve.
//!
//! The oracle is that arithmetic and nothing else. Service times arrive as arguments, so a
//! hedged request's latency is exactly `min(primary, hedge_after + hedge)` and the tester can
//! assert it to the millisecond, including at the threshold itself. The seeded run at the
//! end puts the same twenty requests through the client twice, once plain and once hedged,
//! and asserts both halves of the trade: the tail falls and the message count rises.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;

/// Stage 76.
pub fn stage() -> Stage {
    Stage {
        number: 76,
        slug: "hedged_requests",
        name: "Hedged requests",
        ext: true,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `hedging`: service times arrive as `[primary_ms, hedge_ms]`; never time anything yourself",
            "Nothing is hedged below the threshold: `primary < hedge_after_ms` costs one message",
            "A hedged request answers in min(primary, hedge_after_ms + hedge) — the hedge starts late",
            "Cancel the loser: `inflight` is zero the moment either replica has answered",
        ],
        examples,
        tests: vec![
            Test::new("a fresh client has sent nothing", a_fresh_client),
            Test::new(
                "a request that beats the threshold sends one message",
                a_fast_request_is_not_hedged,
            ),
            Test::new(
                "a request at the threshold is hedged and a request below it is not",
                the_threshold_boundary,
            ),
            Test::new(
                "a slow request answers when the hedge does",
                a_slow_request_is_rescued,
            ),
            Test::new(
                "a hedge slower than the primary wins nothing and still costs a message",
                a_useless_hedge_still_costs,
            ),
            Test::new(
                "the loser is cancelled, so nothing is ever left in flight",
                nothing_is_left_in_flight,
            ),
            Test::new(
                "a threshold of zero hedges everything and doubles the load",
                hedging_everything,
            ),
            Test::new(
                "the statistics are computed from the recorded latencies",
                the_statistics,
            ),
            Test::new(
                "twenty seeded requests: the tail falls and the load rises",
                the_trade,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("One fast request and one slow one", "hedging", || {
            lines(&["init 50", "request a [20,5]", "request b [900,8]", "stats"])
        })
        .request("a client that hedges anything still outstanding after fifty milliseconds")
        .response("the fast request costs one message; the slow one costs two and answers in 58")
        .note(
            "The hedge does not start until the threshold, so its 8 milliseconds of service \
             time become 58 on the wire. Hedging cannot make a request faster than the \
             threshold — it can only stop one being very much slower.",
        ),
        prim_example("A hedge that was never going to help", "hedging", || {
            lines(&["init 50", "request c [200,500]", "stats"])
        })
        .request("a request whose second replica would have been slower than the first")
        .response("the primary still wins at 200, and the cluster served two requests for one")
        .note(
            "This is the case that decides whether hedging is worth it at all. If the second \
             replica is as slow as the first, every hedge is pure extra load and nothing is \
             gained; hedging pays only when the tail is much worse than the median, which is \
             why the threshold is usually set near the 95th percentile.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// The tester's own arithmetic. Nothing below consults the program.
// ---------------------------------------------------------------------------------------

/// The latency and message count of one request, hedged or not.
fn hedged(primary: i64, hedge: i64, hedge_after_ms: i64) -> (i64, i64) {
    if primary < hedge_after_ms {
        return (primary, 1);
    }
    (primary.min(hedge_after_ms + hedge), 2)
}

/// The nearest-rank percentile, the definition the topic uses.
fn percentile(latencies: &[i64], p: u64) -> i64 {
    if latencies.is_empty() {
        return 0;
    }
    let mut sorted = latencies.to_vec();
    sorted.sort_unstable();
    let len = sorted.len() as u64;
    let rank = ((p * len).div_ceil(100)).max(1) as usize;
    sorted[rank.min(sorted.len()) - 1]
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(a_fresh_client, |ctx| {
    let p = ctx.prim("hedging").await?;
    let init = p.send("init 50").await?;
    let s = p.send("stats").await?;
    let mut c = Check::new("a client that has issued no requests");
    c.eq(
        "init.hedge_after_ms",
        50,
        p.expect_i64(&init, "init 50", "hedge_after_ms")?,
    );
    c.eq("stats.requests", 0, p.expect_i64(&s, "stats", "requests")?);
    c.eq("stats.messages", 0, p.expect_i64(&s, "stats", "messages")?);
    c.eq(
        "stats.extra_load_pct",
        0,
        p.expect_i64(&s, "stats", "extra_load_pct")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_fast_request_is_not_hedged, |ctx| {
    let p = ctx.prim("hedging").await?;
    p.send("init 50").await?;
    let r = p.send("request a [20,5]").await?;
    let s = p.send("stats").await?;
    let mut c = Check::new("a request that answered well inside the threshold");
    c.eq(
        "request.latency",
        20,
        p.expect_i64(&r, "request a [20,5]", "latency")?,
    );
    c.eq(
        "request.messages",
        1,
        p.expect_i64(&r, "request a [20,5]", "messages")?,
    );
    c.eq(
        "request.winner",
        "primary".to_string(),
        p.expect_str(&r, "request a [20,5]", "winner")?,
    );
    // The common case must cost exactly what it cost before, or hedging is a tax on every
    // request rather than an insurance policy on a few.
    c.eq("stats.messages", 1, p.expect_i64(&s, "stats", "messages")?);
    c.eq(
        "stats.extra_load_pct",
        0,
        p.expect_i64(&s, "stats", "extra_load_pct")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_threshold_boundary, |ctx| {
    let p = ctx.prim("hedging").await?;
    p.send("init 50").await?;
    let below = p.send("request a [49,5]").await?;
    let at = p.send("request b [50,5]").await?;
    let mut c = Check::new("a primary one millisecond either side of the threshold");
    c.eq(
        "request([49,5]).messages",
        1,
        p.expect_i64(&below, "request", "messages")?,
    );
    // The test is `primary < hedge_after_ms`, so a request that takes exactly the threshold
    // is hedged — and its hedge still loses, because the primary was already finishing.
    c.eq(
        "request([50,5]).messages",
        2,
        p.expect_i64(&at, "request", "messages")?,
    );
    c.eq(
        "request([50,5]).latency",
        50,
        p.expect_i64(&at, "request", "latency")?,
    );
    c.eq(
        "request([50,5]).winner",
        "primary".to_string(),
        p.expect_str(&at, "request", "winner")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_slow_request_is_rescued, |ctx| {
    let p = ctx.prim("hedging").await?;
    p.send("init 50").await?;
    let r = p.send("request a [900,8]").await?;
    let s = p.send("stats").await?;
    let mut c = Check::new("a request the hedge answered first");
    // 58, not 8: the hedge was not sent until the threshold had passed.
    c.eq(
        "request.latency",
        58,
        p.expect_i64(&r, "request a [900,8]", "latency")?,
    );
    c.eq(
        "request.winner",
        "hedge".to_string(),
        p.expect_str(&r, "request", "winner")?,
    );
    c.eq(
        "request.messages",
        2,
        p.expect_i64(&r, "request", "messages")?,
    );
    c.eq("stats.max", 58, p.expect_i64(&s, "stats", "max")?);
    c.eq(
        "stats.extra_load_pct",
        100,
        p.expect_i64(&s, "stats", "extra_load_pct")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_useless_hedge_still_costs, |ctx| {
    let p = ctx.prim("hedging").await?;
    p.send("init 50").await?;
    let r = p.send("request a [200,500]").await?;
    let s = p.send("stats").await?;
    let mut c = Check::new("a hedge sent to a replica that was slower still");
    c.eq(
        "request.latency",
        200,
        p.expect_i64(&r, "request", "latency")?,
    );
    c.eq(
        "request.winner",
        "primary".to_string(),
        p.expect_str(&r, "request", "winner")?,
    );
    // Nothing was gained and a whole extra request was served. When the tail is caused by
    // load rather than by luck, every hedge makes the load worse.
    c.eq(
        "request.messages",
        2,
        p.expect_i64(&r, "request", "messages")?,
    );
    c.eq("stats.messages", 2, p.expect_i64(&s, "stats", "messages")?);
    c.eq("stats.requests", 1, p.expect_i64(&s, "stats", "requests")?);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(nothing_is_left_in_flight, |ctx| {
    let p = ctx.prim("hedging").await?;
    p.send("init 50").await?;
    let mut inflight = Vec::new();
    for spec in ["[10,5]", "[900,8]", "[200,500]", "[50,1]"] {
        let r = p.send(&format!("request x {spec}")).await?;
        inflight.push(p.expect_i64(&r, "request", "inflight")?);
    }
    let mut c = Check::new("what is left outstanding after each request");
    // Leaving the loser running is the classic hedging bug: the load it was supposed to
    // cost briefly becomes load it costs for its whole service time.
    c.eq(
        "the inflight count after each request",
        vec![0, 0, 0, 0],
        inflight,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(hedging_everything, |ctx| {
    let p = ctx.prim("hedging").await?;
    p.send("init 0").await?;
    let mut messages = Vec::new();
    for spec in ["[10,3]", "[4,9]", "[100,2]"] {
        let r = p.send(&format!("request x {spec}")).await?;
        messages.push(p.expect_i64(&r, "request", "messages")?);
    }
    let s = p.send("stats").await?;
    let mut c = Check::new("a threshold of zero, which hedges every request there is");
    c.eq("the message count of each request", vec![2, 2, 2], messages);
    c.eq("stats.requests", 3, p.expect_i64(&s, "stats", "requests")?);
    c.eq("stats.messages", 6, p.expect_i64(&s, "stats", "messages")?);
    // Twice the traffic for the whole service. This is the degenerate end of the knob, and
    // it is what happens when the threshold is set below the median by accident.
    c.eq(
        "stats.extra_load_pct",
        100,
        p.expect_i64(&s, "stats", "extra_load_pct")?,
    );
    // With the threshold at zero every request answers in min(primary, hedge), so even the
    // slowest of the three came back in 4 ms. Latency is not the reason to be careful here;
    // the doubled load is.
    c.eq("stats.max", 4, p.expect_i64(&s, "stats", "max")?);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_statistics, |ctx| {
    let p = ctx.prim("hedging").await?;
    p.send("init 1000").await?;
    // The threshold is far above every service time here, so nothing is hedged and the
    // recorded latencies are exactly the primaries: the percentiles have nowhere to hide.
    let sent: Vec<i64> = (1..=20).map(|i| i * 10).collect();
    for (i, ms) in sent.iter().enumerate() {
        p.send(&format!("request r{i} [{ms},1]")).await?;
    }
    let s = p.send("stats").await?;
    let mut c = Check::new("the statistics over twenty known latencies");
    c.eq("stats.requests", 20, p.expect_i64(&s, "stats", "requests")?);
    c.eq("stats.messages", 20, p.expect_i64(&s, "stats", "messages")?);
    c.eq(
        "stats.p50",
        percentile(&sent, 50),
        p.expect_i64(&s, "stats", "p50")?,
    );
    c.eq(
        "stats.p99",
        percentile(&sent, 99),
        p.expect_i64(&s, "stats", "p99")?,
    );
    c.eq("stats.max", 200, p.expect_i64(&s, "stats", "max")?);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_trade, |ctx| {
    // The same twenty requests are put through the client twice. Every latency and every
    // message is worked out here first, from the arithmetic alone; the program is asked the
    // same questions afterwards and never consulted about the answers.
    const THRESHOLD: i64 = 50;
    let mut pairs = Vec::new();
    for i in 0..20 {
        // One request in five is a straggler, which is roughly what a real tail looks like.
        let primary = if i % 5 == 4 {
            ctx.rng.random_range(700..1300)
        } else {
            ctx.rng.random_range(5..45)
        };
        let hedge = ctx.rng.random_range(5..20);
        pairs.push((primary, hedge));
    }
    let plain: Vec<i64> = pairs.iter().map(|(primary, _)| *primary).collect();
    let mut hedged_latencies = Vec::new();
    let mut hedged_messages = 0i64;
    for (primary, hedge) in &pairs {
        let (latency, messages) = hedged(*primary, *hedge, THRESHOLD);
        hedged_latencies.push(latency);
        hedged_messages += messages;
    }
    let seed = ctx.seed;

    let p = ctx.prim("hedging").await?;
    p.send(&format!("init {THRESHOLD}")).await?;
    for (i, (primary, hedge)) in pairs.iter().enumerate() {
        p.send(&format!("plain r{i} [{primary},{hedge}]")).await?;
    }
    let plain_stats = p.send("stats").await?;
    let plain_p99 = p.expect_i64(&plain_stats, "stats", "p99")?;
    let plain_messages = p.expect_i64(&plain_stats, "stats", "messages")?;
    let plain_load = p.expect_i64(&plain_stats, "stats", "extra_load_pct")?;

    p.send(&format!("init {THRESHOLD}")).await?;
    for (i, (primary, hedge)) in pairs.iter().enumerate() {
        p.send(&format!("request r{i} [{primary},{hedge}]")).await?;
    }
    let hedged_stats = p.send("stats").await?;
    let hedged_p99 = p.expect_i64(&hedged_stats, "stats", "p99")?;
    let hedged_max = p.expect_i64(&hedged_stats, "stats", "max")?;
    let hedged_msgs = p.expect_i64(&hedged_stats, "stats", "messages")?;
    let hedged_load = p.expect_i64(&hedged_stats, "stats", "extra_load_pct")?;
    let transcript = p.transcript_block();

    let mut c = Check::new("twenty seeded requests, sent plain and then hedged");
    c.note(format!("seed {seed}, threshold {THRESHOLD} ms"));
    c.eq("plain stats.p99", percentile(&plain, 99), plain_p99);
    c.eq("plain stats.messages", 20, plain_messages);
    c.eq("plain stats.extra_load_pct", 0, plain_load);
    c.eq(
        "hedged stats.p99",
        percentile(&hedged_latencies, 99),
        hedged_p99,
    );
    c.eq(
        "hedged stats.max",
        hedged_latencies.iter().copied().max().unwrap_or(0),
        hedged_max,
    );
    c.eq("hedged stats.messages", hedged_messages, hedged_msgs);
    c.eq(
        "hedged stats.extra_load_pct",
        (hedged_messages - 20) * 100 / 20,
        hedged_load,
    );
    // Both halves of the trade in one place: the tail comes in by an order of magnitude and
    // the cluster serves several requests it did not have to.
    c.that(
        "the tail",
        "a strictly lower 99th percentile once the stragglers are hedged",
        hedged_p99 < plain_p99,
        (plain_p99, hedged_p99),
    );
    c.that(
        "the load",
        "strictly more messages than the plain client sent",
        hedged_msgs > plain_messages,
        (plain_messages, hedged_msgs),
    );
    c.block("transcript", transcript);
    c.finish()
});
