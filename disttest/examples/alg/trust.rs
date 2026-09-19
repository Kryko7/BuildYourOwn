//! Reference topics: Byzantine quorums, clock uncertainty and snapshot isolation. See
//! `examples/reference_algorithms.rs`.
//!
//! **Reading this file spoils stages 86 to 88.**
//!
//! The topics here are `byzantine`, `commit-wait` and `snapshot`.

use crate::{err, int, word, Topic};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Build the state machine for one of this file's topics.
pub fn make(topic: &str) -> Option<Box<dyn Topic>> {
    match topic {
        "byzantine" => Some(Box::new(Byzantine::default())),
        "commit-wait" => Some(Box::new(CommitWait::default())),
        "snapshot" => Some(Box::new(Snapshot::default())),
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
// byzantine — quorum arithmetic when replicas may lie
// ---------------------------------------------------------------------------------------

#[derive(Default)]
struct Byzantine {
    n: i64,
    f: i64,
}

impl Byzantine {
    /// The smallest quorum that is safe against f liars out of n.
    fn quorum(&self) -> i64 {
        if self.f == 0 {
            // Crash faults only: any majority intersects in at least one node.
            self.n / 2 + 1
        } else {
            // Two quorums must share more than f nodes, so at least one shared node is
            // honest. 2q - n > f gives q > (n + f) / 2.
            (self.n + self.f) / 2 + 1
        }
    }

    fn safe(&self) -> bool {
        if self.f == 0 {
            self.n >= 1
        } else {
            // A quorum must be available while f are silent, and two quorums must share an
            // honest node. Both hold exactly when n > 3f.
            self.n > 3 * self.f
        }
    }
}

impl Topic for Byzantine {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            // init <nodes> <faulty>
            "init" => {
                let n = arg!(int(args, 0));
                let f = arg!(int(args, 1));
                if n <= 0 || f < 0 {
                    return err("nodes must be positive and faults must not be negative");
                }
                self.n = n;
                self.f = f;
                json!({
                    "ok": true, "nodes": n, "faults": f,
                    "quorum": self.quorum(), "safe": self.safe(),
                })
            }
            "quorum" => json!({ "ok": true, "quorum": self.quorum() }),
            "safe" => json!({
                "ok": true, "safe": self.safe(),
                "needed": if self.f == 0 { 1 } else { 3 * self.f + 1 },
                "have": self.n,
            }),
            // intersect — how much two quorums must share, and how much of that is honest
            "intersect" => {
                let q = self.quorum();
                let shared = 2 * q - self.n;
                json!({
                    "ok": true, "quorum": q, "shared": shared,
                    "honest_shared": shared - self.f,
                    "enough": shared - self.f >= 1,
                })
            }
            // decide <value>... — the answer a client takes from a set of replies
            "decide" => {
                if args.is_empty() {
                    return err("decide needs at least one reply");
                }
                let mut counts: BTreeMap<String, i64> = BTreeMap::new();
                for a in args {
                    *counts.entry((*a).to_string()).or_insert(0) += 1;
                }
                // A value is only believable once f + 1 replies agree, because f of them
                // could be lies and the remaining one is then honest.
                let needed = self.f + 1;
                let winner = counts
                    .iter()
                    .filter(|(_, c)| **c >= needed)
                    .max_by_key(|(_, c)| **c)
                    .map(|(v, c)| (v.clone(), *c));
                match winner {
                    Some((value, matching)) => json!({
                        "ok": true, "decided": true, "value": value,
                        "matching": matching, "needed": needed,
                    }),
                    None => json!({
                        "ok": true, "decided": false, "needed": needed,
                        "reason": "no value has f + 1 matching replies",
                    }),
                }
            }
            "state" => json!({
                "ok": true, "nodes": self.n, "faults": self.f,
                "quorum": self.quorum(), "safe": self.safe(),
            }),
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// commit-wait — bounded clock uncertainty, and what it buys
// ---------------------------------------------------------------------------------------

/// A transaction that has been given a timestamp.
#[derive(Clone)]
struct Committed {
    name: String,
    ts: i64,
    /// When the commit was actually released to the world.
    released: i64,
}

#[derive(Default)]
struct CommitWait {
    /// Half-width of the uncertainty interval: now is somewhere in [t - e, t + e].
    epsilon: i64,
    /// The true time, which no node can read directly.
    now: i64,
    wait: bool,
    committed: Vec<Committed>,
}

impl CommitWait {
    fn earliest(&self) -> i64 {
        self.now - self.epsilon
    }

    fn latest(&self) -> i64 {
        self.now + self.epsilon
    }
}

impl Topic for CommitWait {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            // init <epsilon>
            "init" => {
                let e = arg!(int(args, 0));
                if e < 0 {
                    return err("uncertainty cannot be negative");
                }
                self.epsilon = e;
                self.now = 100;
                self.wait = true;
                self.committed.clear();
                json!({ "ok": true, "epsilon": e, "commit_wait": true })
            }
            "commit-wait" => {
                let w = arg!(word(args, 0));
                self.wait = match w.as_str() {
                    "on" => true,
                    "off" => false,
                    other => return err(format!("expected on or off, got {other:?}")),
                };
                json!({ "ok": true, "commit_wait": self.wait })
            }
            "tick" => {
                let d = arg!(int(args, 0));
                if d < 0 {
                    return err("time does not run backwards");
                }
                self.now += d;
                json!({ "ok": true, "now": self.now })
            }
            "now" => json!({
                "ok": true, "earliest": self.earliest(), "latest": self.latest(),
                "uncertainty": 2 * self.epsilon,
            }),
            // commit <name> — take a timestamp and, with commit-wait, wait out the uncertainty
            "commit" => {
                let name = arg!(word(args, 0));
                // The timestamp is the latest the clock could be, so it is not in the past
                // for anybody.
                let ts = self.latest();
                let released = if self.wait {
                    // Wait until the *earliest* possible now is past the timestamp: only
                    // then is every clock in the system certainly beyond it.
                    let until = ts + self.epsilon;
                    self.now = self.now.max(until);
                    self.now
                } else {
                    self.now
                };
                self.committed.push(Committed {
                    name: name.to_string(),
                    ts,
                    released,
                });
                json!({
                    "ok": true, "transaction": name, "ts": ts,
                    "released_at": released, "waited": released - (ts - self.epsilon),
                })
            }
            // external-order — do the timestamps agree with the order commits were released?
            "external-order" => {
                let mut violations = Vec::new();
                for (i, a) in self.committed.iter().enumerate() {
                    for b in self.committed.iter().skip(i + 1) {
                        // b started strictly after a was released, so b must have the later
                        // timestamp. Anything else is visible to an outside observer.
                        if b.released >= a.released && b.ts <= a.ts {
                            violations.push(json!({
                                "earlier": a.name, "later": b.name,
                                "earlier_ts": a.ts, "later_ts": b.ts,
                            }));
                        }
                    }
                }
                json!({
                    "ok": true,
                    "holds": violations.is_empty(),
                    "violations": violations,
                })
            }
            "state" => json!({
                "ok": true, "now": self.now, "epsilon": self.epsilon,
                "commit_wait": self.wait,
                "committed": self
                    .committed
                    .iter()
                    .map(|c| json!({ "name": c.name, "ts": c.ts, "released_at": c.released }))
                    .collect::<Vec<_>>(),
            }),
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// snapshot — Percolator-style snapshot isolation over an MVCC store
// ---------------------------------------------------------------------------------------

#[derive(Clone)]
struct Version {
    commit_ts: i64,
    value: i64,
}

#[derive(Clone, Default)]
struct Txn {
    start_ts: i64,
    writes: BTreeMap<String, i64>,
    prewritten: bool,
    committed: Option<i64>,
    aborted: bool,
}

#[derive(Default)]
struct Snapshot {
    clock: i64,
    /// Committed versions per key, oldest first.
    data: BTreeMap<String, Vec<Version>>,
    /// Locks taken at prewrite: key → (owner, start_ts).
    locks: BTreeMap<String, (String, i64)>,
    txns: BTreeMap<String, Txn>,
}

impl Snapshot {
    fn tick(&mut self) -> i64 {
        self.clock += 1;
        self.clock
    }

    /// The value of `key` as of `ts`, ignoring anything committed later.
    fn read_at(&self, key: &str, ts: i64) -> i64 {
        self.data
            .get(key)
            .and_then(|vs| vs.iter().rev().find(|v| v.commit_ts <= ts))
            .map(|v| v.value)
            .unwrap_or(0)
    }
}

impl Topic for Snapshot {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                self.clock = 0;
                self.data.clear();
                self.locks.clear();
                self.txns.clear();
                json!({ "ok": true, "clock": 0 })
            }
            // begin <txn>
            "begin" => {
                let name = arg!(word(args, 0)).to_string();
                if self.txns.contains_key(&name) {
                    return err(format!("transaction {name:?} already exists"));
                }
                let start_ts = self.tick();
                self.txns.insert(
                    name.clone(),
                    Txn {
                        start_ts,
                        ..Default::default()
                    },
                );
                json!({ "ok": true, "transaction": name, "start_ts": start_ts })
            }
            // read <txn> <key> — as of the transaction's start, never later
            "read" => {
                let name = arg!(word(args, 0));
                let key = arg!(word(args, 1));
                let Some(t) = self.txns.get(name.as_str()) else {
                    return err(format!("no transaction {name:?}"));
                };
                // A transaction sees its own writes even before they are committed.
                if let Some(v) = t.writes.get(key.as_str()) {
                    return json!({
                        "ok": true, "value": v, "from": "own writes",
                        "start_ts": t.start_ts,
                    });
                }
                json!({
                    "ok": true, "value": self.read_at(&key, t.start_ts),
                    "from": "snapshot", "start_ts": t.start_ts,
                })
            }
            // write <txn> <key> <value> — buffered until prewrite
            "write" => {
                let name = arg!(word(args, 0));
                let key = arg!(word(args, 1));
                let value = arg!(int(args, 2));
                let Some(t) = self.txns.get_mut(name.as_str()) else {
                    return err(format!("no transaction {name:?}"));
                };
                if t.committed.is_some() || t.aborted {
                    return err("the transaction has already finished");
                }
                t.writes.insert(key.to_string(), value);
                json!({ "ok": true, "buffered": t.writes.len() })
            }
            // prewrite <txn> — take the locks, and find the conflicts
            "prewrite" => {
                let name = arg!(word(args, 0));
                let Some(t) = self.txns.get(name.as_str()).cloned() else {
                    return err(format!("no transaction {name:?}"));
                };
                if t.aborted {
                    return err("the transaction has already aborted");
                }
                for key in t.writes.keys() {
                    // Somebody else holds the lock: a concurrent writer got here first.
                    if let Some((owner, _)) = self.locks.get(key) {
                        if owner != name.as_str() {
                            if let Some(t) = self.txns.get_mut(name.as_str()) {
                                t.aborted = true;
                            }
                            return json!({
                                "ok": true, "prewritten": false, "reason": "locked",
                                "key": key, "owner": owner,
                            });
                        }
                    }
                    // Somebody committed a newer version than this transaction's snapshot:
                    // the write-write conflict snapshot isolation must refuse.
                    let newer = self
                        .data
                        .get(key)
                        .map(|vs| vs.iter().any(|v| v.commit_ts > t.start_ts))
                        .unwrap_or(false);
                    if newer {
                        if let Some(t) = self.txns.get_mut(name.as_str()) {
                            t.aborted = true;
                        }
                        return json!({
                            "ok": true, "prewritten": false, "reason": "write conflict",
                            "key": key,
                        });
                    }
                }
                for key in t.writes.keys() {
                    self.locks
                        .insert(key.clone(), (name.to_string(), t.start_ts));
                }
                if let Some(t) = self.txns.get_mut(name.as_str()) {
                    t.prewritten = true;
                }
                json!({ "ok": true, "prewritten": true, "locks": t.writes.len() })
            }
            // commit <txn>
            "commit" => {
                let name = arg!(word(args, 0));
                let Some(t) = self.txns.get(name.as_str()).cloned() else {
                    return err(format!("no transaction {name:?}"));
                };
                if t.aborted {
                    return json!({ "ok": true, "committed": false, "reason": "aborted" });
                }
                if !t.prewritten {
                    return json!({
                        "ok": true, "committed": false, "reason": "not prewritten",
                    });
                }
                let commit_ts = self.tick();
                for (key, value) in &t.writes {
                    self.data.entry(key.clone()).or_default().push(Version {
                        commit_ts,
                        value: *value,
                    });
                    self.locks.remove(key);
                }
                if let Some(t) = self.txns.get_mut(name.as_str()) {
                    t.committed = Some(commit_ts);
                }
                json!({ "ok": true, "committed": true, "commit_ts": commit_ts })
            }
            // abort <txn> — drop the locks, keep nothing
            "abort" => {
                let name = arg!(word(args, 0));
                let Some(t) = self.txns.get(name.as_str()).cloned() else {
                    return err(format!("no transaction {name:?}"));
                };
                for key in t.writes.keys() {
                    if self.locks.get(key).map(|(o, _)| o.as_str()) == Some(name.as_str()) {
                        self.locks.remove(key);
                    }
                }
                if let Some(t) = self.txns.get_mut(name.as_str()) {
                    t.aborted = true;
                }
                json!({ "ok": true, "aborted": true })
            }
            "state" => json!({
                "ok": true,
                "clock": self.clock,
                "locks": self
                    .locks
                    .iter()
                    .map(|(k, (o, ts))| json!({ "key": k, "owner": o, "start_ts": ts }))
                    .collect::<Vec<_>>(),
                "keys": self
                    .data
                    .iter()
                    .map(|(k, vs)| json!({
                        "key": k,
                        "versions": vs
                            .iter()
                            .map(|v| json!({ "commit_ts": v.commit_ts, "value": v.value }))
                            .collect::<Vec<_>>(),
                    }))
                    .collect::<Vec<_>>(),
            }),
            other => err(format!("unknown command {other:?}")),
        }
    }
}
