//! Stage 43 — Fuzz: 500 seeded mutations of valid modules. **[ext]**
//!
//! Five hundred modules derived from four valid ones — a numeric module, one with a memory
//! and two data segments, one with a table and a `call_indirect`, and a WASI command — each
//! corrupted by one family of mutations and each run in its own process. Every choice comes
//! from `--seed`, so a run is reproducible and a failure names the family and the index that
//! produced it.
//!
//! Only three things are asserted, because only three are promised:
//!
//! * the runtime is **never killed by a signal and never hangs**. Both are the harness's own
//!   failure kinds — a `RuntimeCrash` or a `Timeout` comes straight out of `ctx.invoke` —
//!   so the check is simply that the error is propagated rather than swallowed;
//! * whatever the runtime does, it does one of two legitimate things: it **refuses** the
//!   module (a non-zero exit and not a byte on stdout, because a module that does not decode
//!   or validate never executes) or it **runs** it and prints output a valid module could
//!   have printed — one line per returned value, each one a number;
//! * nothing is written outside the test's temporary directory.
//!
//! A mutation that leaves the module **still valid** is a pass, not a failure. Flipping a
//! bit in a custom section's payload, in an unused byte of a data segment, or in one of the
//! high bits of an over-long LEB128 changes nothing a decoder is allowed to care about, and
//! a module that still runs afterwards is exactly as correct as one that is refused. The
//! tally at the end of each test says how the five hundred split — refused, run, trapped —
//! and that split is informational, never asserted.
//!
//! Two expectations here are looser than they first look, and deliberately so:
//!
//! * a **trap** is not a rejection. A mutated module can validate perfectly and then divide
//!   by zero or run off the end of its memory, which is a non-zero exit with a trap message;
//!   the stdout-is-empty rule is what separates "refused before running" from "ran and
//!   trapped", and the three `--invoke` seeds cannot write to stdout at all, so for them the
//!   rule holds either way;
//! * the WASI seed **can** legitimately print before it fails, because its `_start` writes
//!   to fd 1 and a later mutation-induced trap does not unwrite those bytes. For that seed
//!   the output rule is only that it stays bounded — an unbounded stream of bytes out of a
//!   five-byte data segment would be a real finding, a short or garbled one would not.
//!
//! Each test gives every invocation five seconds of its own, inside a sixty-second budget for
//! the whole family: a mutation that turns the module into an infinite loop is reported as a
//! timeout after five seconds rather than eating the test.

use crate::assert::{Check, Failure, FailureKind};
use crate::examples::ExampleSpec;
use crate::runtime::Run;
use crate::stages::wasi::{self, W};
use crate::stages::{first_meaningful_line, Ctx, Stage, Test};
use crate::wasm::{
    ftype, global_i32, op, uleb, Expr, Func, Limits, Module, ModuleBuilder, TableType, ValType,
};
use crate::wasm_test;
use rand::rngs::StdRng;
use rand::Rng;
use std::time::Duration;

/// Mutations per family; five of these plus [`MIXED`] make the five hundred the plan asks for.
const PER_FAMILY: usize = 84;
/// The mixed family takes the remainder, so the total is exactly 500.
const MIXED: usize = 500 - 5 * PER_FAMILY;
/// How long one mutated module gets before the harness calls it a hang.
const PER_INVOCATION: Duration = Duration::from_secs(5);
/// More stdout than this out of a module of a few dozen bytes is a finding in itself.
const MAX_OUTPUT: usize = 1 << 20;

/// One mutation family: the seeded RNG and a valid module in, a corrupted module and a
/// sentence describing what was done to it out.
type Mutation = fn(&mut StdRng, &Module) -> (Module, String);

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 43,
        slug: "fuzz",
        name: "Fuzz: 500 seeded mutations of valid modules",
        ext: true,
        hints: &[
            "Every length in a module is a claim, not a fact: check a section size, a vector count and a name length against the bytes you actually have before you index with them",
            "A malformed module is input, never a reason to abort — refuse it with a message and a non-zero exit, and never print a byte on stdout, because a module that does not validate has not run",
            "A uLEB128 that keeps setting the continuation bit must stop being read at the width of the field: ten 0xff bytes in a row is a malformed index, not a very large one",
            "Run the fuzzer against your own runtime with --seed: the seed and the mutation index in a failure are enough to rebuild the exact bytes that broke it",
        ],
        examples,
        tests: vec![
            fuzz_test(
                "a module with one to three bits flipped is either refused or run, never a crash",
                bit_flips,
            ),
            fuzz_test(
                "a module truncated or padded at random is either refused or run, never a crash",
                truncation,
            ),
            fuzz_test(
                "a lying section size or vector count leaves it refused or run, never crashed",
                lying_lengths,
            ),
            fuzz_test(
                "a ten-byte LEB128 spliced over an index leaves it refused or run, never crashed",
                huge_lebs,
            ),
            fuzz_test(
                "section surgery leaves the module refused or run, never crashed",
                section_surgery,
            ),
            fuzz_test(
                "two mutations at once leave the module refused or run, never crashed",
                mixed,
            ),
        ],
    }
}

/// A test of this stage: `ext`, `slow`, and given a minute for its eighty-odd processes.
fn fuzz_test(name: &'static str, run: crate::stages::TestFn) -> Test {
    Test::new(name, run)
        .ext()
        .tag("slow")
        .min_timeout_ms(60_000)
}

// ---------------------------------------------------------------------------------------
// The valid modules every mutation starts from
// ---------------------------------------------------------------------------------------

/// A valid module, how it is invoked, and what it is called in a failure message.
struct Seed {
    /// Short human name.
    what: &'static str,
    /// The module itself, before any mutation.
    module: Module,
    /// The export to invoke, or `None` for the WASI command path.
    export: Option<&'static str>,
}

/// Arithmetic and a direct call: `f() = double(21)`.
fn numeric_seed() -> Module {
    let mut b = ModuleBuilder::new("fuzz-numeric");
    let unary = b.add_type(ftype(&[ValType::I32], &[ValType::I32]));
    let double = b.add_func(
        unary,
        Func::new(Expr::new().local_get(0).i32_const(2).op(op::I32_MUL)),
    );
    let nullary = b.add_type(ftype(&[], &[ValType::I32]));
    let f = b.add_func(nullary, Func::new(Expr::new().i32_const(21).call(double)));
    b.export_func("f", f)
        .custom("producers", b"wasmtest", None)
        .build()
}

/// A memory and two active data segments, read back with two byte loads.
fn memory_seed() -> Module {
    let mut b = ModuleBuilder::new("fuzz-memory");
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let f = b.add_func(
        ty,
        Func::new(
            Expr::new()
                .i32_const(0)
                .mem(op::I32_LOAD8_U, 0, 3)
                .i32_const(0)
                .mem(op::I32_LOAD8_U, 0, 64)
                .op(op::I32_ADD),
        ),
    );
    b.memory(Limits::min(1))
        .data_active(0, b"wasmtest fuzz seed")
        .data_active(64, &[9, 9, 9])
        .export_func("f", f)
        .build()
}

/// A table, an element segment, a global and a `call_indirect` through slot 1.
fn table_seed() -> Module {
    let mut b = ModuleBuilder::new("fuzz-table");
    let unary = b.add_type(ftype(&[ValType::I32], &[ValType::I32]));
    let inc = b.add_func(
        unary,
        Func::new(Expr::new().local_get(0).i32_const(1).op(op::I32_ADD)),
    );
    let neg = b.add_func(
        unary,
        Func::new(Expr::new().i32_const(0).local_get(0).op(op::I32_SUB)),
    );
    let nullary = b.add_type(ftype(&[], &[ValType::I32]));
    let f = b.add_func(
        nullary,
        Func::new(
            Expr::new()
                .i32_const(10)
                .i32_const(1)
                .call_indirect(unary, 0),
        ),
    );
    b.table(TableType {
        elem: ValType::FuncRef,
        limits: Limits::range(4, 8),
    })
    .elem_active(0, &[inc, neg])
    .global(global_i32(5, false))
    .export_func("f", f)
    .build()
}

/// A WASI command that writes five bytes to stdout and returns.
fn wasi_seed() -> Module {
    let (mut b, _) = wasi::wasi_command("fuzz-wasi", 1, &[W::FdWrite]);
    let ty = b.add_type(ftype(&[], &[]));
    let start = b.add_func(ty, Func::new(wasi::puts(wasi::STDOUT, 0, 5, 0)));
    b.export_func("_start", start)
        .data_active(0, b"hello")
        .build()
}

/// The four modules the fuzzer mutates, in the order it cycles through them.
fn seeds() -> Vec<Seed> {
    vec![
        Seed {
            what: "the numeric module",
            module: numeric_seed(),
            export: Some("f"),
        },
        Seed {
            what: "the memory-and-data module",
            module: memory_seed(),
            export: Some("f"),
        },
        Seed {
            what: "the table-and-call_indirect module",
            module: table_seed(),
            export: Some("f"),
        },
        Seed {
            what: "the WASI command",
            module: wasi_seed(),
            export: None,
        },
    ]
}

// ---------------------------------------------------------------------------------------
// Looking at a module the way a decoder does
// ---------------------------------------------------------------------------------------

/// One section as the decoder walks it: where its id, size field and body are.
struct Sec {
    /// Offset of the id byte.
    at: usize,
    /// Offset and length of the size uLEB128.
    size_at: usize,
    /// Length of that uLEB128, in bytes.
    size_len: usize,
    /// Offset of the first body byte.
    body_at: usize,
    /// Declared body length.
    body_len: usize,
}

impl Sec {
    /// The whole section, id byte included.
    fn span(&self) -> std::ops::Range<usize> {
        self.at..self.body_at + self.body_len
    }
}

/// Read a uLEB128 at `at`, returning its value and its length.
fn read_uleb(bytes: &[u8], at: usize) -> Option<(u64, usize)> {
    let mut value = 0u64;
    let mut shift = 0u32;
    let mut n = 0usize;
    loop {
        let b = *bytes.get(at + n)?;
        value |= u64::from(b & 0x7f) << shift.min(63);
        n += 1;
        if b & 0x80 == 0 {
            return Some((value, n));
        }
        shift += 7;
        if n >= 10 {
            return None;
        }
    }
}

/// Every section the decoder can reach before the module stops making sense.
fn sections(bytes: &[u8]) -> Vec<Sec> {
    let mut out = Vec::new();
    let mut at = 8;
    while at < bytes.len() {
        let Some((len, n)) = read_uleb(bytes, at + 1) else {
            break;
        };
        let body_at = at + 1 + n;
        let body_len = len as usize;
        if body_len > bytes.len() || body_at + body_len > bytes.len() {
            break;
        }
        out.push(Sec {
            at,
            size_at: at + 1,
            size_len: n,
            body_at,
            body_len,
        });
        at = body_at + body_len;
    }
    out
}

/// Replace `range` of `m`'s bytes with `with`, keeping the annotations that still fit.
///
/// Annotations that the mutation moved or overran are dropped rather than left pointing at
/// the wrong bytes, and the mutated region gets one of its own, so the hex listing under a
/// failure says exactly where the corruption is.
fn splice(m: &Module, range: std::ops::Range<usize>, with: &[u8], how: &str) -> Module {
    let mut bytes = m.bytes.clone();
    let end = range.end.min(bytes.len());
    let start = range.start.min(end);
    bytes.splice(start..end, with.iter().copied());
    let mut anns: Vec<crate::wasm::Ann> = m
        .anns
        .iter()
        .filter(|a| a.offset + a.length <= start)
        .cloned()
        .collect();
    anns.push(crate::wasm::Ann {
        offset: start,
        length: with.len(),
        field: "(mutation)".to_string(),
        value: how.to_string(),
    });
    Module {
        bytes,
        anns,
        label: format!("{}-fuzz", m.label),
    }
}

/// A module built from raw bytes, with the mutated region annotated and nothing else.
fn remade(m: &Module, bytes: Vec<u8>, how: &str) -> Module {
    Module {
        anns: vec![crate::wasm::Ann {
            offset: 0,
            length: bytes.len(),
            field: "(mutated module)".to_string(),
            value: how.to_string(),
        }],
        bytes,
        label: format!("{}-fuzz", m.label),
    }
}

// ---------------------------------------------------------------------------------------
// The mutation families
// ---------------------------------------------------------------------------------------

/// One to three bits flipped anywhere in the module.
fn flip_bits(rng: &mut StdRng, m: &Module) -> (Module, String) {
    let mut bytes = m.bytes.clone();
    if bytes.is_empty() {
        return (remade(m, bytes, "the module was empty"), "nothing".into());
    }
    let flips = rng.random_range(1..=3);
    let mut where_ = Vec::new();
    for _ in 0..flips {
        let at = rng.random_range(0..bytes.len());
        let bit = rng.random_range(0..8u8);
        bytes[at] ^= 1 << bit;
        where_.push(format!("byte {at} bit {bit}"));
    }
    let how = format!("{} flipped", where_.join(", "));
    (remade(m, bytes, &how), how)
}

/// The module cut at a random point, or with random bytes appended.
fn truncate_or_pad(rng: &mut StdRng, m: &Module) -> (Module, String) {
    if m.bytes.is_empty() {
        return (remade(m, Vec::new(), "already empty"), "nothing".into());
    }
    if rng.random_bool(0.5) {
        let keep = rng.random_range(0..m.bytes.len());
        let how = format!("truncated to {keep} of {} bytes", m.bytes.len());
        (m.truncated(keep).labelled(format!("{}-fuzz", m.label)), how)
    } else {
        let extra = rng.random_range(1..=64usize);
        let mut bytes = m.bytes.clone();
        for _ in 0..extra {
            bytes.push(rng.random::<u8>());
        }
        let how = format!("{extra} random bytes appended");
        (remade(m, bytes, &how), how)
    }
}

/// A section size or a vector count overwritten with 0, something huge, or something that
/// is merely wrong by a few bytes.
fn lying_length(rng: &mut StdRng, m: &Module) -> (Module, String) {
    let secs = sections(&m.bytes);
    if secs.is_empty() {
        return flip_bits(rng, m);
    }
    let sec = &secs[rng.random_range(0..secs.len())];
    // Half the time the section's own size, half the time the first uLEB128 of its body,
    // which is the element count of whatever vector the section holds.
    let (range, what, real) = if rng.random_bool(0.5) || sec.body_len == 0 {
        (
            sec.size_at..sec.size_at + sec.size_len,
            "the section size",
            sec.body_len as u64,
        )
    } else {
        match read_uleb(&m.bytes, sec.body_at) {
            Some((v, n)) => (sec.body_at..sec.body_at + n, "the vector count", v),
            None => (
                sec.size_at..sec.size_at + sec.size_len,
                "the section size",
                sec.body_len as u64,
            ),
        }
    };
    let (value, note) = match rng.random_range(0..5) {
        0 => (0u64, "0".to_string()),
        1 => (u64::from(u32::MAX), "4294967295".to_string()),
        2 => (0x7fff_ffff, "2147483647".to_string()),
        3 => (
            real.saturating_add(rng.random_range(1..=8)),
            "a few too many".to_string(),
        ),
        _ => (
            real.saturating_sub(rng.random_range(1..=8)),
            "a few too few".to_string(),
        ),
    };
    let how = format!(
        "{what} of the section at byte {} rewritten from {real} to {value} ({note})",
        sec.at
    );
    let bytes = uleb(value);
    (splice(m, range, &bytes, &how), how)
}

/// Ten bytes of continuation-flagged LEB128 spliced over a size, a count or a raw offset.
///
/// `ff ff ff ff ff ff ff ff ff 7f` is a syntactically complete uLEB128 that no field in the
/// format is wide enough to hold: a decoder that keeps shifting until the continuation bit
/// clears reads it happily and then indexes with the result.
fn huge_leb(rng: &mut StdRng, m: &Module) -> (Module, String) {
    const HUGE: [u8; 10] = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f];
    let secs = sections(&m.bytes);
    if secs.is_empty() || rng.random_bool(0.25) {
        if m.bytes.len() < 10 {
            return (
                remade(m, HUGE.to_vec(), "ten 0xff bytes and nothing else"),
                "replaced wholesale by a ten-byte LEB128".to_string(),
            );
        }
        let at = rng.random_range(0..m.bytes.len() - 9);
        let how = format!("a ten-byte LEB128 written over bytes {at}..{}", at + 10);
        return (splice(m, at..at + 10, &HUGE, &how), how);
    }
    let sec = &secs[rng.random_range(0..secs.len())];
    let (range, what) = if rng.random_bool(0.5) || sec.body_len == 0 {
        (sec.size_at..sec.size_at + sec.size_len, "the section size")
    } else {
        match read_uleb(&m.bytes, sec.body_at) {
            Some((_, n)) => (sec.body_at..sec.body_at + n, "the vector count"),
            None => (sec.size_at..sec.size_at + sec.size_len, "the section size"),
        }
    };
    let how = format!(
        "a ten-byte LEB128 spliced over {what} of the section at byte {}",
        sec.at
    );
    (splice(m, range, &HUGE, &how), how)
}

/// A module taken apart into the pieces section surgery moves around.
struct Parts {
    /// The eight-byte header, which no mutation in this family touches.
    header: Vec<u8>,
    /// One whole section per entry, id byte and size field included.
    chunks: Vec<Vec<u8>>,
    /// Whatever followed the last section the decoder could walk.
    tail: Vec<u8>,
}

/// Split a module into its header, its sections as whole byte strings, and any tail the
/// decoder could not parse.
fn split(m: &Module) -> Option<Parts> {
    if m.bytes.len() < 8 {
        return None;
    }
    let secs = sections(&m.bytes);
    if secs.is_empty() {
        return None;
    }
    let end = secs.last().map(|s| s.span().end).unwrap_or(8);
    Some(Parts {
        header: m.bytes[..8].to_vec(),
        chunks: secs
            .iter()
            .map(|s| m.bytes[s.span()].to_vec())
            .collect::<Vec<_>>(),
        tail: m.bytes[end..].to_vec(),
    })
}

/// A section duplicated, dropped, swapped with another, or given a different id byte.
fn section_op(rng: &mut StdRng, m: &Module) -> (Module, String) {
    let Some(Parts {
        header,
        mut chunks,
        tail,
    }) = split(m)
    else {
        return flip_bits(rng, m);
    };
    let n = chunks.len();
    let how = match rng.random_range(0..4) {
        0 => {
            let i = rng.random_range(0..n);
            let copy = chunks[i].clone();
            chunks.insert(i + 1, copy);
            format!("section {i} (id {}) duplicated", chunks[i][0])
        }
        1 => {
            let i = rng.random_range(0..n);
            let id = chunks[i][0];
            chunks.remove(i);
            format!("section {i} (id {id}) dropped")
        }
        2 if n >= 2 => {
            let i = rng.random_range(0..n);
            let mut j = rng.random_range(0..n);
            if j == i {
                j = (i + 1) % n;
            }
            let (a, b) = (chunks[i][0], chunks[j][0]);
            chunks.swap(i, j);
            format!("sections {i} (id {a}) and {j} (id {b}) swapped")
        }
        _ => {
            let i = rng.random_range(0..n);
            let was = chunks[i][0];
            let now = rng.random::<u8>();
            chunks[i][0] = now;
            format!("section {i}'s id changed from {was} to {now}")
        }
    };
    let mut bytes = header;
    for c in &chunks {
        bytes.extend_from_slice(c);
    }
    bytes.extend_from_slice(&tail);
    (remade(m, bytes, &how), how)
}

/// Two of the other families, applied in turn.
fn two_at_once(rng: &mut StdRng, m: &Module) -> (Module, String) {
    const STEPS: [Mutation; 5] = [
        flip_bits,
        truncate_or_pad,
        lying_length,
        huge_leb,
        section_op,
    ];
    let first = STEPS[rng.random_range(0..STEPS.len())];
    let second = STEPS[rng.random_range(0..STEPS.len())];
    let (once, how1) = first(rng, m);
    let (twice, how2) = second(rng, &once);
    let how = format!("{how1}, then {how2}");
    (twice.labelled(format!("{}-fuzz", m.label)), how)
}

// ---------------------------------------------------------------------------------------
// What the runtime is allowed to have done
// ---------------------------------------------------------------------------------------

/// How one mutated module ended, for the tally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Refused before anything ran: a non-zero exit and an empty stdout.
    Refused,
    /// It validated, ran, and trapped.
    Trapped,
    /// It validated, ran, and finished.
    Ran,
}

/// True when a line is something a runtime could print as a returned value.
///
/// Integers of both widths and both signednesses, floats the way Rust's `Display` writes
/// them, and the three special float spellings `inf`, `-inf` and `NaN` — all of which
/// `f64::from_str` accepts, which is why one parse covers the lot.
fn is_value_line(line: &str) -> bool {
    !line.is_empty() && line.parse::<f64>().is_ok()
}

/// Check one invocation against the only three things this stage promises.
///
/// Returns how it ended, or the sentence explaining what it did that no runtime may.
fn inspect(seed: &Seed, run: &Run) -> Result<Outcome, String> {
    if run.stdout.len() > MAX_OUTPUT {
        return Err(format!(
            "{} bytes on stdout from a module of a few dozen bytes",
            run.stdout.len()
        ));
    }
    // The three `--invoke` seeds import nothing and have no way to reach a stream, so
    // anything on their stdout is a returned value and nothing else.
    if seed.export.is_some() {
        if run.exit.failed() && !run.stdout.is_empty() {
            return Err(format!(
                "a non-zero exit ({}) after printing {:?} — a module that is refused has not \
                 run, and one that traps has no result to print",
                run.exit.label(),
                run.stdout
            ));
        }
        if run.exit.success() {
            if let Some(bad) = run.lines().iter().find(|l| !is_value_line(l)) {
                return Err(format!(
                    "exit 0 after printing {bad:?}, which is not a value a valid module could \
                     have returned"
                ));
            }
        }
    }
    Ok(if run.exit.success() {
        Outcome::Ran
    } else if run.stderr.to_lowercase().contains("trap") {
        Outcome::Trapped
    } else {
        Outcome::Refused
    })
}

/// Anything in the test's temporary directory that the harness did not put there.
fn stray_files(ctx: &Ctx) -> Vec<String> {
    let Ok(dir) = std::fs::read_dir(&ctx.tmp) else {
        return Vec::new();
    };
    dir.flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.ends_with(".wasm"))
        .collect()
}

// ---------------------------------------------------------------------------------------
// The driver
// ---------------------------------------------------------------------------------------

/// Run `count` mutations of one family and tally what the runtime did with them.
fn sweep(
    ctx: &mut Ctx,
    family: &'static str,
    count: usize,
    mutate: Mutation,
) -> Result<(), Failure> {
    // Each mutated module gets its own short deadline, well inside the test's own, so a
    // mutation that turns into an infinite loop is reported as a hang instead of eating
    // the whole family.
    ctx.timeout = PER_INVOCATION;
    let seeds = seeds();
    let replay = format!(
        "replay it with --seed {:#x} --stage 43 --only \"{family}\"",
        ctx.seed
    );
    let mut refused = 0usize;
    let mut trapped = 0usize;
    let mut ran = 0usize;
    for i in 0..count {
        let seed = &seeds[i % seeds.len()];
        let (m, how) = mutate(&mut ctx.rng, &seed.module);
        let context = format!("mutation {i} of '{family}': {} {how}", seed.what);
        let run = match seed.export {
            Some(name) => ctx.invoke(&m, name, &[]),
            None => ctx.start(&m, &[]),
        }
        .map_err(|f| f.note(context.clone()).note(replay.clone()))?;
        match inspect(seed, &run) {
            Ok(Outcome::Refused) => refused += 1,
            Ok(Outcome::Trapped) => trapped += 1,
            Ok(Outcome::Ran) => ran += 1,
            Err(why) => {
                let mut c = Check::new(context.clone(), &run);
                c.module(&m);
                c.that(
                    "outcome",
                    "either a refusal — a non-zero exit with nothing on stdout — or a run \
                     whose output a valid module could have produced",
                    false,
                    why,
                );
                c.note(format!(
                    "the runtime said: {}",
                    first_meaningful_line(&run.stderr)
                ));
                c.note("a mutated module that is still valid is a pass; this one was neither");
                c.note(replay.clone());
                c.finish()?;
            }
        }
    }
    let stray = stray_files(ctx);
    if !stray.is_empty() {
        return Err(Failure::new(
            FailureKind::Assertion,
            format!(
                "{} file(s) appeared in the test's temporary directory that the harness did \
                 not write: {}",
                stray.len(),
                stray.join(", ")
            ),
        )
        .note("a runtime given a module writes nothing but what the module asks it to")
        .note(replay));
    }
    ctx.note(format!(
        "{count} '{family}' modules (seed {:#x}): {refused} refused, {trapped} ran and \
         trapped, {ran} still valid enough to run to completion",
        ctx.seed
    ));
    Ok(())
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

wasm_test!(bit_flips, |ctx| {
    sweep(ctx, "bits flipped", PER_FAMILY, flip_bits)
});

wasm_test!(truncation, |ctx| {
    sweep(ctx, "truncated or padded", PER_FAMILY, truncate_or_pad)
});

wasm_test!(lying_lengths, |ctx| {
    sweep(ctx, "lying section size", PER_FAMILY, lying_length)
});

wasm_test!(huge_lebs, |ctx| {
    sweep(ctx, "ten-byte LEB128", PER_FAMILY, huge_leb)
});

wasm_test!(section_surgery, |ctx| {
    sweep(ctx, "section surgery", PER_FAMILY, section_op)
});

wasm_test!(mixed, |ctx| {
    sweep(ctx, "two mutations at once", MIXED, two_at_once)
});

// ---------------------------------------------------------------------------------------
// Examples
// ---------------------------------------------------------------------------------------

/// The numeric seed with the size of its code section rewritten to zero.
fn example_lying_size() -> Module {
    let m = numeric_seed();
    let secs = sections(&m.bytes);
    // Section id 10 is the code section; its size is the one field nothing else can check.
    match secs.iter().find(|s| m.bytes[s.at] == 10) {
        Some(s) => splice(
            &m,
            s.size_at..s.size_at + s.size_len,
            &[0x00],
            "the code section's size, rewritten from its real length to 0",
        )
        .labelled("fuzz-lying-size"),
        None => m,
    }
}

/// Worked examples: one mutation with a known answer, and the loop that makes five hundred.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("The numeric seed, untouched", numeric_seed)
            .summary(
                "the module the first quarter of every family starts from: two types, two \
                 functions — `double(x) = x * 2` and `f() = double(21)` — one export and a \
                 custom section the format says must be ignored",
            )
            .command("run --invoke f mod.wasm")
            .output("42")
            .note(
                "Three more seeds join it: one with a memory and two data segments, one with \
                 a table and a `call_indirect`, and a WASI command that writes five bytes to \
                 stdout. The fuzzer cycles through all four so no family only ever corrupts \
                 one shape of module.",
            ),
        ExampleSpec::module(
            "The same module, with a code section of length 0",
            example_lying_size,
        )
        .summary(
            "one byte changed: the code section's size uLEB128 now says 0, so the two \
                 function bodies that follow it belong to no section at all and the decoder \
                 reaches them expecting the id byte of the next one",
        )
        .command("run --invoke f mod.wasm")
        .output(
            "nothing on stdout, a message on stderr, and a non-zero exit status — the \
                 module never runs",
        )
        .note(
            "This is the whole stage in one module. A size field is a claim about bytes \
                 the decoder has not looked at yet; believing it and then indexing past the \
                 end of the buffer is the commonest way a hand-written decoder dies. wasmtime \
                 reads an empty code section, tries the abandoned body bytes as the next \
                 section header, and reports an unexpected end of file — but any non-zero \
                 exit with an empty stdout is right, because the suite never compares the \
                 wording.",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    fn rng() -> StdRng {
        StdRng::seed_from_u64(43)
    }

    #[test]
    fn the_families_add_up_to_five_hundred() {
        assert_eq!(5 * PER_FAMILY + MIXED, 500);
        assert_ne!(MIXED, 0, "the mixed family must actually run");
    }

    #[test]
    fn every_seed_module_is_well_formed_before_it_is_touched() {
        for s in seeds() {
            assert!(
                s.module.bytes.starts_with(b"\0asm\x01\0\0\0"),
                "{} does not start with the header",
                s.what
            );
            assert!(
                !sections(&s.module.bytes).is_empty(),
                "{} has no sections the decoder can walk",
                s.what
            );
        }
    }

    #[test]
    fn the_section_walk_covers_the_whole_module() {
        for s in seeds() {
            let secs = sections(&s.module.bytes);
            let end = secs.last().map(|x| x.span().end).unwrap_or(0);
            assert_eq!(
                end,
                s.module.bytes.len(),
                "{}: the walk stopped at {end} of {} bytes",
                s.what,
                s.module.bytes.len()
            );
        }
    }

    #[test]
    fn every_family_produces_a_module_and_a_sentence() {
        let mut r = rng();
        for f in [
            flip_bits,
            truncate_or_pad,
            lying_length,
            huge_leb,
            section_op,
            two_at_once,
        ] {
            for s in seeds() {
                for _ in 0..20 {
                    let (m, how) = f(&mut r, &s.module);
                    assert!(!how.is_empty(), "a mutation must say what it did");
                    for a in &m.anns {
                        assert!(
                            a.offset + a.length <= m.bytes.len(),
                            "annotation {a:?} runs past a {}-byte module",
                            m.bytes.len()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_mutation_usually_changes_something() {
        let mut r = rng();
        let seed = numeric_seed();
        let mut changed = 0;
        for _ in 0..50 {
            if flip_bits(&mut r, &seed).0.bytes != seed.bytes {
                changed += 1;
            }
        }
        assert_eq!(changed, 50, "a bit flip always changes a byte");
    }

    #[test]
    fn value_lines_are_the_ones_a_runtime_could_print() {
        for good in [
            "0",
            "-1",
            "42",
            "1.5",
            "-0",
            "inf",
            "-inf",
            "NaN",
            "18446744073709551615",
        ] {
            assert!(is_value_line(good), "{good} is a value a runtime prints");
        }
        for bad in [
            "",
            "Error: bad module",
            "thread 'main' panicked",
            "0x2a",
            "??",
        ] {
            assert!(!is_value_line(bad), "{bad:?} is not a returned value");
        }
    }

    #[test]
    fn a_ten_byte_leb_is_read_as_syntactically_complete() {
        let huge = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f];
        assert_eq!(read_uleb(&huge, 0).map(|(_, n)| n), Some(10));
        // Eleven continuation bytes are not a LEB128 at all.
        assert_eq!(read_uleb(&[0xff; 12], 0), None);
        assert_eq!(read_uleb(&[0x7f], 0), Some((127, 1)));
    }
}
