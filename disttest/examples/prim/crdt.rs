//! Reference topics: `g-counter`, `pn-counter`, `lww-register`, `or-set` and `rga`.
//!
//! **Reading this file spoils stages 14, 15 and 16.** It is the whole CRDT section
//! written out. It exists for one reason: so that `disttest --target reference_primitives
//! --validate` can prove the suite's own expectations are right. Nothing here is tuned,
//! packed or clever, and the state representations were chosen for how obvious they are
//! rather than how small they are.
//!
//! The `state` / `merge` pair is the only way two replicas exchange anything. `state`
//! answers a JSON object of this file's own shape and `merge <dst> <state-json>` takes one
//! back, so the harness can ship a whole replica to another process without knowing or
//! caring what is inside it — which is exactly why the suite can test a merge it has never
//! seen the representation of. Every merge below is a union or a maximum, because those
//! are the operations that are commutative, associative and idempotent by construction.
//!
//! One process holds one replica. The `<replica>` argument names whose entry a write
//! touches; a second, independent replica is a second process, which is what
//! `Ctx::prim_fresh` starts.

use crate::{err, int, json_arg, word, Topic};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Build the state machine for one of this file's topics.
pub fn make(topic: &str) -> Option<Box<dyn Topic>> {
    match topic {
        "g-counter" => Some(Box::new(Counter::new(false))),
        "pn-counter" => Some(Box::new(Counter::new(true))),
        "lww-register" => Some(Box::new(Lww::default())),
        "or-set" => Some(Box::new(OrSet::default())),
        "rga" => Some(Box::new(Rga::default())),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------
// Shared plumbing
// ---------------------------------------------------------------------------------------

/// A counter map as JSON: replica name → count.
fn counts_to_json(m: &BTreeMap<String, i64>) -> Value {
    Value::Object(m.iter().map(|(k, v)| (k.clone(), json!(v))).collect())
}

/// Read one number out of a state object, accepting the JSON string form too.
fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// Merge a counter map componentwise by maximum.
///
/// The maximum is the whole trick: a replica's entry only ever grows, so a state that
/// arrives late — or twice, or out of order — can never lower what is already there.
fn merge_max(dst: &mut BTreeMap<String, i64>, incoming: Option<&Value>) -> Result<(), Value> {
    let Some(incoming) = incoming else {
        return Ok(()); // a G-Counter state carries no decrement half
    };
    let Some(obj) = incoming.as_object() else {
        return Err(err("a counter half of a state must be an object"));
    };
    for (replica, count) in obj {
        let Some(count) = as_i64(count) else {
            return Err(err(format!("the entry for {replica:?} is not a number")));
        };
        let slot = dst.entry(replica.clone()).or_insert(0);
        *slot = (*slot).max(count);
    }
    Ok(())
}

/// Read a list of strings out of a state object, treating a missing field as empty.
fn string_list(v: Option<&Value>) -> Result<Vec<String>, Value> {
    let Some(v) = v else { return Ok(Vec::new()) };
    let Some(items) = v.as_array() else {
        return Err(err("expected an array of strings in the state"));
    };
    items
        .iter()
        .map(|x| {
            x.as_str()
                .map(str::to_string)
                .ok_or_else(|| err("expected an array of strings in the state"))
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// g-counter and pn-counter
// ---------------------------------------------------------------------------------------

/// A G-Counter, or a PN-Counter when `signed`.
///
/// A PN-Counter is not one signed number; it is two G-Counters, one counting up and one
/// counting down, and the value is their difference. Keeping them apart is what lets a
/// stale state be merged harmlessly: a single signed number has no maximum that means
/// "whichever saw more".
struct Counter {
    signed: bool,
    p: BTreeMap<String, i64>,
    n: BTreeMap<String, i64>,
}

impl Counter {
    /// An empty counter; `signed` turns it into a PN-Counter.
    fn new(signed: bool) -> Counter {
        Counter {
            signed,
            p: BTreeMap::new(),
            n: BTreeMap::new(),
        }
    }

    /// The sum over every replica's increments, less every replica's decrements.
    fn value(&self) -> i64 {
        self.p.values().sum::<i64>() - self.n.values().sum::<i64>()
    }

    /// The shippable state.
    fn state(&self) -> Value {
        if self.signed {
            json!({ "p": counts_to_json(&self.p), "n": counts_to_json(&self.n) })
        } else {
            json!({ "p": counts_to_json(&self.p) })
        }
    }

    fn run(&mut self, command: &str, args: &[&str]) -> Result<Value, Value> {
        match command {
            "inc" | "dec" => {
                if command == "dec" && !self.signed {
                    return Err(err("a g-counter only ever increases"));
                }
                let replica = word(args, 0)?;
                let by = int(args, 1)?;
                if by < 0 {
                    return Err(err("a counter entry may not move backwards"));
                }
                let half = if command == "inc" {
                    &mut self.p
                } else {
                    &mut self.n
                };
                *half.entry(replica).or_insert(0) += by;
                Ok(json!({ "value": self.value() }))
            }
            "value" => {
                word(args, 0)?;
                Ok(json!({ "value": self.value() }))
            }
            "state" => {
                word(args, 0)?;
                Ok(json!({ "state": self.state() }))
            }
            "merge" => {
                word(args, 0)?;
                let incoming = json_arg(args, 1)?;
                merge_max(&mut self.p, incoming.get("p"))?;
                merge_max(&mut self.n, incoming.get("n"))?;
                Ok(json!({ "value": self.value() }))
            }
            other => Err(err(format!("unknown command {other:?}"))),
        }
    }
}

impl Topic for Counter {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        self.run(command, args).unwrap_or_else(|e| e)
    }
}

// ---------------------------------------------------------------------------------------
// lww-register
// ---------------------------------------------------------------------------------------

/// A last-writer-wins register: one value, the timestamp it was written at, and the name
/// of the replica that wrote it.
///
/// The replica name is not decoration. Two replicas that write at the same timestamp must
/// still agree, and the only thing they both know about each other's write is who made it,
/// so the pair `(timestamp, replica)` is what gets compared.
#[derive(Default)]
struct Lww {
    value: Value,
    ts: i64,
    replica: String,
}

impl Lww {
    /// Take the incoming write when it beats what is held, on timestamp then on name.
    fn take(&mut self, value: Value, ts: i64, replica: String) {
        if (ts, replica.as_str()) > (self.ts, self.replica.as_str()) {
            self.value = value;
            self.ts = ts;
            self.replica = replica;
        }
    }

    fn run(&mut self, command: &str, args: &[&str]) -> Result<Value, Value> {
        match command {
            "set" => {
                let replica = word(args, 0)?;
                let value = word(args, 1)?;
                let ts = int(args, 2)?;
                self.take(Value::String(value), ts, replica);
                Ok(json!({ "value": self.value, "ts": self.ts }))
            }
            "value" => {
                word(args, 0)?;
                Ok(json!({ "value": self.value, "ts": self.ts }))
            }
            "state" => {
                word(args, 0)?;
                Ok(json!({ "state": { "v": self.value, "ts": self.ts, "r": self.replica } }))
            }
            "merge" => {
                word(args, 0)?;
                let incoming = json_arg(args, 1)?;
                let value = incoming.get("v").cloned().unwrap_or(Value::Null);
                let ts = incoming.get("ts").and_then(as_i64).unwrap_or(0);
                let replica = incoming
                    .get("r")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                self.take(value, ts, replica);
                Ok(json!({ "value": self.value }))
            }
            other => Err(err(format!("unknown command {other:?}"))),
        }
    }
}

impl Topic for Lww {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        self.run(command, args).unwrap_or_else(|e| e)
    }
}

// ---------------------------------------------------------------------------------------
// or-set
// ---------------------------------------------------------------------------------------

/// An observed-remove set: every add mints a tag, and a remove only takes away the tags it
/// could see.
///
/// A 2P-Set would record "x was removed" and lose an add that happened concurrently, and
/// could never take x back afterwards. Here a remove names tags, so an add it never saw
/// survives it, and a re-add mints a tag nothing has tombstoned.
#[derive(Default)]
struct OrSet {
    seq: u64,
    adds: BTreeMap<String, BTreeSet<String>>,
    removed: BTreeSet<String>,
}

impl OrSet {
    /// The elements with at least one tag nobody has removed, in sorted order.
    fn elements(&self) -> Vec<String> {
        self.adds
            .iter()
            .filter(|(_, tags)| tags.iter().any(|t| !self.removed.contains(t)))
            .map(|(e, _)| e.clone())
            .collect()
    }

    /// The shippable state: the tags of every add, and the tags that have been removed.
    fn state(&self) -> Value {
        let adds: serde_json::Map<String, Value> = self
            .adds
            .iter()
            .map(|(e, tags)| (e.clone(), json!(tags.iter().collect::<Vec<_>>())))
            .collect();
        json!({ "a": adds, "r": self.removed.iter().collect::<Vec<_>>() })
    }

    fn run(&mut self, command: &str, args: &[&str]) -> Result<Value, Value> {
        match command {
            "add" => {
                let replica = word(args, 0)?;
                let element = word(args, 1)?;
                self.seq += 1;
                let tag = format!("{replica}.{}", self.seq);
                self.adds.entry(element).or_default().insert(tag);
                Ok(json!({ "elements": self.elements() }))
            }
            "remove" => {
                word(args, 0)?;
                let element = word(args, 1)?;
                if let Some(tags) = self.adds.get(&element) {
                    for tag in tags {
                        self.removed.insert(tag.clone());
                    }
                }
                Ok(json!({ "elements": self.elements() }))
            }
            "elements" => {
                word(args, 0)?;
                Ok(json!({ "elements": self.elements() }))
            }
            "state" => {
                word(args, 0)?;
                Ok(json!({ "state": self.state() }))
            }
            "merge" => {
                word(args, 0)?;
                let incoming = json_arg(args, 1)?;
                if let Some(adds) = incoming.get("a") {
                    let Some(adds) = adds.as_object() else {
                        return Err(err("the add half of an or-set state must be an object"));
                    };
                    for (element, tags) in adds {
                        let tags = string_list(Some(tags))?;
                        self.adds.entry(element.clone()).or_default().extend(tags);
                    }
                }
                self.removed.extend(string_list(incoming.get("r"))?);
                Ok(json!({ "elements": self.elements() }))
            }
            other => Err(err(format!("unknown command {other:?}"))),
        }
    }
}

impl Topic for OrSet {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        self.run(command, args).unwrap_or_else(|e| e)
    }
}

// ---------------------------------------------------------------------------------------
// rga
// ---------------------------------------------------------------------------------------

/// One element of the sequence.
struct Elem {
    ch: String,
    after: String,
    deleted: bool,
}

/// A replicated growable array: a tree of characters, read out depth first.
///
/// Nothing is ever moved and nothing is ever dropped. An element remembers the id it was
/// inserted after, so the sequence is a walk of that tree; a delete only sets a flag, which
/// is why an insert that arrives later and points at the deleted element still knows where
/// it belongs. Siblings inserted after the same element are ordered by id, largest first,
/// so two replicas that inserted concurrently at the same spot end up with the same string
/// without talking to each other.
#[derive(Default)]
struct Rga {
    elems: BTreeMap<String, Elem>,
}

impl Rga {
    /// The sequence as it reads now, tombstones skipped.
    fn text(&self) -> String {
        let mut out = String::new();
        self.walk("root", &mut out);
        out
    }

    fn walk(&self, parent: &str, out: &mut String) {
        let mut kids: Vec<&str> = self
            .elems
            .iter()
            .filter(|(_, e)| e.after == parent)
            .map(|(id, _)| id.as_str())
            .collect();
        kids.sort_unstable();
        kids.reverse();
        for id in kids {
            if let Some(e) = self.elems.get(id) {
                if !e.deleted {
                    out.push_str(&e.ch);
                }
                self.walk(id, out);
            }
        }
    }

    /// The shippable state: every element, tombstones included.
    fn state(&self) -> Value {
        let elems: serde_json::Map<String, Value> = self
            .elems
            .iter()
            .map(|(id, e)| {
                (
                    id.clone(),
                    json!({ "c": e.ch, "a": e.after, "d": e.deleted }),
                )
            })
            .collect();
        json!({ "e": elems })
    }

    fn run(&mut self, command: &str, args: &[&str]) -> Result<Value, Value> {
        match command {
            "insert" => {
                word(args, 0)?;
                let after = word(args, 1)?;
                let id = word(args, 2)?;
                let ch = word(args, 3)?;
                if id == "root" {
                    return Err(err("'root' is the name of the start of the sequence"));
                }
                if self.elems.contains_key(&id) {
                    return Err(err(format!("there is already an element {id:?}")));
                }
                if after != "root" && !self.elems.contains_key(&after) {
                    return Err(err(format!(
                        "there is no element {after:?} to insert after"
                    )));
                }
                self.elems.insert(
                    id,
                    Elem {
                        ch,
                        after,
                        deleted: false,
                    },
                );
                Ok(json!({ "text": self.text() }))
            }
            "delete" => {
                word(args, 0)?;
                let id = word(args, 1)?;
                match self.elems.get_mut(&id) {
                    Some(e) => e.deleted = true,
                    None => return Err(err(format!("there is no element {id:?}"))),
                }
                Ok(json!({ "text": self.text() }))
            }
            "text" => {
                word(args, 0)?;
                Ok(json!({ "text": self.text() }))
            }
            "state" => {
                word(args, 0)?;
                Ok(json!({ "state": self.state() }))
            }
            "merge" => {
                word(args, 0)?;
                let incoming = json_arg(args, 1)?;
                let Some(elems) = incoming.get("e").and_then(Value::as_object) else {
                    return Err(err("an rga state must carry an object of elements"));
                };
                for (id, e) in elems {
                    let ch = e.get("c").and_then(Value::as_str).unwrap_or_default();
                    let after = e.get("a").and_then(Value::as_str).unwrap_or("root");
                    let deleted = e.get("d").and_then(Value::as_bool).unwrap_or(false);
                    match self.elems.get_mut(id) {
                        // A tombstone is permanent: once anyone has deleted an element,
                        // no state that predates the delete may bring it back.
                        Some(existing) => existing.deleted |= deleted,
                        None => {
                            self.elems.insert(
                                id.clone(),
                                Elem {
                                    ch: ch.to_string(),
                                    after: after.to_string(),
                                    deleted,
                                },
                            );
                        }
                    }
                }
                Ok(json!({ "text": self.text() }))
            }
            other => Err(err(format!("unknown command {other:?}"))),
        }
    }
}

impl Topic for Rga {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        self.run(command, args).unwrap_or_else(|e| e)
    }
}
