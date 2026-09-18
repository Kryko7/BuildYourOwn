//! Reference topics: clocks. See `examples/reference_primitives.rs`.
//!
//! **Reading this file spoils stages 01 to 04.** It is here so that
//! `disttest --target reference_primitives --validate --stage 1` can prove the suite's own
//! expectations, and for no other reason. Nothing is packed, tuned or clever: every topic
//! is the shortest state machine that satisfies the grammar in README.md §2.2, written so
//! that it is obviously right rather than quickly right.
//!
//! Four topics live here: `lamport`, `vector-clock`, `version-vector` and `hlc`.

use crate::{err, int, json_arg, word, Topic};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// A vector of counters, process name to count. A name that is absent counts as zero.
type Clock = BTreeMap<String, i64>;

/// Build the state machine for one of this file's topics.
pub fn make(topic: &str) -> Option<Box<dyn Topic>> {
    match topic {
        "lamport" => Some(Box::new(Lamport::default())),
        "vector-clock" => Some(Box::new(VectorClocks::default())),
        "version-vector" => Some(Box::new(VersionVectors::default())),
        "hlc" => Some(Box::new(Hlc::default())),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------------------

/// A clock as a JSON object.
fn clock_json(c: &Clock) -> Value {
    Value::Object(c.iter().map(|(k, v)| (k.clone(), json!(v))).collect())
}

/// Read a clock out of a JSON argument.
fn clock_arg(args: &[&str], i: usize) -> Result<Clock, Value> {
    let v = json_arg(args, i)?;
    let Some(map) = v.as_object() else {
        return Err(err(format!("argument {i} is not a clock object")));
    };
    let mut out = Clock::new();
    for (name, count) in map {
        match count.as_i64() {
            Some(n) => {
                out.insert(name.clone(), n);
            }
            None => return Err(err(format!("{name} does not hold a whole number"))),
        }
    }
    Ok(out)
}

/// The componentwise maximum of `other` folded into `into`.
fn merge_into(into: &mut Clock, other: &Clock) {
    for (name, count) in other {
        let slot = into.entry(name.clone()).or_insert(0);
        *slot = (*slot).max(*count);
    }
}

/// How `a` relates to `b`, in the words the wire uses.
fn relation(a: &Clock, b: &Clock) -> &'static str {
    let (mut le, mut ge) = (true, true);
    for name in a.keys().chain(b.keys()) {
        let x = a.get(name).copied().unwrap_or(0);
        let y = b.get(name).copied().unwrap_or(0);
        if x > y {
            le = false;
        }
        if x < y {
            ge = false;
        }
    }
    match (le, ge) {
        (true, true) => "equal",
        (true, false) => "before",
        (false, true) => "after",
        (false, false) => "concurrent",
    }
}

// ---------------------------------------------------------------------------------------
// `lamport`
// ---------------------------------------------------------------------------------------

/// One counter per process.
#[derive(Default)]
struct Lamport {
    clocks: Clock,
}

impl Topic for Lamport {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "local" | "send" => {
                let p = match word(args, 0) {
                    Ok(p) => p,
                    Err(e) => return e,
                };
                let slot = self.clocks.entry(p).or_insert(0);
                *slot += 1;
                json!({ "ts": *slot })
            }
            "recv" => {
                let p = match word(args, 0) {
                    Ok(p) => p,
                    Err(e) => return e,
                };
                let ts = match int(args, 1) {
                    Ok(ts) => ts,
                    Err(e) => return e,
                };
                let slot = self.clocks.entry(p).or_insert(0);
                *slot = (*slot).max(ts) + 1;
                json!({ "ts": *slot })
            }
            "get" => {
                let p = match word(args, 0) {
                    Ok(p) => p,
                    Err(e) => return e,
                };
                json!({ "ts": self.clocks.get(&p).copied().unwrap_or(0) })
            }
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// `vector-clock`
// ---------------------------------------------------------------------------------------

/// One vector per process, plus a comparison that belongs to nobody.
#[derive(Default)]
struct VectorClocks {
    clocks: BTreeMap<String, Clock>,
}

impl VectorClocks {
    /// Increment one process's own entry in its own clock and answer with it.
    fn tick(&mut self, p: String) -> Value {
        let vc = self.clocks.entry(p.clone()).or_default();
        *vc.entry(p).or_insert(0) += 1;
        json!({ "vc": clock_json(vc) })
    }
}

impl Topic for VectorClocks {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "local" | "send" => match word(args, 0) {
                Ok(p) => self.tick(p),
                Err(e) => e,
            },
            "recv" => {
                let p = match word(args, 0) {
                    Ok(p) => p,
                    Err(e) => return e,
                };
                let msg = match clock_arg(args, 1) {
                    Ok(m) => m,
                    Err(e) => return e,
                };
                let vc = self.clocks.entry(p.clone()).or_default();
                merge_into(vc, &msg);
                *vc.entry(p).or_insert(0) += 1;
                json!({ "vc": clock_json(vc) })
            }
            "get" => match word(args, 0) {
                Ok(p) => json!({ "vc": clock_json(self.clocks.entry(p).or_default()) }),
                Err(e) => e,
            },
            "cmp" => {
                let a = match clock_arg(args, 0) {
                    Ok(a) => a,
                    Err(e) => return e,
                };
                let b = match clock_arg(args, 1) {
                    Ok(b) => b,
                    Err(e) => return e,
                };
                json!({ "rel": relation(&a, &b) })
            }
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// `version-vector`
// ---------------------------------------------------------------------------------------

/// One value, and the vector of the write that produced it.
#[derive(Clone, Debug)]
struct Sibling {
    value: String,
    vv: Clock,
}

/// A key's siblings, per replica; replicas only learn of each other through `sync`.
#[derive(Default)]
struct VersionVectors {
    replicas: BTreeMap<String, BTreeMap<String, Vec<Sibling>>>,
}

/// Drop every sibling another sibling strictly dominates, then sort and deduplicate.
fn normalize(siblings: &mut Vec<Sibling>) {
    let all = siblings.clone();
    siblings.retain(|s| !all.iter().any(|t| relation(&s.vv, &t.vv) == "before"));
    siblings.sort_by(|a, b| (&a.value, &a.vv).cmp(&(&b.value, &b.vv)));
    siblings.dedup_by(|a, b| a.value == b.value && a.vv == b.vv);
}

impl VersionVectors {
    /// The siblings a replica holds for a key, created empty on first use.
    fn slot(&mut self, replica: &str, key: &str) -> &mut Vec<Sibling> {
        self.replicas
            .entry(replica.to_string())
            .or_default()
            .entry(key.to_string())
            .or_default()
    }

    /// Store one write and answer with the vector it was stamped with.
    fn write(&mut self, replica: &str, key: &str, value: String, vv: Clock) -> Value {
        let siblings = self.slot(replica, key);
        siblings.push(Sibling {
            value,
            vv: vv.clone(),
        });
        normalize(siblings);
        json!({ "vv": clock_json(&vv) })
    }
}

impl Topic for VersionVectors {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            // A write with no context has seen exactly what its own replica holds, so its
            // vector dominates every sibling there and replaces the lot.
            "put" => {
                let (replica, key, value) = match (word(args, 0), word(args, 1), word(args, 2)) {
                    (Ok(r), Ok(k), Ok(v)) => (r, k, v),
                    (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return e,
                };
                let mut vv = Clock::new();
                for s in self.slot(&replica, &key).iter() {
                    merge_into(&mut vv, &s.vv);
                }
                *vv.entry(replica.clone()).or_insert(0) += 1;
                self.write(&replica, &key, value, vv)
            }
            // A write that names its context dominates only what that context covers; a
            // sibling written since then survives.
            "put-ctx" => {
                let (replica, key, value) = match (word(args, 0), word(args, 1), word(args, 2)) {
                    (Ok(r), Ok(k), Ok(v)) => (r, k, v),
                    (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return e,
                };
                let mut vv = match clock_arg(args, 3) {
                    Ok(vv) => vv,
                    Err(e) => return e,
                };
                *vv.entry(replica.clone()).or_insert(0) += 1;
                self.write(&replica, &key, value, vv)
            }
            "read" => {
                let (replica, key) = match (word(args, 0), word(args, 1)) {
                    (Ok(r), Ok(k)) => (r, k),
                    (Err(e), _) | (_, Err(e)) => return e,
                };
                let siblings = self.slot(&replica, &key);
                normalize(siblings);
                let items: Vec<Value> = siblings
                    .iter()
                    .map(|s| json!({ "value": s.value, "vv": clock_json(&s.vv) }))
                    .collect();
                json!({ "siblings": items })
            }
            "sync" => {
                let (a, b) = match (word(args, 0), word(args, 1)) {
                    (Ok(a), Ok(b)) => (a, b),
                    (Err(e), _) | (_, Err(e)) => return e,
                };
                let left = self.replicas.get(&a).cloned().unwrap_or_default();
                let right = self.replicas.get(&b).cloned().unwrap_or_default();
                let mut keys: Vec<String> = left.keys().chain(right.keys()).cloned().collect();
                keys.sort();
                keys.dedup();
                let mut merged: BTreeMap<String, Vec<Sibling>> = BTreeMap::new();
                for key in keys {
                    let mut siblings = left.get(&key).cloned().unwrap_or_default();
                    siblings.extend(right.get(&key).cloned().unwrap_or_default());
                    normalize(&mut siblings);
                    merged.insert(key, siblings);
                }
                self.replicas.insert(a, merged.clone());
                self.replicas.insert(b, merged);
                json!({ "ok": true })
            }
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// `hlc`
// ---------------------------------------------------------------------------------------

/// One `(l, c)` pair per process.
#[derive(Default)]
struct Hlc {
    clocks: BTreeMap<String, (i64, i64)>,
}

impl Topic for Hlc {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "now" => {
                let p = match word(args, 0) {
                    Ok(p) => p,
                    Err(e) => return e,
                };
                let wall = match int(args, 1) {
                    Ok(w) => w,
                    Err(e) => return e,
                };
                let slot = self.clocks.entry(p).or_insert((0, 0));
                let l = slot.0.max(wall);
                slot.1 = if l == slot.0 { slot.1 + 1 } else { 0 };
                slot.0 = l;
                json!({ "l": slot.0, "c": slot.1 })
            }
            "recv" => {
                let p = match word(args, 0) {
                    Ok(p) => p,
                    Err(e) => return e,
                };
                let (wall, lm, cm) = match (int(args, 1), int(args, 2), int(args, 3)) {
                    (Ok(w), Ok(l), Ok(c)) => (w, l, c),
                    (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return e,
                };
                let slot = self.clocks.entry(p).or_insert((0, 0));
                let (lo, co) = *slot;
                let l = lo.max(lm).max(wall);
                let c = if l == lo && l == lm {
                    co.max(cm) + 1
                } else if l == lo {
                    co + 1
                } else if l == lm {
                    cm + 1
                } else {
                    0
                };
                *slot = (l, c);
                json!({ "l": l, "c": c })
            }
            "get" => {
                let p = match word(args, 0) {
                    Ok(p) => p,
                    Err(e) => return e,
                };
                let (l, c) = self.clocks.get(&p).copied().unwrap_or((0, 0));
                json!({ "l": l, "c": c })
            }
            other => err(format!("unknown command {other:?}")),
        }
    }
}
