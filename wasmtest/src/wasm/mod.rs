//! The suite's own WebAssembly encoder.
//!
//! Every module a test runs is built here, byte by byte: LEB128 integers, section framing,
//! the type/import/function/table/memory/global/export/start/element/code/data/datacount
//! sections, and an instruction builder. Nothing is read from a `.wat` file and no encoder
//! crate is used, for three reasons:
//!
//! * the malformed-module stages need to write bytes no well-behaved encoder would emit —
//!   an overlong LEB128, a section size that lies, two type sections, a truncated body;
//! * a failure can then print the module it built as an **annotated hex listing**, so the
//!   report shows which byte the runtime disagreed about and what that byte is for;
//! * the worked examples in `catalog.json` are the same annotations, produced offline, so
//!   an example can never drift away from the module a test actually runs.
//!
//! [`ModuleBuilder`] is the high-level path (well-formed modules), [`Enc`] the low-level one
//! (anything else). Both produce a [`Module`]: the exact bytes plus a list of [`Ann`]s.

pub mod encode;
pub mod hex;
pub mod instr;

pub use encode::*;
pub use instr::*;
