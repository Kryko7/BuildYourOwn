//! Stage 02 — Vector clocks and causal comparison.
//!
//! A Lamport clock can say "this must have come first"; it cannot say "these two never saw
//! each other". A vector clock can, and the whole of the rest of the track — siblings,
//! causal delivery, conflict detection — is built on that one extra answer.
//!
//! The oracle is [`compare_clocks`], which does the slow, obvious thing: it walks the union
//! of both clocks' names, treats an absent name as a zero, and decides `before`, `after`,
//! `equal` or `concurrent` from two booleans. The stage replays every exchange itself and
//! compares clock for clock, then asks the program to compare every pair of the clocks that
//! exchange produced — an O(n²) sweep where a rule that only looks at the names both clocks
//! happen to mention has nowhere to hide.

use crate::assert::{Check, Failure};
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::prim::oracles::{clock, compare_clocks, Clock, Relation};
use crate::prim::PrimProc;
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;
use serde_json::Value;

/// Stage 02.
pub fn stage() -> Stage {
    Stage {
        number: 2,
        slug: "vector_clocks",
        name: "Vector clocks and causal comparison",
        ext: false,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `vector-clock`: a map from process name to counter, and a missing entry is a zero",
            "`cmp` answers before, after, equal or concurrent — four cases, not three",
            "Merging on receive takes the componentwise maximum, then increments the receiver's own entry",
            "Two clocks are concurrent when neither dominates: test both directions",
        ],
        examples,
        tests: vec![
            Test::new("a process that has done nothing has an empty clock", a_fresh_clock_is_empty),
            Test::new("a local event moves only the entry of the process that acted", a_local_event_moves_one_entry),
            Test::new("a receive merges componentwise and then increments the receiver", a_receive_merges_then_increments),
            Test::new("a name nobody mentions counts as zero", a_missing_entry_is_a_zero),
            Test::new("cmp names all four relations", cmp_names_all_four_relations),
            Test::new("a clock is equal to itself", a_clock_equals_itself),
            Test::new("before and after are mirrors, and concurrent is symmetric", the_relations_are_symmetric),
            Test::new("a delivered message is strictly behind the clock that received it", a_receive_dominates_the_message),
            Test::new("a merge never lowers a component", a_merge_never_lowers_a_component).ext(),
            Test::new("every pair of a seeded exchange agrees with the oracle", a_seeded_exchange).ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example(
            "A message from `a` delivered to `b`",
            "vector-clock",
            || lines(&["local a", "send a", "recv b {\"a\":2}", "get b", "get a"]),
        )
        .request("`a` ticks twice and sends; the message is delivered to `b`; both are read back")
        .response("`b` ends at {\"a\":2,\"b\":1} — the sender's entry merged, its own incremented")
        .note(
            "The receiver's own entry is incremented *after* the merge, and only its own. A \
             receive that increments every entry, or that increments before merging, still \
             looks plausible on one exchange and orders nothing correctly on two.",
        ),
        prim_example("Four comparisons, one per relation", "vector-clock", || {
            lines(&[
                "cmp {\"a\":1,\"b\":2} {\"a\":2,\"b\":3}",
                "cmp {\"a\":2,\"b\":3} {\"a\":1,\"b\":2}",
                "cmp {\"a\":1} {\"a\":1,\"b\":0}",
                "cmp {\"a\":2,\"b\":1} {\"a\":1,\"b\":2}",
            ])
        })
        .request("the same pair of clocks compared both ways, a zero entry, and a fork")
        .response("before, after, equal, concurrent")
        .note(
            "The third comparison is the one that catches a map compared by equality: \
             {\"a\":1} and {\"a\":1,\"b\":0} are the same clock written two ways. Compare the \
             union of the names, not the keys of either side.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Reading and writing clocks
// ---------------------------------------------------------------------------------------

/// Read a clock out of one answer, dropping zero entries so that `{}` and `{"a":0}` — the
/// same clock written two ways — compare equal here, exactly as they do on the wire.
fn clock_of(p: &PrimProc, v: &Value, command: &str, field: &str) -> Result<Clock, Failure> {
    let Some(map) = v.get(field) else {
        return Err(p.shape(command, &format!("the answer has no {field:?} field")));
    };
    let Some(map) = map.as_object() else {
        return Err(p.shape(command, &format!("{field} should be an object, got {map}")));
    };
    let mut out = Clock::new();
    for (name, count) in map {
        let n = match count {
            Value::Number(n) => n.as_i64(),
            Value::String(s) => s.parse().ok(),
            _ => None,
        };
        match n {
            Some(0) => {}
            Some(n) => {
                out.insert(name.clone(), n);
            }
            None => {
                return Err(p.shape(command, &format!("{field}.{name} is not a number: {count}")))
            }
        }
    }
    Ok(out)
}

/// Send one command and read the clock it answers with.
async fn vc(p: &mut PrimProc, command: &str) -> Result<Clock, Failure> {
    let v = p.send(command).await?;
    clock_of(p, &v, command, "vc")
}

/// A clock as a command-line argument: compact JSON, because commands split on whitespace.
fn as_arg(c: &Clock) -> String {
    let object: serde_json::Map<String, Value> = c
        .iter()
        .map(|(name, count)| (name.clone(), Value::from(*count)))
        .collect();
    crate::prim::arg(&Value::Object(object))
}

/// Ask the program how two clocks relate.
async fn cmp(p: &mut PrimProc, a: &Clock, b: &Clock) -> Result<(String, String), Failure> {
    let command = format!("cmp {} {}", as_arg(a), as_arg(b));
    let v = p.send(&command).await?;
    let rel = p.expect_str(&v, &command, "rel")?;
    Ok((command, rel))
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(a_fresh_clock_is_empty, |ctx| {
    let p = ctx.prim("vector-clock").await?;
    let a = vc(p, "get a").await?;
    let b = vc(p, "get b").await?;
    let first = vc(p, "local a").await?;
    let mut c = Check::new("the clock of a process that has done nothing");
    c.eq("get(a).vc", Clock::new(), a);
    c.eq("get(b).vc", Clock::new(), b);
    c.eq("local(a).vc", clock(&[("a", 1)]), first);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_local_event_moves_one_entry, |ctx| {
    let p = ctx.prim("vector-clock").await?;
    let one = vc(p, "local a").await?;
    let two = vc(p, "local a").await?;
    let sent = vc(p, "send a").await?;
    let b = vc(p, "get b").await?;
    let a = vc(p, "get a").await?;
    let mut c = Check::new("two local events and a send, all on `a`");
    c.eq("local(a).vc", clock(&[("a", 1)]), one);
    c.eq("the second local(a).vc", clock(&[("a", 2)]), two);
    c.eq("send(a).vc", clock(&[("a", 3)]), sent);
    c.eq("get(b).vc", Clock::new(), b);
    c.eq("get(a).vc", clock(&[("a", 3)]), a);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_receive_merges_then_increments, |ctx| {
    let p = ctx.prim("vector-clock").await?;
    let before = vc(p, "local b").await?;
    let merged = vc(p, "recv b {\"a\":3,\"c\":2}").await?;
    let read_back = vc(p, "get b").await?;
    let mut c = Check::new("a message carrying names the receiver had never heard of");
    c.eq("local(b).vc", clock(&[("b", 1)]), before);
    // max componentwise is {a:3, b:1, c:2}; b's own entry then ticks to 2.
    c.eq("recv(b).vc", clock(&[("a", 3), ("b", 2), ("c", 2)]), merged);
    c.eq(
        "get(b).vc",
        clock(&[("a", 3), ("b", 2), ("c", 2)]),
        read_back,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_missing_entry_is_a_zero, |ctx| {
    let p = ctx.prim("vector-clock").await?;
    let pairs: Vec<(Clock, Clock)> = vec![
        (clock(&[("a", 1)]), clock(&[("a", 1), ("b", 0)])),
        (Clock::new(), clock(&[("a", 0), ("b", 0)])),
        (Clock::new(), clock(&[("a", 1)])),
        (clock(&[("a", 1)]), clock(&[("b", 1)])),
    ];
    let mut answers = Vec::new();
    for (a, b) in &pairs {
        answers.push(cmp(p, a, b).await?);
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("clocks that mention different names");
    for ((a, b), (command, rel)) in pairs.iter().zip(&answers) {
        c.eq(
            &format!("{command} -> rel"),
            compare_clocks(a, b).as_str().to_string(),
            rel.clone(),
        );
    }
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(cmp_names_all_four_relations, |ctx| {
    let a = clock(&[("a", 1), ("b", 2)]);
    let later = clock(&[("a", 2), ("b", 3)]);
    let forked = clock(&[("a", 3), ("b", 1)]);
    let p = ctx.prim("vector-clock").await?;
    let before = cmp(p, &a, &later).await?;
    let after = cmp(p, &later, &a).await?;
    let equal = cmp(p, &a, &a.clone()).await?;
    let concurrent = cmp(p, &a, &forked).await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("one pair of clocks compared four ways");
    c.eq("cmp(a, later).rel", "before".to_string(), before.1);
    c.eq("cmp(later, a).rel", "after".to_string(), after.1);
    c.eq("cmp(a, a).rel", "equal".to_string(), equal.1);
    c.eq("cmp(a, forked).rel", "concurrent".to_string(), concurrent.1);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_clock_equals_itself, |ctx| {
    // Seeded, so the clock is not one the program could have been written around.
    let mut mine = Clock::new();
    for name in ["a", "b", "c", "d"] {
        let n = ctx.rng.random_range(0..9);
        if n > 0 {
            mine.insert(name.to_string(), n);
        }
    }
    let seed = ctx.seed;
    let p = ctx.prim("vector-clock").await?;
    let (command, rel) = cmp(p, &mine, &mine.clone()).await?;
    let empty = cmp(p, &Clock::new(), &Clock::new()).await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a clock compared with a copy of itself");
    c.note(format!("seed {seed}"));
    c.eq(&format!("{command} -> rel"), "equal".to_string(), rel);
    c.eq("cmp({}, {}).rel", "equal".to_string(), empty.1);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(the_relations_are_symmetric, |ctx| {
    let clocks: Vec<Clock> = vec![
        Clock::new(),
        clock(&[("a", 1)]),
        clock(&[("b", 1)]),
        clock(&[("a", 1), ("b", 1)]),
        clock(&[("a", 2), ("b", 1)]),
        clock(&[("a", 1), ("b", 2), ("c", 1)]),
    ];
    let p = ctx.prim("vector-clock").await?;
    let mut answers = Vec::new();
    for (i, a) in clocks.iter().enumerate() {
        for (j, b) in clocks.iter().enumerate() {
            let (_, rel) = cmp(p, a, b).await?;
            answers.push((i, j, rel));
        }
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("every ordered pair of six clocks");
    let mut by_pair = std::collections::BTreeMap::new();
    for (i, j, rel) in &answers {
        by_pair.insert((*i, *j), rel.clone());
    }
    for ((i, j), rel) in &by_pair {
        let expected = compare_clocks(&clocks[*i], &clocks[*j]);
        c.eq(
            &format!("cmp(clocks[{i}], clocks[{j}]).rel"),
            expected.as_str().to_string(),
            rel.clone(),
        );
        let mirror = by_pair.get(&(*j, *i)).cloned().unwrap_or_default();
        let want_mirror = match expected {
            Relation::Before => "after",
            Relation::After => "before",
            Relation::Equal => "equal",
            Relation::Concurrent => "concurrent",
        };
        c.eq(
            &format!("cmp(clocks[{j}], clocks[{i}]).rel (the mirror of {rel})"),
            want_mirror.to_string(),
            mirror,
        );
    }
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_receive_dominates_the_message, |ctx| {
    let p = ctx.prim("vector-clock").await?;
    let sent = vc(p, "local a").await?;
    let stamped = vc(p, "send a").await?;
    let received = vc(p, &format!("recv b {}", as_arg(&stamped))).await?;
    let (_, rel) = cmp(p, &stamped, &received).await?;
    let (_, back) = cmp(p, &received, &stamped).await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("the message's clock against the clock that received it");
    c.observe("local(a).vc", sent);
    c.eq("send(a).vc", clock(&[("a", 2)]), stamped.clone());
    c.eq("recv(b).vc", clock(&[("a", 2), ("b", 1)]), received.clone());
    c.eq(
        "cmp(message, receiver).rel",
        compare_clocks(&stamped, &received).as_str().to_string(),
        rel,
    );
    c.eq("cmp(receiver, message).rel", "after".to_string(), back);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_merge_never_lowers_a_component, |ctx| {
    let p = ctx.prim("vector-clock").await?;
    let before = vc(p, "recv b {\"a\":5,\"c\":4}").await?;
    // Everything in this message is older than what `b` already knows, so nothing but b's
    // own entry may move.
    let after = vc(p, "recv b {\"a\":1,\"c\":0}").await?;
    let read_back = vc(p, "get b").await?;
    let mut c = Check::new("a message every component of which is stale");
    c.eq(
        "the first recv(b).vc",
        clock(&[("a", 5), ("b", 1), ("c", 4)]),
        before,
    );
    c.eq(
        "the second recv(b).vc",
        clock(&[("a", 5), ("b", 2), ("c", 4)]),
        after.clone(),
    );
    c.eq("get(b).vc", after, read_back);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_seeded_exchange, |ctx| {
    // The exchange is replayed here first: three processes, some local events, some
    // messages, delivered in an order the seed chooses. Every clock the program will be
    // asked for is known before the program is started.
    let names = ["a", "b", "c"];
    let mut state: Vec<Clock> = vec![Clock::new(), Clock::new(), Clock::new()];
    let mut in_flight: Vec<Clock> = Vec::new();
    let mut script: Vec<(String, Clock)> = Vec::new();
    for _ in 0..30 {
        let who = ctx.rng.random_range(0..names.len());
        let choice = ctx.rng.random_range(0..3);
        if choice == 2 && !in_flight.is_empty() {
            let idx = ctx.rng.random_range(0..in_flight.len());
            let msg = in_flight.remove(idx);
            for (name, count) in &msg {
                let slot = state[who].entry(name.clone()).or_insert(0);
                *slot = (*slot).max(*count);
            }
            *state[who].entry(names[who].to_string()).or_insert(0) += 1;
            script.push((
                format!("recv {} {}", names[who], as_arg(&msg)),
                state[who].clone(),
            ));
        } else {
            *state[who].entry(names[who].to_string()).or_insert(0) += 1;
            if choice == 1 {
                in_flight.push(state[who].clone());
                script.push((format!("send {}", names[who]), state[who].clone()));
            } else {
                script.push((format!("local {}", names[who]), state[who].clone()));
            }
        }
    }
    // Twelve of the clocks that exchange produced, compared every way round: 144 questions
    // whose answers all come from `compare_clocks`, not from the program.
    let sampled: Vec<Clock> = script
        .iter()
        .rev()
        .take(12)
        .map(|(_, c)| c.clone())
        .collect();
    let seed = ctx.seed;

    let p = ctx.prim("vector-clock").await?;
    let mut answers = Vec::with_capacity(script.len());
    for (command, _) in &script {
        answers.push(vc(p, command).await?);
    }
    let mut relations = Vec::new();
    for a in &sampled {
        for b in &sampled {
            relations.push(cmp(p, a, b).await?);
        }
    }
    let transcript = p.transcript_block();

    let mut c = Check::new("thirty seeded events and every pair of the clocks they produced");
    c.note(format!(
        "seed {seed}, {} events, {} comparisons",
        script.len(),
        relations.len()
    ));
    for (i, ((command, expected), actual)) in script.iter().zip(&answers).enumerate() {
        c.eq(
            &format!("step[{i}] {command}"),
            expected.clone(),
            actual.clone(),
        );
        if !c.ok() {
            break;
        }
    }
    if c.ok() {
        let mut k = 0;
        for a in &sampled {
            for b in &sampled {
                let (command, rel) = &relations[k];
                k += 1;
                c.eq(
                    &format!("{command} -> rel"),
                    compare_clocks(a, b).as_str().to_string(),
                    rel.clone(),
                );
            }
        }
    }
    c.block("transcript", transcript);
    c.finish()
});
