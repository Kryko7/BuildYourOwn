//! Reference topics: atomic commit. See `examples/reference_algorithms.rs`.
//!
//! **Reading this file spoils stages 63 to 65.** It is here so that
//! `disttest --target reference_algorithms --validate` can prove the suite's own
//! expectations, and for no other reason. Nothing is packed, tuned or clever: every topic
//! is the shortest state machine that satisfies the grammar in README.md §3.2, written so
//! that it is obviously right rather than quickly right.
//!
//! The topics here are `two-phase-commit`, `three-phase-commit` and `commit-recovery`.

use crate::{err, flag, word, word_list, Topic};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Build the state machine for one of this file's topics.
pub fn make(topic: &str) -> Option<Box<dyn Topic>> {
    match topic {
        "two-phase-commit" => Some(Box::new(TwoPhase::default())),
        "three-phase-commit" => Some(Box::new(ThreePhase::default())),
        "commit-recovery" => Some(Box::new(Recovery::default())),
        _ => None,
    }
}

/// Unwrap an argument parse, answering with the error object when it failed.
///
/// `step` returns a `Value`, so `?` is no use here and five nested matches would bury the
/// protocol under its own argument handling.
macro_rules! arg {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(e) => return e,
        }
    };
}

/// What one participant is doing. `PreCommitted` only ever occurs under three-phase commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum PState {
    #[default]
    Working,
    Prepared,
    PreCommitted,
    Committed,
    Aborted,
}

impl PState {
    fn as_str(self) -> &'static str {
        match self {
            PState::Working => "working",
            PState::Prepared => "prepared",
            PState::PreCommitted => "pre-committed",
            PState::Committed => "committed",
            PState::Aborted => "aborted",
        }
    }

    /// The decision this participant has settled on, if it has settled on one.
    fn decision(self) -> Option<&'static str> {
        match self {
            PState::Committed => Some("commit"),
            PState::Aborted => Some("abort"),
            _ => None,
        }
    }
}

/// A map from participant name to a JSON value, in a stable order.
fn map_of(entries: impl IntoIterator<Item = (String, Value)>) -> Value {
    Value::Object(entries.into_iter().collect::<Map<String, Value>>())
}

// ---------------------------------------------------------------------------------------
// `two-phase-commit` — the coordinator and every participant in one process
// ---------------------------------------------------------------------------------------

/// One transaction driven by a two-phase commit coordinator.
struct TwoPhase {
    participants: Vec<String>,
    /// `init`, `preparing`, `committed` or `aborted`.
    state: String,
    decision: Option<String>,
    votes: BTreeMap<String, String>,
    p_state: BTreeMap<String, PState>,
    coordinator_up: bool,
    /// True once the coordinator has actually put the decision on the wire.
    broadcast: bool,
}

impl Default for TwoPhase {
    fn default() -> TwoPhase {
        TwoPhase {
            participants: Vec::new(),
            state: "init".to_string(),
            decision: None,
            votes: BTreeMap::new(),
            p_state: BTreeMap::new(),
            coordinator_up: true,
            broadcast: false,
        }
    }
}

impl TwoPhase {
    fn known(&self, p: &str) -> bool {
        self.participants.iter().any(|x| x == p)
    }

    fn pstate(&self, p: &str) -> PState {
        self.p_state.get(p).copied().unwrap_or_default()
    }

    /// Work out whether the coordinator can decide yet, from the votes it holds.
    ///
    /// One `no` is enough to abort; commit needs every vote, which is the whole reason a
    /// coordinator that crashes here leaves everyone waiting.
    fn reconsider(&mut self) {
        if self.decision.is_some() {
            return;
        }
        if self.votes.values().any(|v| v == "no") {
            self.decision = Some("abort".to_string());
            self.state = "aborted".to_string();
        } else if !self.participants.is_empty() && self.votes.len() == self.participants.len() {
            self.decision = Some("commit".to_string());
            self.state = "committed".to_string();
        }
    }

    /// Settle one participant on a decision, unless it has already settled on one.
    fn settle(&mut self, p: &str, decision: &str) {
        if self.pstate(p).decision().is_some() {
            return;
        }
        let target = if decision == "commit" {
            PState::Committed
        } else {
            PState::Aborted
        };
        self.p_state.insert(p.to_string(), target);
    }

    fn state_value(&self) -> Value {
        json!({
            "state": self.state,
            "decision": self.decision,
            "votes": map_of(self.votes.iter().map(|(k, v)| (k.clone(), json!(v)))),
            "coordinator": if self.coordinator_up { "up" } else { "crashed" },
        })
    }
}

impl Topic for TwoPhase {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let names = arg!(word_list(args, 0));
                *self = TwoPhase {
                    participants: names.clone(),
                    ..TwoPhase::default()
                };
                for p in &names {
                    self.p_state.insert(p.clone(), PState::Working);
                }
                json!({ "state": self.state, "participants": names })
            }
            "prepare" => {
                if !self.coordinator_up {
                    return json!({ "state": self.state, "sent": 0 });
                }
                if self.state == "init" {
                    self.state = "preparing".to_string();
                }
                json!({ "state": self.state, "sent": self.participants.len() })
            }
            "vote" => {
                let p = arg!(word(args, 0));
                if !self.known(&p) {
                    return err(format!("{p:?} is not a participant"));
                }
                let yes = arg!(flag(args, 1, "yes", "no"));
                // A vote is a promise: the first one a participant casts is the one that
                // stands, so a retransmitted prepare cannot change an answer already given.
                if !self.votes.contains_key(&p) {
                    self.votes
                        .insert(p.clone(), if yes { "yes" } else { "no" }.to_string());
                    // Voting no costs the participant nothing: it may forget the
                    // transaction at once. Voting yes gives up exactly that right.
                    self.p_state.insert(
                        p.clone(),
                        if yes {
                            PState::Prepared
                        } else {
                            PState::Aborted
                        },
                    );
                    self.reconsider();
                }
                json!({
                    "state": self.state,
                    "votes": self.votes.len(),
                    "decision": self.decision,
                })
            }
            "decide" => {
                if !self.coordinator_up || self.decision.is_none() {
                    return json!({ "decision": Value::Null, "sent": 0 });
                }
                self.broadcast = true;
                json!({ "decision": self.decision, "sent": self.participants.len() })
            }
            "deliver" => {
                let p = arg!(word(args, 0));
                if !self.known(&p) {
                    return err(format!("{p:?} is not a participant"));
                }
                if self.broadcast {
                    if let Some(d) = self.decision.clone() {
                        self.settle(&p, &d);
                    }
                }
                json!({ "state": self.pstate(&p).as_str() })
            }
            "p-state" => {
                let p = arg!(word(args, 0));
                if !self.known(&p) {
                    return err(format!("{p:?} is not a participant"));
                }
                json!({ "state": self.pstate(&p).as_str() })
            }
            "p-timeout" => {
                let p = arg!(word(args, 0));
                if !self.known(&p) {
                    return err(format!("{p:?} is not a participant"));
                }
                if let Some(d) = self.pstate(&p).decision() {
                    return json!({ "decision": d, "blocked": false, "reason": "already decided" });
                }
                if self.pstate(&p) == PState::Prepared {
                    // The blocking window: it promised to commit if asked, so it may not
                    // abort, and only the coordinator knows whether it will be asked.
                    return json!({
                        "decision": Value::Null,
                        "blocked": true,
                        "reason": "prepared, waiting for the coordinator",
                    });
                }
                // It has promised nothing, so it aborts and answers the coordinator's
                // prepare with a no — which is what makes commit impossible from here on.
                self.p_state.insert(p.clone(), PState::Aborted);
                self.votes.entry(p).or_insert_with(|| "no".to_string());
                self.reconsider();
                json!({ "decision": "abort", "blocked": false, "reason": "has not voted" })
            }
            "p-consult" => {
                let p = arg!(word(args, 0));
                if !self.known(&p) {
                    return err(format!("{p:?} is not a participant"));
                }
                if let Some(d) = self.pstate(&p).decision() {
                    return json!({ "decision": d, "blocked": false, "reason": "already decided" });
                }
                if self.pstate(&p) == PState::Working {
                    self.p_state.insert(p.clone(), PState::Aborted);
                    self.votes.entry(p).or_insert_with(|| "no".to_string());
                    self.reconsider();
                    return json!({
                        "decision": "abort",
                        "blocked": false,
                        "reason": "has not voted",
                    });
                }
                let peers: Vec<String> = self
                    .participants
                    .iter()
                    .filter(|x| **x != p)
                    .cloned()
                    .collect();
                for peer in &peers {
                    if let Some(d) = self.pstate(peer).decision() {
                        self.settle(&p, d);
                        return json!({
                            "decision": d,
                            "blocked": false,
                            "reason": "a peer already decided",
                        });
                    }
                }
                if peers.iter().any(|x| self.pstate(x) == PState::Working) {
                    // A peer that never voted means the coordinator cannot have seen every
                    // yes, so abort is the only decision it could ever have reached.
                    self.p_state.insert(p, PState::Aborted);
                    return json!({
                        "decision": "abort",
                        "blocked": false,
                        "reason": "a peer never voted",
                    });
                }
                // Everyone is prepared and the coordinator is gone: asking around has
                // learnt nothing, which is what makes two-phase commit a blocking protocol.
                json!({
                    "decision": Value::Null,
                    "blocked": true,
                    "reason": "every peer is prepared",
                })
            }
            "crash" => {
                let what = arg!(word(args, 0));
                if what != "coordinator" {
                    return err(format!("only the coordinator can crash, not {what:?}"));
                }
                self.coordinator_up = false;
                json!({ "ok": true, "coordinator": "crashed" })
            }
            "state" => self.state_value(),
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// `three-phase-commit` — the same, plus a pre-commit round and a network that can split
// ---------------------------------------------------------------------------------------

/// One transaction driven by a three-phase commit coordinator.
struct ThreePhase {
    participants: Vec<String>,
    /// `init`, `voting`, `pre-committed`, `committed` or `aborted`.
    state: String,
    decision: Option<String>,
    votes: BTreeMap<String, String>,
    p_state: BTreeMap<String, PState>,
    acks: BTreeSet<String>,
    coordinator_up: bool,
    broadcast: bool,
    /// The participants cut off from the coordinator and from everyone else.
    cut_off: BTreeSet<String>,
    split: bool,
}

impl Default for ThreePhase {
    fn default() -> ThreePhase {
        ThreePhase {
            participants: Vec::new(),
            state: "init".to_string(),
            decision: None,
            votes: BTreeMap::new(),
            p_state: BTreeMap::new(),
            acks: BTreeSet::new(),
            coordinator_up: true,
            broadcast: false,
            cut_off: BTreeSet::new(),
            split: false,
        }
    }
}

impl ThreePhase {
    fn known(&self, p: &str) -> bool {
        self.participants.iter().any(|x| x == p)
    }

    fn pstate(&self, p: &str) -> PState {
        self.p_state.get(p).copied().unwrap_or_default()
    }

    /// The coordinator sits with everyone the partition did not cut off.
    fn reachable(&self, p: &str) -> bool {
        !self.split || !self.cut_off.contains(p)
    }

    fn majority(&self) -> usize {
        self.participants.len() / 2 + 1
    }

    fn all_yes(&self) -> bool {
        !self.participants.is_empty()
            && self.votes.len() == self.participants.len()
            && self.votes.values().all(|v| v == "yes")
    }

    fn settle(&mut self, p: &str, decision: &str) {
        if self.pstate(p).decision().is_some() {
            return;
        }
        let target = if decision == "commit" {
            PState::Committed
        } else {
            PState::Aborted
        };
        self.p_state.insert(p.to_string(), target);
    }
}

impl Topic for ThreePhase {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let names = arg!(word_list(args, 0));
                *self = ThreePhase {
                    participants: names.clone(),
                    ..ThreePhase::default()
                };
                for p in &names {
                    self.p_state.insert(p.clone(), PState::Working);
                }
                json!({ "state": self.state, "participants": names })
            }
            "vote" => {
                let p = arg!(word(args, 0));
                if !self.known(&p) {
                    return err(format!("{p:?} is not a participant"));
                }
                let yes = arg!(flag(args, 1, "yes", "no"));
                if !self.votes.contains_key(&p) {
                    self.votes
                        .insert(p.clone(), if yes { "yes" } else { "no" }.to_string());
                    self.p_state.insert(
                        p.clone(),
                        if yes {
                            PState::Prepared
                        } else {
                            PState::Aborted
                        },
                    );
                    if self.state == "init" {
                        self.state = "voting".to_string();
                    }
                    if !yes && self.decision.is_none() {
                        self.decision = Some("abort".to_string());
                        self.state = "aborted".to_string();
                        self.broadcast = true;
                    }
                }
                json!({
                    "state": self.state,
                    "votes": self.votes.len(),
                    "decision": self.decision,
                })
            }
            "pre-commit" => {
                if !self.coordinator_up {
                    return json!({
                        "ok": false,
                        "state": self.state,
                        "sent": 0,
                        "error": "the coordinator has crashed",
                    });
                }
                if !self.all_yes() {
                    return json!({
                        "ok": false,
                        "state": self.state,
                        "sent": 0,
                        "error": "not every participant has voted yes",
                    });
                }
                self.state = "pre-committed".to_string();
                let sent = self
                    .participants
                    .iter()
                    .filter(|p| self.reachable(p))
                    .count();
                json!({
                    "ok": true,
                    "state": "pre-committed",
                    "sent": sent,
                    "error": Value::Null,
                })
            }
            "p-precommit" => {
                let p = arg!(word(args, 0));
                if !self.known(&p) {
                    return err(format!("{p:?} is not a participant"));
                }
                // A partition stops the coordinator's broadcast reaching the far side, so
                // the participant simply never hears about the pre-commit round.
                if self.state == "pre-committed"
                    && self.reachable(&p)
                    && self.pstate(&p) == PState::Prepared
                {
                    self.p_state.insert(p.clone(), PState::PreCommitted);
                }
                json!({ "state": self.pstate(&p).as_str() })
            }
            "ack" => {
                let p = arg!(word(args, 0));
                if !self.known(&p) {
                    return err(format!("{p:?} is not a participant"));
                }
                if self.pstate(&p) == PState::PreCommitted && self.reachable(&p) {
                    self.acks.insert(p);
                }
                json!({ "acks": self.acks.len() })
            }
            "commit" => {
                if !self.coordinator_up {
                    return json!({
                        "ok": false,
                        "decision": Value::Null,
                        "error": "the coordinator has crashed",
                    });
                }
                if self.state != "pre-committed" {
                    return json!({
                        "ok": false,
                        "decision": Value::Null,
                        "error": "no pre-commit round has finished",
                    });
                }
                if self.acks.len() < self.majority() {
                    return json!({
                        "ok": false,
                        "decision": Value::Null,
                        "error": "fewer than a majority have acknowledged the pre-commit",
                    });
                }
                self.decision = Some("commit".to_string());
                self.state = "committed".to_string();
                self.broadcast = true;
                json!({ "ok": true, "decision": "commit", "error": Value::Null })
            }
            "deliver" => {
                let p = arg!(word(args, 0));
                if !self.known(&p) {
                    return err(format!("{p:?} is not a participant"));
                }
                if self.broadcast && self.reachable(&p) {
                    if let Some(d) = self.decision.clone() {
                        self.settle(&p, &d);
                    }
                }
                json!({ "state": self.pstate(&p).as_str() })
            }
            "p-state" => {
                let p = arg!(word(args, 0));
                if !self.known(&p) {
                    return err(format!("{p:?} is not a participant"));
                }
                json!({ "state": self.pstate(&p).as_str() })
            }
            "p-timeout" => {
                let p = arg!(word(args, 0));
                if !self.known(&p) {
                    return err(format!("{p:?} is not a participant"));
                }
                if let Some(d) = self.pstate(&p).decision() {
                    return json!({ "decision": d, "blocked": false, "reason": "already decided" });
                }
                match self.pstate(&p) {
                    // It saw the pre-commit, so it knows every participant voted yes, and
                    // commit is the only decision the coordinator could have reached.
                    PState::PreCommitted => {
                        self.p_state.insert(p, PState::Committed);
                        json!({
                            "decision": "commit",
                            "blocked": false,
                            "reason": "pre-commit received, every participant voted yes",
                        })
                    }
                    // It is prepared and no pre-commit arrived, so the coordinator cannot
                    // have committed — under the protocol's synchrony assumption.
                    PState::Prepared => {
                        self.p_state.insert(p, PState::Aborted);
                        json!({
                            "decision": "abort",
                            "blocked": false,
                            "reason": "no pre-commit received",
                        })
                    }
                    _ => {
                        self.p_state.insert(p.clone(), PState::Aborted);
                        self.votes.entry(p).or_insert_with(|| "no".to_string());
                        json!({
                            "decision": "abort",
                            "blocked": false,
                            "reason": "has not voted",
                        })
                    }
                }
            }
            "crash" => {
                let what = arg!(word(args, 0));
                if what != "coordinator" {
                    return err(format!("only the coordinator can crash, not {what:?}"));
                }
                self.coordinator_up = false;
                json!({ "ok": true, "coordinator": "crashed" })
            }
            "partition" => {
                let group = arg!(word_list(args, 0));
                for p in &group {
                    if !self.known(p) {
                        return err(format!("{p:?} is not a participant"));
                    }
                }
                self.cut_off = group.iter().cloned().collect();
                self.split = true;
                let rest: Vec<String> = self
                    .participants
                    .iter()
                    .filter(|p| !self.cut_off.contains(*p))
                    .cloned()
                    .collect();
                json!({ "split": true, "groups": [group, rest] })
            }
            "heal" => {
                self.split = false;
                self.cut_off.clear();
                json!({ "split": false, "groups": [] })
            }
            "decisions" => {
                let decisions = map_of(self.participants.iter().map(|p| {
                    (
                        p.clone(),
                        match self.pstate(p).decision() {
                            Some(d) => json!(d),
                            None => Value::Null,
                        },
                    )
                }));
                let committed = self
                    .participants
                    .iter()
                    .any(|p| self.pstate(p) == PState::Committed);
                let aborted = self
                    .participants
                    .iter()
                    .any(|p| self.pstate(p) == PState::Aborted);
                json!({ "decisions": decisions, "inconsistent": committed && aborted })
            }
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// `commit-recovery` — one participant's write-ahead log across a crash
// ---------------------------------------------------------------------------------------

/// One participant, its durable log, its durable store and its volatile state.
#[derive(Default)]
struct Recovery {
    participant: String,
    /// Durable: the write-ahead log, in the order the records were forced out.
    log: Vec<String>,
    /// Durable: the transactions whose effect actually reached the store.
    applied: BTreeSet<String>,
    /// Volatile: what the process believes, which a crash takes with it.
    states: BTreeMap<String, String>,
    crashed: bool,
}

impl Recovery {
    fn logged(&self, kind: &str, tx: &str) -> bool {
        let needle = format!("{kind}:{tx}");
        self.log.contains(&needle)
    }

    /// Every transaction the log mentions, in the order it first appears.
    fn transactions(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for record in &self.log {
            if let Some((_, tx)) = record.split_once(':') {
                if !out.iter().any(|x| x == tx) {
                    out.push(tx.to_string());
                }
            }
        }
        out
    }

    /// What the log alone says about one transaction, and what recovery must do about it.
    fn state_from_log(&self, tx: &str) -> (&'static str, &'static str) {
        if self.logged("commit", tx) {
            if self.applied.contains(tx) {
                ("committed", "none")
            } else {
                // The decision is durable but the effect never landed: redo it.
                ("committed", "redo")
            }
        } else if self.logged("abort", tx) {
            ("aborted", "none")
        } else if self.logged("prepared", tx) {
            // It voted yes, so it may not guess: only the coordinator knows the outcome.
            ("prepared", "ask-coordinator")
        } else {
            // No prepare record, so the coordinator can never have seen a yes for it, and
            // presumed abort is safe.
            ("aborted", "abort")
        }
    }
}

impl Topic for Recovery {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let name = arg!(word(args, 0));
                *self = Recovery {
                    participant: name,
                    ..Recovery::default()
                };
                json!({ "ok": true, "log": Vec::<String>::new() })
            }
            "begin" => {
                let tx = arg!(word(args, 0));
                self.log.push(format!("begin:{tx}"));
                self.states.insert(tx, "working".to_string());
                json!({ "state": "working" })
            }
            "prepare" => {
                let tx = arg!(word(args, 0));
                let yes = arg!(flag(args, 1, "yes", "no"));
                // The record is forced out before the vote is sent. A participant that
                // answers first and logs afterwards can lose the record in a crash and
                // then abort a transaction the coordinator has already committed.
                if yes {
                    self.log.push(format!("prepared:{tx}"));
                    self.states.insert(tx, "prepared".to_string());
                    json!({ "vote": "yes", "logged": "prepared", "state": "prepared" })
                } else {
                    self.log.push(format!("abort:{tx}"));
                    self.states.insert(tx, "aborted".to_string());
                    json!({ "vote": "no", "logged": "abort", "state": "aborted" })
                }
            }
            "vote-sent" => {
                let _tx = arg!(word(args, 0));
                json!({ "ok": true })
            }
            "decide" => {
                let tx = arg!(word(args, 0));
                let commit = arg!(flag(args, 1, "commit", "abort"));
                let (record, state) = if commit {
                    ("commit", "committed")
                } else {
                    ("abort", "aborted")
                };
                self.log.push(format!("{record}:{tx}"));
                self.states.insert(tx, state.to_string());
                json!({ "state": state, "logged": record })
            }
            "apply" => {
                let tx = arg!(word(args, 0));
                // Redo is idempotent: applying an effect that is already there is free.
                self.applied.insert(tx);
                json!({ "applied": true })
            }
            "crash" => {
                self.states.clear();
                self.crashed = true;
                json!({ "ok": true, "lost": "volatile state" })
            }
            "recover" => {
                self.crashed = false;
                self.states.clear();
                let mut pending: Vec<String> = Vec::new();
                let mut decided: Vec<String> = Vec::new();
                let mut actions: Vec<(String, Value)> = Vec::new();
                for tx in self.transactions() {
                    let (state, action) = self.state_from_log(&tx);
                    self.states.insert(tx.clone(), state.to_string());
                    if state == "prepared" {
                        pending.push(tx.clone());
                    } else {
                        decided.push(tx.clone());
                    }
                    actions.push((tx, json!(action)));
                }
                json!({ "pending": pending, "decided": decided, "actions": map_of(actions) })
            }
            "outcome" => {
                let tx = arg!(word(args, 0));
                let state = if self.crashed {
                    "unknown"
                } else {
                    self.states
                        .get(&tx)
                        .map(String::as_str)
                        .unwrap_or("unknown")
                };
                json!({ "state": state, "applied": self.applied.contains(&tx) })
            }
            "coordinator-says" => {
                let tx = arg!(word(args, 0));
                let commit = arg!(flag(args, 1, "commit", "abort"));
                let (record, state) = if commit {
                    ("commit", "committed")
                } else {
                    ("abort", "aborted")
                };
                self.log.push(format!("{record}:{tx}"));
                self.states.insert(tx.clone(), state.to_string());
                if commit {
                    self.applied.insert(tx.clone());
                }
                json!({ "state": state, "applied": self.applied.contains(&tx) })
            }
            "log" => json!({ "records": self.log, "participant": self.participant }),
            other => err(format!("unknown command {other:?}")),
        }
    }
}
