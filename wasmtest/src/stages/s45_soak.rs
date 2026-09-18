//! Stage 45 — Soak: compute, repetition and determinism. **[ext]**
//!
//! Six long-running modules, every one of them checked against an answer this file computes
//! independently in Rust. Nothing here compares a runtime against a stopwatch: the only hard
//! limit is the per-test timeout, which is raised to two minutes so a straightforward
//! interpreter has room, and the wall-clock times are reported through `ctx.note` so a green
//! run still says how long the work took. A runtime that is twice as slow as `wasmtime`
//! passes; a runtime that is fast and wrong does not.
//!
//! The floating-point test is deliberately built out of values that are exact in binary —
//! the sum of the first ten million integers is an integer far below `2^53`, so every
//! partial sum is representable and the comparison is about arithmetic rather than about
//! how many digits a formatter prints.
//!
//! Two of the tests run the same module again and again — two hundred and twenty times, and
//! fifty times — and demand that every answer is identical, byte for byte on stdout.
//! Determinism is the one property a WebAssembly runtime may never trade away: the same
//! module with the same arguments produces the same bytes, whether it is interpreted the
//! first time or compiled the thousandth.
//!
//! The work was sized against the reference and then given room: the Collatz sweep is 131
//! million steps, which `wasmtime` compiles and runs in about two hundred milliseconds. An
//! interpreter reading its own bytecode is perhaps a hundred times slower, which is still
//! comfortably inside the two-minute floor; if your runtime is slower than that, raise
//! `--timeout-ms` rather than shrinking the sweep, because the whole point of the stage is
//! that a long run and a short one give the same answer.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::stages::{expect_line, expect_lines, show_f32, show_f64, Stage, Test};
use crate::wasm::{
    ftype, global_i32, op, BlockType, Expr, Func, Limits, Module, ModuleBuilder, TableType, ValType,
};
use crate::wasm_test;
use std::time::Instant;

/// Collatz starting values swept by the integer benchmark.
const COLLATZ_N: i64 = 1_000_000;
/// Integers summed, in order, by the floating-point benchmark.
const FP_N: i64 = 10_000_000;
/// Rounds of the 64-bit hash the repetition test runs.
const HASH_ROUNDS: i64 = 200_000;
/// Invocations of that module, each in its own process.
const REPEATS: usize = 220;
/// Invocations of the everything-at-once module.
const IDENTICAL_RUNS: usize = 50;
/// Rounds of the per-argument hash.
const ARG_ROUNDS: u32 = 64;
/// Arguments the per-argument test sweeps.
const ARG_COUNT: u32 = 64;
/// Pages the memory-heavy module grows to: 128 × 64 KiB is eight mebibytes.
const PAGES: u32 = 128;
/// Bytes in that memory, written and read back one at a time.
const MEM_BYTES: i64 = PAGES as i64 * 65_536;

/// A test of this stage: `ext`, `slow`, and given two minutes before the harness gives up.
fn soak_test(name: &'static str, run: crate::stages::TestFn) -> Test {
    Test::new(name, run)
        .ext()
        .tag("slow")
        .min_timeout_ms(120_000)
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 45,
        slug: "soak",
        name: "Soak: compute, repetition and determinism",
        ext: true,
        hints: &[
            "A hundred million loop iterations must not allocate: the operand stack of a loop body returns to the same height every time round, so reuse the frame instead of pushing a new one",
            "The same module with the same arguments must print byte-identical output every single time — determinism is the one thing a runtime may never trade for speed",
            "Integer arithmetic wraps and is exact; float arithmetic is IEEE-754 to the letter, so summing the first ten million integers in f64 gives exactly 50000005000000 and nothing near it",
            "Growing a memory to eight megabytes and touching every byte is a normal thing for a module to do — keep the pages in one allocation and index it, do not copy on grow more than you must",
        ],
        examples,
        tests: vec![
            soak_test(
                "a million Collatz sweeps produce the step count Rust computes",
                collatz,
            ),
            soak_test(
                "ten million floating-point additions produce an exact sum",
                float_sum,
            ),
            soak_test(
                "two hundred and twenty invocations of one module all answer the same",
                repeated_invocations,
            ),
            soak_test(
                "a module using every feature at once prints byte-identical output fifty times",
                everything_at_once,
            ),
            soak_test(
                "sixty-four different arguments each produce the answer Rust computes",
                many_arguments,
            ),
            soak_test(
                "eight megabytes of memory are grown, filled and checksummed",
                memory_soak,
            ),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// The answers, computed here so the module is never compared against itself
// ---------------------------------------------------------------------------------------

/// Total Collatz stopping time over `1..=n`, in wrapping 64-bit arithmetic.
fn collatz_steps(n: i64) -> i64 {
    let mut total: u64 = 0;
    for start in 1..=n as u64 {
        let mut cur = start;
        while cur != 1 {
            cur = if cur & 1 == 0 {
                cur / 2
            } else {
                cur.wrapping_mul(3).wrapping_add(1)
            };
            total = total.wrapping_add(1);
        }
    }
    total as i64
}

/// `1.0 + 2.0 + … + n` in f64, accumulated in the same order the module does.
fn float_sum_of(n: i64) -> f64 {
    let mut acc = 0.0f64;
    let mut i = 1i64;
    while i <= n {
        acc += i as f64;
        i += 1;
    }
    acc
}

/// The 64-bit FNV-style hash the repetition test computes.
fn hash64(rounds: i64) -> i64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for i in 0..rounds as u64 {
        h ^= i;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h as i64
}

/// The 32-bit hash the per-argument test computes for one argument.
fn hash32(n: u32, rounds: u32) -> i32 {
    let mut h: u32 = 2_166_136_261;
    for k in 0..rounds {
        h ^= n.wrapping_add(k);
        h = h.wrapping_mul(16_777_619);
    }
    h as i32
}

/// The bytes the everything-at-once module keeps at address 0.
const MIXED_DATA: [u8; 8] = [3, 1, 4, 1, 5, 9, 2, 6];
/// Its mutable global's initial value.
const MIXED_GLOBAL: i32 = 100;

/// The four values [`mixed_module`] must print, one per line.
///
/// This is the whole module written out again in Rust: the loop over the data bytes, the
/// indirect call chosen by the low bit, the `if`/`else`, the global, the byte written back
/// into memory and read out again, and then the three numeric widths derived from the
/// accumulator.
fn mixed_answer() -> Vec<String> {
    let mut acc: i32 = 0;
    let mut global: i32 = MIXED_GLOBAL;
    let mut written = [0u8; 8];
    for (i, b) in MIXED_DATA.iter().map(|b| i32::from(*b)).enumerate() {
        let arg = acc.wrapping_add(b);
        // Table slot 0 is `x + 1`, slot 1 is `x * 3`; the low bit of the byte picks one.
        acc = if b & 1 == 0 {
            arg.wrapping_add(1)
        } else {
            arg.wrapping_mul(3)
        };
        acc = acc.wrapping_add(if b > 4 { 100 } else { 1 });
        global = global.wrapping_add(b);
        written[i] = acc as u8;
    }
    let last = i32::from(written[7]);
    let i32_result = acc.wrapping_add(global).wrapping_add(last);
    let i64_result = (acc as i64).wrapping_mul(1_000_003).wrapping_add(7);
    let f32_result = acc as f32 / 4.0;
    let f64_result = acc as f64 * 0.5 + 0.125;
    vec![
        i32_result.to_string(),
        i64_result.to_string(),
        show_f32(f32_result),
        show_f64(f64_result),
    ]
}

// ---------------------------------------------------------------------------------------
// The modules
// ---------------------------------------------------------------------------------------

/// `while (i <= n) { … }` as a `block`/`loop` pair; `cond` must leave the *exit* condition.
///
/// Label 0 inside `body` is the loop itself, so `br 0` is another turn; label 1 is the
/// block, so `br_if 1` leaves. Every loop in this stage is built from this one shape, which
/// is also the shape a runtime has to make cheap: the operand stack is the same height at
/// the top of every iteration.
fn while_loop(exit_when: Expr, body: Expr) -> Expr {
    Expr::new().block(
        BlockType::Empty,
        Expr::new().loop_(BlockType::Empty, exit_when.br_if(1).then(body).br(0)),
    )
}

/// Sum the Collatz stopping time of every start value in `1..=n`.
///
/// Locals: 0 the start value, 1 the current value, 2 the running total, all i64.
fn collatz_module(n: i64) -> Module {
    let inner = while_loop(
        Expr::new().local_get(1).i64_const(1).op(op::I64_EQ),
        Expr::new()
            .local_get(1)
            .i64_const(1)
            .op(op::I64_AND)
            .op(op::I64_EQZ)
            .if_else(
                BlockType::Empty,
                Expr::new()
                    .local_get(1)
                    .i64_const(2)
                    .op(op::I64_DIV_U)
                    .local_set(1),
                Expr::new()
                    .local_get(1)
                    .i64_const(3)
                    .op(op::I64_MUL)
                    .i64_const(1)
                    .op(op::I64_ADD)
                    .local_set(1),
            )
            .local_get(2)
            .i64_const(1)
            .op(op::I64_ADD)
            .local_set(2),
    );
    let outer = while_loop(
        Expr::new().local_get(0).i64_const(n).op(op::I64_GT_U),
        Expr::new()
            .local_get(0)
            .local_set(1)
            .then(inner)
            .local_get(0)
            .i64_const(1)
            .op(op::I64_ADD)
            .local_set(0),
    );
    let body = Expr::new()
        .i64_const(1)
        .local_set(0)
        .then(outer)
        .local_get(2);
    let mut b = ModuleBuilder::new(format!("collatz-{n}"));
    let ty = b.add_type(ftype(&[], &[ValType::I64]));
    let f = b.add_func(ty, Func::with_locals(&[(3, ValType::I64)], body));
    b.export_func("f", f).build()
}

/// Add `1.0, 2.0, … n` to an f64 accumulator in that order.
///
/// Locals: 0 the counter (i64), 1 the accumulator (f64).
fn float_sum_module(n: i64) -> Module {
    let body = Expr::new()
        .i64_const(1)
        .local_set(0)
        .then(while_loop(
            Expr::new().local_get(0).i64_const(n).op(op::I64_GT_S),
            Expr::new()
                .local_get(1)
                .local_get(0)
                .op(op::F64_CONVERT_I64_S)
                .op(op::F64_ADD)
                .local_set(1)
                .local_get(0)
                .i64_const(1)
                .op(op::I64_ADD)
                .local_set(0),
        ))
        .local_get(1);
    let mut b = ModuleBuilder::new(format!("float-sum-{n}"));
    let ty = b.add_type(ftype(&[], &[ValType::F64]));
    let f = b.add_func(
        ty,
        Func::with_locals(&[(1, ValType::I64), (1, ValType::F64)], body),
    );
    b.export_func("f", f).build()
}

/// `h = 0xcbf29ce484222325; for i in 0..rounds { h ^= i; h *= 0x100000001b3 }`.
fn hash64_module(rounds: i64) -> Module {
    let body = Expr::new()
        .i64_const(0xcbf2_9ce4_8422_2325u64 as i64)
        .local_set(1)
        .i64_const(0)
        .local_set(0)
        .then(while_loop(
            Expr::new().local_get(0).i64_const(rounds).op(op::I64_GE_U),
            Expr::new()
                .local_get(1)
                .local_get(0)
                .op(op::I64_XOR)
                .i64_const(0x0000_0100_0000_01b3)
                .op(op::I64_MUL)
                .local_set(1)
                .local_get(0)
                .i64_const(1)
                .op(op::I64_ADD)
                .local_set(0),
        ))
        .local_get(1);
    let mut b = ModuleBuilder::new(format!("hash64-{rounds}"));
    let ty = b.add_type(ftype(&[], &[ValType::I64]));
    let f = b.add_func(ty, Func::with_locals(&[(2, ValType::I64)], body));
    b.export_func("f", f).build()
}

/// The same 32-bit hash, over the argument the invocation is given.
///
/// Locals: 0 is the parameter, 1 the round counter, 2 the hash.
fn hash32_module(rounds: u32) -> Module {
    let body = Expr::new()
        .i32_const(2_166_136_261u32 as i32)
        .local_set(2)
        .i32_const(0)
        .local_set(1)
        .then(while_loop(
            Expr::new()
                .local_get(1)
                .i32_const(rounds as i32)
                .op(op::I32_GE_U),
            Expr::new()
                .local_get(2)
                .local_get(0)
                .local_get(1)
                .op(op::I32_ADD)
                .op(op::I32_XOR)
                .i32_const(16_777_619)
                .op(op::I32_MUL)
                .local_set(2)
                .local_get(1)
                .i32_const(1)
                .op(op::I32_ADD)
                .local_set(1),
        ))
        .local_get(2);
    let mut b = ModuleBuilder::new(format!("hash32-{rounds}"));
    let ty = b.add_type(ftype(&[ValType::I32], &[ValType::I32]));
    let f = b.add_func(ty, Func::with_locals(&[(2, ValType::I32)], body));
    b.export_func("f", f).build()
}

/// One module that uses everything the previous forty-four stages tested.
///
/// A memory with a data segment, a table with two entries reached by `call_indirect`, a
/// mutable global, a `loop` with a `br_if` and an `if`/`else` inside it, a byte written to
/// memory and read back, and finally the accumulator rendered as i32, i64, f32 and f64.
fn mixed_module() -> Module {
    let mut b = ModuleBuilder::new("everything-at-once");
    let unary = b.add_type(ftype(&[ValType::I32], &[ValType::I32]));
    let plus_one = b.add_func(
        unary,
        Func::new(Expr::new().local_get(0).i32_const(1).op(op::I32_ADD)),
    );
    let times_three = b.add_func(
        unary,
        Func::new(Expr::new().local_get(0).i32_const(3).op(op::I32_MUL)),
    );
    let quad = b.add_type(ftype(
        &[],
        &[ValType::I32, ValType::I64, ValType::F32, ValType::F64],
    ));

    // Locals: 0 the index, 1 the accumulator, 2 the byte just read.
    let loop_body = Expr::new()
        // byte = data[i]
        .local_get(0)
        .mem(op::I32_LOAD8_U, 0, 0)
        .local_set(2)
        // acc = table[byte & 1](acc + byte)
        .local_get(1)
        .local_get(2)
        .op(op::I32_ADD)
        .local_get(2)
        .i32_const(1)
        .op(op::I32_AND)
        .call_indirect(unary, 0)
        .local_set(1)
        // acc += byte > 4 ? 100 : 1
        .local_get(2)
        .i32_const(4)
        .op(op::I32_GT_U)
        .if_else(
            BlockType::Empty,
            Expr::new()
                .local_get(1)
                .i32_const(100)
                .op(op::I32_ADD)
                .local_set(1),
            Expr::new()
                .local_get(1)
                .i32_const(1)
                .op(op::I32_ADD)
                .local_set(1),
        )
        // global += byte
        .global_get(0)
        .local_get(2)
        .op(op::I32_ADD)
        .global_set(0)
        // memory[64 + i] = acc as u8
        .local_get(0)
        .i32_const(64)
        .op(op::I32_ADD)
        .local_get(1)
        .mem(op::I32_STORE8, 0, 0)
        .local_get(0)
        .i32_const(1)
        .op(op::I32_ADD)
        .local_set(0);

    let body = Expr::new()
        .i32_const(0)
        .local_set(0)
        .i32_const(0)
        .local_set(1)
        .then(while_loop(
            Expr::new()
                .local_get(0)
                .i32_const(MIXED_DATA.len() as i32)
                .op(op::I32_GE_U),
            loop_body,
        ))
        // i32: acc + global + the last byte written back into memory
        .local_get(1)
        .global_get(0)
        .op(op::I32_ADD)
        .i32_const(64 + MIXED_DATA.len() as i32 - 1)
        .mem(op::I32_LOAD8_U, 0, 0)
        .op(op::I32_ADD)
        // i64: widened, multiplied, offset
        .local_get(1)
        .op(op::I64_EXTEND_I32_S)
        .i64_const(1_000_003)
        .op(op::I64_MUL)
        .i64_const(7)
        .op(op::I64_ADD)
        // f32: an exact quarter
        .local_get(1)
        .op(op::F32_CONVERT_I32_S)
        .f32_const(4.0)
        .op(op::F32_DIV)
        // f64: an exact half plus an exact eighth
        .local_get(1)
        .op(op::F64_CONVERT_I32_S)
        .f64_const(0.5)
        .op(op::F64_MUL)
        .f64_const(0.125)
        .op(op::F64_ADD);

    let f = b.add_func(quad, Func::with_locals(&[(3, ValType::I32)], body));
    b.memory(Limits::min(1))
        .data_active(0, &MIXED_DATA)
        .global(global_i32(MIXED_GLOBAL, true))
        .table(TableType {
            elem: ValType::FuncRef,
            limits: Limits::min(2),
        })
        .elem_active(0, &[plus_one, times_three])
        .export_func("f", f)
        .build()
}

/// Grow the memory to `pages`, write `i & 0xff` into every byte, then add them all up.
///
/// Returns the page count `memory.grow` reported, the page count after it, and the
/// checksum: `memory.grow` answering `-1` and a fill that silently stops at the first page
/// are both wrong lines rather than crashes.
fn memory_soak_module(pages: u32) -> Module {
    let bytes = pages as i64 * 65_536;
    let fill = while_loop(
        Expr::new()
            .local_get(0)
            .i32_const(bytes as i32)
            .op(op::I32_GE_U),
        Expr::new()
            .local_get(0)
            .local_get(0)
            .i32_const(255)
            .op(op::I32_AND)
            .mem(op::I32_STORE8, 0, 0)
            .local_get(0)
            .i32_const(1)
            .op(op::I32_ADD)
            .local_set(0),
    );
    let sum = while_loop(
        Expr::new()
            .local_get(0)
            .i32_const(bytes as i32)
            .op(op::I32_GE_U),
        Expr::new()
            .local_get(1)
            .local_get(0)
            .mem(op::I32_LOAD8_U, 0, 0)
            .op(op::I32_ADD)
            .local_set(1)
            .local_get(0)
            .i32_const(1)
            .op(op::I32_ADD)
            .local_set(0),
    );
    let body = Expr::new()
        .i32_const(pages as i32 - 1)
        .memory_grow()
        .i32_const(0)
        .local_set(0)
        .then(fill)
        .i32_const(0)
        .local_set(0)
        .i32_const(0)
        .local_set(1)
        .then(sum)
        .memory_size()
        .local_get(1);
    let mut b = ModuleBuilder::new(format!("memory-soak-{pages}"));
    let ty = b.add_type(ftype(&[], &[ValType::I32; 3]));
    let f = b.add_func(ty, Func::with_locals(&[(2, ValType::I32)], body));
    b.memory(Limits::range(1, pages))
        .export_func("f", f)
        .build()
}

/// The checksum [`memory_soak_module`] must report: every byte of `n` is `i & 0xff`.
fn memory_checksum(n: i64) -> i32 {
    let whole = n / 256;
    (whole.wrapping_mul(255 * 256 / 2) & 0xffff_ffff) as u32 as i32
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

/// `1.23 s` or `412 ms`, whichever reads better.
fn took(started: Instant) -> String {
    let ms = started.elapsed().as_millis();
    if ms >= 1_000 {
        format!("{:.2} s", ms as f64 / 1_000.0)
    } else {
        format!("{ms} ms")
    }
}

wasm_test!(collatz, |ctx| {
    let want = collatz_steps(COLLATZ_N);
    let m = collatz_module(COLLATZ_N);
    let started = Instant::now();
    expect_line(ctx, &m, "f", &[], &want.to_string())?;
    ctx.note(format!(
        "the Collatz sweep over 1..={COLLATZ_N} took {} steps and {} of wall clock",
        want,
        took(started)
    ));
    Ok(())
});

wasm_test!(float_sum, |ctx| {
    let want = float_sum_of(FP_N);
    let m = float_sum_module(FP_N);
    let started = Instant::now();
    let run = expect_line(ctx, &m, "f", &[], &show_f64(want))?;
    let mut c = Check::new("that the sum is exact rather than merely close", &run);
    c.eq("sum", (FP_N * (FP_N + 1) / 2).to_string(), run.first_line());
    c.note(
        "every partial sum is a whole number below 2^53, so f64 holds each one exactly and \
         the answer is the closed form n(n+1)/2 to the digit",
    );
    c.finish()?;
    ctx.note(format!(
        "{FP_N} f64 additions in {}, summing to {}",
        took(started),
        show_f64(want)
    ));
    Ok(())
});

wasm_test!(repeated_invocations, |ctx| {
    let want = hash64(HASH_ROUNDS).to_string();
    let m = hash64_module(HASH_ROUNDS);
    let started = Instant::now();
    let mut first: Option<String> = None;
    for i in 0..REPEATS {
        let run = ctx.invoke(&m, "f", &[])?;
        let got = run.stdout.clone();
        let mut c = Check::new(format!("invocation {} of {REPEATS}", i + 1), &run);
        c.module(&m);
        c.eq("stdout.line[0]", want.as_str(), run.first_line().as_str());
        if let Some(seen) = &first {
            c.eq("stdout", seen.as_str(), got.as_str());
            c.note("every invocation of one module must produce the same bytes on stdout");
        }
        c.finish()?;
        first.get_or_insert(got);
    }
    ctx.note(format!(
        "{REPEATS} invocations of the same module, all answering {want}, in {} ({:.0} ms each)",
        took(started),
        started.elapsed().as_millis() as f64 / REPEATS as f64
    ));
    Ok(())
});

wasm_test!(everything_at_once, |ctx| {
    let want = mixed_answer();
    let refs: Vec<&str> = want.iter().map(String::as_str).collect();
    let m = mixed_module();
    let started = Instant::now();
    let first = expect_lines(ctx, &m, "f", &[], &refs)?;
    for i in 1..IDENTICAL_RUNS {
        let run = ctx.invoke(&m, "f", &[])?;
        let mut c = Check::new(format!("run {} of {IDENTICAL_RUNS}", i + 1), &run);
        c.module(&m);
        c.eq("stdout", first.stdout.as_str(), run.stdout.as_str());
        c.note(
            "the module has no clock, no randomness and no imports: the same bytes in must \
             give the same bytes out every single time",
        );
        c.finish()?;
    }
    ctx.note(format!(
        "{IDENTICAL_RUNS} runs of the memory/table/global/float module, all printing {:?}, in {}",
        want.join(" "),
        took(started)
    ));
    Ok(())
});

wasm_test!(many_arguments, |ctx| {
    let m = hash32_module(ARG_ROUNDS);
    let started = Instant::now();
    for k in 0..ARG_COUNT {
        let n = k.wrapping_mul(997);
        let arg = n.to_string();
        let want = hash32(n, ARG_ROUNDS).to_string();
        expect_line(ctx, &m, "f", &[&arg], &want).map_err(|f: Failure| {
            f.note(format!(
                "the argument was {n}; `run --invoke f mod.wasm {n}` replays it"
            ))
        })?;
    }
    ctx.note(format!(
        "{ARG_COUNT} arguments, {ARG_ROUNDS} hash rounds each, in {}",
        took(started)
    ));
    Ok(())
});

wasm_test!(memory_soak, |ctx| {
    let m = memory_soak_module(PAGES);
    let want = [
        "1".to_string(),
        PAGES.to_string(),
        memory_checksum(MEM_BYTES).to_string(),
    ];
    let refs: Vec<&str> = want.iter().map(String::as_str).collect();
    let started = Instant::now();
    let run = expect_lines(ctx, &m, "f", &[], &refs)?;
    let mut c = Check::new("that the memory really grew before it was filled", &run);
    c.module(&m);
    c.ne("memory.grow", "-1", run.first_line().as_str());
    c.note(
        "memory.grow answers the page count *before* the growth, so 1 here; a -1 means the \
         growth was refused and the fill would have trapped instead",
    );
    c.finish()?;
    ctx.note(format!(
        "{MEM_BYTES} bytes ({} pages) written and read back one byte at a time in {}",
        PAGES,
        took(started)
    ));
    Ok(())
});

/// Worked examples: the shape every loop in this stage is built from, and the module that
/// uses every feature at once.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("A counted loop, ten thousand rounds", || {
            hash64_module(10_000)
        })
        .summary(
            "`h = 0xcbf29ce484222325; for i in 0..10000 { h ^= i; h *= 0x100000001b3 }`, \
             written as a `block` holding a `loop` whose `br_if 1` leaves and whose `br 0` \
             goes round again. The test runs two hundred thousand rounds, two hundred and \
             twenty times",
        )
        .command("run --invoke f mod.wasm")
        .output("2284529141941492341")
        .note(
            "The whole loop is two locals-worth of state and no allocation: the operand \
             stack is empty at the top of every iteration, which is what makes a `loop` \
             cheap. i64 results print signed, so a hash whose top bit is set comes out \
             negative.",
        ),
        ExampleSpec::module("Every feature in one module", mixed_module)
            .summary(
                "a memory with a data segment, a two-entry table reached by `call_indirect`, \
                 a mutable global, a loop with an `if`/`else` inside it, a byte stored and \
                 read back, and four results — i32, i64, f32 and f64 — derived from the \
                 accumulator",
            )
            .command("run --invoke f mod.wasm")
            .output("1993\n1699005104\n424.75\n849.625")
            .note(
                "Four returned values, four lines, in declaration order. The test runs this \
                 module fifty times and demands the same four lines, byte for byte, every \
                 time: nothing in it can observe the clock, the host or the run number.",
            ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_answers_are_computed_not_copied() {
        // Hand-worked: 1 needs 0 steps, 2 needs 1, 3 needs 7, 4 needs 2, 5 needs 5.
        assert_eq!(collatz_steps(1), 0);
        assert_eq!(collatz_steps(2), 1);
        assert_eq!(
            collatz_steps(5),
            1 + 7 + 2 + 5,
            "1 itself needs no steps at all"
        );
    }

    #[test]
    fn the_float_sum_is_the_closed_form_exactly() {
        for n in [10i64, 1_000, 1_000_000] {
            assert_eq!(float_sum_of(n), (n * (n + 1) / 2) as f64);
            assert_eq!(show_f64(float_sum_of(n)), (n * (n + 1) / 2).to_string());
        }
    }

    #[test]
    fn the_hashes_move_with_their_input() {
        assert_ne!(hash64(1), hash64(2));
        assert_ne!(hash32(0, 64), hash32(1, 64));
        assert_eq!(hash32(0, 0), 2_166_136_261u32 as i32);
    }

    #[test]
    fn the_memory_checksum_is_the_sum_of_every_byte() {
        // 256 bytes hold 0..=255, which add up to 32 640.
        assert_eq!(memory_checksum(256), 32_640);
        assert_eq!(
            memory_checksum(MEM_BYTES),
            (MEM_BYTES / 256) as i32 * 32_640
        );
    }

    #[test]
    fn the_loop_shape_leaves_by_label_one_and_repeats_by_label_zero() {
        let e = while_loop(Expr::new().i32_const(1), Expr::new().nop());
        assert!(e.text().contains("br_if 1"), "{}", e.text());
        assert!(e.text().contains("br 0"), "{}", e.text());
    }
}
