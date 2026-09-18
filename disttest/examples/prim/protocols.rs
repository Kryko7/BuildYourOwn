//! Reference topics: `snapshot` (Chandy-Lamport) and `causal-broadcast`.
//!
//! Reading this spoils stages 17 and 18 of ladder A. It is here for one reason: so
//! `disttest --target reference_primitives --validate` can prove the suite's own
//! expectations against something. Nothing is tuned or clever; both topics are the
//! shortest state machine that satisfies the grammar in README.md §2.2.
//!
//! The whole distributed system lives inside one process here. A channel is a FIFO queue
//! the harness drains by hand with `deliver`, which is what makes an interleaving
//! reproducible: no thread ever decides anything.

use crate::{err, int, json_arg, word, Topic};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Build the state machine for one of this file's topics.
pub fn make(topic: &str) -> Option<Box<dyn Topic>> {
    match topic {
        "snapshot" => Some(Box::new(Snapshot::default())),
        "causal-broadcast" => Some(Box::new(CausalBroadcast::default())),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------
// snapshot — Chandy-Lamport
// ---------------------------------------------------------------------------------------

/// What travels down a channel.
enum Item {
    /// An amount of money, which leaves the sender at once and arrives on `deliver`.
    Money(i64),
    /// The marker that ends a channel's recording.
    Marker,
}

/// A directed channel, named by its two ends.
type Link = (String, String);

/// Processes holding money, channels holding messages, and one snapshot in progress.
#[derive(Default)]
struct Snapshot {
    balances: BTreeMap<String, i64>,
    channels: BTreeMap<Link, VecDeque<Item>>,
    started: bool,
    balance_at: BTreeMap<String, i64>,
    in_channel: BTreeMap<Link, i64>,
    closed: BTreeSet<Link>,
}

impl Snapshot {
    /// Record a process's own state, then send a marker down every outgoing channel and
    /// begin recording every incoming one. Doing this only the first time is the rule the
    /// whole algorithm rests on.
    fn record(&mut self, p: &str) {
        if self.balance_at.contains_key(p) {
            return;
        }
        let balance = self.balances.get(p).copied().unwrap_or(0);
        self.balance_at.insert(p.to_string(), balance);
        let outgoing: Vec<Link> = self
            .channels
            .keys()
            .filter(|(from, _)| from == p)
            .cloned()
            .collect();
        for link in outgoing {
            if let Some(queue) = self.channels.get_mut(&link) {
                queue.push_back(Item::Marker);
            }
        }
        let incoming: Vec<Link> = self
            .channels
            .keys()
            .filter(|(_, to)| to == p)
            .cloned()
            .collect();
        for link in incoming {
            if !self.closed.contains(&link) {
                self.in_channel.entry(link).or_insert(0);
            }
        }
    }

    /// How many messages are sitting in channels, markers excluded.
    fn in_flight(&self) -> usize {
        self.channels
            .values()
            .flat_map(|q| q.iter())
            .filter(|i| matches!(i, Item::Money(_)))
            .count()
    }

    fn run(&mut self, command: &str, args: &[&str]) -> Result<Value, Value> {
        match command {
            "init" => {
                let p = word(args, 0)?;
                let balance = int(args, 1)?;
                self.balances.insert(p, balance);
                Ok(json!({ "processes": self.balances.len() }))
            }
            "channel" => {
                let from = word(args, 0)?;
                let to = word(args, 1)?;
                self.channels.entry((from, to)).or_default();
                Ok(json!({ "channels": self.channels.len() }))
            }
            "send" => {
                let from = word(args, 0)?;
                let to = word(args, 1)?;
                let amount = int(args, 2)?;
                if !self.balances.contains_key(&from) {
                    return Err(err(format!("no process {from}")));
                }
                self.channels
                    .entry((from.clone(), to))
                    .or_default()
                    .push_back(Item::Money(amount));
                *self.balances.entry(from).or_insert(0) -= amount;
                Ok(json!({ "in_flight": self.in_flight() }))
            }
            "deliver" => {
                let from = word(args, 0)?;
                let to = word(args, 1)?;
                let link = (from.clone(), to.clone());
                let Some(queue) = self.channels.get_mut(&link) else {
                    return Err(err(format!("no channel {from}->{to}")));
                };
                let Some(item) = queue.pop_front() else {
                    return Err(err(format!("channel {from}->{to} is empty")));
                };
                match item {
                    Item::Marker => {
                        self.record(&to);
                        self.in_channel.entry(link.clone()).or_insert(0);
                        self.closed.insert(link);
                    }
                    Item::Money(amount) => {
                        *self.balances.entry(to.clone()).or_insert(0) += amount;
                        // Recorded only while this channel is between its receiver's own
                        // marker and its own: anything else belongs to a balance instead.
                        if self.started
                            && self.balance_at.contains_key(&to)
                            && !self.closed.contains(&link)
                        {
                            *self.in_channel.entry(link).or_insert(0) += amount;
                        }
                    }
                }
                Ok(json!({ "balance": self.balances.get(&to).copied().unwrap_or(0) }))
            }
            "snapshot" => {
                let p = word(args, 0)?;
                if !self.balances.contains_key(&p) {
                    return Err(err(format!("no process {p}")));
                }
                // A new snapshot supersedes whatever the last one left behind, markers
                // still in the channels included.
                for queue in self.channels.values_mut() {
                    queue.retain(|i| matches!(i, Item::Money(_)));
                }
                self.balance_at.clear();
                self.in_channel.clear();
                self.closed.clear();
                self.started = true;
                self.record(&p);
                Ok(json!({ "started": true }))
            }
            "result" => {
                let complete = self.started
                    && self
                        .balances
                        .keys()
                        .all(|p| self.balance_at.contains_key(p))
                    && self.channels.keys().all(|link| self.closed.contains(link));
                let total: i64 =
                    self.balance_at.values().sum::<i64>() + self.in_channel.values().sum::<i64>();
                let balances: Map<String, Value> = self
                    .balance_at
                    .iter()
                    .map(|(p, b)| (p.clone(), Value::from(*b)))
                    .collect();
                let channels: Map<String, Value> = self
                    .in_channel
                    .iter()
                    .map(|((from, to), amount)| (format!("{from}->{to}"), Value::from(*amount)))
                    .collect();
                Ok(json!({
                    "complete": complete,
                    "total": total,
                    "balances": balances,
                    "channels": channels,
                }))
            }
            other => Err(err(format!("unknown command {other:?}"))),
        }
    }
}

impl Topic for Snapshot {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        self.run(command, args).unwrap_or_else(|e| e)
    }
}

// ---------------------------------------------------------------------------------------
// causal-broadcast
// ---------------------------------------------------------------------------------------

/// A vector clock; a name nobody mentions counts as zero, so zeros are never stored.
type Clock = BTreeMap<String, i64>;

/// One receiver's view: what it has delivered, and what is waiting for a gap to be filled.
#[derive(Default)]
struct View {
    seen: Clock,
    log: Vec<String>,
    buffer: Vec<(String, String, Clock)>,
}

/// Every process's view, kept side by side in one program.
#[derive(Default)]
struct CausalBroadcast {
    views: BTreeMap<String, View>,
}

/// The causal delivery rule: exactly the sender's next message, and nothing the receiver
/// has not already seen.
fn deliverable(seen: &Clock, from: &str, vc: &Clock) -> bool {
    if vc.get(from).copied().unwrap_or(0) != seen.get(from).copied().unwrap_or(0) + 1 {
        return false;
    }
    vc.iter()
        .filter(|(name, _)| name.as_str() != from)
        .all(|(name, count)| *count <= seen.get(name).copied().unwrap_or(0))
}

/// A clock as JSON, without the zero entries.
fn clock_json(c: &Clock) -> Value {
    Value::Object(
        c.iter()
            .filter(|(_, count)| **count != 0)
            .map(|(name, count)| (name.clone(), Value::from(*count)))
            .collect(),
    )
}

/// Read a clock argument, accepting numbers written as JSON strings.
fn clock_from(v: &Value) -> Result<Clock, Value> {
    let Some(map) = v.as_object() else {
        return Err(err("a vector clock must be a JSON object"));
    };
    let mut out = Clock::new();
    for (name, count) in map {
        let n = match count {
            Value::Number(n) => n.as_i64(),
            Value::String(s) => s.parse().ok(),
            _ => None,
        };
        match n {
            Some(0) => {}
            Some(n) => {
                out.insert(name.clone(), n);
            }
            None => return Err(err(format!("{name} is not a whole number"))),
        }
    }
    Ok(out)
}

impl CausalBroadcast {
    fn view(&mut self, p: &str) -> &mut View {
        self.views.entry(p.to_string()).or_default()
    }

    /// Deliver everything the buffer now allows, oldest first, re-scanning after each one
    /// because a delivery can release another.
    fn drain(view: &mut View) -> Vec<String> {
        let mut released = Vec::new();
        loop {
            let next = view
                .buffer
                .iter()
                .position(|(from, _, vc)| deliverable(&view.seen, from, vc));
            let Some(index) = next else { return released };
            let (from, msg, vc) = view.buffer.remove(index);
            *view.seen.entry(from).or_insert(0) += 1;
            for (name, count) in &vc {
                let slot = view.seen.entry(name.clone()).or_insert(0);
                *slot = (*slot).max(*count);
            }
            view.log.push(msg.clone());
            released.push(msg);
        }
    }

    fn run(&mut self, command: &str, args: &[&str]) -> Result<Value, Value> {
        match command {
            "init" => {
                let names = word(args, 0)?;
                for name in names.split(',').filter(|n| !n.is_empty()) {
                    self.views.entry(name.to_string()).or_default();
                }
                Ok(json!({ "processes": self.views.len() }))
            }
            "bcast" => {
                let p = word(args, 0)?;
                let msg = word(args, 1)?;
                let view = self.view(&p);
                *view.seen.entry(p.clone()).or_insert(0) += 1;
                view.log.push(msg);
                Ok(json!({ "vc": clock_json(&view.seen) }))
            }
            "recv" => {
                let p = word(args, 0)?;
                let from = word(args, 1)?;
                let msg = word(args, 2)?;
                let vc = clock_from(&json_arg(args, 3)?)?;
                let view = self.view(&p);
                let known = view.seen.get(&from).copied().unwrap_or(0);
                let already_buffered = view.buffer.iter().any(|(f, m, _)| *f == from && *m == msg);
                // A message the receiver has already delivered, or is already holding, is
                // a duplicate of the network's making and is simply dropped.
                if vc.get(&from).copied().unwrap_or(0) > known && !already_buffered {
                    view.buffer.push((from, msg, vc));
                }
                let delivered = CausalBroadcast::drain(view);
                Ok(json!({ "delivered": delivered, "buffered": view.buffer.len() }))
            }
            "delivered" => {
                let p = word(args, 0)?;
                let view = self.view(&p);
                Ok(json!({ "messages": view.log.clone() }))
            }
            other => Err(err(format!("unknown command {other:?}"))),
        }
    }
}

impl Topic for CausalBroadcast {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        self.run(command, args).unwrap_or_else(|e| e)
    }
}
