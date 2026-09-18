//! The linearizability checker, tested hard.
//!
//! The unit tests inside `src/lin/mod.rs` cover the small, hand-written histories — a stale
//! read, a vanished acknowledged write, two compare-and-swaps that both claim to have won.
//! These go the other way: they *generate* histories, thousands of them, from a simulator
//! that is linearizable by construction, and then break them one operation at a time and
//! demand the checker notices.
//!
//! This is the part of the suite most likely to be subtly wrong, and it is the part whose
//! verdict a learner is least able to argue with, so it gets the most tests.

use disttest::lin::{check, check_with_budget, Entry, History, Op, Outcome, Verdict};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

/// A sequential store, and a history that is linearizable because it was produced by one.
///
/// Every operation is given a call/return window around the instant it was applied, with
/// the windows deliberately overlapping so the checker has real work to do: the total order
/// that produced the history is one of many the windows admit.
fn simulate(seed: u64, keys: usize, ops: usize, clients: usize) -> History {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut state: Vec<Option<Vec<u8>>> = vec![None; keys];
    let mut history = History::new();
    let mut now: u64 = 1_000;
    for i in 0..ops {
        let k = rng.random_range(0..keys);
        let client = rng.random_range(0..clients);
        let choice = rng.random_range(0..100);
        let (op, outcome) = if choice < 45 {
            let value = format!("v{i}").into_bytes();
            state[k] = Some(value.clone());
            (Op::Write(value), Outcome::Ok)
        } else if choice < 80 {
            (Op::Read, Outcome::Value(state[k].clone()))
        } else if choice < 92 {
            let expect = if rng.random_bool(0.7) {
                state[k].clone()
            } else {
                Some(b"never-written".to_vec())
            };
            let new = format!("c{i}").into_bytes();
            let swapped = expect == state[k];
            if swapped {
                state[k] = Some(new.clone());
            }
            (Op::Cas { expect, new }, Outcome::Swapped(swapped))
        } else {
            state[k] = None;
            (Op::Delete, Outcome::Ok)
        };
        // The window straddles the instant the operation was applied, so operations from
        // different clients overlap and the real-time order is only a partial one.
        let spread = rng.random_range(1..90);
        history.push(Entry {
            id: 0,
            client,
            key: format!("k{k}"),
            op,
            outcome,
            call_ns: now.saturating_sub(spread),
            ret_ns: now + spread,
            note: String::new(),
        });
        now += 100;
    }
    history
}

#[test]
fn generated_histories_are_linearizable() {
    for seed in 0..40u64 {
        let h = simulate(seed, 3, 60, 4);
        assert_eq!(
            check(&h),
            Verdict::Linearizable,
            "seed {seed} produced a history a sequential store had just generated"
        );
    }
}

#[test]
fn a_single_flipped_read_is_always_caught() {
    // Take a generated history and change one read's answer to a value the store never
    // holds at that point. Every such history must be rejected.
    let mut caught = 0;
    let mut attempted = 0;
    for seed in 0..40u64 {
        let mut h = simulate(seed, 1, 40, 3);
        let victim = h
            .entries
            .iter()
            .position(|e| matches!(e.op, Op::Read) && e.outcome != Outcome::Unknown);
        let Some(i) = victim else { continue };
        attempted += 1;
        h.entries[i].outcome = Outcome::Value(Some(b"a-value-nobody-ever-wrote".to_vec()));
        match check(&h) {
            Verdict::NotLinearizable(_) => caught += 1,
            Verdict::Inconclusive { .. } => {}
            Verdict::Linearizable => panic!(
                "seed {seed}: a read answering a value nobody ever wrote was accepted:\n{}",
                h.render()
            ),
        }
    }
    assert!(attempted >= 30, "the generator produced too few reads");
    assert_eq!(caught, attempted, "every flipped read must be caught");
}

#[test]
fn an_acknowledged_write_that_vanishes_is_always_caught() {
    // The failure a leader kill must never produce: a write was acknowledged, a later read
    // saw it, and a read after that did not.
    for seed in 0..25u64 {
        let mut h = History::new();
        let base = simulate(seed, 1, 20, 2);
        h.entries.extend(base.entries);
        let last_write = h.entries.iter().rposition(|e| matches!(e.op, Op::Write(_)));
        let Some(i) = last_write else { continue };
        let t = h.entries[i].ret_ns + 10;
        h.push(Entry {
            id: 0,
            client: 0,
            key: h.entries[i].key.clone(),
            op: Op::Read,
            outcome: Outcome::Value(None),
            call_ns: t,
            ret_ns: t + 5,
            note: String::new(),
        });
        assert!(
            matches!(check(&h), Verdict::NotLinearizable(_)),
            "seed {seed}: an acknowledged write vanished and the checker accepted it:\n{}",
            h.render()
        );
    }
}

#[test]
fn a_read_moved_after_a_write_it_could_not_have_seen_is_caught() {
    // Two writes that both returned, and a read that returned after both but answers the
    // first: impossible, because the second write had already been applied.
    let mut h = History::new();
    for (i, v) in ["a", "b"].iter().enumerate() {
        h.push(Entry {
            id: 0,
            client: 0,
            key: "k".into(),
            op: Op::Write(v.as_bytes().to_vec()),
            outcome: Outcome::Ok,
            call_ns: 100 * i as u64,
            ret_ns: 100 * i as u64 + 10,
            note: String::new(),
        });
    }
    h.push(Entry {
        id: 0,
        client: 1,
        key: "k".into(),
        op: Op::Read,
        outcome: Outcome::Value(Some(b"a".to_vec())),
        call_ns: 500,
        ret_ns: 510,
        note: String::new(),
    });
    let Verdict::NotLinearizable(v) = check(&h) else {
        panic!(
            "a read of a superseded value must be caught:\n{}",
            h.render()
        );
    };
    assert_eq!(v.culprit.id, 2, "{}", v.render());
    assert!(v.render().contains(">>"), "{}", v.render());
}

#[test]
fn lost_responses_never_cause_a_false_failure() {
    // Mark a quarter of a generated history's operations as "no answer". That can only ever
    // make a history easier to linearize, so nothing may start failing.
    for seed in 0..40u64 {
        let mut h = simulate(seed, 2, 50, 4);
        let mut rng = StdRng::seed_from_u64(seed ^ 0xa11);
        for e in h.entries.iter_mut() {
            if rng.random_bool(0.25) {
                e.outcome = Outcome::Unknown;
                e.ret_ns = u64::MAX;
            }
        }
        assert!(
            check(&h).ok(),
            "seed {seed}: losing responses turned a good history bad:\n{}",
            h.render()
        );
    }
}

#[test]
fn keys_are_checked_independently_and_the_broken_one_is_named() {
    let mut h = simulate(7, 4, 60, 3);
    // Break exactly one key by inserting an impossible read on it.
    let t = h.entries.iter().map(|e| e.ret_ns).max().unwrap_or(0) + 1_000;
    h.push(Entry {
        id: 0,
        client: 0,
        key: "k2".into(),
        op: Op::Read,
        outcome: Outcome::Value(Some(b"impossible".to_vec())),
        call_ns: t,
        ret_ns: t + 5,
        note: String::new(),
    });
    let Verdict::NotLinearizable(v) = check(&h) else {
        panic!("the broken key must be found");
    };
    assert_eq!(v.key, "k2");
    assert!(
        v.entries.len() < h.entries.len(),
        "the report must show a sub-history, not the whole run: {} of {}",
        v.entries.len(),
        h.entries.len()
    );
}

#[test]
fn the_rendered_violation_answers_the_obvious_questions() {
    let mut h = History::new();
    h.push(Entry {
        id: 0,
        client: 1,
        key: "t054/k1".into(),
        op: Op::Write(b"a".to_vec()),
        outcome: Outcome::Ok,
        call_ns: 12_004_000,
        ret_ns: 12_310_000,
        note: "m1".into(),
    });
    h.push(Entry {
        id: 0,
        client: 0,
        key: "t054/k1".into(),
        op: Op::Read,
        outcome: Outcome::Value(None),
        call_ns: 13_001_000,
        ret_ns: 13_204_000,
        note: "m3".into(),
    });
    let Verdict::NotLinearizable(v) = check(&h) else {
        panic!("a stale read must be caught");
    };
    let text = v.render();
    for want in [
        "no linearization exists",
        "t054/k1",
        "write a",
        "<absent>",
        ">>",
        "cannot be placed anywhere inside its own window",
        "states explored",
    ] {
        assert!(
            text.contains(want),
            "the report never says {want:?}:\n{text}"
        );
    }
    assert!(
        text.contains("m3"),
        "the member a call went to is shown:\n{text}"
    );
}

#[test]
fn the_checker_is_fast_enough_for_a_real_workload() {
    // Eight clients hammering three keys for a few hundred operations is what stage 54
    // records; the checker has to decide it in well under a second.
    let h = simulate(99, 3, 400, 8);
    let started = std::time::Instant::now();
    let verdict = check(&h);
    let elapsed = started.elapsed();
    assert!(verdict.ok(), "{verdict:?}");
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "the checker took {elapsed:?} on 400 operations"
    );
}

#[test]
fn an_exhausted_budget_is_inconclusive_rather_than_a_failure() {
    let h = simulate(3, 1, 80, 8);
    match check_with_budget(&h, 200) {
        Verdict::Inconclusive { states, .. } => assert!(states >= 200),
        Verdict::Linearizable => {} // a lucky first path is fine
        Verdict::NotLinearizable(v) => {
            panic!("a budget must never invent a violation:\n{}", v.render())
        }
    }
}

#[test]
fn a_history_of_one_key_with_only_unknowns_is_linearizable() {
    let mut h = History::new();
    for i in 0..20 {
        h.push(Entry {
            id: 0,
            client: i % 3,
            key: "k".into(),
            op: Op::Write(format!("v{i}").into_bytes()),
            outcome: Outcome::Unknown,
            call_ns: i as u64 * 10,
            ret_ns: u64::MAX,
            note: String::new(),
        });
    }
    assert_eq!(check(&h), Verdict::Linearizable);
    assert_eq!(h.unknowns(), 20);
}
