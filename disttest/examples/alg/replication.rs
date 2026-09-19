//! Reference topics: replication strategies. See `examples/reference_algorithms.rs`.
//!
//! **Reading this file spoils stages 80 to 82.**
//!
//! The topics here are `abd`, `chain` and `raft-reads`.

use crate::{err, int, uint, word, Topic};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Build the state machine for one of this file's topics.
pub fn make(topic: &str) -> Option<Box<dyn Topic>> {
    match topic {
        "abd" => Some(Box::new(Abd::default())),
        "chain" => Some(Box::new(Chain::default())),
        "raft-reads" => Some(Box::new(RaftReads::default())),
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
// abd — a linearizable register from quorums, with no consensus anywhere
// ---------------------------------------------------------------------------------------

/// One replica's register: a tagged value, ordered by (timestamp, writer).
#[derive(Default, Clone, Copy, PartialEq, Eq)]
struct Tagged {
    ts: i64,
    writer: i64,
    value: i64,
}

impl Tagged {
    fn tag(&self) -> (i64, i64) {
        (self.ts, self.writer)
    }
}

struct Abd {
    replicas: Vec<Tagged>,
    up: Vec<bool>,
    /// Whether a read writes back what it found, which is what makes it linearizable.
    writeback: bool,
    /// Messages sent, so a stage can show the two phases costing two round trips.
    messages: i64,
}

impl Default for Abd {
    fn default() -> Abd {
        Abd {
            replicas: Vec::new(),
            up: Vec::new(),
            writeback: true,
            messages: 0,
        }
    }
}

impl Abd {
    fn quorum(&self) -> usize {
        self.replicas.len() / 2 + 1
    }

    /// The reachable replicas, or None when a majority cannot be reached.
    fn reachable(&self) -> Option<Vec<usize>> {
        let live: Vec<usize> = (0..self.replicas.len()).filter(|i| self.up[*i]).collect();
        (live.len() >= self.quorum()).then_some(live)
    }

    /// Phase one: the highest tagged value a quorum holds.
    fn read_quorum(&mut self) -> Option<Tagged> {
        let live = self.reachable()?;
        let mut best = Tagged::default();
        for i in live.iter().take(self.quorum()) {
            self.messages += 1;
            if self.replicas[*i].tag() > best.tag() {
                best = self.replicas[*i];
            }
        }
        Some(best)
    }

    /// Phase two: put a tagged value on a quorum, never overwriting a higher tag.
    fn write_quorum(&mut self, t: Tagged) -> Option<usize> {
        let live = self.reachable()?;
        let mut written = 0;
        for i in live.iter().take(self.quorum()) {
            self.messages += 1;
            if t.tag() > self.replicas[*i].tag() {
                self.replicas[*i] = t;
            }
            written += 1;
        }
        Some(written)
    }
}

impl Topic for Abd {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let n = arg!(uint(args, 0));
                if n == 0 {
                    return err("a register needs at least one replica");
                }
                self.replicas = vec![Tagged::default(); n];
                self.up = vec![true; n];
                self.writeback = true;
                self.messages = 0;
                json!({ "ok": true, "replicas": n, "quorum": n / 2 + 1 })
            }
            "writeback" => {
                let w = arg!(word(args, 0));
                self.writeback = match w.as_str() {
                    "on" => true,
                    "off" => false,
                    other => return err(format!("expected on or off, got {other:?}")),
                };
                json!({ "ok": true, "writeback": self.writeback })
            }
            "down" | "up" => {
                let i = arg!(uint(args, 0));
                if i >= self.replicas.len() {
                    return err(format!("no replica {i}"));
                }
                self.up[i] = command == "up";
                json!({ "ok": true, "replica": i, "up": self.up[i] })
            }
            // write <writer> <value>
            "write" => {
                let writer = arg!(int(args, 0));
                let value = arg!(int(args, 1));
                let Some(found) = self.read_quorum() else {
                    return json!({ "ok": true, "committed": false, "reason": "no quorum" });
                };
                let t = Tagged {
                    ts: found.ts + 1,
                    writer,
                    value,
                };
                match self.write_quorum(t) {
                    Some(n) => json!({
                        "ok": true, "committed": true, "ts": t.ts, "writer": writer,
                        "replicas_written": n,
                    }),
                    None => json!({ "ok": true, "committed": false, "reason": "no quorum" }),
                }
            }
            "read" => {
                let Some(found) = self.read_quorum() else {
                    return json!({ "ok": true, "served": false, "reason": "no quorum" });
                };
                let mut wrote_back = false;
                if self.writeback {
                    wrote_back = self.write_quorum(found).is_some();
                }
                json!({
                    "ok": true, "served": true, "value": found.value,
                    "ts": found.ts, "writer": found.writer, "wrote_back": wrote_back,
                })
            }
            // Put a tagged value on one replica directly: how a stage arranges a partial write.
            // poke <replica> <ts> <writer> <value>
            "poke" => {
                let i = arg!(uint(args, 0));
                if i >= self.replicas.len() {
                    return err(format!("no replica {i}"));
                }
                let ts = arg!(int(args, 1));
                let writer = arg!(int(args, 2));
                let value = arg!(int(args, 3));
                self.replicas[i] = Tagged { ts, writer, value };
                json!({ "ok": true, "replica": i, "ts": ts, "value": value })
            }
            "state" => json!({
                "ok": true,
                "quorum": self.quorum(),
                "messages": self.messages,
                "writeback": self.writeback,
                "replicas": self
                    .replicas
                    .iter()
                    .enumerate()
                    .map(|(i, r)| json!({
                        "up": self.up[i], "ts": r.ts, "writer": r.writer, "value": r.value
                    }))
                    .collect::<Vec<_>>(),
            }),
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// chain — chain replication: writes at the head, reads at the tail
// ---------------------------------------------------------------------------------------

/// One node of the chain: what it has applied, and what it has not yet passed on.
#[derive(Default, Clone)]
struct Link {
    applied: BTreeMap<String, i64>,
    /// Updates received but not yet forwarded, oldest first.
    pending: Vec<(String, i64)>,
    alive: bool,
}

#[derive(Default)]
struct Chain {
    nodes: Vec<Link>,
}

impl Chain {
    fn live(&self) -> Vec<usize> {
        (0..self.nodes.len())
            .filter(|i| self.nodes[*i].alive)
            .collect()
    }

    fn head(&self) -> Option<usize> {
        self.live().first().copied()
    }

    fn tail(&self) -> Option<usize> {
        self.live().last().copied()
    }

    fn state(&self) -> Value {
        json!({
            "ok": true,
            "head": self.head(),
            "tail": self.tail(),
            "nodes": self
                .nodes
                .iter()
                .map(|n| json!({
                    "alive": n.alive,
                    "pending": n.pending.len(),
                    "applied": n.applied.len(),
                }))
                .collect::<Vec<_>>(),
        })
    }
}

impl Topic for Chain {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let n = arg!(uint(args, 0));
                if n == 0 {
                    return err("a chain needs at least one node");
                }
                self.nodes = vec![
                    Link {
                        alive: true,
                        ..Default::default()
                    };
                    n
                ];
                json!({ "ok": true, "nodes": n, "head": 0, "tail": n - 1 })
            }
            // write <key> <value> — accepted at the head only
            "write" => {
                let key = arg!(word(args, 0));
                let value = arg!(int(args, 1));
                let Some(head) = self.head() else {
                    return json!({ "ok": true, "accepted": false, "reason": "no live node" });
                };
                let node = &mut self.nodes[head];
                node.applied.insert(key.clone(), value);
                node.pending.push((key, value));
                json!({ "ok": true, "accepted": true, "at": head })
            }
            // propagate — every live node hands its pending updates one hop down the chain
            "propagate" => {
                let live = self.live();
                let mut moved = 0;
                // Back to front, so an update that moves into a node during this sweep is
                // not carried onwards by the same sweep: one `propagate` is exactly one hop.
                for w in live.windows(2).rev() {
                    let (from, to) = (w[0], w[1]);
                    let pending = std::mem::take(&mut self.nodes[from].pending);
                    for (key, value) in pending {
                        self.nodes[to].applied.insert(key.clone(), value);
                        self.nodes[to].pending.push((key, value));
                        moved += 1;
                    }
                }
                // Whatever reached the tail is committed, so it forwards nothing on.
                if let Some(tail) = self.tail() {
                    self.nodes[tail].pending.clear();
                }
                json!({ "ok": true, "moved": moved })
            }
            // read <key> — the tail answers, and only the tail
            "read" => {
                let key = arg!(word(args, 0));
                let Some(tail) = self.tail() else {
                    return json!({ "ok": true, "served": false, "reason": "no live node" });
                };
                let value = self.nodes[tail].applied.get(&key).copied().unwrap_or(0);
                json!({ "ok": true, "served": true, "value": value, "from": tail })
            }
            // read-at <node> <key> — reading a middle node, which may be ahead of the tail
            "read-at" => {
                let i = arg!(uint(args, 0));
                let key = arg!(word(args, 1));
                if i >= self.nodes.len() {
                    return err(format!("no node {i}"));
                }
                if !self.nodes[i].alive {
                    return json!({ "ok": true, "served": false, "reason": "node is down" });
                }
                let value = self.nodes[i].applied.get(&key).copied().unwrap_or(0);
                json!({
                    "ok": true, "served": true, "value": value, "from": i,
                    "committed": Some(i) == self.tail(),
                })
            }
            // fail <node> — and the chain closes up around it
            "fail" => {
                let i = arg!(uint(args, 0));
                if i >= self.nodes.len() {
                    return err(format!("no node {i}"));
                }
                let was_head = Some(i) == self.head();
                let was_tail = Some(i) == self.tail();
                let predecessor = self.live().into_iter().rev().find(|j| *j < i);
                let orphaned = std::mem::take(&mut self.nodes[i].pending);
                self.nodes[i].alive = false;
                // Whatever the failed node had taken but not forwarded is recoverable only
                // from the node before it, which still holds it: the predecessor re-sends on
                // the next hop. A failing *head* has no predecessor, so those updates are
                // gone — which is allowed, because no client was ever told they committed.
                if let Some(prev) = predecessor {
                    for (key, value) in orphaned {
                        self.nodes[prev].pending.push((key, value));
                    }
                }
                json!({
                    "ok": true, "failed": i, "was_head": was_head, "was_tail": was_tail,
                    "head": self.head(), "tail": self.tail(),
                })
            }
            "state" => self.state(),
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// raft-reads — pre-vote, ReadIndex and leadership transfer
// ---------------------------------------------------------------------------------------

#[derive(Default)]
struct RaftReads {
    nodes: usize,
    term: i64,
    leader: Option<usize>,
    commit_index: i64,
    applied: i64,
    /// Nodes that cannot hear the leader.
    partitioned: Vec<bool>,
    prevote: bool,
}

impl RaftReads {
    fn quorum(&self) -> usize {
        self.nodes / 2 + 1
    }

    /// How many nodes the leader can still hear from, itself included.
    fn reachable(&self) -> usize {
        (0..self.nodes).filter(|i| !self.partitioned[*i]).count()
    }
}

impl Topic for RaftReads {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let n = arg!(uint(args, 0));
                if n == 0 {
                    return err("a cluster needs at least one node");
                }
                self.nodes = n;
                self.term = 1;
                self.leader = Some(0);
                self.commit_index = 0;
                self.applied = 0;
                self.partitioned = vec![false; n];
                self.prevote = true;
                json!({ "ok": true, "nodes": n, "term": 1, "leader": 0 });
                json!({ "ok": true, "nodes": n, "term": self.term, "leader": self.leader })
            }
            "prevote" => {
                let w = arg!(word(args, 0));
                self.prevote = match w.as_str() {
                    "on" => true,
                    "off" => false,
                    other => return err(format!("expected on or off, got {other:?}")),
                };
                json!({ "ok": true, "prevote": self.prevote })
            }
            // partition <node> / heal <node>
            "partition" | "heal" => {
                let i = arg!(uint(args, 0));
                if i >= self.nodes {
                    return err(format!("no node {i}"));
                }
                self.partitioned[i] = command == "partition";
                json!({ "ok": true, "node": i, "partitioned": self.partitioned[i] })
            }
            // append — the leader commits one entry, if it still has a quorum
            "append" => {
                if self.leader.is_none() || self.reachable() < self.quorum() {
                    return json!({ "ok": true, "committed": false, "reason": "no quorum" });
                }
                self.commit_index += 1;
                json!({ "ok": true, "committed": true, "index": self.commit_index })
            }
            "apply" => {
                self.applied = self.commit_index;
                json!({ "ok": true, "applied": self.applied })
            }
            // campaign <node> — the isolated node tries to become leader
            "campaign" => {
                let i = arg!(uint(args, 0));
                if i >= self.nodes {
                    return err(format!("no node {i}"));
                }
                // A partitioned candidate can reach only itself.
                let votes = if self.partitioned[i] {
                    1
                } else {
                    self.reachable()
                };
                if self.prevote && votes < self.quorum() {
                    // Pre-vote: ask first, and do not raise the term when the answer is no.
                    return json!({
                        "ok": true, "became_leader": false, "term": self.term,
                        "term_raised": false, "reason": "pre-vote failed",
                    });
                }
                self.term += 1;
                let won = votes >= self.quorum();
                if won {
                    self.leader = Some(i);
                }
                json!({
                    "ok": true, "became_leader": won, "term": self.term,
                    "term_raised": true,
                    "reason": if won { "elected" } else { "no majority" },
                })
            }
            // read-index — a linearizable read from the leader
            "read-index" => {
                if self.leader.is_none() {
                    return json!({ "ok": true, "served": false, "reason": "no leader" });
                }
                // Two conditions, and both matter: the leader must still hear a quorum (or
                // it may have been deposed), and it must have applied everything it has
                // committed (or it would answer from a stale state machine).
                if self.reachable() < self.quorum() {
                    return json!({
                        "ok": true, "served": false, "reason": "lost quorum",
                        "read_index": self.commit_index,
                    });
                }
                if self.applied < self.commit_index {
                    return json!({
                        "ok": true, "served": false, "reason": "state machine is behind",
                        "read_index": self.commit_index, "applied": self.applied,
                    });
                }
                json!({
                    "ok": true, "served": true, "read_index": self.commit_index,
                    "applied": self.applied,
                })
            }
            // transfer <to> — leadership transfer, which does not go through an election
            "transfer" => {
                let to = arg!(uint(args, 0));
                if to >= self.nodes {
                    return err(format!("no node {to}"));
                }
                if self.leader.is_none() {
                    return json!({ "ok": true, "transferred": false, "reason": "no leader" });
                }
                if self.partitioned[to] {
                    return json!({
                        "ok": true, "transferred": false, "reason": "target is unreachable",
                    });
                }
                self.term += 1;
                self.leader = Some(to);
                json!({ "ok": true, "transferred": true, "leader": to, "term": self.term })
            }
            "state" => json!({
                "ok": true, "term": self.term, "leader": self.leader,
                "commit_index": self.commit_index, "applied": self.applied,
                "quorum": self.quorum(), "reachable": self.reachable(),
                "prevote": self.prevote,
            }),
            other => err(format!("unknown command {other:?}")),
        }
    }
}
