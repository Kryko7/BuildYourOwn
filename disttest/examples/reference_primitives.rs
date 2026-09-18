//! `reference_primitives` — the reference for the **primitives** ladder.
//!
//! **Reading this file spoils ladder A.** It exists for one reason: so that
//! `disttest --target reference_primitives --validate --tag primitives` can prove the
//! suite's own expectations are right, the way `broken_node` exists to prove the suite's
//! red output is real. Nothing here is written for a learner to copy, and none of it is
//! tuned, packed or clever; it is the shortest implementation that satisfies the grammar in
//! README.md and nothing else.
//!
//! The program is started as `reference_primitives <topic>`, reads one command per line on
//! stdin, and writes exactly one JSON object per line on stdout.

use serde_json::{json, Value};
use std::io::{BufRead, Write};

#[path = "prim/clocks.rs"]
mod clocks;
#[path = "prim/crdt.rs"]
mod crdt;
#[path = "prim/placement.rs"]
mod placement;
#[path = "prim/protocols.rs"]
mod protocols;
#[path = "prim/sketches.rs"]
mod sketches;
#[path = "prim/timing.rs"]
mod timing;

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

/// Parse a floating-point argument.
pub fn num(args: &[&str], i: usize) -> Result<f64, Value> {
    args.get(i)
        .ok_or_else(|| err(format!("argument {i} is missing")))?
        .parse()
        .map_err(|_| err(format!("argument {i} is not a number")))
}

/// Read a required string argument.
pub fn word(args: &[&str], i: usize) -> Result<String, Value> {
    args.get(i)
        .map(|s| (*s).to_string())
        .ok_or_else(|| err(format!("argument {i} is missing")))
}

/// Parse a JSON argument (a clock, a state, a list).
pub fn json_arg(args: &[&str], i: usize) -> Result<Value, Value> {
    let raw = word(args, i)?;
    serde_json::from_str(&raw).map_err(|e| err(format!("argument {i} is not JSON: {e}")))
}

fn make(topic: &str) -> Option<Box<dyn Topic>> {
    clocks::make(topic)
        .or_else(|| placement::make(topic))
        .or_else(|| sketches::make(topic))
        .or_else(|| crdt::make(topic))
        .or_else(|| protocols::make(topic))
        .or_else(|| timing::make(topic))
}

fn main() {
    let topic = match std::env::args().nth(1) {
        Some(t) => t,
        None => {
            eprintln!("usage: reference_primitives <topic>");
            std::process::exit(2);
        }
    };
    let Some(mut state) = make(&topic) else {
        eprintln!("reference_primitives: unknown topic {topic:?}");
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
