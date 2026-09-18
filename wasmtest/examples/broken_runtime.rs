//! A deliberately broken WebAssembly runtime, used to show what a red `wasmtest` run looks
//! like without anyone having to write a real one first.
//!
//! It does the two things every runtime starts with — parse the command line the way
//! `wasmtime` does, and check the eight-byte module header — and then gives up. It never
//! decodes a section, never validates anything and never executes an instruction:
//!
//! * `--invoke` always prints `0`, whatever the function was supposed to return;
//! * a module that traps exits 0 with nothing on stderr;
//! * a module that does not decode or does not validate is accepted, because nothing after
//!   the header is ever looked at;
//! * a WASI command prints nothing.
//!
//! So stage 01 is green — the header really is checked — and everything after it is red,
//! with the failure blocks showing the module's annotated bytes, the exact command line and
//! both streams.
//!
//! ```text
//! cargo build --release --example broken_runtime
//! ./target/release/wasmtest --runtime examples/broken_runtime --until 5
//! ```

use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(parsed) = parse(&args) else {
        eprintln!(
            "usage: broken_runtime run [--invoke <export>] <module.wasm> [args...]\n\
             (a deliberately incomplete runtime, shipped with wasmtest to demonstrate \
             failing output)"
        );
        return ExitCode::from(2);
    };

    let bytes = match std::fs::read(&parsed.module) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("broken_runtime: cannot read {}: {e}", parsed.module);
            return ExitCode::from(1);
        }
    };

    // The one thing it gets right.
    if bytes.len() < 8 {
        eprintln!(
            "broken_runtime: {} is {} bytes; a module starts with a four-byte magic and a \
             four-byte version",
            parsed.module,
            bytes.len()
        );
        return ExitCode::from(1);
    }
    if &bytes[0..4] != b"\0asm" {
        eprintln!(
            "broken_runtime: {} does not start with the magic 00 61 73 6d",
            parsed.module
        );
        return ExitCode::from(1);
    }
    if bytes[4..8] != [0x01, 0x00, 0x00, 0x00] {
        eprintln!(
            "broken_runtime: {} declares binary format version {}, and only 1 exists",
            parsed.module,
            u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]])
        );
        return ExitCode::from(1);
    }

    // ... and everything it gets wrong. No sections are read, so whatever the module says,
    // an --invoke answers with one line holding zero, and a command answers with silence.
    if parsed.invoke.is_some() {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "0");
        let _ = out.flush();
    }
    ExitCode::SUCCESS
}

/// What the command line asked for.
struct Parsed {
    /// The path of the module to run.
    module: String,
    /// The export named by `--invoke`, when there was one.
    invoke: Option<String>,
}

/// `run [--invoke <export>] <module.wasm> [args...]`, the subset of wasmtime's CLI the
/// `wasmtest` contract uses.
fn parse(args: &[String]) -> Option<Parsed> {
    let mut it = args.iter();
    if it.next().map(String::as_str) != Some("run") {
        return None;
    }
    let mut invoke = None;
    let mut module = None;
    while let Some(a) = it.next() {
        if let Some(name) = a.strip_prefix("--invoke=") {
            invoke = Some(name.to_string());
        } else if a == "--invoke" {
            invoke = Some(it.next()?.clone());
        } else if module.is_none() {
            module = Some(a.clone());
        } else {
            // Everything after the module path belongs to the module, and this runtime has
            // nothing to give it to.
        }
    }
    Some(Parsed {
        module: module?,
        invoke,
    })
}
