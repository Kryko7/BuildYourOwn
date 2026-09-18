//! `wasmtest` — a black-box conformance tester for WebAssembly runtimes.
//!
//! The binary lives in `src/main.rs`; everything it uses is here so that the integration
//! tests under `tests/` can exercise the module encoder, the annotated hex listing and the
//! stage catalog without going through the CLI.

#![deny(missing_docs)]
// `Failure` carries the whole module under test plus its annotations, so it is deliberately
// fat. Boxing it everywhere would make every stage test read `Box::new(...)`; the cost is
// one allocation on a path that is already about to print a few hundred bytes of hex.
#![allow(clippy::result_large_err)]

pub mod assert;
pub mod catalog;
pub mod config;
pub mod examples;
pub mod report;
pub mod runner;
pub mod runtime;
pub mod stages;
pub mod wasm;
