//! Reference topics: placement — `consistent-hash`, `rendezvous` and `quorum`.
//!
//! **Reading this file spoils stages 05 to 09.** It exists so the suite can check its own
//! expectations against something known to be right, and for nothing else. The hash is a
//! plain FNV-1a with a finalising mix, written out here rather than pulled from a crate, so
//! that every answer in this file is reproducible from the source alone.
//!
//! Nothing here is tuned. The ring is rebuilt from scratch on every membership change and
//! the rendezvous topic scores every node on every lookup, because the suite never asserts
//! anything about speed and a slow implementation that is obviously correct is the one worth
//! validating against.

use crate::{err, int, word, Topic};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// Build the state machine for one of this file's topics.
pub fn make(topic: &str) -> Option<Box<dyn Topic>> {
    match topic {
        "consistent-hash" => Some(Box::new(Ring::default())),
        "rendezvous" => Some(Box::new(Rendezvous::default())),
        "quorum" => Some(Box::new(Quorum::default())),
        _ => None,
    }
}

/// FNV-1a over the bytes, then a splitmix64 finalising mix.
///
/// FNV-1a on a short string barely stirs the low bits, and both topics here care about the
/// low bits: the ring sorts positions and the rendezvous scores are compared whole.
fn hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^= h >> 30;
    h = h.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^ (h >> 31)
}

/// The vnode count `add` uses when the command did not name one.
const DEFAULT_VNODES: i64 = 128;

// ---------------------------------------------------------------------------------------
// consistent-hash
// ---------------------------------------------------------------------------------------

/// A hash ring: every node owns `vnodes` points, and a key belongs to the first point
/// clockwise of it.
#[derive(Default)]
struct Ring {
    /// Node name → how many points it put on the ring.
    nodes: BTreeMap<String, i64>,
    /// Every point, sorted by position then by name so equal positions still order.
    ring: Vec<(u64, String)>,
}

impl Ring {
    /// Rebuild the sorted point list from the membership.
    ///
    /// A point's position depends only on the node name and the point's index, so removing
    /// a node and adding it back with the same count restores the ring exactly — which is
    /// the property stage 06 leans on.
    fn rebuild(&mut self) {
        self.ring.clear();
        for (node, count) in &self.nodes {
            for i in 0..*count {
                self.ring.push((hash(&format!("{node}#{i}")), node.clone()));
            }
        }
        self.ring.sort();
    }

    /// The node owning `key`, or `None` when the ring holds no points at all.
    fn locate(&self, key: &str) -> Option<&str> {
        if self.ring.is_empty() {
            return None;
        }
        let h = hash(key);
        let at = self.ring.partition_point(|(pos, _)| *pos < h);
        let (_, node) = self.ring.get(at).or_else(|| self.ring.first())?;
        Some(node)
    }
}

impl Topic for Ring {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "add" => {
                let node = match word(args, 0) {
                    Ok(n) => n,
                    Err(e) => return e,
                };
                let vnodes = match args.get(1) {
                    Some(_) => match int(args, 1) {
                        Ok(v) => v,
                        Err(e) => return e,
                    },
                    None => DEFAULT_VNODES,
                };
                if vnodes < 1 {
                    return err("a node needs at least one point on the ring");
                }
                self.nodes.insert(node, vnodes);
                self.rebuild();
                json!({ "nodes": self.nodes.len(), "vnodes": self.ring.len() })
            }
            "remove" => {
                let node = match word(args, 0) {
                    Ok(n) => n,
                    Err(e) => return e,
                };
                if self.nodes.remove(&node).is_none() {
                    return err(format!("no such node: {node}"));
                }
                self.rebuild();
                json!({ "nodes": self.nodes.len() })
            }
            "locate" => {
                let key = match word(args, 0) {
                    Ok(k) => k,
                    Err(e) => return e,
                };
                match self.locate(&key) {
                    Some(node) => json!({ "node": node }),
                    None => err("the ring is empty"),
                }
            }
            "stats" => {
                let count = match int(args, 0) {
                    Ok(c) => c,
                    Err(e) => return e,
                };
                if count < 0 {
                    return err("a sample size cannot be negative");
                }
                if self.ring.is_empty() {
                    return err("the ring is empty");
                }
                let mut counts: Map<String, Value> =
                    self.nodes.keys().map(|n| (n.clone(), json!(0))).collect();
                for i in 0..count {
                    let Some(node) = self.locate(&format!("key{i}")) else {
                        return err("the ring is empty");
                    };
                    let seen = counts.get(node).and_then(Value::as_i64).unwrap_or_default();
                    counts.insert(node.to_string(), json!(seen + 1));
                }
                json!({ "nodes": counts })
            }
            other => err(format!("unknown command: {other}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// rendezvous
// ---------------------------------------------------------------------------------------

/// Highest random weight: no ring, no points, just a score per (key, node) pair.
#[derive(Default)]
struct Rendezvous {
    /// The node names, in no particular order until they are scored.
    nodes: Vec<String>,
}

impl Rendezvous {
    /// Every node, best score first. Equal scores break on the name so the order is total.
    fn ranked(&self, key: &str) -> Vec<String> {
        let mut scored: Vec<(u64, &String)> = self
            .nodes
            .iter()
            .map(|n| (hash(&format!("{n}|{key}")), n))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        scored.into_iter().map(|(_, n)| n.clone()).collect()
    }
}

impl Topic for Rendezvous {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "add" => {
                let node = match word(args, 0) {
                    Ok(n) => n,
                    Err(e) => return e,
                };
                if !self.nodes.contains(&node) {
                    self.nodes.push(node);
                }
                json!({ "nodes": self.nodes.len() })
            }
            "remove" => {
                let node = match word(args, 0) {
                    Ok(n) => n,
                    Err(e) => return e,
                };
                let before = self.nodes.len();
                self.nodes.retain(|n| *n != node);
                if self.nodes.len() == before {
                    return err(format!("no such node: {node}"));
                }
                json!({ "nodes": self.nodes.len() })
            }
            "locate" => {
                let key = match word(args, 0) {
                    Ok(k) => k,
                    Err(e) => return e,
                };
                match self.ranked(&key).first() {
                    Some(node) => json!({ "node": node }),
                    None => err("there are no nodes"),
                }
            }
            "locate-k" => {
                let key = match word(args, 0) {
                    Ok(k) => k,
                    Err(e) => return e,
                };
                let k = match int(args, 1) {
                    Ok(k) => k,
                    Err(e) => return e,
                };
                if k < 0 {
                    return err("k cannot be negative");
                }
                let mut ranked = self.ranked(&key);
                // k larger than the membership is not an error: it asks for everybody.
                ranked.truncate(k as usize);
                json!({ "nodes": ranked })
            }
            other => err(format!("unknown command: {other}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// quorum
// ---------------------------------------------------------------------------------------

/// Quorum arithmetic, and the hint store a sloppy quorum needs.
#[derive(Default)]
struct Quorum {
    /// Owner → the keys held for it, each with the stand-in that took the write.
    hints: BTreeMap<String, BTreeMap<String, String>>,
}

impl Quorum {
    /// How many hints are held for one owner.
    fn held(&self, owner: &str) -> usize {
        self.hints.get(owner).map(BTreeMap::len).unwrap_or(0)
    }
}

impl Topic for Quorum {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "check" => {
                let (n, r, w) = match (int(args, 0), int(args, 1), int(args, 2)) {
                    (Ok(n), Ok(r), Ok(w)) => (n, r, w),
                    (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return e,
                };
                if n < 1 || r < 0 || w < 0 {
                    return err("n must be at least one, and r and w cannot be negative");
                }
                json!({
                    "overlap": r + w > n,
                    "write_conflict": 2 * w <= n,
                    "min_r": (n - w + 1).max(1),
                })
            }
            "sloppy" => {
                let (n, w, alive) = match (int(args, 0), int(args, 1), int(args, 2)) {
                    (Ok(n), Ok(w), Ok(a)) => (n, w, a),
                    (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return e,
                };
                if n < 1 || w < 0 || alive < 0 || alive > n {
                    return err("alive must be between zero and n, and w cannot be negative");
                }
                if w > n {
                    // More replicas were demanded than the preference list holds; a
                    // stand-in cannot make up for a number that was never available.
                    return json!({ "accepted": false, "hints": 0, "strict": false });
                }
                let missing = (w - alive).max(0);
                json!({
                    "accepted": true,
                    "hints": missing,
                    "strict": missing == 0,
                })
            }
            "hint" => {
                let (owner, standin, key) = match (word(args, 0), word(args, 1), word(args, 2)) {
                    (Ok(o), Ok(s), Ok(k)) => (o, s, k),
                    (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return e,
                };
                self.hints
                    .entry(owner.clone())
                    .or_default()
                    .insert(key, standin);
                json!({ "hints": self.held(&owner) })
            }
            "handoff" => {
                let owner = match word(args, 0) {
                    Ok(o) => o,
                    Err(e) => return e,
                };
                let delivered = self.hints.remove(&owner).map(|h| h.len()).unwrap_or(0);
                json!({ "delivered": delivered, "hints": 0 })
            }
            "hints" => {
                let owner = match word(args, 0) {
                    Ok(o) => o,
                    Err(e) => return e,
                };
                let keys: Vec<&String> = self
                    .hints
                    .get(&owner)
                    .map(|h| h.keys().collect())
                    .unwrap_or_default();
                json!({ "keys": keys })
            }
            other => err(format!("unknown command: {other}")),
        }
    }
}
