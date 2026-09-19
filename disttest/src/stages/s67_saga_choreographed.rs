//! Stage 67 — Choreographed saga.
//!
//! The same guarantee as stage 66 with the coordinator taken away. Nobody drives the saga;
//! each service subscribes to the previous one's event and publishes its own, and the
//! compensation chain is just another set of events running the other way. The happy path
//! reaches exactly the ledger the orchestrated saga reached, which is the point: the
//! ordering guarantee belongs to the pattern, not to the thing that was driving it.
//!
//! The oracle is the reaction table, transcribed: which event applies in which state, what
//! it emits, and what it writes to the ledger. The traps are the ones a bus brings with it.
//! An at-least-once broker redelivers, so an event handled twice must emit once; and two
//! services can both shout that they have failed, so a second failure must be absorbed
//! rather than start a second compensation chain that refunds the customer twice.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;
use std::collections::BTreeSet;

/// Stage 67.
pub fn stage() -> Stage {
    Stage {
        number: 67,
        slug: "saga_choreographed",
        name: "Choreographed saga",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `saga-choreo`: each service reacts to one event and emits the next",
            "`<step>.fail` starts the chain; `<step>.undone` walks it one step further back",
            "The bus is at-least-once, so an event already handled must emit nothing",
            "Once a chain is running, a second failure is absorbed — one saga, one chain",
        ],
        examples,
        tests: vec![
            Test::new(
                "an event chain with no failure commits every step",
                a_chain_with_no_failure_commits,
            ),
            Test::new(
                "the choreographed happy path reaches the orchestrated ledger",
                the_happy_path_matches_the_orchestrator,
            ),
            Test::new(
                "a failure undoes the earlier steps in reverse",
                a_failure_undoes_in_reverse,
            ),
            Test::new(
                "an event delivered twice emits only once",
                a_duplicate_event_emits_once,
            ),
            Test::new(
                "two failure events start only one compensation chain",
                two_failures_run_one_chain,
            ),
            Test::new(
                "an event that does not apply yet is dropped, not queued",
                a_premature_event_is_dropped,
            ),
            Test::new(
                "the saga commits only when the last step answers",
                the_saga_commits_on_the_last_step,
            ),
            Test::new(
                "a failure at the first step compensates nothing",
                a_failure_at_the_first_step,
            )
            .ext(),
            Test::new(
                "a repeated undone event does not undo twice",
                a_repeated_undone_does_not_undo_twice,
            )
            .ext(),
            Test::new(
                "forty seeded events match the tester's own reaction table",
                a_seeded_event_stream,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("A failure two steps in", "saga-choreo", || {
            lines(&[
                r#"init ["reserve","charge","ship"]"#,
                "event start",
                "event reserve.ok",
                "event charge.fail",
                "event reserve.undone",
                "ledger",
                "outcome",
            ])
        })
        .request("the chain started, one step acknowledged, the next failed")
        .response("each event emits exactly one, and the ledger ends with undo:reserve")
        .note(
            "Nothing here decides anything: `charge.fail` emits `reserve.undo` because \
             that is the rule the reserve service subscribes to, and the chain stops when \
             it runs out of earlier steps. The order is the reverse of the forward chain \
             for the same reason it is in an orchestrated saga — each undo is triggered by \
             the one after it.",
        ),
        prim_example(
            "A bus that delivers the same event twice",
            "saga-choreo",
            || {
                lines(&[
                    r#"init ["reserve","charge","ship"]"#,
                    "event start",
                    "event reserve.ok",
                    "event reserve.ok",
                    "ledger",
                ])
            },
        )
        .request("one acknowledgement, delivered twice")
        .response("the first emits charge.do; the second emits nothing and says so")
        .note(
            "At-least-once is the only delivery a broker can really offer, so the second \
             copy is not an error — it is the normal case. A handler that is not idempotent \
             per event charges the card twice here, and no amount of care further down the \
             chain undoes that.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// The tester's own reaction table. Nothing below consults the program for an answer.
// ---------------------------------------------------------------------------------------

/// The three-step saga the stage uses throughout.
const THREE: &str = r#"init ["reserve","charge","ship"]"#;
/// Its step names.
const THREE_NAMES: [&str; 3] = ["reserve", "charge", "ship"];

/// The rules of the choreography, written out rather than implemented.
struct Model {
    names: Vec<String>,
    handled: Vec<String>,
    ledger: Vec<String>,
    inflight_do: BTreeSet<String>,
    inflight_undo: BTreeSet<String>,
    compensating: bool,
    outcome: &'static str,
}

impl Model {
    fn new(names: &[&str]) -> Model {
        Model {
            names: names.iter().map(|n| (*n).to_string()).collect(),
            handled: Vec::new(),
            ledger: Vec::new(),
            inflight_do: BTreeSet::new(),
            inflight_undo: BTreeSet::new(),
            compensating: false,
            outcome: "running",
        }
    }

    fn undo_before(&mut self, idx: usize) -> Vec<String> {
        match idx.checked_sub(1).and_then(|i| self.names.get(i)).cloned() {
            Some(prev) => {
                self.inflight_undo.insert(prev.clone());
                vec![format!("{prev}.undo")]
            }
            None => {
                self.outcome = "compensated";
                Vec::new()
            }
        }
    }

    fn apply(&mut self, name: &str) -> Vec<String> {
        if name == "start" {
            if !self.handled.is_empty() {
                return Vec::new();
            }
            let Some(first) = self.names.first().cloned() else {
                return Vec::new();
            };
            self.handled.push(name.to_string());
            self.ledger.push(format!("do:{first}"));
            self.inflight_do.insert(first.clone());
            return vec![format!("{first}.do")];
        }
        let Some((step, kind)) = name.rsplit_once('.') else {
            return Vec::new();
        };
        let Some(idx) = self.names.iter().position(|n| n.as_str() == step) else {
            return Vec::new();
        };
        let out = match kind {
            "ok" if !self.compensating && self.inflight_do.remove(step) => {
                match self.names.get(idx + 1).cloned() {
                    Some(next) => {
                        self.ledger.push(format!("do:{next}"));
                        self.inflight_do.insert(next.clone());
                        vec![format!("{next}.do")]
                    }
                    None => {
                        self.outcome = "committed";
                        Vec::new()
                    }
                }
            }
            "fail" if !self.compensating && self.inflight_do.remove(step) => {
                self.compensating = true;
                self.undo_before(idx)
            }
            "undone" if self.inflight_undo.remove(step) => {
                self.ledger.push(format!("undo:{step}"));
                self.undo_before(idx)
            }
            _ => return Vec::new(),
        };
        self.handled.push(name.to_string());
        out
    }

    /// True when this event has already been acted upon and must be absorbed as a duplicate.
    fn is_duplicate(&self, name: &str) -> bool {
        self.handled.iter().any(|h| h.as_str() == name)
    }

    fn inflight(&self) -> usize {
        self.inflight_do.len() + self.inflight_undo.len()
    }
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(a_chain_with_no_failure_commits, |ctx| {
    let p = ctx.prim("saga-choreo").await?;
    p.send(THREE).await?;
    let started = p.send("event start").await?;
    let first = p.send("event reserve.ok").await?;
    let second = p.send("event charge.ok").await?;
    let last = p.send("event ship.ok").await?;
    let outcome = p.send("outcome").await?;
    let mut c = Check::new("the forward chain of a three-step saga");
    c.eq(
        "event(start).emitted",
        vec!["reserve.do".to_string()],
        p.expect_strs(&started, "event start", "emitted")?,
    );
    c.eq(
        "event(reserve.ok).emitted",
        vec!["charge.do".to_string()],
        p.expect_strs(&first, "event reserve.ok", "emitted")?,
    );
    c.eq(
        "event(charge.ok).emitted",
        vec!["ship.do".to_string()],
        p.expect_strs(&second, "event charge.ok", "emitted")?,
    );
    // The last step has nobody to hand on to, so the chain ends by committing.
    c.eq(
        "event(ship.ok).emitted",
        Vec::<String>::new(),
        p.expect_strs(&last, "event ship.ok", "emitted")?,
    );
    c.eq(
        "outcome.outcome",
        "committed".to_string(),
        p.expect_str(&outcome, "outcome", "outcome")?,
    );
    c.eq(
        "outcome.inflight",
        0,
        p.expect_i64(&outcome, "outcome", "inflight")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_happy_path_matches_the_orchestrator, |ctx| {
    let p = ctx.prim("saga-choreo").await?;
    p.send(THREE).await?;
    for event in ["start", "reserve.ok", "charge.ok", "ship.ok"] {
        p.send(&format!("event {event}")).await?;
    }
    let ledger = p.send("ledger").await?;
    // Stage 66 drives the same three steps through an orchestrator and writes exactly this.
    // The guarantee belongs to the pattern; the coordinator was never what provided it.
    let orchestrated: Vec<String> = THREE_NAMES.iter().map(|n| format!("do:{n}")).collect();
    let mut c = Check::new("the ledger a choreographed happy path leaves behind");
    c.eq(
        "ledger.entries",
        orchestrated,
        p.expect_strs(&ledger, "ledger", "entries")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_failure_undoes_in_reverse, |ctx| {
    let p = ctx.prim("saga-choreo").await?;
    p.send(THREE).await?;
    for event in ["start", "reserve.ok", "charge.ok"] {
        p.send(&format!("event {event}")).await?;
    }
    let failed = p.send("event ship.fail").await?;
    let undone_charge = p.send("event charge.undone").await?;
    let undone_reserve = p.send("event reserve.undone").await?;
    let ledger = p.send("ledger").await?;
    let outcome = p.send("outcome").await?;
    let mut c = Check::new("a failure at the last step, walked back to the first");
    c.eq(
        "event(ship.fail).emitted",
        vec!["charge.undo".to_string()],
        p.expect_strs(&failed, "event ship.fail", "emitted")?,
    );
    c.eq(
        "event(charge.undone).emitted",
        vec!["reserve.undo".to_string()],
        p.expect_strs(&undone_charge, "event charge.undone", "emitted")?,
    );
    c.eq(
        "event(reserve.undone).emitted",
        Vec::<String>::new(),
        p.expect_strs(&undone_reserve, "event reserve.undone", "emitted")?,
    );
    c.eq(
        "ledger.entries",
        vec![
            "do:reserve".to_string(),
            "do:charge".to_string(),
            "do:ship".to_string(),
            "undo:charge".to_string(),
            "undo:reserve".to_string(),
        ],
        p.expect_strs(&ledger, "ledger", "entries")?,
    );
    c.eq(
        "outcome.outcome",
        "compensated".to_string(),
        p.expect_str(&outcome, "outcome", "outcome")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_duplicate_event_emits_once, |ctx| {
    let p = ctx.prim("saga-choreo").await?;
    p.send(THREE).await?;
    p.send("event start").await?;
    let first = p.send("event reserve.ok").await?;
    let second = p.send("event reserve.ok").await?;
    let third = p.send("event reserve.ok").await?;
    let ledger = p.send("ledger").await?;
    let mut c = Check::new("one acknowledgement delivered three times");
    c.eq(
        "the first event(reserve.ok).emitted",
        vec!["charge.do".to_string()],
        p.expect_strs(&first, "event reserve.ok", "emitted")?,
    );
    c.eq(
        "the first event(reserve.ok).duplicate",
        false,
        p.expect_bool(&first, "event reserve.ok", "duplicate")?,
    );
    // An at-least-once bus redelivers whenever an acknowledgement is lost, so the copies are
    // the normal case; a handler that is not idempotent charges the card once per copy.
    c.eq(
        "the second event(reserve.ok).emitted",
        Vec::<String>::new(),
        p.expect_strs(&second, "event reserve.ok", "emitted")?,
    );
    c.eq(
        "the second event(reserve.ok).duplicate",
        true,
        p.expect_bool(&second, "event reserve.ok", "duplicate")?,
    );
    c.eq(
        "the third event(reserve.ok).duplicate",
        true,
        p.expect_bool(&third, "event reserve.ok", "duplicate")?,
    );
    c.eq(
        "ledger.entries",
        vec!["do:reserve".to_string(), "do:charge".to_string()],
        p.expect_strs(&ledger, "ledger", "entries")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(two_failures_run_one_chain, |ctx| {
    let p = ctx.prim("saga-choreo").await?;
    p.send(THREE).await?;
    p.send("event start").await?;
    p.send("event reserve.ok").await?;
    let first = p.send("event charge.fail").await?;
    // A second failure — a stale copy from the bus, or another service reporting the same
    // trouble. Starting a chain per failure event undoes the reservation twice, and if the
    // compensation is not itself idempotent the customer is refunded twice with it.
    let stale = p.send("event ship.fail").await?;
    let repeat = p.send("event charge.fail").await?;
    p.send("event reserve.undone").await?;
    let ledger = p.send("ledger").await?;
    let outcome = p.send("outcome").await?;
    let entries = p.expect_strs(&ledger, "ledger", "entries")?;
    let mut c = Check::new("two failure events reaching one saga");
    c.eq(
        "event(charge.fail).emitted",
        vec!["reserve.undo".to_string()],
        p.expect_strs(&first, "event charge.fail", "emitted")?,
    );
    c.eq(
        "event(ship.fail).emitted",
        Vec::<String>::new(),
        p.expect_strs(&stale, "event ship.fail", "emitted")?,
    );
    c.eq(
        "the repeated event(charge.fail).duplicate",
        true,
        p.expect_bool(&repeat, "event charge.fail", "duplicate")?,
    );
    c.eq(
        "the number of undo:reserve entries",
        1,
        entries.iter().filter(|e| *e == "undo:reserve").count(),
    );
    c.eq(
        "ledger.entries",
        vec![
            "do:reserve".to_string(),
            "do:charge".to_string(),
            "undo:reserve".to_string(),
        ],
        entries,
    );
    c.eq(
        "outcome.outcome",
        "compensated".to_string(),
        p.expect_str(&outcome, "outcome", "outcome")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_premature_event_is_dropped, |ctx| {
    let p = ctx.prim("saga-choreo").await?;
    p.send(THREE).await?;
    p.send("event start").await?;
    // Nobody has asked the ship service to do anything yet, so its acknowledgement is not a
    // duplicate — it is nonsense, and it must be dropped rather than buffered.
    let early = p.send("event ship.ok").await?;
    p.send("event reserve.ok").await?;
    p.send("event charge.ok").await?;
    let proper = p.send("event ship.ok").await?;
    let outcome = p.send("outcome").await?;
    let mut c = Check::new("an acknowledgement that arrived before its own command");
    c.eq(
        "the early event(ship.ok).emitted",
        Vec::<String>::new(),
        p.expect_strs(&early, "event ship.ok", "emitted")?,
    );
    c.eq(
        "the early event(ship.ok).duplicate",
        false,
        p.expect_bool(&early, "event ship.ok", "duplicate")?,
    );
    // Had the early copy been recorded as handled, the real one would now be swallowed as a
    // duplicate and the saga would never commit.
    c.eq(
        "the proper event(ship.ok).duplicate",
        false,
        p.expect_bool(&proper, "event ship.ok", "duplicate")?,
    );
    c.eq(
        "outcome.outcome",
        "committed".to_string(),
        p.expect_str(&outcome, "outcome", "outcome")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_saga_commits_on_the_last_step, |ctx| {
    let p = ctx.prim("saga-choreo").await?;
    p.send(THREE).await?;
    p.send("event start").await?;
    let after_start = p.send("outcome").await?;
    p.send("event reserve.ok").await?;
    p.send("event charge.ok").await?;
    let before_last = p.send("outcome").await?;
    p.send("event ship.ok").await?;
    let after_last = p.send("outcome").await?;
    let mut c = Check::new("the outcome at three points of the forward chain");
    c.eq(
        "outcome after start",
        "running".to_string(),
        p.expect_str(&after_start, "outcome", "outcome")?,
    );
    c.eq(
        "inflight after start",
        1,
        p.expect_i64(&after_start, "outcome", "inflight")?,
    );
    // Two steps acknowledged is not a committed saga: the parcel has not shipped.
    c.eq(
        "outcome with one step still out",
        "running".to_string(),
        p.expect_str(&before_last, "outcome", "outcome")?,
    );
    c.eq(
        "inflight with one step still out",
        1,
        p.expect_i64(&before_last, "outcome", "inflight")?,
    );
    c.eq(
        "outcome after the last step",
        "committed".to_string(),
        p.expect_str(&after_last, "outcome", "outcome")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_failure_at_the_first_step, |ctx| {
    let p = ctx.prim("saga-choreo").await?;
    p.send(THREE).await?;
    p.send("event start").await?;
    let failed = p.send("event reserve.fail").await?;
    let ledger = p.send("ledger").await?;
    let outcome = p.send("outcome").await?;
    let mut c = Check::new("the first step failing, with nothing behind it");
    // There is no earlier service to notify, so the chain ends where it began.
    c.eq(
        "event(reserve.fail).emitted",
        Vec::<String>::new(),
        p.expect_strs(&failed, "event reserve.fail", "emitted")?,
    );
    c.eq(
        "ledger.entries",
        vec!["do:reserve".to_string()],
        p.expect_strs(&ledger, "ledger", "entries")?,
    );
    c.eq(
        "outcome.outcome",
        "compensated".to_string(),
        p.expect_str(&outcome, "outcome", "outcome")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_repeated_undone_does_not_undo_twice, |ctx| {
    let p = ctx.prim("saga-choreo").await?;
    p.send(THREE).await?;
    for event in ["start", "reserve.ok", "charge.ok", "ship.fail"] {
        p.send(&format!("event {event}")).await?;
    }
    p.send("event charge.undone").await?;
    let repeat = p.send("event charge.undone").await?;
    p.send("event reserve.undone").await?;
    let repeat_reserve = p.send("event reserve.undone").await?;
    let ledger = p.send("ledger").await?;
    let entries = p.expect_strs(&ledger, "ledger", "entries")?;
    let mut c = Check::new("compensation acknowledgements delivered twice");
    // The undo chain rides the same at-least-once bus as the forward chain, and a second
    // undo entry would mean a second refund.
    c.eq(
        "the repeated event(charge.undone).duplicate",
        true,
        p.expect_bool(&repeat, "event charge.undone", "duplicate")?,
    );
    c.eq(
        "the repeated event(reserve.undone).duplicate",
        true,
        p.expect_bool(&repeat_reserve, "event reserve.undone", "duplicate")?,
    );
    c.eq(
        "the number of undo entries",
        2,
        entries.iter().filter(|e| e.starts_with("undo:")).count(),
    );
    c.eq(
        "ledger.entries",
        vec![
            "do:reserve".to_string(),
            "do:charge".to_string(),
            "do:ship".to_string(),
            "undo:charge".to_string(),
            "undo:reserve".to_string(),
        ],
        entries,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_seeded_event_stream, |ctx| {
    // Events are drawn from the whole vocabulary, in any order: most of them will not apply,
    // a good many will be duplicates, and the reaction table says exactly what each one does.
    let names = ["a", "b", "c"];
    let mut pool = vec!["start".to_string()];
    for n in names {
        for kind in ["ok", "fail", "undone"] {
            pool.push(format!("{n}.{kind}"));
        }
    }
    let mut model = Model::new(&names);
    let mut script: Vec<(String, Vec<String>, bool, Vec<String>)> = Vec::new();
    for _ in 0..40 {
        let name = pool[ctx.rng.random_range(0..pool.len())].clone();
        let duplicate = model.is_duplicate(&name);
        let emitted = if duplicate {
            Vec::new()
        } else {
            model.apply(&name)
        };
        script.push((name, emitted, duplicate, model.ledger.clone()));
    }
    let seed = ctx.seed;
    let want_outcome = model.outcome;
    let want_inflight = model.inflight();
    let p = ctx.prim("saga-choreo").await?;
    p.send(r#"init ["a","b","c"]"#).await?;
    let mut actual: Vec<(Vec<String>, bool, Vec<String>)> = Vec::new();
    for (name, ..) in &script {
        let r = p.send(&format!("event {name}")).await?;
        let command = format!("event {name}");
        actual.push((
            p.expect_strs(&r, &command, "emitted")?,
            p.expect_bool(&r, &command, "duplicate")?,
            p.expect_strs(&r, &command, "ledger")?,
        ));
    }
    let outcome = p.send("outcome").await?;
    let got_outcome = p.expect_str(&outcome, "outcome", "outcome")?;
    let got_inflight = p.expect_i64(&outcome, "outcome", "inflight")?;
    let transcript = p.transcript_block();
    let mut c = Check::new("forty seeded events replayed against the reaction table");
    c.note(format!("seed {seed}, {} events", script.len()));
    for (i, ((name, emitted, duplicate, ledger), (got_e, got_d, got_l))) in
        script.iter().zip(&actual).enumerate()
    {
        c.eq(
            &format!("step[{i}] event {name} → emitted"),
            emitted.clone(),
            got_e.clone(),
        );
        c.eq(
            &format!("step[{i}] event {name} → duplicate"),
            *duplicate,
            *got_d,
        );
        c.eq(
            &format!("step[{i}] event {name} → ledger"),
            ledger.clone(),
            got_l.clone(),
        );
        if !c.ok() {
            break;
        }
    }
    c.eq("outcome.outcome", want_outcome.to_string(), got_outcome);
    c.eq("outcome.inflight", want_inflight as i64, got_inflight);
    c.block("transcript", transcript);
    c.finish()
});
