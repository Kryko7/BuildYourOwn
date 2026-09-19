//! Reference topics: Paxos. See `examples/reference_algorithms.rs`.
//!
//! **Reading this file spoils stages 61 and 62.** It is here so that
//! `disttest --target reference_algorithms --validate` can prove the suite's own
//! expectations, and for no other reason. Nothing is packed, tuned or clever: every topic
//! is the shortest state machine that satisfies the grammar in README.md §3.2, written so
//! that it is obviously right rather than quickly right.
//!
//! The topics here are `paxos` and `multi-paxos`.

use crate::{err, int, uint, word, Topic};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Build the state machine for one of this file's topics.
pub fn make(topic: &str) -> Option<Box<dyn Topic>> {
    match topic {
        "paxos" => Some(Box::new(Paxos::default())),
        "multi-paxos" => Some(Box::new(MultiPaxos::default())),
        _ => None,
    }
}

/// Resolve an acceptor name (`a1`..`a<n>`) to its index.
fn acceptor_index(count: usize, args: &[&str], i: usize) -> Result<usize, Value> {
    let name = word(args, i)?;
    let parsed = name
        .strip_prefix('a')
        .and_then(|rest| rest.parse::<usize>().ok());
    let Some(k) = parsed else {
        return Err(err(format!("{name:?} is not an acceptor name like \"a1\"")));
    };
    if !(1..=count).contains(&k) {
        return Err(err(format!("there is no acceptor {name:?}")));
    }
    Ok(k - 1)
}

// ---------------------------------------------------------------------------------------
// `paxos` — single-decree Paxos: every acceptor and one proposer in one process
// ---------------------------------------------------------------------------------------

/// One acceptor: the highest number it has promised, and the highest proposal it accepted.
#[derive(Clone, Default)]
struct Acceptor {
    promised_n: i64,
    last_n: Option<i64>,
    last_value: Option<String>,
}

/// The proposer of one proposal, collecting promises before it may propose a value.
#[derive(Default)]
struct Proposer {
    n: i64,
    own_value: String,
    /// Keyed by acceptor, so one acceptor answering twice is still one promise.
    promises: BTreeMap<String, (Option<i64>, Option<String>)>,
}

/// One instance of single-decree Paxos.
#[derive(Default)]
struct Paxos {
    acceptors: Vec<Acceptor>,
    proposer: Option<Proposer>,
}

impl Paxos {
    fn majority(&self) -> usize {
        self.acceptors.len() / 2 + 1
    }

    /// The value a proposer must use: the one from the highest-numbered accepted proposal
    /// among its promises, and its own only when no promise reported one at all.
    fn proposer_value(p: &Proposer) -> String {
        let mut best: Option<(i64, String)> = None;
        for (last_n, last_value) in p.promises.values() {
            if let (Some(n), Some(v)) = (last_n, last_value) {
                if best.as_ref().is_none_or(|(seen, _)| n > seen) {
                    best = Some((*n, v.clone()));
                }
            }
        }
        match best {
            Some((_, value)) => value,
            None => p.own_value.clone(),
        }
    }

    fn proposer_state(&self) -> Value {
        let Some(p) = &self.proposer else {
            return err("no proposal has been started");
        };
        let ready = p.promises.len() >= self.majority();
        json!({
            "n": p.n,
            "phase": if ready { "accept" } else { "prepare" },
            "promises": p.promises.len(),
            "value": Paxos::proposer_value(p),
            "ready": ready,
        })
    }

    /// Has a majority accepted one and the same proposal number?
    fn learn(&self) -> Value {
        let majority = self.majority();
        let mut numbers: Vec<i64> = self.acceptors.iter().filter_map(|a| a.last_n).collect();
        numbers.sort_unstable();
        numbers.dedup();
        // Ascending, so the last one that clears the bar is the highest-numbered choice.
        let mut chosen: Option<String> = None;
        for n in numbers {
            let holders: Vec<&Acceptor> = self
                .acceptors
                .iter()
                .filter(|a| a.last_n == Some(n))
                .collect();
            if holders.len() >= majority {
                if let Some(v) = holders.first().and_then(|a| a.last_value.clone()) {
                    chosen = Some(v);
                }
            }
        }
        match chosen {
            Some(value) => {
                let count = self
                    .acceptors
                    .iter()
                    .filter(|a| a.last_value.as_deref() == Some(value.as_str()))
                    .count();
                json!({ "chosen": true, "value": value, "count": count })
            }
            None => json!({ "chosen": false, "value": Value::Null, "count": 0 }),
        }
    }
}

impl Topic for Paxos {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let n = match uint(args, 0) {
                    Ok(n) if n >= 1 => n,
                    Ok(_) => return err("Paxos needs at least one acceptor"),
                    Err(e) => return e,
                };
                *self = Paxos {
                    acceptors: vec![Acceptor::default(); n],
                    proposer: None,
                };
                json!({ "ok": true, "acceptors": n, "majority": self.majority() })
            }
            "prepare" => {
                let i = match acceptor_index(self.acceptors.len(), args, 0) {
                    Ok(i) => i,
                    Err(e) => return e,
                };
                let n = match int(args, 1) {
                    Ok(n) => n,
                    Err(e) => return e,
                };
                let a = &mut self.acceptors[i];
                // Strictly greater: promising the same number twice would let one proposer
                // count a single acceptor as two members of its majority.
                let promised = n > a.promised_n;
                if promised {
                    a.promised_n = n;
                }
                json!({
                    "promised": promised,
                    "promised_n": a.promised_n,
                    "last_n": a.last_n,
                    "last_value": a.last_value,
                })
            }
            "accept" => {
                let i = match acceptor_index(self.acceptors.len(), args, 0) {
                    Ok(i) => i,
                    Err(e) => return e,
                };
                let n = match int(args, 1) {
                    Ok(n) => n,
                    Err(e) => return e,
                };
                let value = match word(args, 2) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                let a = &mut self.acceptors[i];
                // An acceptor may accept the proposal it promised to, and anything above it.
                let accepted = n >= a.promised_n;
                if accepted {
                    a.promised_n = n;
                    a.last_n = Some(n);
                    a.last_value = Some(value);
                }
                json!({ "accepted": accepted, "promised_n": a.promised_n })
            }
            "acceptor" => {
                let i = match acceptor_index(self.acceptors.len(), args, 0) {
                    Ok(i) => i,
                    Err(e) => return e,
                };
                let a = &self.acceptors[i];
                json!({
                    "promised_n": a.promised_n,
                    "last_n": a.last_n,
                    "last_value": a.last_value,
                })
            }
            "learn" => self.learn(),
            "propose" => {
                let n = match int(args, 0) {
                    Ok(n) => n,
                    Err(e) => return e,
                };
                let value = match word(args, 1) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                self.proposer = Some(Proposer {
                    n,
                    own_value: value.clone(),
                    promises: BTreeMap::new(),
                });
                json!({ "phase": "prepare", "n": n, "value": value })
            }
            "promise" => {
                let name = match word(args, 0) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                if acceptor_index(self.acceptors.len(), args, 0).is_err() {
                    return err(format!("there is no acceptor {name:?}"));
                }
                let last_n = match int(args, 1) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                let last_value = match word(args, 2) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                let reported = if last_n < 0 || last_value == "-" {
                    (None, None)
                } else {
                    (Some(last_n), Some(last_value))
                };
                let majority = self.majority();
                let Some(p) = self.proposer.as_mut() else {
                    return err("no proposal has been started");
                };
                p.promises.insert(name, reported);
                let ready = p.promises.len() >= majority;
                json!({
                    "promises": p.promises.len(),
                    "value": Paxos::proposer_value(p),
                    "ready": ready,
                })
            }
            "proposer" => self.proposer_state(),
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// `multi-paxos` — one prepare for every slot, and what preemption costs
// ---------------------------------------------------------------------------------------

/// One slot of the replicated log.
#[derive(Clone, Default)]
struct Slot {
    value: Option<String>,
    /// The ballot the current proposal was made under.
    ballot: i64,
    accepted_by: BTreeSet<String>,
    chosen: bool,
}

/// One Multi-Paxos instance: the acceptors, the leader's ballot, and the slots.
#[derive(Default)]
struct MultiPaxos {
    promised: Vec<i64>,
    /// The ballot this proposer holds; zero until it has won one.
    ballot: i64,
    leader: bool,
    /// True once leadership has been held at least once, which is how a proposer tells
    /// "nobody has run phase one yet" from "somebody took it away from me".
    ever_leader: bool,
    slots: BTreeMap<i64, Slot>,
}

impl MultiPaxos {
    fn majority(&self) -> usize {
        self.promised.len() / 2 + 1
    }

    /// The contiguous prefix the state machine may apply, and the holes below the highest
    /// chosen slot.
    fn progress(&self) -> (i64, Vec<i64>) {
        let is_chosen = |i: i64| self.slots.get(&i).is_some_and(|s| s.chosen);
        let highest = self
            .slots
            .iter()
            .filter(|(_, s)| s.chosen)
            .map(|(k, _)| *k)
            .max()
            .unwrap_or(0);
        let mut index = 0;
        while is_chosen(index + 1) {
            index += 1;
        }
        let gaps: Vec<i64> = (1..=highest).filter(|i| !is_chosen(*i)).collect();
        (index, gaps)
    }
}

impl Topic for MultiPaxos {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let n = match uint(args, 0) {
                    Ok(n) if n >= 1 => n,
                    Ok(_) => return err("Multi-Paxos needs at least one acceptor"),
                    Err(e) => return e,
                };
                *self = MultiPaxos {
                    promised: vec![0; n],
                    ..MultiPaxos::default()
                };
                json!({ "ok": true, "acceptors": n, "majority": self.majority() })
            }
            "prepare" => {
                let n = match int(args, 0) {
                    Ok(n) => n,
                    Err(e) => return e,
                };
                let majority = self.majority();
                let mut promised = 0usize;
                for p in &mut self.promised {
                    if n > *p {
                        *p = n;
                        promised += 1;
                    }
                }
                // One phase one covers every slot from here on; that is the whole idea.
                if promised >= majority {
                    self.leader = true;
                    self.ever_leader = true;
                    self.ballot = n;
                }
                json!({ "promised": promised, "leader": self.leader, "n": n })
            }
            "phase" => {
                if int(args, 0).is_err() {
                    return err("argument 0 is not a slot number");
                }
                if self.leader {
                    json!({ "phase": "accept", "reason": "stable leader" })
                } else if self.ever_leader {
                    json!({ "phase": "prepare", "reason": "preempted" })
                } else {
                    json!({ "phase": "prepare", "reason": "no leader" })
                }
            }
            "propose" => {
                let slot = match int(args, 0) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                let value = match word(args, 1) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                if !self.leader {
                    return json!({
                        "ok": false,
                        "phase": "prepare",
                        "round_trips": 2,
                        "error": "phase one has to run first: this proposer is not the leader",
                    });
                }
                let ballot = self.ballot;
                let entry = self.slots.entry(slot).or_default();
                // A chosen value is final; the leader may only re-propose what is there.
                if !entry.chosen {
                    if entry.ballot != ballot {
                        entry.accepted_by.clear();
                        entry.ballot = ballot;
                    }
                    entry.value = Some(value);
                }
                json!({
                    "ok": true,
                    "phase": "accept",
                    "round_trips": 1,
                    "error": Value::Null,
                })
            }
            "accepted" => {
                let slot = match int(args, 0) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                let name = match word(args, 1) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                if let Err(e) = acceptor_index(self.promised.len(), args, 1) {
                    return e;
                }
                let majority = self.majority();
                let Some(s) = self.slots.get_mut(&slot) else {
                    return err(format!("slot {slot} has no proposal yet"));
                };
                if s.value.is_none() {
                    return err(format!("slot {slot} has no proposal yet"));
                }
                s.accepted_by.insert(name);
                if s.accepted_by.len() >= majority {
                    s.chosen = true;
                }
                json!({
                    "chosen": s.chosen,
                    "value": s.value,
                    "count": s.accepted_by.len(),
                })
            }
            "chosen" => {
                let slot = match int(args, 0) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                match self.slots.get(&slot) {
                    Some(s) if s.chosen => json!({ "chosen": true, "value": s.value }),
                    _ => json!({ "chosen": false, "value": Value::Null }),
                }
            }
            "preempt" => {
                let n = match int(args, 0) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                for p in &mut self.promised {
                    if n > *p {
                        *p = n;
                    }
                }
                if n > self.ballot {
                    self.leader = false;
                }
                let highest = self.promised.iter().copied().max().unwrap_or(0);
                json!({ "leader": self.leader, "promised_n": highest })
            }
            "applied" => {
                let (index, gaps) = self.progress();
                json!({ "index": index, "gaps": gaps })
            }
            "state" => {
                let (index, _) = self.progress();
                let slots: serde_json::Map<String, Value> = self
                    .slots
                    .iter()
                    .map(|(k, s)| {
                        (
                            k.to_string(),
                            json!({ "chosen": s.chosen, "value": s.value }),
                        )
                    })
                    .collect();
                json!({
                    "n": self.ballot,
                    "leader": self.leader,
                    "slots": slots,
                    "applied": index,
                })
            }
            other => err(format!("unknown command {other:?}")),
        }
    }
}
