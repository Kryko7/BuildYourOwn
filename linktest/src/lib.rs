//! `linktest` — a black-box conformance tester for static ELF64 x86-64 linkers.
//!
//! The binary lives in `src/main.rs`; everything it uses is here so that the integration
//! tests under `tests/` can exercise the ELF writer, the archive writer, the instruction
//! encoders and the stage catalog without going through the CLI.
//!
//! The shape of a run is always the same three legs:
//!
//! 1. **run it** — the produced binary is executed in a sandboxed temporary directory with a
//!    timeout, and its stdout and exit status are compared against what the fixture's own
//!    machine code says they must be ([`stages::Ctx::expect_output`]);
//! 2. **re-parse it** — the output is read back with [`elf::read`] and asserted on
//!    structurally ([`stages::assert_runnable_layout`]);
//! 3. **compare** — for the properties that must match GNU ld, the *invariant* is asserted
//!    (the relocated value implied by the addresses the linker itself chose), never the
//!    layout, because a linker's layout is its own business.
#![warn(missing_docs)]
// A `Failure` carries hex dumps, header tables and the linker's own diagnostics, so it is
// deliberately fat. Boxing it everywhere would make every stage test read `Box::new(...)`.
#![allow(clippy::result_large_err)]

pub mod asm;
pub mod assert;
pub mod catalog;
pub mod config;
pub mod elf;
pub mod examples;
pub mod exec;
pub mod link;
pub mod linker;
pub mod report;
pub mod runner;
pub mod stages;
