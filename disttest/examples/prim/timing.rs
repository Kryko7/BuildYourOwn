//! Reference topics: `token-bucket`, `leaky-bucket`, `backoff`, `phi-accrual` and `swim`.
//!
//! Reading this spoils stages 19 and 20 of ladder A. It is here for one reason: so
//! `disttest --target reference_primitives --validate` can prove the suite's own
//! expectations against something. Nothing is tuned or clever; each topic is the shortest
//! state machine that satisfies the grammar in README.md §2.2.
//!
//! Every command carries the instant it happens at, and none of these topics ever reads a
//! clock of its own. That is the only reason a rate limiter or a failure detector can be
//! tested at all: the same script gives the same answers on any machine, at any speed.

use crate::{num, word, Topic};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Build the state machine for one of this file's topics.
pub fn make(topic: &str) -> Option<Box<dyn Topic>> {
    match topic {
        "token-bucket" => Some(Box::new(TokenBucket::default())),
        "leaky-bucket" => Some(Box::new(LeakyBucket::default())),
        "backoff" => Some(Box::new(Backoff::default())),
        "phi-accrual" => Some(Box::new(PhiAccrual::default())),
        "swim" => Some(Box::new(Swim::default())),
        _ => None,
    }
}

/// The slack a comparison of two floating-point token counts allows: an amount this small
/// is arithmetic noise, never a token.
const EPSILON: f64 = 1e-9;

// ---------------------------------------------------------------------------------------
// token-bucket
// ---------------------------------------------------------------------------------------

/// Tokens accrue at `rate` per second up to `burst`, and a take spends them.
#[derive(Default)]
struct TokenBucket {
    rate: f64,
    burst: f64,
    tokens: f64,
    last_ms: f64,
}

impl Topic for TokenBucket {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => match (num(args, 0), num(args, 1)) {
                (Ok(rate), Ok(burst)) => {
                    self.rate = rate;
                    self.burst = burst;
                    self.tokens = burst;
                    self.last_ms = 0.0;
                    json!({ "ok": true })
                }
                (Err(e), _) | (_, Err(e)) => e,
            },
            "take" => match (num(args, 0), num(args, 1)) {
                (Ok(t_ms), Ok(n)) => {
                    // Time that goes backwards adds nothing; the cap is the whole point.
                    let elapsed = (t_ms - self.last_ms).max(0.0);
                    self.tokens = (self.tokens + self.rate * elapsed / 1000.0).min(self.burst);
                    self.last_ms = t_ms;
                    let allowed = self.tokens + EPSILON >= n;
                    if allowed {
                        self.tokens -= n;
                    }
                    json!({ "allowed": allowed, "tokens": self.tokens })
                }
                (Err(e), _) | (_, Err(e)) => e,
            },
            other => crate::err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// leaky-bucket
// ---------------------------------------------------------------------------------------

/// A queue that drains at `rate` per second and refuses whatever would overflow it.
#[derive(Default)]
struct LeakyBucket {
    rate: f64,
    capacity: f64,
    queued: f64,
    last_ms: f64,
}

impl Topic for LeakyBucket {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => match (num(args, 0), num(args, 1)) {
                (Ok(rate), Ok(capacity)) => {
                    self.rate = rate;
                    self.capacity = capacity;
                    self.queued = 0.0;
                    self.last_ms = 0.0;
                    json!({ "ok": true })
                }
                (Err(e), _) | (_, Err(e)) => e,
            },
            "offer" => match (num(args, 0), num(args, 1)) {
                (Ok(t_ms), Ok(n)) => {
                    let elapsed = (t_ms - self.last_ms).max(0.0);
                    self.queued = (self.queued - self.rate * elapsed / 1000.0).max(0.0);
                    self.last_ms = t_ms;
                    let accepted = self.queued + n <= self.capacity + EPSILON;
                    if accepted {
                        self.queued += n;
                    }
                    json!({ "accepted": accepted, "queue": self.queued })
                }
                (Err(e), _) | (_, Err(e)) => e,
            },
            other => crate::err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// backoff
// ---------------------------------------------------------------------------------------

/// Exponential backoff with full jitter.
#[derive(Default)]
struct Backoff {
    base_ms: f64,
    cap_ms: f64,
}

impl Topic for Backoff {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => match (num(args, 0), num(args, 1)) {
                (Ok(base), Ok(cap)) => {
                    self.base_ms = base;
                    self.cap_ms = cap;
                    json!({ "ok": true })
                }
                (Err(e), _) | (_, Err(e)) => e,
            },
            "next" => match (num(args, 0), num(args, 1)) {
                (Ok(attempt), Ok(u)) => {
                    // The sample arrives with the command, so the jitter is reproducible.
                    let ceiling = self.cap_ms.min(self.base_ms * 2f64.powi(attempt as i32));
                    json!({ "delay": u * ceiling })
                }
                (Err(e), _) | (_, Err(e)) => e,
            },
            other => crate::err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// phi-accrual
// ---------------------------------------------------------------------------------------

/// Suspicion as a number rather than a verdict: phi is how unlikely this much silence is,
/// on a log scale, given the intervals the heartbeats have been arriving at.
#[derive(Default)]
struct PhiAccrual {
    last_ms: Option<f64>,
    intervals: Vec<f64>,
}

impl Topic for PhiAccrual {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "heartbeat" => match num(args, 0) {
                Ok(t_ms) => {
                    if let Some(last) = self.last_ms {
                        if t_ms > last {
                            self.intervals.push(t_ms - last);
                        }
                    }
                    self.last_ms = Some(t_ms);
                    json!({ "samples": self.intervals.len() })
                }
                Err(e) => e,
            },
            "phi" => match num(args, 0) {
                Ok(t_ms) => {
                    let mean = if self.intervals.is_empty() {
                        0.0
                    } else {
                        self.intervals.iter().sum::<f64>() / self.intervals.len() as f64
                    };
                    let silence = self.last_ms.map(|l| (t_ms - l).max(0.0)).unwrap_or(0.0);
                    // An exponential arrival model: the probability of this much silence
                    // is exp(-silence/mean), and phi is minus its base-ten logarithm. One
                    // mean interval of silence is phi 0.43; ten is 4.3.
                    let phi = if mean <= 0.0 {
                        0.0
                    } else {
                        (silence / mean) / std::f64::consts::LN_10
                    };
                    json!({ "phi": phi })
                }
                Err(e) => e,
            },
            other => crate::err(format!("unknown command {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// swim
// ---------------------------------------------------------------------------------------

/// Two deadlines measured from the last ack: alive, then suspect, then dead.
#[derive(Default)]
struct Swim {
    suspect_ms: f64,
    dead_ms: f64,
    last_ack: BTreeMap<String, f64>,
}

impl Topic for Swim {
    fn step(&mut self, command: &str, args: &[&str]) -> Value {
        match command {
            "init" => match (num(args, 0), num(args, 1)) {
                (Ok(suspect), Ok(dead)) => {
                    self.suspect_ms = suspect;
                    self.dead_ms = dead;
                    json!({ "ok": true })
                }
                (Err(e), _) | (_, Err(e)) => e,
            },
            "ack" => match (word(args, 0), num(args, 1)) {
                (Ok(node), Ok(t_ms)) => {
                    self.last_ack.insert(node, t_ms);
                    json!({ "ok": true })
                }
                (Err(e), _) | (_, Err(e)) => e,
            },
            "status" => match (word(args, 0), num(args, 1)) {
                (Ok(node), Ok(t_ms)) => {
                    let since = t_ms - self.last_ack.get(&node).copied().unwrap_or(0.0);
                    let state = if since < self.suspect_ms {
                        "alive"
                    } else if since < self.dead_ms {
                        "suspect"
                    } else {
                        "dead"
                    };
                    json!({ "state": state })
                }
                (Err(e), _) | (_, Err(e)) => e,
            },
            other => crate::err(format!("unknown command {other:?}")),
        }
    }
}
