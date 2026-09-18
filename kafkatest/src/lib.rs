//! `kafkatest` — a black-box conformance tester for Kafka brokers.
//!
//! The binary lives in `src/main.rs`; everything it uses is here so that the integration
//! tests under `tests/` can exercise the record codec, the fixture writer and the stage
//! catalog without going through the CLI.

#![deny(missing_docs)]
// `Failure` carries hex dumps and decoded field paths, so it is deliberately fat. Boxing it
// everywhere would make every stage test read `Box::new(...)`; the cost is one allocation on
// a path that is already about to print several hundred bytes.
#![allow(clippy::result_large_err)]

pub mod assert;
pub mod broker;
pub mod catalog;
pub mod config;
pub mod examples;
pub mod fixtures;
pub mod proto;
pub mod report;
pub mod runner;
pub mod stages;
