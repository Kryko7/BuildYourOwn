//! Reference topics: consistency models and session guarantees. See
//! `examples/reference_algorithms.rs`.
//!
//! **Reading this file spoils stages 78 and 79.** It exists so that
//! `disttest --target reference_algorithms --validate` can prove the suite's own
//! expectations, and for no other reason.
//!
//! The topics here are `consistency` and `session`.

use crate::{err, uint, word, Topic};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Build the state machine for one of this file's topics.
pub fn make(topic: &str) -> Option<Box<dyn Topic>> {
    match topic {
        "consistency" => Some(Box::new(Consistency::default())),
        "session" => Some(Box::new(Session::default())),
        _ => None,
    }
}

macro_rules! arg {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(e) => return e,
        }
    };
}

// ---------------------------------------------------------------------------------------
// consistency — classify a recorded history
// ---------------------------------------------------------------------------------------

/// One recorded operation on a single register.
#[derive(Debug, Clone)]
struct Op {
    client: String,
    write: bool,
    value: i64,
    start: i64,
    end: i64,
}

/// A history of operations on one register, and the two questions worth asking about it.
#[derive(Default)]
struct Consistency {
    ops: Vec<Op>,
}

/// Does this total order of operations read like a register? Every read must return the
/// value of the most recent write before it, and zero when there has been none.
fn register_legal(order: &[&Op]) -> bool {
    let mut current = 0i64;
    for op in order {
        if op.write {
            current = op.value;
        } else if op.value != current {
            return false;
        }
    }
    true
}

/// Every ordering of `ops` that respects `precedes`, tried until one reads like a register.
///
/// Histories here are small on purpose: the search is a plain backtrack over which
/// operation goes next, and "next" means any operation all of whose predecessors are
/// already placed.
fn exists_legal_order(ops: &[Op], precedes: &dyn Fn(&Op, &Op) -> bool) -> bool {
    let n = ops.len();
    let mut placed = vec![false; n];
    let mut order: Vec<&Op> = Vec::with_capacity(n);
    fn go<'a>(
        ops: &'a [Op],
        placed: &mut Vec<bool>,
        order: &mut Vec<&'a Op>,
        precedes: &dyn Fn(&Op, &Op) -> bool,
    ) -> bool {
        if order.len() == ops.len() {
            return register_legal(order);
        }
        for i in 0..ops.len() {
            if placed[i] {
                continue;
            }
            // Anything that must come before ops[i] and is not placed yet blocks it.
            let blocked =
                (0..ops.len()).any(|j| !placed[j] && j != i && precedes(&ops[j], &ops[i]));
            if blocked {
                continue;
            }
            placed[i] = true;
            order.push(&ops[i]);
            if register_legal(order) && go(ops, placed, order, precedes) {
                return true;
            }
            order.pop();
            placed[i] = false;
        }
        false
    }
    go(ops, &mut placed, &mut order, precedes)
}

impl Consistency {
    fn state(&self) -> Value {
        json!({
            "ok": true,
            "ops": self.ops.len(),
            "clients": self
                .ops
                .iter()
                .map(|o| o.client.clone())
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
        })
    }
}

impl Topic for Consistency {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                self.ops.clear();
                json!({ "ok": true, "ops": 0 })
            }
            // op <client> <read|write> <value> <start> <end>
            "op" => {
                let client = arg!(word(args, 0)).to_string();
                let kind = arg!(word(args, 1));
                let write = match kind.as_str() {
                    "write" | "w" => true,
                    "read" | "r" => false,
                    other => return err(format!("kind must be read or write, got {other:?}")),
                };
                let value = arg!(crate::int(args, 2));
                let start = arg!(crate::int(args, 3));
                let end = arg!(crate::int(args, 4));
                if end < start {
                    return err("an operation cannot end before it starts");
                }
                self.ops.push(Op {
                    client,
                    write,
                    value,
                    start,
                    end,
                });
                json!({ "ok": true, "ops": self.ops.len() })
            }
            "check" => {
                let model = arg!(word(args, 0));
                let answer = match model.as_str() {
                    // Linearizable: a total order that respects real time — if a finished
                    // before b started, a comes first — and reads like a register.
                    "linearizable" => exists_legal_order(&self.ops, &|a, b| a.end < b.start),
                    // Sequential: the same, but only each client's own program order has to
                    // be respected. Operations by different clients may be reordered freely,
                    // however far apart in real time they were.
                    "sequential" => exists_legal_order(&self.ops, &|a, b| {
                        a.client == b.client && a.end < b.start
                    }),
                    other => return err(format!("unknown model {other:?}")),
                };
                json!({ "ok": true, "model": model, "holds": answer })
            }
            "state" => self.state(),
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// session — the four session guarantees over lagging replicas
// ---------------------------------------------------------------------------------------

/// A replica holding a prefix of the writes, identified by how many it has applied.
#[derive(Default, Clone)]
struct Replica {
    /// Version applied per key.
    applied: BTreeMap<String, i64>,
    /// The highest global version this replica has seen.
    version: i64,
}

/// What one client session has seen, which is what the guarantees are stated over.
#[derive(Default, Clone)]
struct SessionState {
    /// The highest version this session has written.
    writes: i64,
    /// The highest version this session has read.
    reads: i64,
}

/// A store of a few replicas, with propagation under the caller's control.
struct Session {
    replicas: Vec<Replica>,
    sessions: BTreeMap<String, SessionState>,
    /// The authoritative log: (key, value, version).
    log: Vec<(String, i64, i64)>,
    guarantees: bool,
}

impl Default for Session {
    fn default() -> Session {
        Session {
            replicas: Vec::new(),
            sessions: BTreeMap::new(),
            log: Vec::new(),
            guarantees: true,
        }
    }
}

impl Session {
    /// The version a session must not read below, given the guarantees in force.
    fn floor(&self, client: &str) -> i64 {
        let s = self.sessions.get(client).cloned().unwrap_or_default();
        // read-your-writes needs the session's own writes; monotonic reads needs its own
        // reads. Both together are the highest of the two.
        s.writes.max(s.reads)
    }

    fn state(&self) -> Value {
        json!({
            "ok": true,
            "replicas": self
                .replicas
                .iter()
                .map(|r| json!({ "version": r.version }))
                .collect::<Vec<_>>(),
            "log": self.log.len(),
            "guarantees": self.guarantees,
        })
    }
}

impl Topic for Session {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            // init <replicas>
            "init" => {
                let n = arg!(uint(args, 0));
                if n == 0 {
                    return err("a store needs at least one replica");
                }
                self.replicas = vec![Replica::default(); n];
                self.sessions.clear();
                self.log.clear();
                self.guarantees = true;
                json!({ "ok": true, "replicas": n })
            }
            // guarantees <on|off> — turning them off is what the stage compares against
            "guarantees" => {
                let w = arg!(word(args, 0));
                self.guarantees = match w.as_str() {
                    "on" => true,
                    "off" => false,
                    other => return err(format!("expected on or off, got {other:?}")),
                };
                json!({ "ok": true, "guarantees": self.guarantees })
            }
            // write <client> <replica> <key> <value>
            "write" => {
                let client = arg!(word(args, 0)).to_string();
                let idx = arg!(uint(args, 1));
                let key = arg!(word(args, 2)).to_string();
                let value = arg!(crate::int(args, 3));
                if idx >= self.replicas.len() {
                    return err(format!("no replica {idx}"));
                }
                let version = self.log.len() as i64 + 1;
                self.log.push((key.clone(), value, version));
                let r = &mut self.replicas[idx];
                r.applied.insert(key, version);
                r.version = r.version.max(version);
                let s = self.sessions.entry(client).or_default();
                s.writes = s.writes.max(version);
                json!({ "ok": true, "version": version, "replica": idx })
            }
            // replicate <replica> — apply everything the log has, to that replica
            "replicate" => {
                let idx = arg!(uint(args, 0));
                if idx >= self.replicas.len() {
                    return err(format!("no replica {idx}"));
                }
                let entries: Vec<(String, i64, i64)> = self.log.clone();
                let r = &mut self.replicas[idx];
                for (key, _value, version) in entries {
                    r.applied.insert(key, version);
                    r.version = r.version.max(version);
                }
                json!({ "ok": true, "replica": idx, "version": r.version })
            }
            // read <client> <replica> <key>
            "read" => {
                let client = arg!(word(args, 0)).to_string();
                let idx = arg!(uint(args, 1));
                let key = arg!(word(args, 2)).to_string();
                if idx >= self.replicas.len() {
                    return err(format!("no replica {idx}"));
                }
                let floor = if self.guarantees {
                    self.floor(&client)
                } else {
                    0
                };
                let replica_version = self.replicas[idx].version;
                if replica_version < floor {
                    // The guarantee is not "return something stale": it is "do not serve a
                    // read that would go backwards". Refusing is the honest answer, and a
                    // real system would retry elsewhere or wait.
                    return json!({
                        "ok": true,
                        "served": false,
                        "reason": "replica is behind this session",
                        "replica_version": replica_version,
                        "session_floor": floor,
                    });
                }
                let version = self.replicas[idx].applied.get(&key).copied().unwrap_or(0);
                let value = self
                    .log
                    .iter()
                    .find(|(k, _, v)| *k == key && *v == version)
                    .map(|(_, val, _)| *val)
                    .unwrap_or(0);
                let s = self.sessions.entry(client).or_default();
                s.reads = s.reads.max(replica_version);
                json!({
                    "ok": true,
                    "served": true,
                    "value": value,
                    "version": version,
                    "replica_version": replica_version,
                })
            }
            "state" => self.state(),
            other => err(format!("unknown command {other:?}")),
        }
    }
}
