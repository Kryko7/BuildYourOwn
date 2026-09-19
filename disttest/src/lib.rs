//! `disttest` — a black-box conformance tester for a distributed key-value system.
//!
//! The track has four ladders, and every stage declares which one it belongs to:
//!
//! * `primitives` — a line-oriented CLI answering one JSON object per command. The suite
//!   computes the answer independently (brute force, exhaustive replay, published test
//!   vectors) and compares.
//! * `algorithms` — the same CLI contract, one topic per classic: Raft, Paxos, two- and
//!   three-phase commit, sagas, outboxes, fencing tokens, gossip, circuit breakers. Every
//!   oracle is the suite's own encoding of the specified rules.
//! * `node` — a single server speaking a subset of the etcd v3 HTTP/JSON API.
//! * `cluster` — three or five of those servers, with a userspace TCP proxy in front of
//!   every peer URL so the harness can partition, delay and drop peer traffic, and a
//!   linearizability checker over the recorded history.
//!
//! The binary lives in `src/main.rs`; everything it uses is here so the integration tests
//! under `tests/` can exercise the checker, the codec and the catalog without the CLI.

#![deny(missing_docs)]
// `Failure` carries decoded field paths, histories and process output, so it is deliberately
// fat. Boxing it everywhere would make every stage read `Box::new(..)` on a path that is
// already about to print several hundred bytes.
#![allow(clippy::result_large_err)]

pub mod assert;
pub mod catalog;
pub mod cluster;
pub mod config;
pub mod etcd;
pub mod examples;
pub mod lin;
pub mod node;
pub mod prim;
pub mod report;
pub mod runner;
pub mod stages;
