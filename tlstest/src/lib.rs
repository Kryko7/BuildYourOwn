//! `tlstest` — a black-box conformance tester for TLS 1.3 servers (RFC 8446).
//!
//! The binary lives in `src/main.rs`; everything it uses is here so that the integration
//! tests under `tests/` can exercise the key schedule, the record layer and the stage
//! catalog without going through the CLI.
//!
//! The interesting half of this crate is [`tls`]: a TLS 1.3 *client* written from
//! primitives, so that every failure can show the handshake message that went wrong, its
//! hex, the field path inside it, the transcript hash that was in force and the
//! key-schedule label being derived at that moment. The rest — [`config`], [`server`],
//! [`runner`], [`report`], [`catalog`] — is the same harness shape the other testers in
//! this repo use.

#![deny(missing_docs)]
// `Failure` carries hex blocks and decoded field paths, so it is deliberately fat. Boxing
// it everywhere would make every stage test read `Box::new(...)`; the cost is one
// allocation on a path that is already about to print several hundred bytes.
#![allow(clippy::result_large_err)]

pub mod assert;
pub mod catalog;
pub mod certs;
pub mod config;
pub mod examples;
pub mod report;
pub mod runner;
pub mod server;
pub mod stages;
pub mod tls;
