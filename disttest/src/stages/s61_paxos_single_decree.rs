//! Stage 61 — Single-decree Paxos.
//!
//! One value, agreed once, by a set of acceptors that may crash and a proposer that may be
//! overtaken at any moment. Paxos is famously hard to read and surprisingly small to write:
//! two promises an acceptor makes, one rule about which value a proposer is allowed to
//! propose, and everything else follows. This stage holds every acceptor and one proposer
//! in a single process, so a whole round — prepare, promise, accept, accepted — can be
//! driven a message at a time and inspected between each one.
//!
//! The oracle is the protocol's own rules, transcribed. The tester keeps each acceptor's
//! promised number and highest accepted proposal itself, replays "promise only a strictly
//! higher number", "refuse an accept below what you promised" and "propose the value of the
//! highest-numbered proposal any promise reported" by hand, and compares. The exhaustive
//! sweep near the end runs every combination of which acceptors took the first proposal and
//! which majority answered the second — sixty-four scenarios — which is where an
//! implementation that keeps the *last* promise it heard rather than the *highest* one, and
//! so quietly overwrites a value that was already chosen, finally shows.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;
use serde_json::Value;

/// Stage 61.
pub fn stage() -> Stage {
    Stage {
        number: 61,
        slug: "paxos_single_decree",
        name: "Single-decree Paxos",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `paxos`: every acceptor keeps a promised number and its highest accepted \
             proposal",
            "An acceptor promises a number only when it is strictly greater than the last one",
            "Accepting implies promising, and an accept below the promise is refused outright",
            "A proposer must propose the value of the highest-numbered proposal its promises \
             reported, and its own only when none did",
        ],
        examples,
        tests: vec![
            Test::new(
                "a fresh acceptor has promised nothing and accepted nothing",
                a_fresh_acceptor_is_empty,
            ),
            Test::new(
                "an acceptor promises a higher number and refuses a lower one",
                a_promise_needs_a_higher_number,
            ),
            Test::new(
                "the number an acceptor has already promised is not higher than itself",
                a_repeated_number_is_refused,
            ),
            Test::new(
                "a refused prepare still says how high a proposer must go",
                a_refusal_reports_the_promise,
            ),
            Test::new(
                "an accept at the promised number is taken and one below it is not",
                an_accept_below_the_promise_is_refused,
            ),
            Test::new("accepting implies promising", accepting_implies_promising),
            Test::new(
                "a promise carries the highest-numbered proposal the acceptor accepted",
                a_promise_carries_the_highest,
            ),
            Test::new(
                "a value is chosen once a majority has accepted the same proposal",
                a_majority_chooses,
            ),
            Test::new(
                "a proposer uses its own value only when no promise reported one",
                an_unconstrained_proposer_uses_its_own_value,
            ),
            Test::new(
                "a proposer must adopt the highest-numbered value, not the last one it heard",
                a_proposer_adopts_the_highest,
            ),
            Test::new(
                "a majority of promises is what makes a proposer ready",
                a_majority_of_promises_makes_it_ready,
            ),
            Test::new(
                "a chosen value cannot be changed by a later proposal",
                a_chosen_value_is_final,
            ),
            Test::new(
                "two duelling proposers can starve each other for ever",
                two_proposers_can_livelock,
            )
            .ext(),
            Test::new(
                "sixty-four scenarios of two proposals never choose two values",
                every_scenario_chooses_at_most_one_value,
            )
            .ext(),
            Test::new(
                "sixty seeded acceptor events match the tester's own replay",
                a_long_seeded_run,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("A round that chooses a value", "paxos", || {
            lines(&[
                "init 3",
                "prepare a1 1",
                "prepare a2 1",
                "accept a1 1 red",
                "learn",
                "accept a2 1 red",
                "learn",
            ])
        })
        .request("three acceptors, a prepare to two of them, and then an accept to each")
        .response("nothing is chosen after one accept, and `red` is chosen after the second")
        .note(
            "Choosing is not something a proposer does; it is a fact about the acceptors \
             that may be true before anybody has noticed. The proposer here never learns \
             the outcome — a separate learner has to go and look, which is why `learn` is \
             a command of its own.",
        ),
        prim_example("A proposer forced to abandon its value", "paxos", || {
            lines(&[
                "init 3",
                "accept a1 2 red",
                "propose 5 blue",
                "promise a2 -1 -",
                "promise a1 2 red",
                "proposer",
            ])
        })
        .request("a proposer that wants `blue`, and two promises, the second reporting `red`")
        .response("the proposer's value becomes `red` as soon as that promise arrives")
        .note(
            "This one rule is the whole of Paxos safety. A proposer that keeps its own \
             value, or that keeps whichever promise arrived last, can choose a second value \
             for a decree that was already decided — and nothing downstream will ever \
             notice, because both rounds look perfectly legal from the outside.",
        ),
        prim_example("Two proposers preempting each other", "paxos", || {
            lines(&[
                "init 3",
                "prepare a1 1",
                "prepare a2 1",
                "prepare a1 2",
                "prepare a2 2",
                "accept a1 1 red",
                "accept a2 1 red",
                "learn",
            ])
        })
        .request("one proposer's prepares, a rival's higher prepares, then the first's accepts")
        .response("both accepts refused, and nothing chosen")
        .note(
            "Repeat this and neither proposer ever finishes: Paxos is safe under duelling \
             proposers but not live. The cure is a distinguished proposer, which is exactly \
             what Multi-Paxos and Raft both build in.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// The tester's own model of the acceptor rules and the proposer's value rule.
// Nothing below consults the program.
// ---------------------------------------------------------------------------------------

/// The acceptors, as the paper states them.
#[derive(Clone)]
struct Model {
    promised: Vec<i64>,
    last_n: Vec<Option<i64>>,
    last_value: Vec<Option<String>>,
}

impl Model {
    fn new(count: usize) -> Model {
        Model {
            promised: vec![0; count],
            last_n: vec![None; count],
            last_value: vec![None; count],
        }
    }

    fn majority(&self) -> usize {
        self.promised.len() / 2 + 1
    }

    /// Returns what the acceptor must answer: `(promised, promised_n)`.
    fn prepare(&mut self, i: usize, n: i64) -> (bool, i64) {
        let granted = n > self.promised[i];
        if granted {
            self.promised[i] = n;
        }
        (granted, self.promised[i])
    }

    /// Returns what the acceptor must answer: `(accepted, promised_n)`.
    fn accept(&mut self, i: usize, n: i64, value: &str) -> (bool, i64) {
        let taken = n >= self.promised[i];
        if taken {
            self.promised[i] = n;
            self.last_n[i] = Some(n);
            self.last_value[i] = Some(value.to_string());
        }
        (taken, self.promised[i])
    }

    /// Returns what a learner must answer: `(chosen, value, count)`.
    fn learn(&self) -> (bool, Option<String>, usize) {
        let majority = self.majority();
        let mut numbers: Vec<i64> = self.last_n.iter().flatten().copied().collect();
        numbers.sort_unstable();
        numbers.dedup();
        let mut chosen: Option<String> = None;
        // Ascending, so the last number that clears the bar is the highest-numbered choice.
        for n in numbers {
            let holders: Vec<usize> = (0..self.last_n.len())
                .filter(|i| self.last_n[*i] == Some(n))
                .collect();
            if holders.len() >= majority {
                if let Some(i) = holders.first() {
                    chosen.clone_from(&self.last_value[*i]);
                }
            }
        }
        match chosen {
            Some(value) => {
                let count = self
                    .last_value
                    .iter()
                    .filter(|v| v.as_deref() == Some(value.as_str()))
                    .count();
                (true, Some(value), count)
            }
            None => (false, None, 0),
        }
    }
}

/// The value a proposer is allowed to propose, given the promises it holds.
fn proposer_value(promises: &[(Option<i64>, Option<String>)], own: &str) -> String {
    let mut best: Option<(i64, String)> = None;
    for (last_n, last_value) in promises {
        if let (Some(n), Some(v)) = (last_n, last_value) {
            if best.as_ref().is_none_or(|(seen, _)| n > seen) {
                best = Some((*n, v.clone()));
            }
        }
    }
    match best {
        Some((_, value)) => value,
        None => own.to_string(),
    }
}

/// What one field of an answer must hold, in whichever JSON shape the contract allows.
#[derive(Clone)]
enum Want {
    Int(i64),
    Bool(bool),
    Text(String),
    Null,
}

/// One scripted command and the fields its answer has to carry.
type Step = (String, Vec<(&'static str, Want)>);

/// The `last_n` and `last_value` arguments of a `promise`, spelled as the grammar wants.
fn promise_args(last_n: Option<i64>, last_value: &Option<String>) -> (i64, String) {
    match (last_n, last_value) {
        (Some(n), Some(v)) => (n, v.clone()),
        _ => (-1, "-".to_string()),
    }
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(a_fresh_acceptor_is_empty, |ctx| {
    let p = ctx.prim("paxos").await?;
    let init = p.send("init 3").await?;
    let a = p.send("acceptor a2").await?;
    let learn = p.send("learn").await?;
    let mut c = Check::new("an acceptor that has been asked nothing");
    c.eq(
        "init.acceptors",
        3,
        p.expect_i64(&init, "init 3", "acceptors")?,
    );
    c.eq(
        "init.majority",
        2,
        p.expect_i64(&init, "init 3", "majority")?,
    );
    c.eq(
        "acceptor(a2).promised_n",
        0,
        p.expect_i64(&a, "acceptor a2", "promised_n")?,
    );
    c.eq("acceptor(a2).last_n", &Value::Null, &a["last_n"]);
    c.eq("acceptor(a2).last_value", &Value::Null, &a["last_value"]);
    c.eq(
        "learn.chosen",
        false,
        p.expect_bool(&learn, "learn", "chosen")?,
    );
    c.eq("learn.value", &Value::Null, &learn["value"]);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_promise_needs_a_higher_number, |ctx| {
    let p = ctx.prim("paxos").await?;
    p.send("init 3").await?;
    let high = p.send("prepare a1 5").await?;
    let low = p.send("prepare a1 3").await?;
    let higher = p.send("prepare a1 6").await?;
    let mut c = Check::new("three prepares at falling and rising numbers");
    c.eq(
        "prepare(a1, 5).promised",
        true,
        p.expect_bool(&high, "prepare a1 5", "promised")?,
    );
    c.eq(
        "prepare(a1, 3).promised",
        false,
        p.expect_bool(&low, "prepare a1 3", "promised")?,
    );
    // A refusal must not move the promise: an acceptor that fell back to 3 here would let
    // the stale proposer's accept through afterwards.
    c.eq(
        "prepare(a1, 3).promised_n",
        5,
        p.expect_i64(&low, "prepare a1 3", "promised_n")?,
    );
    c.eq(
        "prepare(a1, 6).promised",
        true,
        p.expect_bool(&higher, "prepare a1 6", "promised")?,
    );
    c.eq(
        "prepare(a1, 6).promised_n",
        6,
        p.expect_i64(&higher, "prepare a1 6", "promised_n")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_repeated_number_is_refused, |ctx| {
    let p = ctx.prim("paxos").await?;
    p.send("init 5").await?;
    let first = p.send("prepare a1 4").await?;
    let again = p.send("prepare a1 4").await?;
    let mut c = Check::new("the same prepare arriving twice");
    c.eq(
        "the first prepare(a1, 4).promised",
        true,
        p.expect_bool(&first, "prepare a1 4", "promised")?,
    );
    // The promise is for numbers *strictly* greater. Answering true twice would let one
    // proposer count a single acceptor as two members of its majority, which is how a
    // minority quietly becomes a quorum.
    c.eq(
        "the second prepare(a1, 4).promised",
        false,
        p.expect_bool(&again, "prepare a1 4", "promised")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_refusal_reports_the_promise, |ctx| {
    let p = ctx.prim("paxos").await?;
    p.send("init 3").await?;
    p.send("prepare a1 9").await?;
    p.send("accept a1 9 red").await?;
    let refused = p.send("prepare a1 2").await?;
    let mut c = Check::new("a prepare refused by an acceptor that has already accepted");
    c.eq(
        "prepare(a1, 2).promised",
        false,
        p.expect_bool(&refused, "prepare a1 2", "promised")?,
    );
    // A bare "no" would leave the losing proposer guessing how high to go next; the
    // promised number is what lets it skip straight past its rival.
    c.eq(
        "prepare(a1, 2).promised_n",
        9,
        p.expect_i64(&refused, "prepare a1 2", "promised_n")?,
    );
    c.eq(
        "prepare(a1, 2).last_n",
        9,
        p.expect_i64(&refused, "prepare a1 2", "last_n")?,
    );
    c.eq(
        "prepare(a1, 2).last_value",
        "red".to_string(),
        p.expect_str(&refused, "prepare a1 2", "last_value")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(an_accept_below_the_promise_is_refused, |ctx| {
    let p = ctx.prim("paxos").await?;
    p.send("init 3").await?;
    p.send("prepare a1 4").await?;
    let at = p.send("accept a1 4 red").await?;
    let below = p.send("accept a1 3 blue").await?;
    let a = p.send("acceptor a1").await?;
    let mut c = Check::new("accepts at and below the promised number");
    // Equal is fine: this is the very proposal the acceptor promised to.
    c.eq(
        "accept(a1, 4, red).accepted",
        true,
        p.expect_bool(&at, "accept a1 4 red", "accepted")?,
    );
    c.eq(
        "accept(a1, 3, blue).accepted",
        false,
        p.expect_bool(&below, "accept a1 3 blue", "accepted")?,
    );
    c.eq(
        "acceptor(a1).last_value",
        "red".to_string(),
        p.expect_str(&a, "acceptor a1", "last_value")?,
    );
    c.eq(
        "acceptor(a1).last_n",
        4,
        p.expect_i64(&a, "acceptor a1", "last_n")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(accepting_implies_promising, |ctx| {
    let p = ctx.prim("paxos").await?;
    p.send("init 3").await?;
    // No prepare at all: an acceptor that has promised nothing accepts anything.
    let taken = p.send("accept a1 7 red").await?;
    let after = p.send("prepare a1 6").await?;
    let mut c = Check::new("an accept arriving before any prepare");
    c.eq(
        "accept(a1, 7, red).accepted",
        true,
        p.expect_bool(&taken, "accept a1 7 red", "accepted")?,
    );
    c.eq(
        "accept(a1, 7, red).promised_n",
        7,
        p.expect_i64(&taken, "accept a1 7 red", "promised_n")?,
    );
    // Having accepted 7, the acceptor may no longer promise 6 to anyone: a proposal it has
    // accepted must be visible to every proposal numbered above it.
    c.eq(
        "prepare(a1, 6).promised",
        false,
        p.expect_bool(&after, "prepare a1 6", "promised")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_promise_carries_the_highest, |ctx| {
    let p = ctx.prim("paxos").await?;
    p.send("init 3").await?;
    p.send("accept a1 2 red").await?;
    p.send("accept a1 5 blue").await?;
    let stale = p.send("accept a1 3 green").await?;
    let promise = p.send("prepare a1 9").await?;
    let mut c = Check::new("what a promise reports after several accepts");
    // 3 is below the 5 the acceptor has promised, so `green` never lands at all.
    c.eq(
        "accept(a1, 3, green).accepted",
        false,
        p.expect_bool(&stale, "accept a1 3 green", "accepted")?,
    );
    c.eq(
        "prepare(a1, 9).last_n",
        5,
        p.expect_i64(&promise, "prepare a1 9", "last_n")?,
    );
    // "Highest-numbered", not "most recent": a proposer that is told `green` here would be
    // free to re-choose a value that a higher proposal had already moved past.
    c.eq(
        "prepare(a1, 9).last_value",
        "blue".to_string(),
        p.expect_str(&promise, "prepare a1 9", "last_value")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_majority_chooses, |ctx| {
    let p = ctx.prim("paxos").await?;
    p.send("init 5").await?;
    p.send("accept a1 1 red").await?;
    let one = p.send("learn").await?;
    p.send("accept a2 1 red").await?;
    let two = p.send("learn").await?;
    p.send("accept a3 1 red").await?;
    let three = p.send("learn").await?;
    let mut c = Check::new("a value accepted by one, two and then three of five acceptors");
    c.eq(
        "learn.chosen after one",
        false,
        p.expect_bool(&one, "learn", "chosen")?,
    );
    c.eq(
        "learn.chosen after two",
        false,
        p.expect_bool(&two, "learn", "chosen")?,
    );
    // Five acceptors, so three is the majority — and choosing happens at the acceptors,
    // with nobody having been told about it.
    c.eq(
        "learn.chosen after three",
        true,
        p.expect_bool(&three, "learn", "chosen")?,
    );
    c.eq(
        "learn.value",
        "red".to_string(),
        p.expect_str(&three, "learn", "value")?,
    );
    c.eq("learn.count", 3, p.expect_i64(&three, "learn", "count")?);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(an_unconstrained_proposer_uses_its_own_value, |ctx| {
    let p = ctx.prim("paxos").await?;
    p.send("init 3").await?;
    let started = p.send("propose 4 blue").await?;
    let first = p.send("promise a1 -1 -").await?;
    let second = p.send("promise a2 -1 -").await?;
    let mut c = Check::new("a proposer whose promises reported nothing");
    c.eq(
        "propose(4, blue).n",
        4,
        p.expect_i64(&started, "propose 4 blue", "n")?,
    );
    c.eq(
        "the first promise's value",
        "blue".to_string(),
        p.expect_str(&first, "promise a1 -1 -", "value")?,
    );
    c.eq(
        "the second promise's value",
        "blue".to_string(),
        p.expect_str(&second, "promise a2 -1 -", "value")?,
    );
    // Only when no acceptor in the majority has accepted anything is the proposer free to
    // put forward the value it actually wanted.
    c.eq(
        "the second promise's ready",
        true,
        p.expect_bool(&second, "promise a2 -1 -", "ready")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_proposer_adopts_the_highest, |ctx| {
    let p = ctx.prim("paxos").await?;
    p.send("init 5").await?;
    p.send("propose 9 green").await?;
    let high_first = p.send("promise a1 7 blue").await?;
    // The lower-numbered promise arrives afterwards, which is exactly the order that
    // catches an implementation keeping "whichever one I heard last".
    let low_second = p.send("promise a2 3 red").await?;
    let empty = p.send("promise a3 -1 -").await?;
    let state = p.send("proposer").await?;
    let mut c = Check::new("promises arriving out of numbered order");
    c.eq(
        "after promise(a1, 7, blue), value",
        "blue".to_string(),
        p.expect_str(&high_first, "promise a1 7 blue", "value")?,
    );
    c.eq(
        "after promise(a2, 3, red), value",
        "blue".to_string(),
        p.expect_str(&low_second, "promise a2 3 red", "value")?,
    );
    c.eq(
        "after promise(a3, -1, -), value",
        "blue".to_string(),
        p.expect_str(&empty, "promise a3 -1 -", "value")?,
    );
    c.eq(
        "proposer.value",
        "blue".to_string(),
        p.expect_str(&state, "proposer", "value")?,
    );
    c.eq(
        "proposer.phase",
        "accept".to_string(),
        p.expect_str(&state, "proposer", "phase")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_majority_of_promises_makes_it_ready, |ctx| {
    let p = ctx.prim("paxos").await?;
    p.send("init 5").await?;
    p.send("propose 2 red").await?;
    let one = p.send("promise a1 -1 -").await?;
    // The same acceptor answering twice is still one promise; a proposer that counts the
    // messages rather than the acceptors proceeds on a minority.
    let repeat = p.send("promise a1 -1 -").await?;
    let two = p.send("promise a2 -1 -").await?;
    let three = p.send("promise a3 -1 -").await?;
    let mut c = Check::new("promises counted towards a majority of five");
    c.eq(
        "after one, promises",
        1,
        p.expect_i64(&one, "promise", "promises")?,
    );
    c.eq(
        "after the repeat, promises",
        1,
        p.expect_i64(&repeat, "promise", "promises")?,
    );
    c.eq(
        "after two, promises",
        2,
        p.expect_i64(&two, "promise", "promises")?,
    );
    c.eq(
        "after two, ready",
        false,
        p.expect_bool(&two, "promise", "ready")?,
    );
    c.eq(
        "after three, promises",
        3,
        p.expect_i64(&three, "promise", "promises")?,
    );
    c.eq(
        "after three, ready",
        true,
        p.expect_bool(&three, "promise", "ready")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_chosen_value_is_final, |ctx| {
    let p = ctx.prim("paxos").await?;
    p.send("init 3").await?;
    // Round one chooses `red` on a1 and a2, which is a majority of three.
    for a in ["a1", "a2"] {
        p.send(&format!("prepare {a} 1")).await?;
        p.send(&format!("accept {a} 1 red")).await?;
    }
    let chosen = p.send("learn").await?;
    // Round two wants `blue`, and collects its majority from a1 (which accepted) and a3
    // (which did not). Any two acceptors out of three share at least one member with any
    // other two, which is why the second round cannot miss the first round's value.
    p.send("propose 2 blue").await?;
    let with_a1 = p.send("prepare a1 2").await?;
    p.send("promise a1 1 red").await?;
    p.send("prepare a3 2").await?;
    let forced = p.send("promise a3 -1 -").await?;
    let value = p.expect_str(&forced, "promise a3 -1 -", "value")?;
    p.send(&format!("accept a1 2 {value}")).await?;
    p.send(&format!("accept a3 2 {value}")).await?;
    let after = p.send("learn").await?;
    let mut c = Check::new("a second proposal over a decree that was already chosen");
    c.eq(
        "the first learn.value",
        "red".to_string(),
        p.expect_str(&chosen, "learn", "value")?,
    );
    c.eq(
        "prepare(a1, 2).last_value",
        "red".to_string(),
        p.expect_str(&with_a1, "prepare a1 2", "last_value")?,
    );
    c.eq(
        "the value the proposer is forced to use",
        "red".to_string(),
        value,
    );
    c.eq(
        "the second learn.value",
        "red".to_string(),
        p.expect_str(&after, "learn", "value")?,
    );
    c.eq(
        "the second learn.count",
        3,
        p.expect_i64(&after, "learn", "count")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(two_proposers_can_livelock, |ctx| {
    let p = ctx.prim("paxos").await?;
    p.send("init 3").await?;
    let mut refusals = Vec::new();
    let mut n = 1i64;
    for _ in 0..6 {
        // One proposer wins a majority of promises at n ...
        for a in ["a1", "a2"] {
            let r = p.send(&format!("prepare {a} {n}")).await?;
            refusals.push((
                format!("prepare {a} {n}"),
                true,
                p.expect_bool(&r, "prepare", "promised")?,
            ));
        }
        // ... its rival preempts at n + 1 before the accepts arrive ...
        for a in ["a1", "a2"] {
            let r = p.send(&format!("prepare {a} {}", n + 1)).await?;
            refusals.push((
                format!("prepare {a} {}", n + 1),
                true,
                p.expect_bool(&r, "prepare", "promised")?,
            ));
        }
        // ... and every accept from the first proposer is now too late.
        for a in ["a1", "a2"] {
            let cmd = format!("accept {a} {n} red");
            let r = p.send(&cmd).await?;
            refusals.push((cmd, false, p.expect_bool(&r, "accept", "accepted")?));
        }
        n += 2;
    }
    let learn = p.send("learn").await?;
    let a1 = p.send("acceptor a1").await?;
    let mut c = Check::new("six rounds of two proposers preempting each other");
    for (command, want, got) in &refusals {
        c.eq(command, *want, *got);
        if !c.ok() {
            break;
        }
    }
    // Nothing is ever chosen, and nothing is ever wrong: Paxos is safe under duelling
    // proposers and simply not live, which is the whole argument for a distinguished one.
    c.eq(
        "learn.chosen",
        false,
        p.expect_bool(&learn, "learn", "chosen")?,
    );
    c.eq("acceptor(a1).last_n", &Value::Null, &a1["last_n"]);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(every_scenario_chooses_at_most_one_value, |ctx| {
    // The whole sweep is worked out here first — every command and the answer it must
    // produce — by replaying the acceptor rules and the proposer's value rule over each
    // scenario. The program is then asked the same questions and never consulted about the
    // answers.
    let names = ["a1", "a2", "a3"];
    let majorities: [&[usize]; 4] = [&[0, 1], &[0, 2], &[1, 2], &[0, 1, 2]];
    let mut script: Vec<Step> = Vec::new();
    let mut scenarios = 0usize;
    let mut settled = 0usize;
    for bits in 0u32..8 {
        let first: Vec<usize> = (0..3).filter(|i| bits & (1 << i) != 0).collect();
        for majority in majorities {
            for interleaved in [false, true] {
                scenarios += 1;
                let mut model = Model::new(3);
                script.push(("init 3".to_string(), vec![("majority", Want::Int(2))]));

                // Proposal 1 takes `red` to whichever acceptors this scenario names, either
                // prepare-then-accept per acceptor or all the prepares and then all the
                // accepts.
                let mut plan: Vec<(usize, bool)> = Vec::new();
                if interleaved {
                    for &i in &first {
                        plan.push((i, false));
                        plan.push((i, true));
                    }
                } else {
                    for &i in &first {
                        plan.push((i, false));
                    }
                    for &i in &first {
                        plan.push((i, true));
                    }
                }
                for (i, is_accept) in plan {
                    if is_accept {
                        let (taken, promised) = model.accept(i, 1, "red");
                        script.push((
                            format!("accept {} 1 red", names[i]),
                            vec![
                                ("accepted", Want::Bool(taken)),
                                ("promised_n", Want::Int(promised)),
                            ],
                        ));
                    } else {
                        let (granted, promised) = model.prepare(i, 1);
                        script.push((
                            format!("prepare {} 1", names[i]),
                            vec![
                                ("promised", Want::Bool(granted)),
                                ("promised_n", Want::Int(promised)),
                            ],
                        ));
                    }
                }
                let (chosen_first, value_first, _) = model.learn();
                if chosen_first {
                    settled += 1;
                }
                script.push((
                    "learn".to_string(),
                    vec![("chosen", Want::Bool(chosen_first))],
                ));

                // Proposal 2 wants `blue`, and has to collect a majority of promises first.
                script.push(("propose 2 blue".to_string(), vec![("n", Want::Int(2))]));
                let mut promises: Vec<(Option<i64>, Option<String>)> = Vec::new();
                for &i in majority {
                    let (granted, promised) = model.prepare(i, 2);
                    script.push((
                        format!("prepare {} 2", names[i]),
                        vec![
                            ("promised", Want::Bool(granted)),
                            ("promised_n", Want::Int(promised)),
                        ],
                    ));
                    let (arg_n, arg_v) = promise_args(model.last_n[i], &model.last_value[i]);
                    promises.push((model.last_n[i], model.last_value[i].clone()));
                    let running = proposer_value(&promises, "blue");
                    script.push((
                        format!("promise {} {arg_n} {arg_v}", names[i]),
                        vec![
                            ("promises", Want::Int(promises.len() as i64)),
                            ("value", Want::Text(running)),
                        ],
                    ));
                }
                let forced = proposer_value(&promises, "blue");
                // Any two of three acceptors overlap, so a decree already chosen is always
                // visible to the next proposal's majority — which is why `forced` is `red`
                // whenever round one settled anything at all.
                if chosen_first {
                    let want = value_first.clone().unwrap_or_default();
                    if forced != want {
                        return Err(crate::assert::Failure::harness(format!(
                            "the tester's own model proposed {forced:?} over a chosen {want:?}"
                        )));
                    }
                }
                for &i in majority {
                    let (taken, promised) = model.accept(i, 2, &forced);
                    script.push((
                        format!("accept {} 2 {forced}", names[i]),
                        vec![
                            ("accepted", Want::Bool(taken)),
                            ("promised_n", Want::Int(promised)),
                        ],
                    ));
                }
                let (chosen_after, value_after, count_after) = model.learn();
                let mut wants = vec![
                    ("chosen", Want::Bool(chosen_after)),
                    ("count", Want::Int(count_after as i64)),
                ];
                wants.push(match &value_after {
                    Some(v) => ("value", Want::Text(v.clone())),
                    None => ("value", Want::Null),
                });
                script.push(("learn".to_string(), wants));
            }
        }
    }

    let p = ctx.prim("paxos").await?;
    let mut c = Check::new("every combination of who accepted first and who promised second");
    c.note(format!(
        "{scenarios} scenarios, {settled} of them with a value already chosen, \
         {} commands",
        script.len()
    ));
    for (i, (command, wants)) in script.iter().enumerate() {
        let answer = p.send(command).await?;
        for (field, want) in wants {
            let path = format!("step[{i}] {command} → {field}");
            match want {
                Want::Int(n) => {
                    c.eq(&path, *n, p.expect_i64(&answer, command, field)?);
                }
                Want::Bool(b) => {
                    c.eq(&path, *b, p.expect_bool(&answer, command, field)?);
                }
                Want::Text(s) => {
                    c.eq(&path, s.clone(), p.expect_str(&answer, command, field)?);
                }
                Want::Null => {
                    c.eq(&path, &Value::Null, &answer[*field]);
                }
            }
        }
        if !c.ok() {
            break;
        }
    }
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_long_seeded_run, |ctx| {
    // Sixty prepares and accepts over five acceptors, replayed against the tester's own
    // rules before a single one of them is sent.
    let names = ["a1", "a2", "a3", "a4", "a5"];
    let values = ["red", "blue", "green"];
    let mut model = Model::new(names.len());
    let mut script: Vec<Step> = Vec::new();
    for _ in 0..60 {
        let i = ctx.rng.random_range(0..names.len());
        let n = ctx.rng.random_range(1..9);
        if ctx.rng.random_bool(0.5) {
            let (granted, promised) = model.prepare(i, n);
            let (last_n, last_value) = (model.last_n[i], model.last_value[i].clone());
            let mut wants = vec![
                ("promised", Want::Bool(granted)),
                ("promised_n", Want::Int(promised)),
            ];
            wants.push(match last_n {
                Some(v) => ("last_n", Want::Int(v)),
                None => ("last_n", Want::Null),
            });
            wants.push(match last_value {
                Some(v) => ("last_value", Want::Text(v)),
                None => ("last_value", Want::Null),
            });
            script.push((format!("prepare {} {n}", names[i]), wants));
        } else {
            let value = values[ctx.rng.random_range(0..values.len())];
            let (taken, promised) = model.accept(i, n, value);
            script.push((
                format!("accept {} {n} {value}", names[i]),
                vec![
                    ("accepted", Want::Bool(taken)),
                    ("promised_n", Want::Int(promised)),
                ],
            ));
        }
    }
    let (chosen, value, count) = model.learn();
    let mut wants = vec![
        ("chosen", Want::Bool(chosen)),
        ("count", Want::Int(count as i64)),
    ];
    wants.push(match &value {
        Some(v) => ("value", Want::Text(v.clone())),
        None => ("value", Want::Null),
    });
    script.push(("learn".to_string(), wants));

    let seed = ctx.seed;
    let p = ctx.prim("paxos").await?;
    p.send("init 5").await?;
    let mut c = Check::new("sixty seeded acceptor events replayed against the tester's rules");
    c.note(format!("seed {seed}, {} commands", script.len()));
    for (i, (command, wants)) in script.iter().enumerate() {
        let answer = p.send(command).await?;
        for (field, want) in wants {
            let path = format!("step[{i}] {command} → {field}");
            match want {
                Want::Int(n) => {
                    c.eq(&path, *n, p.expect_i64(&answer, command, field)?);
                }
                Want::Bool(b) => {
                    c.eq(&path, *b, p.expect_bool(&answer, command, field)?);
                }
                Want::Text(s) => {
                    c.eq(&path, s.clone(), p.expect_str(&answer, command, field)?);
                }
                Want::Null => {
                    c.eq(&path, &Value::Null, &answer[*field]);
                }
            }
        }
        if !c.ok() {
            break;
        }
    }
    c.block("transcript", p.transcript_block());
    c.finish()
});
