//! Reference topics: sagas and messaging. See `examples/reference_algorithms.rs`.
//!
//! **Reading this file spoils stages 66 to 70.** It is here so that
//! `disttest --target reference_algorithms --validate` can prove the suite's own
//! expectations, and for no other reason. Nothing is packed, tuned or clever: every topic
//! is the shortest state machine that satisfies the grammar in README.md §3.2, written so
//! that it is obviously right rather than quickly right.
//!
//! The topics here are `saga`, `saga-choreo`, `outbox`, `dedup` and `idempotency-key`.

use crate::{err, flag, int, uint, word, word_list, Topic};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Build the state machine for one of this file's topics.
pub fn make(topic: &str) -> Option<Box<dyn Topic>> {
    match topic {
        "saga" => Some(Box::new(Saga::new())),
        "saga-choreo" => Some(Box::new(Choreo::new())),
        "outbox" => Some(Box::new(Outbox::new())),
        "dedup" => Some(Box::new(Dedup::new())),
        "idempotency-key" => Some(Box::new(Keys::new())),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------
// `saga` — an orchestrated saga: forward steps, and compensations in reverse
// ---------------------------------------------------------------------------------------

/// The orchestrator's whole state: what has run, what is left to undo, and the ledger.
struct Saga {
    names: Vec<String>,
    completed: Vec<String>,
    pending: Vec<String>,
    /// Compensations already recorded, so a retry of one is a no-op rather than a second undo.
    undone: BTreeSet<String>,
    ledger: Vec<String>,
    phase: &'static str,
    outcome: &'static str,
}

impl Saga {
    fn new() -> Saga {
        Saga {
            names: Vec::new(),
            completed: Vec::new(),
            pending: Vec::new(),
            undone: BTreeSet::new(),
            ledger: Vec::new(),
            phase: "forward",
            outcome: "running",
        }
    }

    fn reset(&mut self) {
        self.completed.clear();
        self.pending.clear();
        self.undone.clear();
        self.ledger.clear();
        self.phase = "forward";
        self.outcome = "running";
    }

    /// Run the next forward step; `Some(..)` is a misuse of the grammar, not a step failure.
    fn do_step(&mut self, name: &str, ok: bool) -> Option<Value> {
        if self.phase != "forward" {
            return Some(err("the saga is not running forward"));
        }
        let Some(expected) = self.names.get(self.completed.len()).cloned() else {
            return Some(err("every step has already run"));
        };
        if expected.as_str() != name {
            return Some(err(format!("the next step is {expected:?}, not {name:?}")));
        }
        if ok {
            self.completed.push(name.to_string());
            self.ledger.push(format!("do:{name}"));
            if self.completed.len() == self.names.len() {
                self.phase = "done";
                self.outcome = "committed";
            }
        } else {
            // The step that failed never completed, so it has nothing to undo: the
            // compensations are exactly the steps before it, newest first.
            self.phase = "compensating";
            self.pending = self.completed.iter().rev().cloned().collect();
            if self.pending.is_empty() {
                self.phase = "done";
                self.outcome = "compensated";
            }
        }
        None
    }

    /// Run the next compensation; a repeat of one already recorded is a no-op.
    fn do_comp(&mut self, name: &str, ok: bool) -> Option<Value> {
        if self.undone.contains(name) {
            return None;
        }
        if self.phase != "compensating" {
            return Some(err("the saga is not compensating"));
        }
        match self.pending.first() {
            Some(head) if head.as_str() == name => {}
            Some(head) => {
                return Some(err(format!(
                    "the next compensation is {head:?}, not {name:?}"
                )))
            }
            None => return Some(err("there is nothing left to compensate")),
        }
        if ok {
            self.pending.remove(0);
            self.undone.insert(name.to_string());
            self.ledger.push(format!("undo:{name}"));
            if self.pending.is_empty() {
                self.phase = "done";
                self.outcome = "compensated";
            }
        }
        None
    }
}

impl Topic for Saga {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let names = match word_list(args, 0) {
                    Ok(n) if !n.is_empty() => n,
                    Ok(_) => return err("a saga needs at least one step"),
                    Err(e) => return e,
                };
                self.names = names;
                self.reset();
                json!({ "steps": self.names.len(), "names": self.names })
            }
            "step" => {
                let (name, ok) = match (word(args, 0), flag(args, 1, "ok", "fail")) {
                    (Ok(n), Ok(o)) => (n, o),
                    (Err(e), _) | (_, Err(e)) => return e,
                };
                if let Some(e) = self.do_step(&name, ok) {
                    return e;
                }
                json!({
                    "phase": self.phase,
                    "completed": self.completed,
                    "outcome": self.outcome,
                })
            }
            "compensate" => {
                let (name, ok) = match (word(args, 0), flag(args, 1, "ok", "fail")) {
                    (Ok(n), Ok(o)) => (n, o),
                    (Err(e), _) | (_, Err(e)) => return e,
                };
                if let Some(e) = self.do_comp(&name, ok) {
                    return e;
                }
                json!({
                    "phase": self.phase,
                    "pending": self.pending,
                    "outcome": self.outcome,
                })
            }
            "run" => {
                let fail_at = match uint(args, 0) {
                    Ok(n) => n,
                    Err(e) => return e,
                };
                if fail_at > self.names.len() {
                    return err(format!("fail_at {fail_at} is past the last step"));
                }
                self.reset();
                let names = self.names.clone();
                for (i, name) in names.iter().enumerate() {
                    let ok = fail_at == 0 || i + 1 != fail_at;
                    if let Some(e) = self.do_step(name, ok) {
                        return e;
                    }
                    if !ok {
                        break;
                    }
                }
                for name in self.pending.clone() {
                    if let Some(e) = self.do_comp(&name, true) {
                        return e;
                    }
                }
                let executed = self.completed.clone();
                let compensated: Vec<String> = if self.outcome == "compensated" {
                    executed.iter().rev().cloned().collect()
                } else {
                    Vec::new()
                };
                json!({
                    "executed": executed,
                    "compensated": compensated,
                    "outcome": self.outcome,
                })
            }
            "ledger" => json!({ "entries": self.ledger }),
            "state" => json!({
                "phase": self.phase,
                "completed": self.completed,
                "pending": self.pending,
                "outcome": self.outcome,
            }),
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// `saga-choreo` — the same guarantee driven by events, with no coordinator
// ---------------------------------------------------------------------------------------

/// Every service's reaction rule in one place, because there is only one process here.
struct Choreo {
    names: Vec<String>,
    handled: Vec<String>,
    emitted: Vec<String>,
    ledger: Vec<String>,
    inflight_do: BTreeSet<String>,
    inflight_undo: BTreeSet<String>,
    compensating: bool,
    outcome: &'static str,
}

impl Choreo {
    fn new() -> Choreo {
        Choreo {
            names: Vec::new(),
            handled: Vec::new(),
            emitted: Vec::new(),
            ledger: Vec::new(),
            inflight_do: BTreeSet::new(),
            inflight_undo: BTreeSet::new(),
            compensating: false,
            outcome: "running",
        }
    }

    fn reset(&mut self) {
        self.handled.clear();
        self.emitted.clear();
        self.ledger.clear();
        self.inflight_do.clear();
        self.inflight_undo.clear();
        self.compensating = false;
        self.outcome = "running";
    }

    /// Emit the previous step's `.undo`, or finish the saga when the chain is exhausted.
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

    /// `None` means the event does not apply in the current state and is absorbed.
    fn handle(&mut self, name: &str) -> Option<Vec<String>> {
        if name == "start" {
            if !self.handled.is_empty() {
                return None;
            }
            let first = self.names.first()?.clone();
            self.ledger.push(format!("do:{first}"));
            self.inflight_do.insert(first.clone());
            return Some(vec![format!("{first}.do")]);
        }
        let (step, kind) = name.rsplit_once('.')?;
        let idx = self.names.iter().position(|n| n.as_str() == step)?;
        match kind {
            "ok" => {
                if self.compensating || !self.inflight_do.remove(step) {
                    return None;
                }
                match self.names.get(idx + 1).cloned() {
                    Some(next) => {
                        self.ledger.push(format!("do:{next}"));
                        self.inflight_do.insert(next.clone());
                        Some(vec![format!("{next}.do")])
                    }
                    None => {
                        self.outcome = "committed";
                        Some(Vec::new())
                    }
                }
            }
            "fail" => {
                // A second failure while a chain is already running is absorbed: one saga
                // instance compensates once, however many services shout about it.
                if self.compensating || !self.inflight_do.remove(step) {
                    return None;
                }
                self.compensating = true;
                Some(self.undo_before(idx))
            }
            "undone" => {
                if !self.inflight_undo.remove(step) {
                    return None;
                }
                self.ledger.push(format!("undo:{step}"));
                Some(self.undo_before(idx))
            }
            _ => None,
        }
    }
}

impl Topic for Choreo {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let names = match word_list(args, 0) {
                    Ok(n) if !n.is_empty() => n,
                    Ok(_) => return err("a saga needs at least one step"),
                    Err(e) => return e,
                };
                self.names = names;
                self.reset();
                json!({ "steps": self.names.len(), "names": self.names })
            }
            "event" => {
                let name = match word(args, 0) {
                    Ok(n) => n,
                    Err(e) => return e,
                };
                if self.handled.iter().any(|h| h.as_str() == name) {
                    return json!({
                        "emitted": Vec::<String>::new(),
                        "duplicate": true,
                        "ledger": self.ledger,
                    });
                }
                match self.handle(&name) {
                    Some(out) => {
                        self.handled.push(name);
                        self.emitted.extend(out.iter().cloned());
                        json!({ "emitted": out, "duplicate": false, "ledger": self.ledger })
                    }
                    // An event that does not apply is neither handled nor a duplicate: it is
                    // simply dropped, so the one that legitimately follows still works.
                    None => json!({
                        "emitted": Vec::<String>::new(),
                        "duplicate": false,
                        "ledger": self.ledger,
                    }),
                }
            }
            "ledger" => json!({ "entries": self.ledger }),
            "outcome" => json!({
                "outcome": self.outcome,
                "inflight": self.inflight_do.len() + self.inflight_undo.len(),
            }),
            "state" => json!({
                "handled": self.handled,
                "emitted": self.emitted,
                "outcome": self.outcome,
            }),
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// `outbox` — a row and its event committed together, and a relay that drains the table
// ---------------------------------------------------------------------------------------

/// One outbox record: the event, and whether the relay has already put it on the bus.
struct Rec {
    event: String,
    published: bool,
}

/// What the most recent write did, so a crash can be rewound to the right point.
enum LastWrite {
    Transactional {
        key: String,
        prev: Option<String>,
    },
    Naive {
        key: String,
        prev: Option<String>,
        event: String,
    },
}

/// The store, its outbox table, and the bus everything is published to.
struct Outbox {
    rows: BTreeMap<String, String>,
    outbox: Vec<Rec>,
    published: Vec<String>,
    lost: Vec<String>,
    down: bool,
    last: Option<LastWrite>,
}

impl Outbox {
    fn new() -> Outbox {
        Outbox {
            rows: BTreeMap::new(),
            outbox: Vec::new(),
            published: Vec::new(),
            lost: Vec::new(),
            down: false,
            last: None,
        }
    }

    fn unacked(&self) -> usize {
        self.outbox.iter().filter(|r| r.published).count()
    }

    fn restore(rows: &mut BTreeMap<String, String>, key: &str, prev: Option<String>) {
        match prev {
            Some(v) => {
                rows.insert(key.to_string(), v);
            }
            None => {
                rows.remove(key);
            }
        }
    }

    /// Drop the last occurrence of `event` from the bus, for a publish that never happened.
    fn unpublish(&mut self, event: &str) {
        if let Some(i) = self.published.iter().rposition(|e| e == event) {
            self.published.remove(i);
        }
    }
}

impl Topic for Outbox {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        if self.down && !matches!(command, "recover" | "state" | "delivered") {
            return err("the process is down; recover first");
        }
        match command {
            "init" => {
                *self = Outbox::new();
                json!({ "rows": 0, "outbox": 0, "published": 0 })
            }
            "write" => {
                let (key, value, event) = match (word(args, 0), word(args, 1), word(args, 2)) {
                    (Ok(k), Ok(v), Ok(e)) => (k, v, e),
                    (Err(e), ..) | (_, Err(e), _) | (_, _, Err(e)) => return e,
                };
                // The row and the outbox record are one transaction: both, or neither.
                let prev = self.rows.insert(key.clone(), value);
                self.outbox.push(Rec {
                    event,
                    published: false,
                });
                self.last = Some(LastWrite::Transactional { key, prev });
                json!({ "ok": true, "rows": self.rows.len(), "outbox": self.outbox.len() })
            }
            "naive-write" => {
                let (key, value, event) = match (word(args, 0), word(args, 1), word(args, 2)) {
                    (Ok(k), Ok(v), Ok(e)) => (k, v, e),
                    (Err(e), ..) | (_, Err(e), _) | (_, _, Err(e)) => return e,
                };
                let prev = self.rows.insert(key.clone(), value);
                self.published.push(event.clone());
                self.last = Some(LastWrite::Naive { key, prev, event });
                json!({
                    "ok": true,
                    "rows": self.rows.len(),
                    "published": self.published.len(),
                })
            }
            "crash" => {
                let where_ = match word(args, 0) {
                    Ok(w) => w,
                    Err(e) => return e,
                };
                // Whatever the crash point, the relay's in-memory "I sent it" note goes.
                for r in &mut self.outbox {
                    r.published = false;
                }
                self.down = true;
                let lost = match (where_.as_str(), self.last.take()) {
                    ("now", _) => "the relay's unacknowledged publishes",
                    ("after-row", Some(LastWrite::Naive { event, .. })) => {
                        self.unpublish(&event);
                        self.lost.push(event);
                        "the event, which was never in a transaction"
                    }
                    ("after-row", _) => "nothing: the row and the record committed together",
                    ("before-commit", Some(LastWrite::Naive { key, prev, event })) => {
                        Outbox::restore(&mut self.rows, &key, prev);
                        self.unpublish(&event);
                        "the whole transaction"
                    }
                    ("before-commit", Some(LastWrite::Transactional { key, prev })) => {
                        Outbox::restore(&mut self.rows, &key, prev);
                        self.outbox.pop();
                        "the whole transaction"
                    }
                    ("before-commit", None) => "nothing: no write was in flight",
                    (other, _) => return err(format!("unknown crash point {other:?}")),
                };
                json!({ "ok": true, "lost": lost })
            }
            "recover" => {
                self.down = false;
                json!({
                    "rows": self.rows.len(),
                    "outbox": self.outbox.len(),
                    "published": self.published.len(),
                    "lost": self.lost.len(),
                })
            }
            "publish" => {
                let mut sent = Vec::new();
                for r in &mut self.outbox {
                    if !r.published {
                        r.published = true;
                        sent.push(r.event.clone());
                    }
                }
                self.published.extend(sent.iter().cloned());
                json!({
                    "published": sent,
                    "outbox": self.outbox.len(),
                    "unacked": self.unacked(),
                })
            }
            "ack" => {
                let event = match word(args, 0) {
                    Ok(e) => e,
                    Err(e) => return e,
                };
                if let Some(i) = self
                    .outbox
                    .iter()
                    .position(|r| r.published && r.event == event)
                {
                    self.outbox.remove(i);
                }
                json!({ "outbox": self.outbox.len(), "unacked": self.unacked() })
            }
            "delivered" => {
                let event = match word(args, 0) {
                    Ok(e) => e,
                    Err(e) => return e,
                };
                let count = self.published.iter().filter(|e| **e == event).count();
                json!({ "count": count, "distinct": usize::from(count > 0) })
            }
            "state" => {
                let mut rows = Map::new();
                for (k, v) in &self.rows {
                    rows.insert(k.clone(), Value::String(v.clone()));
                }
                let outbox: Vec<String> = self.outbox.iter().map(|r| r.event.clone()).collect();
                json!({
                    "rows": Value::Object(rows),
                    "outbox": outbox,
                    "published": self.published,
                    "lost": self.lost,
                })
            }
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// `dedup` — an idempotent consumer, and the window that makes it only effectively-once
// ---------------------------------------------------------------------------------------

/// A finite table of message ids, oldest first, and the effect they were applied to.
struct Dedup {
    window: usize,
    ids: Vec<String>,
    total: i64,
    applied: usize,
    /// True once an id that was applied has left the table, by eviction or by hand.
    forgotten: bool,
}

impl Dedup {
    fn new() -> Dedup {
        Dedup {
            window: 0,
            ids: Vec::new(),
            total: 0,
            applied: 0,
            forgotten: false,
        }
    }
}

impl Topic for Dedup {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let window = match uint(args, 0) {
                    Ok(w) => w,
                    Err(e) => return e,
                };
                *self = Dedup::new();
                self.window = window;
                json!({ "window": window, "seen": 0, "total": 0 })
            }
            "deliver" => {
                let (id, amount) = match (word(args, 0), int(args, 1)) {
                    (Ok(i), Ok(a)) => (i, a),
                    (Err(e), _) | (_, Err(e)) => return e,
                };
                if self.ids.contains(&id) {
                    return json!({
                        "applied": false,
                        "duplicate": true,
                        "total": self.total,
                        "seen": self.ids.len(),
                    });
                }
                self.total += amount;
                self.applied += 1;
                self.ids.push(id);
                // The table is a queue of first sightings, so a redelivery never refreshes
                // an entry and the oldest id is always the one that goes.
                if self.window > 0 && self.ids.len() > self.window {
                    self.ids.remove(0);
                    self.forgotten = true;
                }
                json!({
                    "applied": true,
                    "duplicate": false,
                    "total": self.total,
                    "seen": self.ids.len(),
                })
            }
            "forget" => {
                let id = match word(args, 0) {
                    Ok(i) => i,
                    Err(e) => return e,
                };
                let found = self.ids.iter().position(|x| *x == id);
                if let Some(i) = found {
                    self.ids.remove(i);
                    self.forgotten = true;
                }
                json!({ "ok": found.is_some(), "seen": self.ids.len() })
            }
            "guarantee" => {
                if self.forgotten {
                    json!({
                        "exactly_once": false,
                        "reason": "a delivered id has been forgotten",
                    })
                } else {
                    json!({
                        "exactly_once": true,
                        "reason": "every delivered id is still remembered",
                    })
                }
            }
            "state" => json!({
                "total": self.total,
                "seen": self.ids.len(),
                "applied": self.applied,
                "ids": self.ids,
            }),
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// `idempotency-key` — a retry that replays an answer instead of charging again
// ---------------------------------------------------------------------------------------

/// What the server remembers about one idempotency key.
struct KeyRec {
    charge_id: Option<String>,
    amount: i64,
    state: &'static str,
}

/// A toy payment service, with and without keys.
struct Keys {
    balance: i64,
    next_id: usize,
    keys: BTreeMap<String, KeyRec>,
}

impl Keys {
    fn new() -> Keys {
        Keys {
            balance: 0,
            next_id: 0,
            keys: BTreeMap::new(),
        }
    }

    fn mint(&mut self) -> String {
        self.next_id += 1;
        format!("c{}", self.next_id)
    }

    /// A key may only ever be used for the request it was first seen with.
    fn mismatch(&self, key: &str, amount: i64) -> Option<Value> {
        let rec = self.keys.get(key)?;
        if rec.amount == amount {
            return None;
        }
        Some(json!({
            "ok": false,
            "error": format!("key {key} was used for {}, not {amount}", rec.amount),
            "balance": self.balance,
        }))
    }
}

impl Topic for Keys {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                *self = Keys::new();
                json!({ "balance": 0, "charges": 0 })
            }
            "charge" => {
                let (key, amount) = match (word(args, 0), int(args, 1)) {
                    (Ok(k), Ok(a)) => (k, a),
                    (Err(e), _) | (_, Err(e)) => return e,
                };
                if let Some(bad) = self.mismatch(&key, amount) {
                    return bad;
                }
                match self.keys.get(&key) {
                    Some(rec) if rec.state == "in-progress" => {
                        return json!({
                            "ok": false,
                            "error": "in progress",
                            "balance": self.balance,
                        })
                    }
                    // The stored answer is replayed, not recomputed: same charge id, same
                    // balance, no second effect.
                    Some(rec) => {
                        return json!({
                            "ok": true,
                            "charge_id": rec.charge_id,
                            "amount": rec.amount,
                            "replayed": true,
                            "balance": self.balance,
                        })
                    }
                    None => {}
                }
                let id = self.mint();
                self.balance += amount;
                self.keys.insert(
                    key,
                    KeyRec {
                        charge_id: Some(id.clone()),
                        amount,
                        state: "done",
                    },
                );
                json!({
                    "ok": true,
                    "charge_id": id,
                    "amount": amount,
                    "replayed": false,
                    "balance": self.balance,
                })
            }
            "naive-charge" => {
                let amount = match int(args, 0) {
                    Ok(a) => a,
                    Err(e) => return e,
                };
                let id = self.mint();
                self.balance += amount;
                json!({ "charge_id": id, "balance": self.balance })
            }
            "begin" => {
                let (key, amount) = match (word(args, 0), int(args, 1)) {
                    (Ok(k), Ok(a)) => (k, a),
                    (Err(e), _) | (_, Err(e)) => return e,
                };
                if let Some(bad) = self.mismatch(&key, amount) {
                    return bad;
                }
                match self.keys.get(&key) {
                    Some(rec) if rec.state == "in-progress" => json!({
                        "ok": false,
                        "state": "in-progress",
                        "error": "in progress",
                    }),
                    Some(_) => json!({ "ok": true, "state": "done" }),
                    None => {
                        self.keys.insert(
                            key,
                            KeyRec {
                                charge_id: None,
                                amount,
                                state: "in-progress",
                            },
                        );
                        json!({ "ok": true, "state": "in-progress" })
                    }
                }
            }
            "finish" => {
                let key = match word(args, 0) {
                    Ok(k) => k,
                    Err(e) => return e,
                };
                let Some(rec) = self.keys.get(&key) else {
                    return json!({ "ok": false, "error": format!("unknown key {key}") });
                };
                if let Some(id) = rec.charge_id.clone() {
                    return json!({ "ok": true, "charge_id": id, "balance": self.balance });
                }
                let amount = rec.amount;
                let id = self.mint();
                self.balance += amount;
                if let Some(rec) = self.keys.get_mut(&key) {
                    rec.charge_id = Some(id.clone());
                    rec.state = "done";
                }
                json!({ "ok": true, "charge_id": id, "balance": self.balance })
            }
            "lost-answer" => match word(args, 0) {
                // Nothing changes on the server: that is the whole problem the client has.
                Ok(_) => json!({ "ok": true }),
                Err(e) => e,
            },
            "state" => {
                let mut keys = Map::new();
                for (k, rec) in &self.keys {
                    keys.insert(
                        k.clone(),
                        json!({
                            "charge_id": rec.charge_id,
                            "amount": rec.amount,
                            "state": rec.state,
                        }),
                    );
                }
                json!({ "balance": self.balance, "keys": Value::Object(keys) })
            }
            other => err(format!("unknown command {other:?}")),
        }
    }
}
