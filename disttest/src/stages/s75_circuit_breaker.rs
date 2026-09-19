//! Stage 75 — Circuit breaker.
//!
//! When a dependency is down, the worst thing a caller can do is keep calling it. Every
//! doomed request holds a connection, a thread and a timeout's worth of latency, and the
//! pile of them is usually what turns one service's outage into everybody's. A circuit
//! breaker counts consecutive failures, gives up after a threshold, and then refuses calls
//! outright for a while — cheaply, without touching the network — before letting a single
//! trial through to find out whether anything has changed.
//!
//! The oracle is the three-state machine and its two thresholds. Time arrives as an
//! argument, so the window between opening and the next trial is an exact number the tester
//! can assert one millisecond either side of; the counters are the tester's own, replayed
//! from the same rules. The seeded run at the end walks two hundred calls at seeded instants
//! and compares the state, the verdict and both counters at every step.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;

/// Stage 75.
pub fn stage() -> Stage {
    Stage {
        number: 75,
        slug: "circuit_breaker",
        name: "Circuit breaker",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `circuit-breaker`: closed, open, half-open, and `now` always arrives as an argument",
            "It is *consecutive* failures that open the circuit; one success resets the count",
            "A call rejected while open never reached the dependency, so it is not evidence about it",
            "A failed trial reopens the circuit and restarts the window from that instant",
        ],
        examples,
        tests: vec![
            Test::new("a fresh breaker is closed", a_fresh_breaker),
            Test::new(
                "consecutive failures open the circuit",
                consecutive_failures_open_it,
            ),
            Test::new(
                "one success in between resets the count",
                a_success_resets_the_count,
            ),
            Test::new(
                "an open circuit rejects without attempting",
                an_open_circuit_rejects,
            ),
            Test::new(
                "a rejection while open is not counted as a failure",
                rejections_are_not_failures,
            ),
            Test::new(
                "the trial is allowed exactly one window after opening",
                the_trial_boundary,
            ),
            Test::new(
                "enough successful trials close the circuit",
                successful_trials_close_it,
            ),
            Test::new(
                "a failed trial reopens the circuit and restarts the window",
                a_failed_trial_reopens_it,
            ),
            Test::new(
                "a failed trial throws away the successes already banked",
                a_failed_trial_discards_progress,
            )
            .ext(),
            Test::new(
                "two hundred seeded calls match the tester's own model",
                a_long_seeded_run,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example(
            "Three failures and the window that follows",
            "circuit-breaker",
            || {
                lines(&[
                    "init 3 1000 2",
                    "call 0 fail",
                    "call 10 fail",
                    "call 20 fail",
                    "call 500 fail",
                    "state 500",
                ])
            },
        )
        .request("a breaker that gives up after three failures and stays open for a second")
        .response("the third failure opens it; the fourth call is refused without being sent")
        .note(
            "Look at `failures` in the last two answers: it is 3 both times. The refused \
             call never reached the dependency, so counting it as a failure would be \
             counting the breaker's own traffic as evidence and would hold the circuit open \
             for as long as anyone kept calling.",
        ),
        prim_example(
            "The trial at the edge of the window",
            "circuit-breaker",
            || {
                lines(&[
                    "init 1 1000 2",
                    "call 0 fail",
                    "call 999 ok",
                    "state 1000",
                    "call 1000 ok",
                    "call 1010 ok",
                ])
            },
        )
        .request("a breaker opened at zero, probed one millisecond early and then on time")
        .response("refused at 999, half-open at 1000, and closed again after two good trials")
        .note(
            "Half-open is not a timer that fires; it is the state the breaker is in when the \
             next call arrives after the window. Nothing happens at 1000 until somebody \
             calls, which is what makes the whole thing free when there is no traffic.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// The tester's own model of the three states. Nothing below consults the program.
// ---------------------------------------------------------------------------------------

/// A circuit breaker, exactly as the rules describe it.
struct Model {
    fail_threshold: i64,
    open_ms: i64,
    success_threshold: i64,
    state: &'static str,
    failures: i64,
    successes: i64,
    opened_at: i64,
}

impl Model {
    fn new(fail_threshold: i64, open_ms: i64, success_threshold: i64) -> Model {
        Model {
            fail_threshold,
            open_ms,
            success_threshold,
            state: "closed",
            failures: 0,
            successes: 0,
            opened_at: 0,
        }
    }

    fn advance(&mut self, now: i64) {
        if self.state == "open" && now >= self.opened_at + self.open_ms {
            self.state = "half-open";
            self.successes = 0;
        }
    }

    fn trip(&mut self, now: i64) {
        self.state = "open";
        self.opened_at = now;
        self.failures = self.fail_threshold;
        self.successes = 0;
    }

    /// Returns `(allowed, reason)` and leaves the model in the state the call produced.
    fn call(&mut self, now: i64, ok: bool) -> (bool, &'static str) {
        self.advance(now);
        if self.state == "open" {
            return (false, "rejected while open");
        }
        let reason = if self.state == "half-open" {
            "trial"
        } else {
            "attempted"
        };
        if self.state == "half-open" {
            if ok {
                self.successes += 1;
                if self.successes >= self.success_threshold {
                    self.state = "closed";
                    self.failures = 0;
                    self.successes = 0;
                }
            } else {
                self.trip(now);
            }
        } else if ok {
            self.failures = 0;
        } else {
            self.failures += 1;
            if self.failures >= self.fail_threshold {
                self.trip(now);
            }
        }
        (true, reason)
    }
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(a_fresh_breaker, |ctx| {
    let p = ctx.prim("circuit-breaker").await?;
    let init = p.send("init 3 1000 2").await?;
    let s = p.send("state 0").await?;
    let call = p.send("call 0 ok").await?;
    let mut c = Check::new("a breaker that has seen nothing go wrong");
    c.eq(
        "init.state",
        "closed".to_string(),
        p.expect_str(&init, "init", "state")?,
    );
    c.eq(
        "state(0).state",
        "closed".to_string(),
        p.expect_str(&s, "state 0", "state")?,
    );
    c.eq(
        "state(0).failures",
        0,
        p.expect_i64(&s, "state 0", "failures")?,
    );
    c.eq(
        "call.allowed",
        true,
        p.expect_bool(&call, "call 0 ok", "allowed")?,
    );
    c.eq(
        "call.reason",
        "attempted".to_string(),
        p.expect_str(&call, "call 0 ok", "reason")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(consecutive_failures_open_it, |ctx| {
    let p = ctx.prim("circuit-breaker").await?;
    p.send("init 3 1000 2").await?;
    let mut seen = Vec::new();
    for now in [0, 10, 20] {
        let r = p.send(&format!("call {now} fail")).await?;
        seen.push((
            p.expect_str(&r, "call", "state")?,
            p.expect_i64(&r, "call", "failures")?,
        ));
    }
    let s = p.send("state 20").await?;
    let mut c = Check::new("three failures in a row against a threshold of three");
    c.eq(
        "the (state, failures) after each failure",
        vec![
            ("closed".to_string(), 1),
            ("closed".to_string(), 2),
            ("open".to_string(), 3),
        ],
        seen,
    );
    c.eq(
        "state.opened_at",
        20,
        p.expect_i64(&s, "state 20", "opened_at")?,
    );
    c.eq(
        "state.retry_at",
        1020,
        p.expect_i64(&s, "state 20", "retry_at")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_success_resets_the_count, |ctx| {
    let p = ctx.prim("circuit-breaker").await?;
    p.send("init 3 1000 2").await?;
    p.send("call 0 fail").await?;
    p.send("call 10 fail").await?;
    // Two failures, then one success. The count is of *consecutive* failures, so this
    // success wipes the slate; a running total would open the circuit on the next failure.
    let recovered = p.send("call 20 ok").await?;
    p.send("call 30 fail").await?;
    let second = p.send("call 40 fail").await?;
    let third = p.send("call 50 fail").await?;
    let mut c = Check::new("two failures, a success, and then two more failures");
    c.eq(
        "call(20, ok).failures",
        0,
        p.expect_i64(&recovered, "call 20 ok", "failures")?,
    );
    c.eq(
        "call(40, fail).state",
        "closed".to_string(),
        p.expect_str(&second, "call", "state")?,
    );
    c.eq(
        "call(40, fail).failures",
        2,
        p.expect_i64(&second, "call", "failures")?,
    );
    c.eq(
        "call(50, fail).state",
        "open".to_string(),
        p.expect_str(&third, "call", "state")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(an_open_circuit_rejects, |ctx| {
    let p = ctx.prim("circuit-breaker").await?;
    p.send("init 2 1000 2").await?;
    p.send("call 0 fail").await?;
    p.send("call 10 fail").await?;
    let rejected = p.send("call 100 ok").await?;
    let s = p.send("state 100").await?;
    let mut c = Check::new("a call arriving while the circuit is open");
    // The call is refused whatever it would have done, and it is refused without a round
    // trip: that saved connection is the entire product the breaker sells.
    c.eq(
        "call.allowed",
        false,
        p.expect_bool(&rejected, "call 100 ok", "allowed")?,
    );
    c.eq(
        "call.state",
        "open".to_string(),
        p.expect_str(&rejected, "call", "state")?,
    );
    c.eq(
        "call.reason",
        "rejected while open".to_string(),
        p.expect_str(&rejected, "call", "reason")?,
    );
    c.eq(
        "state.state",
        "open".to_string(),
        p.expect_str(&s, "state 100", "state")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(rejections_are_not_failures, |ctx| {
    let p = ctx.prim("circuit-breaker").await?;
    p.send("init 2 1000 2").await?;
    p.send("call 0 fail").await?;
    p.send("call 10 fail").await?;
    let mut counts = Vec::new();
    for now in [20, 30, 40, 50] {
        let r = p.send(&format!("call {now} fail")).await?;
        counts.push(p.expect_i64(&r, "call", "failures")?);
    }
    let s = p.send("state 60").await?;
    let mut c = Check::new("four more calls while the circuit is already open");
    // Counting these would be counting the breaker's own refusals as evidence about a
    // dependency it never spoke to, and the window would never end while traffic continued.
    c.eq(
        "the failure count after each rejection",
        vec![2, 2, 2, 2],
        counts,
    );
    c.eq(
        "state.opened_at",
        10,
        p.expect_i64(&s, "state 60", "opened_at")?,
    );
    c.eq(
        "state.retry_at",
        1010,
        p.expect_i64(&s, "state 60", "retry_at")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_trial_boundary, |ctx| {
    let p = ctx.prim("circuit-breaker").await?;
    p.send("init 1 1000 2").await?;
    p.send("call 0 fail").await?;
    let early_state = p.send("state 999").await?;
    let early_call = p.send("call 999 ok").await?;
    let on_time_state = p.send("state 1000").await?;
    let trial = p.send("call 1000 ok").await?;
    let mut c = Check::new("the instant either side of the retry point");
    c.eq(
        "state(999).state",
        "open".to_string(),
        p.expect_str(&early_state, "state 999", "state")?,
    );
    c.eq(
        "call(999).allowed",
        false,
        p.expect_bool(&early_call, "call 999 ok", "allowed")?,
    );
    // The comparison is `now >= opened_at + open_ms`, so the window is closed up to 999 and
    // open at 1000 exactly.
    c.eq(
        "state(1000).state",
        "half-open".to_string(),
        p.expect_str(&on_time_state, "state 1000", "state")?,
    );
    c.eq(
        "call(1000).allowed",
        true,
        p.expect_bool(&trial, "call 1000 ok", "allowed")?,
    );
    c.eq(
        "call(1000).reason",
        "trial".to_string(),
        p.expect_str(&trial, "call", "reason")?,
    );
    c.eq(
        "call(1000).state",
        "half-open".to_string(),
        p.expect_str(&trial, "call", "state")?,
    );
    c.eq(
        "call(1000).successes",
        1,
        p.expect_i64(&trial, "call", "successes")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(successful_trials_close_it, |ctx| {
    let p = ctx.prim("circuit-breaker").await?;
    p.send("init 1 1000 2").await?;
    p.send("call 0 fail").await?;
    let first = p.send("call 1000 ok").await?;
    let second = p.send("call 1010 ok").await?;
    let after = p.send("call 1020 fail").await?;
    let mut c = Check::new("two good trials in a row against a success threshold of two");
    c.eq(
        "the first trial.successes",
        1,
        p.expect_i64(&first, "call", "successes")?,
    );
    c.eq(
        "the second trial.state",
        "closed".to_string(),
        p.expect_str(&second, "call", "state")?,
    );
    // Closing resets both counters: the failures that opened the circuit are history, and
    // the dependency gets the full threshold's worth of rope again.
    c.eq(
        "the second trial.failures",
        0,
        p.expect_i64(&second, "call", "failures")?,
    );
    c.eq(
        "the second trial.successes",
        0,
        p.expect_i64(&second, "call", "successes")?,
    );
    c.eq(
        "the next failure's reason",
        "attempted".to_string(),
        p.expect_str(&after, "call", "reason")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_failed_trial_reopens_it, |ctx| {
    let p = ctx.prim("circuit-breaker").await?;
    p.send("init 1 1000 2").await?;
    p.send("call 0 fail").await?;
    let trial = p.send("call 1000 fail").await?;
    let s = p.send("state 1000").await?;
    let early = p.send("call 1999 ok").await?;
    let next_trial = p.send("call 2000 ok").await?;
    let mut c = Check::new("a trial that found the dependency still broken");
    c.eq(
        "the trial.reason",
        "trial".to_string(),
        p.expect_str(&trial, "call", "reason")?,
    );
    c.eq(
        "the trial.state",
        "open".to_string(),
        p.expect_str(&trial, "call", "state")?,
    );
    // The window restarts from the failed trial, not from the original opening: a
    // dependency that is still down must not be probed every millisecond.
    c.eq(
        "state.opened_at",
        1000,
        p.expect_i64(&s, "state 1000", "opened_at")?,
    );
    c.eq(
        "state.retry_at",
        2000,
        p.expect_i64(&s, "state 1000", "retry_at")?,
    );
    c.eq(
        "call(1999).allowed",
        false,
        p.expect_bool(&early, "call 1999 ok", "allowed")?,
    );
    c.eq(
        "call(2000).reason",
        "trial".to_string(),
        p.expect_str(&next_trial, "call", "reason")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_failed_trial_discards_progress, |ctx| {
    let p = ctx.prim("circuit-breaker").await?;
    p.send("init 1 1000 3").await?;
    p.send("call 0 fail").await?;
    // Two of the three trials needed have already succeeded when the third fails.
    p.send("call 1000 ok").await?;
    let banked = p.send("call 1010 ok").await?;
    let lost = p.send("call 1020 fail").await?;
    let fresh = p.send("call 2020 ok").await?;
    let still_open_next = p.send("call 2030 ok").await?;
    let mut c = Check::new("a failed trial after two successful ones");
    c.eq(
        "the second trial.successes",
        2,
        p.expect_i64(&banked, "call", "successes")?,
    );
    // Half-way to closed is not a state worth remembering: the dependency has just proved
    // it is still broken, so the next window starts the count from nothing.
    c.eq(
        "the failed trial.successes",
        0,
        p.expect_i64(&lost, "call", "successes")?,
    );
    c.eq(
        "the first trial of the next window.successes",
        1,
        p.expect_i64(&fresh, "call", "successes")?,
    );
    c.eq(
        "the second trial of the next window.state",
        "half-open".to_string(),
        p.expect_str(&still_open_next, "call", "state")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_long_seeded_run, |ctx| {
    // The whole conversation is worked out here first — every call and the verdict, state
    // and counters it must produce — by replaying the three-state machine over the seeded
    // plan. The program is then asked the same questions and never consulted.
    let mut model = Model::new(3, 500, 2);
    let mut script: Vec<(String, bool, &'static str, &'static str, i64, i64)> = Vec::new();
    let mut now = 0i64;
    for _ in 0..200 {
        now += ctx.rng.random_range(0..400);
        let ok = ctx.rng.random_bool(0.45);
        let (allowed, reason) = model.call(now, ok);
        script.push((
            format!("call {now} {}", if ok { "ok" } else { "fail" }),
            allowed,
            reason,
            model.state,
            model.failures,
            model.successes,
        ));
    }
    let seed = ctx.seed;
    let p = ctx.prim("circuit-breaker").await?;
    p.send("init 3 500 2").await?;
    let mut actual = Vec::with_capacity(script.len());
    for (command, ..) in &script {
        let r = p.send(command).await?;
        actual.push((
            p.expect_bool(&r, command, "allowed")?,
            p.expect_str(&r, command, "reason")?,
            p.expect_str(&r, command, "state")?,
            p.expect_i64(&r, command, "failures")?,
            p.expect_i64(&r, command, "successes")?,
        ));
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("two hundred seeded calls replayed against the tester's own model");
    c.note(format!("seed {seed}, {} calls", script.len()));
    for (i, (want, got)) in script.iter().zip(&actual).enumerate() {
        let (command, allowed, reason, state, failures, successes) = want;
        c.eq(&format!("step[{i}] {command} → allowed"), *allowed, got.0);
        c.eq(
            &format!("step[{i}] {command} → reason"),
            (*reason).to_string(),
            got.1.clone(),
        );
        c.eq(
            &format!("step[{i}] {command} → state"),
            (*state).to_string(),
            got.2.clone(),
        );
        c.eq(&format!("step[{i}] {command} → failures"), *failures, got.3);
        c.eq(
            &format!("step[{i}] {command} → successes"),
            *successes,
            got.4,
        );
        if !c.ok() {
            break;
        }
    }
    c.block("transcript", transcript);
    c.finish()
});
