//! Reference topics: coordination and resilience. See `examples/reference_algorithms.rs`.
//!
//! **Reading this file spoils stages 71 to 77.** It is here so that
//! `disttest --target reference_algorithms --validate` can prove the suite's own
//! expectations, and for no other reason. Nothing is packed, tuned or clever: every topic
//! is the shortest state machine that satisfies the grammar in README.md §3.2, written so
//! that it is obviously right rather than quickly right.
//!
//! The topics here are `fencing`, `leases`, `anti-entropy`, `gossip`, `circuit-breaker`,
//! `hedging` and `bulkhead`. None of them reads a clock: every instant arrives as an
//! argument, so a run is reproducible.

use crate::{err, flag, int, int_list, json_arg, uint, word, word_list, Topic};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Unwrap an argument parser, returning its error object to the caller.
///
/// Seven topics with a dozen commands each is enough repetition that the `match … Err(e) =>
/// return e` spelled out in `raft.rs` stops being readable.
macro_rules! arg {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(e) => return e,
        }
    };
}

/// Build the state machine for one of this file's topics.
pub fn make(topic: &str) -> Option<Box<dyn Topic>> {
    match topic {
        "fencing" => Some(Box::new(Fencing::default())),
        "leases" => Some(Box::new(Leases::default())),
        "anti-entropy" => Some(Box::new(AntiEntropy::default())),
        "gossip" => Some(Box::new(Gossip::default())),
        "circuit-breaker" => Some(Box::new(Breaker::default())),
        "hedging" => Some(Box::new(Hedging::default())),
        "bulkhead" => Some(Box::new(Bulkhead::default())),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------
// `fencing` — a lock that hands out increasing tokens, and a resource that checks them
// ---------------------------------------------------------------------------------------

/// A lock service, and the one resource its holders write to.
#[derive(Default)]
struct Fencing {
    holder: Option<String>,
    /// The last token handed out; it only ever goes up.
    token: i64,
    /// The highest token the resource has accepted a write from.
    fence: i64,
    value: Option<String>,
    writes: Vec<String>,
}

impl Topic for Fencing {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                *self = Fencing::default();
                json!({ "holder": null, "token": 0, "fence": 0, "value": null })
            }
            "acquire" => {
                let client = arg!(word(args, 0));
                if let Some(h) = self.holder.clone() {
                    return json!({ "granted": false, "token": 0, "holder": h });
                }
                self.token += 1;
                self.holder = Some(client.clone());
                json!({ "granted": true, "token": self.token, "holder": client })
            }
            "release" => {
                let client = arg!(word(args, 0));
                let ok = self.holder.as_deref() == Some(client.as_str());
                if ok {
                    self.holder = None;
                }
                json!({ "ok": ok, "holder": self.holder })
            }
            "expire" => {
                let expired = self.holder.take();
                json!({ "expired": expired, "holder": null })
            }
            "write" => {
                let client = arg!(word(args, 0));
                let token = arg!(int(args, 1));
                let value = arg!(word(args, 2));
                // A client that never acquired the lock has nothing to present.
                let reason = if token <= 0 {
                    "no token"
                } else if token < self.fence {
                    "stale token"
                } else {
                    "accepted"
                };
                if reason == "accepted" {
                    self.fence = token;
                    self.value = Some(value.clone());
                    self.writes.push(format!("{client}:{value}"));
                }
                json!({
                    "ok": reason == "accepted",
                    "reason": reason,
                    "value": self.value,
                    "fence": self.fence,
                })
            }
            "write-unfenced" => {
                let client = arg!(word(args, 0));
                let value = arg!(word(args, 1));
                // The same resource with the check taken out: whoever writes last wins,
                // however long ago they thought they held the lock.
                self.value = Some(value.clone());
                self.writes.push(format!("{client}:{value}"));
                json!({ "ok": true, "value": self.value })
            }
            "state" => json!({
                "holder": self.holder,
                "token": self.token,
                "fence": self.fence,
                "value": self.value,
                "writes": self.writes,
            }),
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// `leases` — a leader lease, and what an uncertain clock costs at each end of it
// ---------------------------------------------------------------------------------------

/// A lease granted by one authority, read by nodes whose clocks are not quite right.
#[derive(Default)]
struct Leases {
    lease_ms: i64,
    clock_error_ms: i64,
    holder: Option<String>,
    granted_at: i64,
    expires_at: i64,
    offsets: BTreeMap<String, i64>,
}

impl Leases {
    fn safe_until(&self) -> i64 {
        self.expires_at - self.clock_error_ms
    }

    fn offset(&self, node: &str) -> i64 {
        self.offsets.get(node).copied().unwrap_or(0)
    }

    fn grant(&mut self, node: &str, now: i64) {
        self.holder = Some(node.to_string());
        self.granted_at = now;
        self.expires_at = now + self.lease_ms;
    }
}

impl Topic for Leases {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let lease_ms = arg!(int(args, 0));
                let clock_error_ms = arg!(int(args, 1));
                *self = Leases {
                    lease_ms,
                    clock_error_ms,
                    ..Leases::default()
                };
                json!({ "lease_ms": lease_ms, "clock_error_ms": clock_error_ms })
            }
            "grant" => {
                let node = arg!(word(args, 0));
                let now = arg!(int(args, 1));
                let held_by_other = self.holder.as_deref().is_some_and(|h| h != node.as_str());
                // The granter must wait out the clock error in the *other* direction too:
                // the old holder's clock may be running slow, so it can believe the lease is
                // still live for a whole clock-error past the nominal expiry.
                if held_by_other && now < self.expires_at + self.clock_error_ms {
                    return json!({
                        "ok": false,
                        "holder": self.holder,
                        "expires_at": self.expires_at,
                        "safe_until": self.safe_until(),
                        "error": "the previous lease may still be live",
                    });
                }
                self.grant(&node, now);
                json!({
                    "ok": true,
                    "holder": self.holder,
                    "expires_at": self.expires_at,
                    "safe_until": self.safe_until(),
                })
            }
            "renew" => {
                let node = arg!(word(args, 0));
                let now = arg!(int(args, 1));
                if self.holder.as_deref() != Some(node.as_str()) {
                    return json!({
                        "ok": false,
                        "expires_at": self.expires_at,
                        "safe_until": self.safe_until(),
                    });
                }
                // A renewal extends from now, not from the old expiry: a renewal that was
                // slow to arrive buys no more time than a prompt one.
                self.expires_at = now + self.lease_ms;
                json!({
                    "ok": true,
                    "expires_at": self.expires_at,
                    "safe_until": self.safe_until(),
                })
            }
            "read" => {
                let node = arg!(word(args, 0));
                let now = arg!(int(args, 1));
                if self.holder.as_deref() != Some(node.as_str()) {
                    return json!({ "served": false, "reason": "not the holder" });
                }
                let mine = now + self.offset(&node);
                let reason = if mine >= self.expires_at {
                    "the lease has expired"
                } else if mine >= self.safe_until() {
                    "inside the clock-error margin"
                } else {
                    "holds the lease"
                };
                json!({ "served": reason == "holds the lease", "reason": reason })
            }
            "skew" => {
                let node = arg!(word(args, 0));
                let offset_ms = arg!(int(args, 1));
                self.offsets.insert(node, offset_ms);
                json!({ "ok": true, "offset_ms": offset_ms })
            }
            "holder" => {
                let now = arg!(int(args, 0));
                let live = self.holder.clone().filter(|_| now < self.expires_at);
                json!({ "holder": live, "expires_at": self.expires_at })
            }
            "state" => json!({
                "holder": self.holder,
                "granted_at": self.granted_at,
                "expires_at": self.expires_at,
                "safe_until": self.safe_until(),
                "offsets": self.offsets,
            }),
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// `anti-entropy` — read repair, hinted handoff, and the sync that is the real guarantee
// ---------------------------------------------------------------------------------------

/// One value on one replica.
#[derive(Clone, Default)]
struct Versioned {
    value: String,
    version: i64,
}

/// A hint held on a stand-in for a replica that was not reachable.
#[derive(Clone)]
struct Hint {
    owner: String,
    standin: String,
    key: String,
    value: Versioned,
}

/// A handful of replicas, some of them down, and the repair machinery between them.
#[derive(Default)]
struct AntiEntropy {
    replicas: Vec<String>,
    down: BTreeSet<String>,
    store: BTreeMap<String, BTreeMap<String, Versioned>>,
    hints: Vec<Hint>,
}

impl AntiEntropy {
    fn live(&self) -> Vec<String> {
        self.replicas
            .iter()
            .filter(|r| !self.down.contains(*r))
            .cloned()
            .collect()
    }

    fn get(&self, replica: &str, key: &str) -> Option<&Versioned> {
        self.store.get(replica).and_then(|m| m.get(key))
    }

    /// Write, unless the replica already holds this key at the same version or later.
    fn merge(&mut self, replica: &str, key: &str, v: &Versioned) -> bool {
        let held = self.get(replica, key).map(|x| x.version).unwrap_or(0);
        if held >= v.version {
            return false;
        }
        self.store
            .entry(replica.to_string())
            .or_default()
            .insert(key.to_string(), v.clone());
        true
    }
}

impl Topic for AntiEntropy {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let replicas = arg!(word_list(args, 0));
                *self = AntiEntropy {
                    replicas: replicas.clone(),
                    ..AntiEntropy::default()
                };
                json!({ "replicas": replicas })
            }
            "put" => {
                let replica = arg!(word(args, 0));
                let key = arg!(word(args, 1));
                let value = arg!(word(args, 2));
                let version = arg!(int(args, 3));
                // `put` is the fixture that creates staleness, so it writes whatever it is
                // told; only repairs and handoffs are version-guarded.
                self.store
                    .entry(replica)
                    .or_default()
                    .insert(key, Versioned { value, version });
                json!({ "ok": true, "version": version })
            }
            "down" | "up" => {
                let replica = arg!(word(args, 0));
                if command == "down" {
                    self.down.insert(replica);
                } else {
                    self.down.remove(&replica);
                }
                let down: Vec<String> = self
                    .replicas
                    .iter()
                    .filter(|r| self.down.contains(*r))
                    .cloned()
                    .collect();
                json!({ "up": self.live(), "down": down })
            }
            "write" => {
                let key = arg!(word(args, 0));
                let value = arg!(word(args, 1));
                let version = arg!(int(args, 2));
                let v = Versioned { value, version };
                let live = self.live();
                let mut stored = Vec::new();
                for r in &live {
                    self.merge(r, &key, &v);
                    stored.push(r.clone());
                }
                // The stand-in is always the first live replica in the order `init` gave,
                // so a hint lands somewhere predictable rather than somewhere random.
                let mut hinted = Vec::new();
                if let Some(standin) = live.first().cloned() {
                    let absent: Vec<String> = self
                        .replicas
                        .iter()
                        .filter(|r| self.down.contains(*r))
                        .cloned()
                        .collect();
                    for owner in absent {
                        self.hints.push(Hint {
                            owner: owner.clone(),
                            standin: standin.clone(),
                            key: key.clone(),
                            value: v.clone(),
                        });
                        hinted.push(json!({ "for": owner, "on": standin }));
                    }
                }
                json!({ "stored": stored, "hinted": hinted })
            }
            "read" => {
                let key = arg!(word(args, 0));
                let live = self.live();
                let best = live
                    .iter()
                    .filter_map(|r| self.get(r, &key).cloned())
                    .max_by_key(|v| v.version);
                let Some(best) = best else {
                    return json!({
                        "value": null,
                        "version": 0,
                        "stale": [],
                        "repaired": [],
                    });
                };
                let stale: Vec<String> = live
                    .iter()
                    .filter(|r| self.get(r, &key).map(|v| v.version).unwrap_or(0) < best.version)
                    .cloned()
                    .collect();
                let mut repaired = Vec::new();
                for r in &stale {
                    if self.merge(r, &key, &best) {
                        repaired.push(r.clone());
                    }
                }
                json!({
                    "value": best.value,
                    "version": best.version,
                    "stale": stale,
                    "repaired": repaired,
                })
            }
            "hints" => {
                let replica = arg!(word(args, 0));
                let mut keys: Vec<String> = self
                    .hints
                    .iter()
                    .filter(|h| h.owner == replica)
                    .map(|h| h.key.clone())
                    .collect();
                keys.sort();
                keys.dedup();
                json!({ "keys": keys })
            }
            "handoff" => {
                // A hint can only be handed off by the replica that is holding it. When the
                // stand-in has itself died, the hint is as lost as the write it stood in
                // for, which is why hinted handoff is an optimisation and never a guarantee.
                let deliverable = |h: &Hint, down: &BTreeSet<String>| {
                    !down.contains(&h.owner) && !down.contains(&h.standin)
                };
                let ready: Vec<Hint> = self
                    .hints
                    .iter()
                    .filter(|h| deliverable(h, &self.down))
                    .cloned()
                    .collect();
                for h in &ready {
                    self.merge(&h.owner, &h.key, &h.value);
                }
                let down = self.down.clone();
                self.hints.retain(|h| !deliverable(h, &down));
                json!({ "delivered": ready.len(), "remaining": self.hints.len() })
            }
            "sync" => {
                let a = arg!(word(args, 0));
                let b = arg!(word(args, 1));
                let mut keys: Vec<String> = self
                    .store
                    .get(&a)
                    .into_iter()
                    .chain(self.store.get(&b))
                    .flat_map(|m| m.keys().cloned())
                    .collect();
                keys.sort();
                keys.dedup();
                let mut moved = Vec::new();
                let mut transferred = 0usize;
                for key in keys {
                    let va = self.get(&a, &key).cloned();
                    let vb = self.get(&b, &key).cloned();
                    let best = match (&va, &vb) {
                        (Some(x), Some(y)) => {
                            if x.version >= y.version {
                                x.clone()
                            } else {
                                y.clone()
                            }
                        }
                        (Some(x), None) => x.clone(),
                        (None, Some(y)) => y.clone(),
                        (None, None) => continue,
                    };
                    let mut touched = false;
                    for side in [&a, &b] {
                        if self.merge(side, &key, &best) {
                            transferred += 1;
                            touched = true;
                        }
                    }
                    if touched {
                        moved.push(key);
                    }
                }
                json!({ "transferred": transferred, "keys": moved })
            }
            "state" => {
                let replica = arg!(word(args, 0));
                let mut keys = Map::new();
                if let Some(m) = self.store.get(&replica) {
                    for (k, v) in m {
                        keys.insert(k.clone(), json!({ "value": v.value, "version": v.version }));
                    }
                }
                json!({ "keys": keys })
            }
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// `gossip` — a deterministic dissemination schedule, so rounds and messages are exact
// ---------------------------------------------------------------------------------------

/// `base^exp mod m`, so the round's step never overflows however many rounds are asked for.
fn pow_mod(base: u64, exp: u32, m: u64) -> u64 {
    if m == 0 {
        return 0;
    }
    let (mut result, mut b, mut e) = (1u64 % m, base % m, exp);
    while e > 0 {
        if e & 1 == 1 {
            result = result * b % m;
        }
        b = b * b % m;
        e >>= 1;
    }
    result
}

/// A rumour spreading over `nodes` nodes with a fixed fanout.
#[derive(Default)]
struct Gossip {
    nodes: usize,
    fanout: usize,
    round: u32,
    infected: BTreeSet<usize>,
    messages: u64,
}

impl Gossip {
    /// Run one round; returns how many nodes it newly infected.
    fn one_round(&mut self) -> usize {
        if self.nodes == 0 {
            return 0;
        }
        self.round += 1;
        let step = pow_mod((self.fanout + 1) as u64, self.round - 1, self.nodes as u64);
        let senders: Vec<usize> = self.infected.iter().copied().collect();
        let before = self.infected.len();
        for i in senders {
            for k in 1..=self.fanout {
                let target = (i as u64 + step * k as u64) % self.nodes as u64;
                self.messages += 1;
                self.infected.insert(target as usize);
            }
        }
        self.infected.len() - before
    }

    fn report(&self, new: usize) -> Value {
        json!({
            "round": self.round,
            "infected": self.infected.len(),
            "new": new,
            "messages": self.messages,
        })
    }
}

impl Topic for Gossip {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let nodes = arg!(uint(args, 0));
                let fanout = arg!(uint(args, 1));
                if nodes == 0 {
                    return err("a gossip run needs at least one node");
                }
                *self = Gossip {
                    nodes,
                    fanout,
                    round: 0,
                    infected: BTreeSet::from([0]),
                    messages: 0,
                };
                json!({ "nodes": nodes, "fanout": fanout, "infected": 1, "round": 0 })
            }
            "round" => {
                let new = self.one_round();
                self.report(new)
            }
            "converge" => {
                // A fanout of zero, or a step that lands only on nodes that are already
                // infected, would spin forever; stopping on a round that achieved nothing
                // reports the truth instead.
                while self.infected.len() < self.nodes {
                    if self.one_round() == 0 {
                        break;
                    }
                }
                json!({
                    "rounds": self.round,
                    "messages": self.messages,
                    "infected": self.infected.len(),
                })
            }
            "infected" => {
                let nodes: Vec<usize> = self.infected.iter().copied().collect();
                json!({ "nodes": nodes, "count": nodes.len() })
            }
            "state" => {
                let nodes: Vec<usize> = self.infected.iter().copied().collect();
                json!({
                    "nodes": self.nodes,
                    "fanout": self.fanout,
                    "round": self.round,
                    "infected": nodes,
                    "messages": self.messages,
                })
            }
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// `circuit-breaker` — closed, open, half-open, and the two thresholds between them
// ---------------------------------------------------------------------------------------

/// A circuit breaker whose time always arrives as an argument.
#[derive(Default)]
struct Breaker {
    fail_threshold: i64,
    open_ms: i64,
    success_threshold: i64,
    state: &'static str,
    /// Consecutive failures while closed.
    failures: i64,
    /// Successful trials while half-open.
    successes: i64,
    opened_at: i64,
}

impl Breaker {
    /// Move an open circuit to half-open once its window has passed.
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

    fn retry_at(&self) -> i64 {
        if self.state == "closed" {
            0
        } else {
            self.opened_at + self.open_ms
        }
    }
}

impl Topic for Breaker {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let fail_threshold = arg!(int(args, 0));
                let open_ms = arg!(int(args, 1));
                let success_threshold = arg!(int(args, 2));
                *self = Breaker {
                    fail_threshold,
                    open_ms,
                    success_threshold,
                    state: "closed",
                    failures: 0,
                    successes: 0,
                    opened_at: 0,
                };
                json!({ "state": "closed", "failures": 0, "successes": 0 })
            }
            "call" => {
                let now = arg!(int(args, 0));
                let ok = arg!(flag(args, 1, "ok", "fail"));
                self.advance(now);
                if self.state == "open" {
                    // The rejection never reaches the downstream, so it is not evidence
                    // about it: counting it as a failure would keep the circuit open for
                    // ever out of its own traffic.
                    return json!({
                        "allowed": false,
                        "state": "open",
                        "failures": self.failures,
                        "successes": self.successes,
                        "reason": "rejected while open",
                    });
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
                json!({
                    "allowed": true,
                    "state": self.state,
                    "failures": self.failures,
                    "successes": self.successes,
                    "reason": reason,
                })
            }
            "state" => {
                let now = arg!(int(args, 0));
                self.advance(now);
                json!({
                    "state": self.state,
                    "failures": self.failures,
                    "successes": self.successes,
                    "opened_at": self.opened_at,
                    "retry_at": self.retry_at(),
                })
            }
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// `hedging` — a second copy of a slow request, and what it costs
// ---------------------------------------------------------------------------------------

/// A client that sends a second copy of any request still outstanding after a threshold.
#[derive(Default)]
struct Hedging {
    hedge_after_ms: i64,
    latencies: Vec<i64>,
    messages: u64,
}

impl Hedging {
    /// The nearest-rank percentile of the recorded latencies.
    fn percentile(&self, p: u64) -> i64 {
        if self.latencies.is_empty() {
            return 0;
        }
        let mut sorted = self.latencies.clone();
        sorted.sort_unstable();
        let len = sorted.len() as u64;
        let rank = ((p * len).div_ceil(100)).max(1) as usize;
        sorted[rank.min(sorted.len()) - 1]
    }
}

impl Topic for Hedging {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let hedge_after_ms = arg!(int(args, 0));
                *self = Hedging {
                    hedge_after_ms,
                    ..Hedging::default()
                };
                json!({ "hedge_after_ms": hedge_after_ms, "requests": 0, "messages": 0 })
            }
            "request" => {
                let _id = arg!(word(args, 0));
                let lat = arg!(int_list(args, 1));
                if lat.len() != 2 {
                    return err("latencies must be [primary_ms, hedge_ms]");
                }
                let (primary, hedge) = (lat[0], lat[1]);
                // Anything that answers before the threshold never gets a second copy, so
                // the common, fast case costs exactly what it did before.
                if primary < self.hedge_after_ms {
                    self.messages += 1;
                    self.latencies.push(primary);
                    return json!({
                        "latency": primary,
                        "messages": 1,
                        "winner": "primary",
                        "inflight": 0,
                    });
                }
                self.messages += 2;
                let hedge_done = self.hedge_after_ms + hedge;
                let latency = primary.min(hedge_done);
                self.latencies.push(latency);
                json!({
                    "latency": latency,
                    "messages": 2,
                    "winner": if primary <= hedge_done { "primary" } else { "hedge" },
                    "inflight": 0,
                })
            }
            "plain" => {
                let _id = arg!(word(args, 0));
                let lat = arg!(int_list(args, 1));
                let Some(primary) = lat.first().copied() else {
                    return err("latencies must be [primary_ms, hedge_ms]");
                };
                self.messages += 1;
                self.latencies.push(primary);
                json!({ "latency": primary, "messages": 1 })
            }
            "stats" => {
                let requests = self.latencies.len() as u64;
                let extra = (self.messages.saturating_sub(requests) * 100)
                    .checked_div(requests)
                    .unwrap_or(0);
                json!({
                    "requests": requests,
                    "messages": self.messages,
                    "extra_load_pct": extra,
                    "p50": self.percentile(50),
                    "p99": self.percentile(99),
                    "max": self.latencies.iter().copied().max().unwrap_or(0),
                })
            }
            "state" => json!({
                "hedge_after_ms": self.hedge_after_ms,
                "latencies": self.latencies,
                "messages": self.messages,
            }),
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// `bulkhead` — separate pools, and shedding work nobody is waiting for any more
// ---------------------------------------------------------------------------------------

/// One pool: a concurrency limit and what has happened to it.
#[derive(Default)]
struct Pool {
    limit: usize,
    /// Call id to the instant it was admitted.
    inflight: BTreeMap<String, i64>,
    admitted: u64,
    rejected: u64,
    shed: u64,
}

/// A set of independent pools, or one shared one.
#[derive(Default)]
struct Bulkhead {
    pools: BTreeMap<String, Pool>,
}

impl Bulkhead {
    fn shape(&self) -> Value {
        let mut out = Map::new();
        for (name, p) in &self.pools {
            out.insert(name.clone(), json!({ "limit": p.limit }));
        }
        json!({ "pools": out })
    }

    fn inflight_counts(&self) -> Value {
        let mut out = Map::new();
        for (name, p) in &self.pools {
            out.insert(name.clone(), json!(p.inflight.len()));
        }
        Value::Object(out)
    }
}

impl Topic for Bulkhead {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let spec = arg!(json_arg(args, 0));
                let Some(map) = spec.as_object() else {
                    return err("argument 0 is not an object of pool name to limit");
                };
                let mut pools = BTreeMap::new();
                for (name, limit) in map {
                    let Some(limit) = limit.as_u64() else {
                        return err(format!("pool {name:?} has no whole-number limit"));
                    };
                    pools.insert(
                        name.clone(),
                        Pool {
                            limit: limit as usize,
                            ..Pool::default()
                        },
                    );
                }
                *self = Bulkhead { pools };
                self.shape()
            }
            "init-shared" => {
                let limit = arg!(uint(args, 0));
                *self = Bulkhead {
                    pools: BTreeMap::from([(
                        "shared".to_string(),
                        Pool {
                            limit,
                            ..Pool::default()
                        },
                    )]),
                };
                self.shape()
            }
            "call" => {
                let pool = arg!(word(args, 0));
                let id = arg!(word(args, 1));
                let now = arg!(int(args, 2));
                let Some(p) = self.pools.get_mut(&pool) else {
                    // Inventing a pool on demand would defeat the whole point: a typo would
                    // silently get its own unbounded share of the machine.
                    return json!({
                        "admitted": false,
                        "pool": pool,
                        "inflight": 0,
                        "rejected": 0,
                        "reason": "no such pool",
                    });
                };
                if p.inflight.len() >= p.limit && !p.inflight.contains_key(&id) {
                    p.rejected += 1;
                    return json!({
                        "admitted": false,
                        "pool": pool,
                        "inflight": p.inflight.len(),
                        "rejected": p.rejected,
                        "reason": "pool is full",
                    });
                }
                p.inflight.insert(id, now);
                p.admitted += 1;
                json!({
                    "admitted": true,
                    "pool": pool,
                    "inflight": p.inflight.len(),
                    "rejected": p.rejected,
                    "reason": "admitted",
                })
            }
            "done" => {
                let pool = arg!(word(args, 0));
                let id = arg!(word(args, 1));
                let Some(p) = self.pools.get_mut(&pool) else {
                    return json!({ "ok": false, "inflight": 0 });
                };
                let ok = p.inflight.remove(&id).is_some();
                json!({ "ok": ok, "inflight": p.inflight.len() })
            }
            "shed" => {
                let now = arg!(int(args, 0));
                let deadline_ms = arg!(int(args, 1));
                let mut shed = Vec::new();
                for p in self.pools.values_mut() {
                    let stale: Vec<String> = p
                        .inflight
                        .iter()
                        .filter(|(_, at)| now - **at > deadline_ms)
                        .map(|(id, _)| id.clone())
                        .collect();
                    for id in stale {
                        p.inflight.remove(&id);
                        p.shed += 1;
                        shed.push(id);
                    }
                }
                shed.sort();
                json!({ "shed": shed, "inflight": self.inflight_counts() })
            }
            "stats" => {
                let mut pools = Map::new();
                let mut total_rejected = 0u64;
                for (name, p) in &self.pools {
                    total_rejected += p.rejected;
                    pools.insert(
                        name.clone(),
                        json!({
                            "inflight": p.inflight.len(),
                            "admitted": p.admitted,
                            "rejected": p.rejected,
                            "shed": p.shed,
                        }),
                    );
                }
                json!({ "pools": pools, "total_rejected": total_rejected })
            }
            other => err(format!("unknown command {other:?}")),
        }
    }
}
