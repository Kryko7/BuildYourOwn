//! `reference_algorithms` — the reference for the **algorithms** ladder.
//!
//! **Reading this file spoils ladder B.** It exists for one reason: so that
//! `disttest --target reference_algorithms --validate --tag algorithms` can prove the
//! suite's own expectations are right, exactly as `examples/reference_primitives.rs` does
//! for ladder A and `broken_node` does for the red output. Nothing here is written for a
//! learner to copy, and none of it is tuned, packed or clever; it is the shortest
//! implementation that satisfies the grammar in README.md §3 and nothing else.
//!
//! The program is started as `reference_algorithms <topic>`, reads one command per line on
//! stdin, and writes exactly one JSON object per line on stdout.

use serde_json::{json, Value};
use std::io::{BufRead, Write};

#[path = "alg/commit.rs"]
mod commit;
#[path = "alg/patterns.rs"]
mod patterns;
#[path = "alg/paxos.rs"]
mod paxos;
#[path = "alg/raft.rs"]
mod raft;
#[path = "alg/resilience.rs"]
mod resilience;

/// One topic's state machine: a command in, one JSON object out.
pub trait Topic {
    /// Handle one command. `args` is the whitespace-split tail of the line.
    fn step(&mut self, command: &str, args: &[&str]) -> Value;
}

/// An error object, the one shape every topic uses when a command makes no sense.
pub fn err(message: impl Into<String>) -> Value {
    json!({ "ok": false, "error": message.into() })
}

/// Parse an integer argument, or explain which one was wrong.
pub fn int(args: &[&str], i: usize) -> Result<i64, Value> {
    args.get(i)
        .ok_or_else(|| err(format!("argument {i} is missing")))?
        .parse()
        .map_err(|_| err(format!("argument {i} is not a whole number")))
}

/// Parse an integer argument that must not be negative.
pub fn uint(args: &[&str], i: usize) -> Result<usize, Value> {
    let n = int(args, i)?;
    usize::try_from(n).map_err(|_| err(format!("argument {i} must not be negative")))
}

/// Read a required string argument.
pub fn word(args: &[&str], i: usize) -> Result<String, Value> {
    args.get(i)
        .map(|s| (*s).to_string())
        .ok_or_else(|| err(format!("argument {i} is missing")))
}

/// Parse a JSON argument (a log, a member list, a set of latencies).
pub fn json_arg(args: &[&str], i: usize) -> Result<Value, Value> {
    let raw = word(args, i)?;
    serde_json::from_str(&raw).map_err(|e| err(format!("argument {i} is not JSON: {e}")))
}

/// Parse a JSON argument that must be an array of whole numbers.
pub fn int_list(args: &[&str], i: usize) -> Result<Vec<i64>, Value> {
    let v = json_arg(args, i)?;
    let Some(items) = v.as_array() else {
        return Err(err(format!("argument {i} is not an array")));
    };
    items
        .iter()
        .map(|x| {
            x.as_i64()
                .ok_or_else(|| err(format!("argument {i} holds {x}, not a whole number")))
        })
        .collect()
}

/// Parse a JSON argument that must be an array of strings.
pub fn word_list(args: &[&str], i: usize) -> Result<Vec<String>, Value> {
    let v = json_arg(args, i)?;
    let Some(items) = v.as_array() else {
        return Err(err(format!("argument {i} is not an array")));
    };
    items
        .iter()
        .map(|x| {
            x.as_str()
                .map(str::to_string)
                .ok_or_else(|| err(format!("argument {i} holds {x}, not a string")))
        })
        .collect()
}

/// Read a `yes`/`no` (or `commit`/`abort`, or `ok`/`fail`) style flag argument.
pub fn flag(args: &[&str], i: usize, truthy: &str, falsy: &str) -> Result<bool, Value> {
    let w = word(args, i)?;
    if w == truthy {
        Ok(true)
    } else if w == falsy {
        Ok(false)
    } else {
        Err(err(format!(
            "argument {i} must be {truthy:?} or {falsy:?}, got {w:?}"
        )))
    }
}

fn make(topic: &str) -> Option<Box<dyn Topic>> {
    raft::make(topic)
        .or_else(|| paxos::make(topic))
        .or_else(|| commit::make(topic))
        .or_else(|| patterns::make(topic))
        .or_else(|| resilience::make(topic))
}

fn main() {
    let topic = match std::env::args().nth(1) {
        Some(t) => t,
        None => {
            eprintln!("usage: reference_algorithms <topic>");
            std::process::exit(2);
        }
    };
    let Some(mut state) = make(&topic) else {
        eprintln!("reference_algorithms: unknown topic {topic:?}");
        std::process::exit(2);
    };
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        let (command, args) = parts.split_first().unwrap_or((&"", &[]));
        let answer = state.step(command, args);
        if writeln!(out, "{answer}").is_err() || out.flush().is_err() {
            break;
        }
    }
}
