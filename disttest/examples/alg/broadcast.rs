//! Reference topics: broadcast abstractions and failure detectors. See
//! `examples/reference_algorithms.rs`.
//!
//! **Reading this file spoils stages 83 to 85.**
//!
//! The topics here are `broadcast`, `ordering` and `detector`.

use crate::{err, int, uint, word, Topic};
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// Build the state machine for one of this file's topics.
pub fn make(topic: &str) -> Option<Box<dyn Topic>> {
    match topic {
        "broadcast" => Some(Box::new(Broadcast::default())),
        "ordering" => Some(Box::new(Ordering::default())),
        "detector" => Some(Box::new(Detector::default())),
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
// broadcast — best-effort, reliable and uniform reliable
// ---------------------------------------------------------------------------------------

/// Which of the three guarantees is in force.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    BestEffort,
    Reliable,
    Uniform,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Mode::BestEffort => "best-effort",
            Mode::Reliable => "reliable",
            Mode::Uniform => "uniform",
        }
    }
}

/// One message in flight or delivered.
#[derive(Clone)]
struct Msg {
    id: i64,
    from: usize,
    /// Who has it in hand but has not delivered it yet.
    held: BTreeSet<usize>,
    /// Who has delivered it.
    delivered: BTreeSet<usize>,
}

struct Broadcast {
    n: usize,
    alive: Vec<bool>,
    mode: Mode,
    msgs: Vec<Msg>,
    next_id: i64,
}

impl Default for Broadcast {
    fn default() -> Broadcast {
        Broadcast {
            n: 0,
            alive: Vec::new(),
            mode: Mode::BestEffort,
            msgs: Vec::new(),
            next_id: 1,
        }
    }
}

impl Broadcast {
    fn live(&self) -> Vec<usize> {
        (0..self.n).filter(|i| self.alive[*i]).collect()
    }

    /// Deliver to one process, and under the stronger modes relay to the others.
    fn deliver(&mut self, id: i64, to: usize) {
        let Some(m) = self.msgs.iter_mut().find(|m| m.id == id) else {
            return;
        };
        if m.delivered.contains(&to) || !self.alive[to] {
            return;
        }
        m.delivered.insert(to);
        m.held.remove(&to);
        // Reliable and uniform broadcast are both built by relaying on first delivery: the
        // sender may die mid-send, but any correct process that delivered will pass it on.
        if self.mode != Mode::BestEffort {
            let others: Vec<usize> = (0..self.n).filter(|i| *i != to).collect();
            for o in others {
                if !m.delivered.contains(&o) {
                    m.held.insert(o);
                }
            }
        }
    }

    fn state(&self) -> Value {
        json!({
            "ok": true,
            "mode": self.mode.as_str(),
            "alive": self.live(),
            "messages": self
                .msgs
                .iter()
                .map(|m| json!({
                    "id": m.id,
                    "from": m.from,
                    "held_by": m.held.iter().copied().collect::<Vec<_>>(),
                    "delivered_to": m.delivered.iter().copied().collect::<Vec<_>>(),
                }))
                .collect::<Vec<_>>(),
        })
    }
}

impl Topic for Broadcast {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let n = arg!(uint(args, 0));
                if n == 0 {
                    return err("a group needs at least one process");
                }
                self.n = n;
                self.alive = vec![true; n];
                self.mode = Mode::BestEffort;
                self.msgs.clear();
                self.next_id = 1;
                json!({ "ok": true, "processes": n, "mode": self.mode.as_str() })
            }
            "mode" => {
                let w = arg!(word(args, 0));
                self.mode = match w.as_str() {
                    "best-effort" => Mode::BestEffort,
                    "reliable" => Mode::Reliable,
                    "uniform" => Mode::Uniform,
                    other => return err(format!("unknown mode {other:?}")),
                };
                json!({ "ok": true, "mode": self.mode.as_str() })
            }
            // broadcast <from> — the sender delivers to itself and hands the rest out
            "broadcast" => {
                let from = arg!(uint(args, 0));
                if from >= self.n {
                    return err(format!("no process {from}"));
                }
                if !self.alive[from] {
                    return json!({ "ok": true, "sent": false, "reason": "sender is down" });
                }
                let id = self.next_id;
                self.next_id += 1;
                self.msgs.push(Msg {
                    id,
                    from,
                    held: (0..self.n).filter(|i| *i != from).collect(),
                    delivered: BTreeSet::new(),
                });
                self.deliver(id, from);
                json!({ "ok": true, "sent": true, "id": id })
            }
            // hand <id> <to> — the network delivers one held message to one process
            "hand" => {
                let id = arg!(int(args, 0));
                let to = arg!(uint(args, 1));
                if to >= self.n {
                    return err(format!("no process {to}"));
                }
                let held = self
                    .msgs
                    .iter()
                    .find(|m| m.id == id)
                    .map(|m| m.held.contains(&to))
                    .unwrap_or(false);
                if !held {
                    return json!({ "ok": true, "handed": false, "reason": "nothing held for it" });
                }
                self.deliver(id, to);
                json!({ "ok": true, "handed": true, "id": id, "to": to })
            }
            // drop <id> <to> — the network loses one message
            "drop" => {
                let id = arg!(int(args, 0));
                let to = arg!(uint(args, 1));
                if let Some(m) = self.msgs.iter_mut().find(|m| m.id == id) {
                    m.held.remove(&to);
                }
                json!({ "ok": true, "dropped": id, "to": to })
            }
            "crash" => {
                let i = arg!(uint(args, 0));
                if i >= self.n {
                    return err(format!("no process {i}"));
                }
                self.alive[i] = false;
                // Whatever this process had not managed to send goes with it. Under the
                // stronger modes a copy survives only if some live process already
                // delivered it and can relay — which is the entire difference between
                // best-effort and reliable broadcast.
                let mode = self.mode;
                let alive = self.alive.clone();
                for m in self.msgs.iter_mut().filter(|m| m.from == i) {
                    let survives = match mode {
                        // Nothing outlives the sender.
                        Mode::BestEffort => false,
                        // A *correct* process that delivered will relay it onwards.
                        Mode::Reliable => m.delivered.iter().any(|d| *d != i && alive[*d]),
                        // If anyone delivered it at all — including the process that has
                        // just crashed — every correct process must deliver it too. That
                        // stronger promise is what "uniform" means, and it is what a system
                        // needs when a delivery has side effects the world can see.
                        Mode::Uniform => !m.delivered.is_empty(),
                    };
                    if !survives {
                        m.held.clear();
                    }
                }
                json!({ "ok": true, "crashed": i })
            }
            // settle — hand over everything still held, repeatedly, until nothing moves
            "settle" => {
                let mut rounds = 0;
                loop {
                    let pending: Vec<(i64, usize)> = self
                        .msgs
                        .iter()
                        .flat_map(|m| {
                            m.held
                                .iter()
                                .filter(|to| self.alive[**to])
                                .map(move |to| (m.id, *to))
                                .collect::<Vec<_>>()
                        })
                        .collect();
                    if pending.is_empty() || rounds > self.n * 4 {
                        break;
                    }
                    for (id, to) in pending {
                        self.deliver(id, to);
                    }
                    rounds += 1;
                }
                json!({ "ok": true, "rounds": rounds })
            }
            // delivered <id> — who has it
            "delivered" => {
                let id = arg!(int(args, 0));
                let Some(m) = self.msgs.iter().find(|m| m.id == id) else {
                    return err(format!("no message {id}"));
                };
                let to: Vec<usize> = m.delivered.iter().copied().collect();
                let correct: Vec<usize> = to.iter().copied().filter(|i| self.alive[*i]).collect();
                json!({
                    "ok": true, "id": id, "delivered_to": to,
                    "correct_processes": correct,
                    "all_correct": self.live().iter().all(|i| m.delivered.contains(i)),
                })
            }
            "state" => self.state(),
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// ordering — FIFO, causal and total order on top of a broadcast
// ---------------------------------------------------------------------------------------

/// One broadcast message, with what it needs before it may be delivered.
#[derive(Clone)]
struct Ordered {
    id: i64,
    from: usize,
    /// The sender's sequence number, for FIFO.
    seq: i64,
    /// The sender's vector clock at send time, for causal.
    clock: Vec<i64>,
    /// The order the sequencer gave it, for total order.
    slot: i64,
}

struct Ordering {
    n: usize,
    order: String,
    /// Per process: how many it has delivered from each sender.
    delivered_from: Vec<Vec<i64>>,
    /// Per process: what it has delivered, in order.
    log: Vec<Vec<i64>>,
    /// Per sender: how many it has sent.
    sent: Vec<i64>,
    /// Messages in the network, not yet delivered anywhere.
    queue: Vec<Ordered>,
    next_id: i64,
    next_slot: i64,
}

impl Default for Ordering {
    fn default() -> Ordering {
        Ordering {
            n: 0,
            order: "fifo".into(),
            delivered_from: Vec::new(),
            log: Vec::new(),
            sent: Vec::new(),
            queue: Vec::new(),
            next_id: 1,
            next_slot: 1,
        }
    }
}

impl Ordering {
    /// May `to` deliver `m` now, under the order in force?
    fn deliverable(&self, m: &Ordered, to: usize) -> bool {
        match self.order.as_str() {
            // FIFO: only the next one from that sender.
            "fifo" => self.delivered_from[to][m.from] + 1 == m.seq,
            // Causal: everything the sender had seen must already be here.
            "causal" => {
                if self.delivered_from[to][m.from] + 1 != m.seq {
                    return false;
                }
                (0..self.n).all(|k| k == m.from || self.delivered_from[to][k] >= m.clock[k])
            }
            // Total: the sequencer's slots, in order, the same everywhere.
            "total" => self.log[to].len() as i64 + 1 == m.slot,
            _ => true,
        }
    }
}

impl Topic for Ordering {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let n = arg!(uint(args, 0));
                if n == 0 {
                    return err("a group needs at least one process");
                }
                self.n = n;
                self.delivered_from = vec![vec![0; n]; n];
                self.log = vec![Vec::new(); n];
                self.sent = vec![0; n];
                self.queue.clear();
                self.next_id = 1;
                self.next_slot = 1;
                json!({ "ok": true, "processes": n, "order": self.order })
            }
            "order" => {
                let w = arg!(word(args, 0));
                match w.as_str() {
                    "fifo" | "causal" | "total" => self.order = w.to_string(),
                    other => return err(format!("unknown order {other:?}")),
                }
                json!({ "ok": true, "order": self.order })
            }
            // send <from> — broadcast one message, tagged with what the sender has seen
            "send" => {
                let from = arg!(uint(args, 0));
                if from >= self.n {
                    return err(format!("no process {from}"));
                }
                self.sent[from] += 1;
                let id = self.next_id;
                self.next_id += 1;
                let slot = self.next_slot;
                self.next_slot += 1;
                let m = Ordered {
                    id,
                    from,
                    seq: self.sent[from],
                    clock: self.delivered_from[from].clone(),
                    slot,
                };
                self.queue.push(m);
                json!({ "ok": true, "id": id, "seq": self.sent[from], "slot": slot })
            }
            // offer <id> <to> — the network offers one message to one process
            "offer" => {
                let id = arg!(int(args, 0));
                let to = arg!(uint(args, 1));
                if to >= self.n {
                    return err(format!("no process {to}"));
                }
                let Some(m) = self.queue.iter().find(|m| m.id == id).cloned() else {
                    return err(format!("no message {id}"));
                };
                if self.log[to].contains(&id) {
                    return json!({ "ok": true, "delivered": false, "reason": "already delivered" });
                }
                if !self.deliverable(&m, to) {
                    return json!({
                        "ok": true, "delivered": false, "reason": "held back",
                        "order": self.order,
                    });
                }
                self.delivered_from[to][m.from] = m.seq;
                self.log[to].push(id);
                json!({ "ok": true, "delivered": true, "id": id, "to": to })
            }
            // log <process>
            "log" => {
                let i = arg!(uint(args, 0));
                if i >= self.n {
                    return err(format!("no process {i}"));
                }
                json!({ "ok": true, "process": i, "log": self.log[i].clone() })
            }
            "state" => json!({
                "ok": true,
                "order": self.order,
                "logs": self.log.clone(),
                "queued": self.queue.len(),
            }),
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// detector — the failure-detector classes, by their two properties
// ---------------------------------------------------------------------------------------

/// A detector, judged by completeness and accuracy rather than by how it is built.
#[derive(Default)]
struct Detector {
    n: usize,
    crashed: Vec<bool>,
    /// Per process: who it currently suspects.
    suspects: Vec<BTreeSet<usize>>,
    class: String,
    /// Whether the detector has stabilised, which is what the ◇ in ◇P and ◇S means.
    stable: bool,
}

impl Detector {
    /// Recompute every suspicion from the class's rules.
    fn refresh(&mut self) {
        let crashed: Vec<usize> = (0..self.n).filter(|i| self.crashed[*i]).collect();
        let alive: Vec<usize> = (0..self.n).filter(|i| !self.crashed[*i]).collect();
        for i in 0..self.n {
            let mut s: BTreeSet<usize> = crashed.iter().copied().filter(|c| *c != i).collect();
            match self.class.as_str() {
                // Perfect: everything crashed is suspected, nothing correct ever is.
                "P" => {}
                // Eventually perfect: before stabilising it may suspect a correct process.
                "eventually-perfect" => {
                    if !self.stable {
                        if let Some(first) = alive.iter().find(|a| **a != i) {
                            s.insert(*first);
                        }
                    }
                }
                // Eventually strong: even after stabilising, only *one* correct process is
                // guaranteed never to be suspected — others may be, for ever.
                "eventually-strong" => {
                    // Process 0, when correct, is the one nobody suspects.
                    let trusted = alive.first().copied();
                    for a in &alive {
                        if Some(*a) != trusted && *a != i {
                            s.insert(*a);
                        }
                    }
                    if !self.stable {
                        if let Some(t) = trusted {
                            if t != i {
                                s.insert(t);
                            }
                        }
                    }
                }
                _ => {}
            }
            self.suspects[i] = s;
        }
    }
}

impl Topic for Detector {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let n = arg!(uint(args, 0));
                if n == 0 {
                    return err("a group needs at least one process");
                }
                self.n = n;
                self.crashed = vec![false; n];
                self.suspects = vec![BTreeSet::new(); n];
                self.class = "P".into();
                self.stable = true;
                self.refresh();
                json!({ "ok": true, "processes": n, "class": self.class })
            }
            "class" => {
                let w = arg!(word(args, 0));
                match w.as_str() {
                    "P" | "eventually-perfect" | "eventually-strong" => {
                        self.class = w.to_string();
                        // Anything with a ◇ in it starts unstable.
                        self.stable = self.class == "P";
                    }
                    other => return err(format!("unknown class {other:?}")),
                }
                self.refresh();
                json!({ "ok": true, "class": self.class, "stable": self.stable })
            }
            "crash" => {
                let i = arg!(uint(args, 0));
                if i >= self.n {
                    return err(format!("no process {i}"));
                }
                self.crashed[i] = true;
                self.refresh();
                json!({ "ok": true, "crashed": i })
            }
            // stabilise — time passes and the eventual guarantees take hold
            "stabilise" => {
                self.stable = true;
                self.refresh();
                json!({ "ok": true, "stable": true })
            }
            // suspects <process>
            "suspects" => {
                let i = arg!(uint(args, 0));
                if i >= self.n {
                    return err(format!("no process {i}"));
                }
                json!({
                    "ok": true, "process": i,
                    "suspects": self.suspects[i].iter().copied().collect::<Vec<_>>(),
                })
            }
            // properties — does the current state satisfy completeness and accuracy?
            "properties" => {
                let alive: Vec<usize> = (0..self.n).filter(|i| !self.crashed[*i]).collect();
                let crashed: Vec<usize> = (0..self.n).filter(|i| self.crashed[*i]).collect();
                // Strong completeness: every crashed process is eventually suspected by
                // every correct one.
                let complete = alive
                    .iter()
                    .all(|i| crashed.iter().all(|c| self.suspects[*i].contains(c)));
                // Strong accuracy: no correct process is ever suspected.
                let strong_accuracy = alive.iter().all(|i| {
                    alive
                        .iter()
                        .all(|a| a == i || !self.suspects[*i].contains(a))
                });
                // Weak accuracy: *some* correct process is never suspected by anyone.
                let weak_accuracy = alive.iter().any(|a| {
                    alive
                        .iter()
                        .all(|i| i == a || !self.suspects[*i].contains(a))
                });
                json!({
                    "ok": true,
                    "class": self.class,
                    "stable": self.stable,
                    "strong_completeness": complete,
                    "strong_accuracy": strong_accuracy,
                    "weak_accuracy": weak_accuracy,
                })
            }
            "state" => json!({
                "ok": true, "class": self.class, "stable": self.stable,
                "crashed": (0..self.n).filter(|i| self.crashed[*i]).collect::<Vec<_>>(),
                "suspects": self
                    .suspects
                    .iter()
                    .map(|s| s.iter().copied().collect::<Vec<_>>())
                    .collect::<Vec<_>>(),
            }),
            other => err(format!("unknown command {other:?}")),
        }
    }
}
