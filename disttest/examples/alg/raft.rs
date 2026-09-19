//! Reference topics: Raft. See `examples/reference_algorithms.rs`.
//!
//! **Reading this file spoils stages 56 to 60.** It is here so that
//! `disttest --target reference_algorithms --validate` can prove the suite's own
//! expectations, and for no other reason. Nothing is packed, tuned or clever: every topic
//! is the shortest state machine that satisfies the grammar in README.md §3.2, written so
//! that it is obviously right rather than quickly right.
//!
//! The topics here are `raft-election`, `raft-log`, `raft-commit`, `raft-snapshot` and
//! `raft-membership`.

use crate::{err, int, int_list, uint, word, word_list, Topic};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Build the state machine for one of this file's topics.
pub fn make(topic: &str) -> Option<Box<dyn Topic>> {
    match topic {
        "raft-election" => Some(Box::new(Election::default())),
        "raft-log" => Some(Box::new(LogReplication::default())),
        "raft-commit" => Some(Box::new(Commitment::default())),
        "raft-snapshot" => Some(Box::new(Snapshots::default())),
        "raft-membership" => Some(Box::new(Membership::default())),
        _ => None,
    }
}

/// Take one parsed argument, or answer with the error that explains which was wrong.
macro_rules! arg {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(e) => return e,
        }
    };
}

// ---------------------------------------------------------------------------------------
// `raft-election` — one node's election state machine (Raft §5.1, §5.2, §5.4.1)
// ---------------------------------------------------------------------------------------

/// The three roles, spelled the way the wire spells them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Role {
    #[default]
    Follower,
    Candidate,
    Leader,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::Follower => "follower",
            Role::Candidate => "candidate",
            Role::Leader => "leader",
        }
    }
}

/// One node of a cluster of `members`, driven entirely by events.
#[derive(Default)]
struct Election {
    members: usize,
    term: i64,
    role: Role,
    voted_for: Option<String>,
    leader: Option<String>,
    /// Who has granted this node a vote in the current term; a repeat never counts twice.
    granters: BTreeSet<String>,
    /// The term of each of this node's own log entries, index 1 upwards.
    log: Vec<i64>,
}

impl Election {
    fn majority(&self) -> usize {
        self.members / 2 + 1
    }

    fn last_index(&self) -> i64 {
        self.log.len() as i64
    }

    fn last_term(&self) -> i64 {
        self.log.last().copied().unwrap_or(0)
    }

    /// Step down into a new, higher term: Raft's one universal rule.
    fn step_down(&mut self, term: i64) {
        self.term = term;
        self.role = Role::Follower;
        self.voted_for = None;
        self.leader = None;
        self.granters.clear();
    }

    /// Is a candidate's log at least as up to date as ours? (§5.4.1.)
    fn up_to_date(&self, last_index: i64, last_term: i64) -> bool {
        match last_term.cmp(&self.last_term()) {
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Less => false,
            std::cmp::Ordering::Equal => last_index >= self.last_index(),
        }
    }

    fn state(&self) -> Value {
        json!({
            "state": self.role.as_str(),
            "term": self.term,
            "voted_for": self.voted_for,
            "votes": self.granters.len(),
            "leader": self.leader,
            "last_index": self.last_index(),
            "last_term": self.last_term(),
        })
    }
}

impl Topic for Election {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let n = match uint(args, 0) {
                    Ok(n) if n >= 1 => n,
                    Ok(_) => return err("a cluster needs at least one member"),
                    Err(e) => return e,
                };
                *self = Election {
                    members: n,
                    ..Election::default()
                };
                json!({ "ok": true, "members": n, "majority": self.majority() })
            }
            "state" => self.state(),
            "log-append" => {
                let term = match int(args, 0) {
                    Ok(t) => t,
                    Err(e) => return e,
                };
                self.log.push(term);
                json!({ "last_index": self.last_index(), "last_term": self.last_term() })
            }
            "timeout" => {
                // A leader has no election timeout: it is already sending heartbeats, and
                // starting an election of its own would only churn the term.
                if self.role == Role::Leader {
                    return self.state();
                }
                self.term += 1;
                self.role = Role::Candidate;
                self.voted_for = Some("self".to_string());
                self.leader = None;
                self.granters.clear();
                self.granters.insert("self".to_string());
                self.state()
            }
            "request-vote" => {
                let (term, candidate, last_index, last_term) =
                    match (int(args, 0), word(args, 1), int(args, 2), int(args, 3)) {
                        (Ok(t), Ok(c), Ok(li), Ok(lt)) => (t, c, li, lt),
                        (Err(e), ..) | (_, Err(e), ..) | (_, _, Err(e), _) | (_, _, _, Err(e)) => {
                            return e
                        }
                    };
                if term < self.term {
                    return json!({
                        "term": self.term,
                        "granted": false,
                        "state": self.role.as_str(),
                    });
                }
                if term > self.term {
                    self.step_down(term);
                }
                let free = match &self.voted_for {
                    None => true,
                    Some(who) => *who == candidate,
                };
                let granted = free && self.up_to_date(last_index, last_term);
                if granted {
                    self.voted_for = Some(candidate);
                }
                json!({
                    "term": self.term,
                    "granted": granted,
                    "state": self.role.as_str(),
                })
            }
            "vote-response" => {
                let (term, voter, granted) = match (
                    int(args, 0),
                    word(args, 1),
                    crate::flag(args, 2, "true", "false"),
                ) {
                    (Ok(t), Ok(v), Ok(g)) => (t, v, g),
                    (Err(e), ..) | (_, Err(e), _) | (_, _, Err(e)) => return e,
                };
                if term > self.term {
                    self.step_down(term);
                    return self.state();
                }
                // A vote from an older term answers a question this node no longer asks.
                if term == self.term && self.role == Role::Candidate && granted {
                    self.granters.insert(voter);
                    if self.granters.len() >= self.majority() {
                        self.role = Role::Leader;
                        self.leader = Some("self".to_string());
                    }
                }
                self.state()
            }
            "append-entries" => {
                let (term, leader) = match (int(args, 0), word(args, 1)) {
                    (Ok(t), Ok(l)) => (t, l),
                    (Err(e), _) | (_, Err(e)) => return e,
                };
                if term < self.term {
                    return json!({
                        "term": self.term,
                        "success": false,
                        "state": self.role.as_str(),
                    });
                }
                if term > self.term {
                    self.step_down(term);
                } else {
                    self.role = Role::Follower;
                }
                self.leader = Some(leader);
                json!({ "term": self.term, "success": true, "state": "follower" })
            }
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// `raft-log` — the follower half of AppendEntries (Raft §5.3)
// ---------------------------------------------------------------------------------------

/// A follower's log: one term per entry, index 1 upwards.
#[derive(Default)]
struct LogReplication {
    term: i64,
    log: Vec<i64>,
    commit_index: i64,
}

impl LogReplication {
    fn last_index(&self) -> i64 {
        self.log.len() as i64
    }

    fn last_term(&self) -> i64 {
        self.log.last().copied().unwrap_or(0)
    }

    fn term_at(&self, index: i64) -> Option<i64> {
        if index >= 1 && index <= self.last_index() {
            self.log.get((index - 1) as usize).copied()
        } else {
            None
        }
    }

    /// The first index of the run of entries that share the term held at `index`.
    ///
    /// This is what a follower sends back so the leader can skip a whole conflicting term
    /// in one round trip instead of backing up one entry at a time.
    fn run_start(&self, index: i64) -> i64 {
        let Some(t) = self.term_at(index) else {
            return index;
        };
        let mut first = index;
        while first > 1 && self.term_at(first - 1) == Some(t) {
            first -= 1;
        }
        first
    }

    fn answer(&self, success: bool, conflict_index: i64) -> Value {
        json!({
            "term": self.term,
            "success": success,
            "last_index": self.last_index(),
            "last_term": self.last_term(),
            "commit_index": self.commit_index,
            "conflict_index": conflict_index,
        })
    }
}

impl Topic for LogReplication {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let terms = arg!(int_list(args, 0));
                self.term = terms.last().copied().unwrap_or(0);
                self.log = terms;
                self.commit_index = 0;
                json!({
                    "term": self.term,
                    "last_index": self.last_index(),
                    "last_term": self.last_term(),
                    "commit_index": self.commit_index,
                })
            }
            "term" => {
                let t = arg!(int(args, 0));
                self.term = t;
                json!({ "term": t })
            }
            "append" => {
                let term = arg!(int(args, 0));
                let prev_index = arg!(int(args, 1));
                let prev_term = arg!(int(args, 2));
                let leader_commit = arg!(int(args, 3));
                let entries = arg!(int_list(args, 4));
                if prev_index < 0 {
                    return err("prev_index must not be negative");
                }
                // A leader from a term that has passed has no authority here, and its
                // message must not be allowed to touch the log.
                if term < self.term {
                    return self.answer(false, 0);
                }
                self.term = term;
                if prev_index > self.last_index() {
                    return self.answer(false, self.last_index() + 1);
                }
                if prev_index >= 1 && self.term_at(prev_index) != Some(prev_term) {
                    let conflict = self.run_start(prev_index);
                    return self.answer(false, conflict);
                }
                for (k, entry) in entries.iter().enumerate() {
                    let idx = prev_index + 1 + k as i64;
                    match self.term_at(idx) {
                        // An entry that already matches is left exactly where it is: a
                        // retransmitted prefix must not take the tail with it.
                        Some(existing) if existing == *entry => {}
                        Some(_) => {
                            self.log.truncate((idx - 1) as usize);
                            self.log.push(*entry);
                        }
                        None => self.log.push(*entry),
                    }
                }
                let last_new = prev_index + entries.len() as i64;
                if leader_commit > self.commit_index {
                    self.commit_index = self.commit_index.max(leader_commit.min(last_new));
                }
                self.answer(true, 0)
            }
            "log" => json!({
                "term": self.term,
                "terms": self.log,
                "last_index": self.last_index(),
                "last_term": self.last_term(),
                "commit_index": self.commit_index,
            }),
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// `raft-commit` — the leader half: matchIndex, nextIndex and §5.4.2
// ---------------------------------------------------------------------------------------

/// A leader's bookkeeping for its followers, and the rule that advances `commitIndex`.
#[derive(Default)]
struct Commitment {
    members: usize,
    term: i64,
    log: Vec<i64>,
    commit_index: i64,
    match_index: BTreeMap<String, i64>,
    next_index: BTreeMap<String, i64>,
}

impl Commitment {
    fn majority(&self) -> usize {
        self.members / 2 + 1
    }

    fn last_index(&self) -> i64 {
        self.log.len() as i64
    }

    fn term_at(&self, index: i64) -> Option<i64> {
        if index >= 1 && index <= self.last_index() {
            self.log.get((index - 1) as usize).copied()
        } else {
            None
        }
    }

    /// How many members hold the entry at `index`. The leader counts itself.
    fn replicas(&self, index: i64) -> usize {
        1 + self.match_index.values().filter(|m| **m >= index).count()
    }

    /// Raft §5.4.2: the highest index a majority holds *from the leader's own term*.
    fn advance_commit(&mut self) {
        let mut n = self.last_index();
        while n > self.commit_index {
            if self.replicas(n) >= self.majority() && self.term_at(n) == Some(self.term) {
                self.commit_index = n;
                return;
            }
            n -= 1;
        }
    }

    fn state(&self) -> Value {
        json!({
            "term": self.term,
            "last_index": self.last_index(),
            "commit_index": self.commit_index,
            "match": self.match_index,
            "next": self.next_index,
        })
    }
}

impl Topic for Commitment {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let n = arg!(uint(args, 0));
                let term = arg!(int(args, 1));
                let terms = arg!(int_list(args, 2));
                if n < 1 {
                    return err("a cluster needs at least one member");
                }
                *self = Commitment {
                    members: n,
                    term,
                    log: terms,
                    ..Commitment::default()
                };
                let next = self.last_index() + 1;
                for i in 2..=n {
                    let peer = format!("s{i}");
                    self.match_index.insert(peer.clone(), 0);
                    self.next_index.insert(peer, next);
                }
                json!({
                    "term": self.term,
                    "last_index": self.last_index(),
                    "commit_index": self.commit_index,
                    "majority": self.majority(),
                })
            }
            "append" => {
                let term = self.term;
                self.log.push(term);
                json!({ "last_index": self.last_index(), "term": term })
            }
            "ack" => {
                let peer = arg!(word(args, 0));
                let index = arg!(int(args, 1));
                if !self.match_index.contains_key(&peer) {
                    return err(format!("{peer} is not a peer of this cluster"));
                }
                // A reply that overtook an earlier one carries an older matchIndex; letting
                // it win would un-replicate entries the follower demonstrably holds.
                let matched = {
                    let slot = self.match_index.entry(peer.clone()).or_insert(0);
                    *slot = (*slot).max(index);
                    *slot
                };
                let next = {
                    let slot = self.next_index.entry(peer).or_insert(1);
                    *slot = (*slot).max(matched + 1);
                    *slot
                };
                self.advance_commit();
                json!({
                    "match_index": matched,
                    "next_index": next,
                    "commit_index": self.commit_index,
                })
            }
            "reject" => {
                let peer = arg!(word(args, 0));
                let conflict = arg!(int(args, 1));
                if !self.next_index.contains_key(&peer) {
                    return err(format!("{peer} is not a peer of this cluster"));
                }
                let next = conflict.max(1);
                self.next_index.insert(peer, next);
                json!({ "next_index": next })
            }
            "commit-safe" => {
                let index = arg!(int(args, 0));
                if index < 1 || index > self.last_index() {
                    return json!({
                        "safe": false,
                        "replicas": 0,
                        "reason": "past the end of the log",
                    });
                }
                let replicas = self.replicas(index);
                if index <= self.commit_index {
                    return json!({
                        "safe": true,
                        "replicas": replicas,
                        "reason": "already committed",
                    });
                }
                if replicas < self.majority() {
                    return json!({
                        "safe": false,
                        "replicas": replicas,
                        "reason": "no majority",
                    });
                }
                // The Figure 8 rule: a majority is not enough for an entry from an earlier
                // term, because a later leader could still be elected without it.
                if self.term_at(index) != Some(self.term) {
                    return json!({
                        "safe": false,
                        "replicas": replicas,
                        "reason": "earlier term",
                    });
                }
                json!({ "safe": true, "replicas": replicas, "reason": "current term" })
            }
            "entry" => {
                let index = arg!(int(args, 0));
                match self.term_at(index) {
                    Some(t) => json!({ "term": t }),
                    None => err(format!("there is no entry at index {index}")),
                }
            }
            "state" => self.state(),
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// `raft-snapshot` — compaction and InstallSnapshot (Raft §7)
// ---------------------------------------------------------------------------------------

/// A log with a prefix replaced by a snapshot. `log` holds only what survived compaction.
#[derive(Default)]
struct Snapshots {
    log: Vec<i64>,
    snapshot_index: i64,
    snapshot_term: i64,
    commit_index: i64,
}

impl Snapshots {
    fn first_index(&self) -> i64 {
        self.snapshot_index + 1
    }

    fn last_index(&self) -> i64 {
        self.snapshot_index + self.log.len() as i64
    }

    /// The term of the entry at `index`, snapshot boundary included.
    fn term_at(&self, index: i64) -> Option<i64> {
        if index == self.snapshot_index {
            return Some(self.snapshot_term);
        }
        if index >= self.first_index() && index <= self.last_index() {
            return self.log.get((index - self.first_index()) as usize).copied();
        }
        None
    }

    fn shape(&self) -> Value {
        json!({
            "first_index": self.first_index(),
            "last_index": self.last_index(),
            "commit_index": self.commit_index,
            "snapshot_index": self.snapshot_index,
            "snapshot_term": self.snapshot_term,
        })
    }
}

impl Topic for Snapshots {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let terms = arg!(int_list(args, 0));
                *self = Snapshots {
                    log: terms,
                    ..Snapshots::default()
                };
                self.shape()
            }
            "commit" => {
                let index = arg!(int(args, 0));
                let wanted = index.min(self.last_index());
                if wanted > self.commit_index {
                    self.commit_index = wanted;
                }
                json!({ "commit_index": self.commit_index })
            }
            "snapshot" => {
                let index = arg!(int(args, 0));
                // Compacting past the commit index would throw away an entry that may yet
                // be truncated by a future leader, and there would be no way to get it back.
                if index > self.commit_index {
                    return json!({
                        "ok": false,
                        "snapshot_index": self.snapshot_index,
                        "snapshot_term": self.snapshot_term,
                        "first_index": self.first_index(),
                        "log_len": self.log.len(),
                        "error": "cannot snapshot past the commit index",
                    });
                }
                if index > self.snapshot_index {
                    let term = self.term_at(index).unwrap_or(self.snapshot_term);
                    let drop = (index - self.snapshot_index) as usize;
                    self.log.drain(..drop.min(self.log.len()));
                    self.snapshot_index = index;
                    self.snapshot_term = term;
                }
                json!({
                    "ok": true,
                    "snapshot_index": self.snapshot_index,
                    "snapshot_term": self.snapshot_term,
                    "first_index": self.first_index(),
                    "log_len": self.log.len(),
                })
            }
            "install" => {
                let _term = arg!(int(args, 0));
                let last_included_index = arg!(int(args, 1));
                let last_included_term = arg!(int(args, 2));
                if last_included_index <= self.commit_index {
                    let mut v = self.shape();
                    if let Some(map) = v.as_object_mut() {
                        map.insert("ok".into(), json!(false));
                        map.insert("action".into(), json!("stale"));
                    }
                    return v;
                }
                let matching = self.term_at(last_included_index) == Some(last_included_term)
                    && last_included_index >= self.first_index();
                let action = if matching {
                    // The follower already holds this entry, so everything after it is
                    // known good and is kept rather than refetched.
                    let keep = (last_included_index - self.snapshot_index) as usize;
                    self.log.drain(..keep.min(self.log.len()));
                    "retained"
                } else {
                    self.log.clear();
                    "discarded"
                };
                self.snapshot_index = last_included_index;
                self.snapshot_term = last_included_term;
                self.commit_index = self.commit_index.max(last_included_index);
                let mut v = self.shape();
                if let Some(map) = v.as_object_mut() {
                    map.insert("ok".into(), json!(true));
                    map.insert("action".into(), json!(action));
                }
                v
            }
            "append" => {
                let _term = arg!(int(args, 0));
                let prev_index = arg!(int(args, 1));
                let prev_term = arg!(int(args, 2));
                let entries = arg!(int_list(args, 3));
                // The entry the leader wants checked has been compacted away, so the
                // follower cannot answer the question at all; a snapshot is the only cure.
                if prev_index < self.snapshot_index {
                    return json!({
                        "success": false,
                        "reason": "compacted",
                        "last_index": self.last_index(),
                    });
                }
                if prev_index > self.last_index() {
                    return json!({
                        "success": false,
                        "reason": "missing",
                        "last_index": self.last_index(),
                    });
                }
                if self.term_at(prev_index) != Some(prev_term) {
                    return json!({
                        "success": false,
                        "reason": "term mismatch",
                        "last_index": self.last_index(),
                    });
                }
                for (k, entry) in entries.iter().enumerate() {
                    let idx = prev_index + 1 + k as i64;
                    match self.term_at(idx) {
                        Some(existing) if existing == *entry => {}
                        Some(_) => {
                            self.log.truncate((idx - self.first_index()) as usize);
                            self.log.push(*entry);
                        }
                        None => self.log.push(*entry),
                    }
                }
                json!({
                    "success": true,
                    "reason": "ok",
                    "last_index": self.last_index(),
                })
            }
            "log" => {
                let mut v = self.shape();
                if let Some(map) = v.as_object_mut() {
                    map.insert("terms".into(), json!(self.log));
                }
                v
            }
            other => err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// `raft-membership` — joint consensus and single-server changes (Raft §6)
// ---------------------------------------------------------------------------------------

/// The configuration of a cluster, and whatever change is in flight over it.
#[derive(Default)]
struct Membership {
    phase: &'static str,
    members: Vec<String>,
    old: Vec<String>,
    new: Vec<String>,
}

/// A majority of a set of voters.
fn quorum_of(members: &[String]) -> usize {
    members.len() / 2 + 1
}

impl Membership {
    /// Every voter of the current configuration; in the joint phase, both configurations.
    fn voters(&self) -> Vec<String> {
        if self.phase == "joint" {
            let mut all: BTreeSet<String> = self.old.iter().cloned().collect();
            all.extend(self.new.iter().cloned());
            all.into_iter().collect()
        } else {
            self.members.clone()
        }
    }

    fn quorum(&self) -> usize {
        if self.phase == "joint" {
            // There is no single number: a decision needs a majority of each side, and the
            // larger of the two is the smallest useful thing to report.
            quorum_of(&self.old).max(quorum_of(&self.new))
        } else {
            quorum_of(&self.members)
        }
    }

    fn state(&self) -> Value {
        json!({
            "phase": self.phase,
            "members": self.voters(),
            "quorum": self.quorum(),
        })
    }

    fn refuse(&self, error: String) -> Value {
        let mut v = self.state();
        if let Some(map) = v.as_object_mut() {
            map.insert("ok".into(), json!(false));
            map.insert("error".into(), json!(error));
        }
        v
    }

    fn accept(&self) -> Value {
        let mut v = self.state();
        if let Some(map) = v.as_object_mut() {
            map.insert("ok".into(), json!(true));
        }
        v
    }
}

/// Is `voters` a majority of `config`?
fn carries(voters: &BTreeSet<String>, config: &[String]) -> bool {
    let held = config.iter().filter(|m| voters.contains(*m)).count();
    held >= quorum_of(config)
}

impl Topic for Membership {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => {
                let members = arg!(word_list(args, 0));
                if members.is_empty() {
                    return err("a cluster needs at least one member");
                }
                *self = Membership {
                    phase: "stable",
                    members,
                    ..Membership::default()
                };
                self.state()
            }
            "joint" => {
                let new = arg!(word_list(args, 0));
                if new.is_empty() {
                    return err("the new configuration needs at least one member");
                }
                if self.phase != "stable" {
                    return self.refuse("a configuration change is already in flight".into());
                }
                self.old = self.members.clone();
                self.new = new;
                self.phase = "joint";
                json!({
                    "ok": true,
                    "phase": "joint",
                    "old": self.old,
                    "new": self.new,
                    "quorum_old": quorum_of(&self.old),
                    "quorum_new": quorum_of(&self.new),
                })
            }
            "commit-joint" => {
                if self.phase != "joint" {
                    return self.refuse("there is no joint configuration to commit".into());
                }
                self.members = self.new.clone();
                self.phase = "new";
                self.accept()
            }
            "commit-new" => {
                if self.phase != "new" {
                    return self.refuse("there is no new configuration to commit".into());
                }
                self.old.clear();
                self.new.clear();
                self.phase = "stable";
                self.accept()
            }
            "agree" => {
                let voters: BTreeSet<String> = arg!(word_list(args, 0)).into_iter().collect();
                if self.phase == "joint" {
                    let old = carries(&voters, &self.old);
                    let new = carries(&voters, &self.new);
                    let reason = match (old, new) {
                        (true, true) => "old and new",
                        (true, false) => "old only",
                        (false, true) => "new only",
                        (false, false) => "neither",
                    };
                    // Both halves, or neither: this is the whole of joint consensus, and it
                    // is what stops C_old and C_new electing two leaders in one term.
                    return json!({ "ok": old && new, "reason": reason });
                }
                let ok = carries(&voters, &self.members);
                json!({
                    "ok": ok,
                    "reason": if ok { "majority" } else { "no majority" },
                })
            }
            "add" | "remove" => {
                let who = arg!(word(args, 0));
                if self.phase != "stable" {
                    return self.refuse("a configuration change is already in flight".into());
                }
                let present = self.members.contains(&who);
                if command == "add" {
                    if present {
                        return self.refuse(format!("{who} is already a member"));
                    }
                    self.members.push(who);
                    self.members.sort();
                } else {
                    if !present {
                        return self.refuse(format!("{who} is not a member"));
                    }
                    self.members.retain(|m| *m != who);
                    if self.members.is_empty() {
                        self.members.push(who);
                        return self.refuse("the last member cannot be removed".into());
                    }
                }
                self.phase = "pending";
                self.accept()
            }
            "commit-change" => {
                if self.phase != "pending" {
                    return self.refuse("there is no single-server change to commit".into());
                }
                self.phase = "stable";
                self.accept()
            }
            "state" => self.state(),
            other => err(format!("unknown command {other:?}")),
        }
    }
}
