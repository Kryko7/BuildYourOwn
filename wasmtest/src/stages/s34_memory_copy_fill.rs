//! Stage 34 — `memory.copy` and `memory.fill`. **[ext]**
//!
//! `memory.copy` takes destination, source and length off the stack, in that push order, and
//! must behave as if the source were read in full before any of the destination were
//! written — `memmove`, never `memcpy`. That is the whole difficulty of the instruction, and
//! it is tested from both sides: a naive forward byte loop corrupts the copy when the
//! destination is above an overlapping source, and a naive backward loop corrupts it when
//! the destination is below. Only an implementation that picks its direction (or copies via
//! a buffer) gets both.
//!
//! `memory.fill` takes destination, byte value and length. The value is an `i32` but only
//! its low eight bits are used, so `0x1ff` fills with `0xff` and `0x100` fills with zeroes.
//!
//! Both instructions check their bounds **first**, before writing a single byte, which is
//! also how they treat a length of 0: legal wherever every end of every range is still
//! inside the memory, and that includes the address one past the last byte. One half of
//! "bounds first" is not observable from inside the instance — a trap ends the invocation,
//! so nothing can look at the memory afterwards, and the next process gets a fresh one.
//! What these tests do pin down is the boundary itself: the largest copy and the largest
//! fill that fit work completely, and one byte more traps. The zero-length cases that belong
//! to the very end of memory are in stage 31, with the rest of the boundary arithmetic.

use crate::examples::ExampleSpec;
use crate::stages::{case_i32, case_void_trap, run_cases_with, trap, Stage, Test};
use crate::wasm::{ftype, op, Expr, Func, Limits, Module, ModuleBuilder, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 34,
        slug: "memory_copy_fill",
        name: "memory.copy and memory.fill",
        ext: true,
        hints: &[
            "`memory.copy` must behave like `memmove`: when the ranges overlap, copy backwards if the destination is above the source and forwards if it is below, or the result is scrambled one way round",
            "Its operands are pushed destination, source, length, so they come off the stack length first — the same order `memory.init` uses",
            "`memory.fill` uses only the low eight bits of its value operand: 0x1ff fills with 0xff and 0x100 fills with zeroes",
            "Check every bound before writing anything, and treat length 0 as legal wherever the addresses are in range — a copy of nothing at the end of memory must not trap",
        ],
        examples,
        tests: vec![
            Test::new("memory.copy handles an overlap with the destination above the source", forwards)
                .ext(),
            Test::new("memory.copy handles an overlap with the destination below the source", backwards)
                .ext(),
            Test::new("a copy of length zero is legal anywhere in range", copy_zero).ext(),
            Test::new("a copy can span the whole page", copy_whole_page).ext(),
            Test::new("a copy that runs past either end traps", copy_out_of_range).ext(),
            Test::new("a copy onto itself leaves the bytes where they were", self_copy).ext(),
            Test::new("memory.fill uses only the low eight bits of its value", fill_value).ext(),
            Test::new("memory.fill covers exactly its range, and an out-of-range fill traps", fill_range)
                .ext(),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Building blocks
// ---------------------------------------------------------------------------------------

/// One page, in bytes.
const PAGE: i32 = 65_536;

/// A builder with one page of memory and nothing else in it.
fn one_page(label: &str) -> ModuleBuilder {
    ModuleBuilder::new(label).memory(Limits::min(1))
}

/// A module with one page of memory exporting `f: () -> i32`.
fn mem_i32(label: &str, body: Expr) -> Module {
    let mut b = one_page(label);
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(ty, Func::new(body));
    b.export_func("f", idx).build()
}

/// `memory.copy` with literal destination, source and length.
fn copy(dest: i32, src: i32, len: i32) -> Expr {
    Expr::new()
        .i32_const(dest)
        .i32_const(src)
        .i32_const(len)
        .memory_copy()
}

/// `memory.fill` with literal destination, value and length.
fn fill(dest: i32, value: i32, len: i32) -> Expr {
    Expr::new()
        .i32_const(dest)
        .i32_const(value)
        .i32_const(len)
        .memory_fill()
}

/// `i32.store8` of `value` at `addr`.
fn put8(addr: i32, value: i32) -> Expr {
    Expr::new()
        .i32_const(addr)
        .i32_const(value)
        .mem(op::I32_STORE8, 0, 0)
}

/// `i32.load8_u` at `addr`.
fn get8(addr: i32) -> Expr {
    Expr::new().i32_const(addr).mem(op::I32_LOAD8_U, 0, 0)
}

/// The bytes 1, 2, … `n` written one at a time from `addr`.
fn ramp(addr: i32, n: i32) -> Expr {
    let mut e = Expr::new();
    for i in 0..n {
        e = e.then(put8(addr + i, i + 1));
    }
    e
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

wasm_test!(forwards, |ctx| {
    // Memory 0..8 holds 1 2 3 4 5 6 7 8; copy eight bytes from 0 to 2. The answer is
    // 1 2 1 2 3 4 5 6 7 8. A forward byte loop reads the bytes it has already overwritten
    // and produces 1 2 1 2 1 2 1 2 1 2 instead.
    let setup = ramp(0, 8).then(copy(2, 0, 8));
    ctx.note("destination 2, source 0, length 8: the ranges overlap by six bytes");
    run_cases_with(
        ctx,
        one_page("copy-forwards"),
        vec![
            case_i32(
                "byte 0 is outside the destination and still 1",
                setup.clone().then(get8(0)),
                1,
            ),
            case_i32(
                "byte 1 is outside the destination and still 2",
                setup.clone().then(get8(1)),
                2,
            ),
            case_i32(
                "byte 2 is the first copied byte, 1",
                setup.clone().then(get8(2)),
                1,
            ),
            case_i32("byte 3 is 2", setup.clone().then(get8(3)), 2),
            case_i32(
                "byte 4 is 3, which a forward byte loop gets wrong",
                setup.clone().then(get8(4)),
                3,
            ),
            case_i32("byte 6 is 5, likewise", setup.clone().then(get8(6)), 5),
            case_i32(
                "byte 9 is the last copied byte, 8",
                setup.clone().then(get8(9)),
                8,
            ),
            case_i32(
                "and byte 10 was never written",
                setup.clone().then(get8(10)),
                0,
            ),
        ],
    )
});

wasm_test!(backwards, |ctx| {
    // Memory 2..10 holds 1 2 3 4 5 6 7 8; copy eight bytes from 2 down to 0. The answer is
    // 1 2 3 4 5 6 7 8 7 8. A backward byte loop produces 1 2 3 4 5 6 7 8 in the wrong places.
    let setup = ramp(2, 8).then(copy(0, 2, 8));
    ctx.note("destination 0, source 2, length 8: the same overlap the other way round");
    run_cases_with(
        ctx,
        one_page("copy-backwards"),
        vec![
            case_i32(
                "byte 0 is the first copied byte, 1",
                setup.clone().then(get8(0)),
                1,
            ),
            case_i32("byte 1 is 2", setup.clone().then(get8(1)), 2),
            case_i32(
                "byte 2 is 3, which a backward byte loop gets wrong",
                setup.clone().then(get8(2)),
                3,
            ),
            case_i32("byte 4 is 5, likewise", setup.clone().then(get8(4)), 5),
            case_i32(
                "byte 7 is the last copied byte, 8",
                setup.clone().then(get8(7)),
                8,
            ),
            case_i32(
                "byte 8 is outside the destination and still holds its 7",
                setup.clone().then(get8(8)),
                7,
            ),
            case_i32(
                "byte 9 is outside the destination and still holds its 8",
                setup.clone().then(get8(9)),
                8,
            ),
        ],
    )
});

wasm_test!(copy_zero, |ctx| {
    run_cases_with(
        ctx,
        one_page("copy-zero"),
        vec![
            case_i32(
                "a zero-length copy at 0 changes nothing",
                put8(0, 7)
                    .then(put8(1, 9))
                    .then(copy(0, 1, 0))
                    .then(get8(0)),
                7,
            ),
            case_i32(
                "a zero-length copy with the destination at the end of memory is legal",
                copy(PAGE, 0, 0).i32_const(1),
                1,
            ),
            case_i32(
                "so is one with the source at the end of memory",
                copy(0, PAGE, 0).i32_const(1),
                1,
            ),
            case_i32(
                "and one with both there",
                copy(PAGE, PAGE, 0).i32_const(1),
                1,
            ),
            case_void_trap(
                "a destination one byte further on traps even with length 0",
                copy(PAGE + 1, 0, 0),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "and so does a source one byte further on",
                copy(0, PAGE + 1, 0),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
        ],
    )
});

wasm_test!(copy_whole_page, |ctx| {
    run_cases_with(
        ctx,
        one_page("copy-page"),
        vec![
            case_i32(
                "copying the whole page onto itself keeps the first byte",
                put8(0, 0xab).then(copy(0, 0, PAGE)).then(get8(0)),
                0xab,
            ),
            case_i32(
                "and the last byte",
                put8(PAGE - 1, 0xcd)
                    .then(copy(0, 0, PAGE))
                    .then(get8(PAGE - 1)),
                0xcd,
            ),
            case_i32(
                "the top half copied over the bottom half arrives",
                put8(PAGE / 2, 7)
                    .then(copy(0, PAGE / 2, PAGE / 2))
                    .then(get8(0)),
                7,
            ),
            case_i32(
                "the bottom half copied over the top half arrives too",
                put8(0, 9)
                    .then(copy(PAGE / 2, 0, PAGE / 2))
                    .then(get8(PAGE / 2)),
                9,
            ),
            case_i32(
                "and the very last byte of that copy arrives",
                put8(PAGE / 2 - 1, 11)
                    .then(copy(PAGE / 2, 0, PAGE / 2))
                    .then(get8(PAGE - 1)),
                11,
            ),
            case_void_trap(
                "one byte more than the page traps",
                copy(0, 0, PAGE + 1),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
        ],
    )
});

wasm_test!(copy_out_of_range, |ctx| {
    ctx.note(
        "both ranges are bounded before anything is copied; a trap ends the invocation, so \
         the observable half is that the largest copy which fits works completely",
    );
    run_cases_with(
        ctx,
        one_page("copy-range"),
        vec![
            case_i32(
                "a copy ending exactly at the end of memory is the last legal one",
                put8(0, 42).then(copy(PAGE - 8, 0, 8)).then(get8(PAGE - 8)),
                42,
            ),
            case_void_trap(
                "a destination one byte further on traps",
                copy(PAGE - 7, 0, 8),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_i32(
                "a source ending exactly at the end of memory is legal",
                put8(PAGE - 8, 43).then(copy(0, PAGE - 8, 8)).then(get8(0)),
                43,
            ),
            case_void_trap(
                "a source one byte further on traps",
                copy(0, PAGE - 7, 8),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "a length of -1 is 4294967295 and traps rather than wrapping",
                copy(0, 0, -1),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "a destination of -1 traps",
                copy(-1, 0, 1),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "a source of -1 traps",
                copy(0, -1, 1),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "a destination in range whose length runs off the end traps",
                copy(PAGE - 1, 0, 2),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
        ],
    )
});

wasm_test!(self_copy, |ctx| {
    run_cases_with(
        ctx,
        one_page("copy-self"),
        vec![
            case_i32(
                "copying 0..8 onto itself keeps byte 0",
                ramp(0, 8).then(copy(0, 0, 8)).then(get8(0)),
                1,
            ),
            case_i32(
                "and byte 3",
                ramp(0, 8).then(copy(0, 0, 8)).then(get8(3)),
                4,
            ),
            case_i32(
                "and byte 7",
                ramp(0, 8).then(copy(0, 0, 8)).then(get8(7)),
                8,
            ),
            case_i32(
                "a self-copy of part of the range leaves the rest alone",
                ramp(0, 8).then(copy(4, 4, 4)).then(get8(4)),
                5,
            ),
            case_i32(
                "and the bytes outside it alone",
                ramp(0, 8).then(copy(4, 4, 4)).then(get8(0)),
                1,
            ),
            case_i32(
                "a self-copy of length 0 is a no-op like any other",
                ramp(0, 8).then(copy(3, 3, 0)).then(get8(3)),
                4,
            ),
        ],
    )
});

wasm_test!(fill_value, |ctx| {
    run_cases_with(
        ctx,
        one_page("fill-value"),
        vec![
            case_i32(
                "filling with 0xff writes 255",
                fill(0, 0xff, 4).then(get8(0)),
                255,
            ),
            case_i32(
                "filling with 0x1ff writes 255 too: only the low byte is used",
                fill(0, 0x1ff, 4).then(get8(0)),
                255,
            ),
            case_i32(
                "filling with 0x100 writes zeroes",
                put8(0, 7).then(fill(0, 0x100, 4)).then(get8(0)),
                0,
            ),
            case_i32(
                "filling with -1 writes 255",
                fill(0, -1, 4).then(get8(0)),
                255,
            ),
            case_i32(
                "filling with 0xabcd1234 writes 0x34",
                fill(0, 0xabcd_1234u32 as i32, 4).then(get8(0)),
                0x34,
            ),
            case_i32(
                "every byte of the range gets the same value",
                fill(0, 0x41, 4).then(Expr::new().i32_const(0).mem(op::I32_LOAD, 2, 0)),
                0x4141_4141,
            ),
            case_i32(
                "filling with 0 clears what was there",
                put8(0, 7).then(fill(0, 0, 1)).then(get8(0)),
                0,
            ),
        ],
    )
});

wasm_test!(fill_range, |ctx| {
    run_cases_with(
        ctx,
        one_page("fill-range"),
        vec![
            case_i32(
                "the byte before the range is untouched",
                fill(4, 0xff, 4).then(get8(3)),
                0,
            ),
            case_i32(
                "the first byte of the range is filled",
                fill(4, 0xff, 4).then(get8(4)),
                255,
            ),
            case_i32(
                "the last byte of the range is filled",
                fill(4, 0xff, 4).then(get8(7)),
                255,
            ),
            case_i32(
                "the byte after the range is untouched",
                fill(4, 0xff, 4).then(get8(8)),
                0,
            ),
            case_i32(
                "a fill covering the whole page reaches the last byte",
                fill(0, 0xff, PAGE).then(get8(PAGE - 1)),
                255,
            ),
            case_i32("and the first", fill(0, 0xff, PAGE).then(get8(0)), 255),
            case_void_trap(
                "one byte more than the page traps",
                fill(0, 0xff, PAGE + 1),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "a fill starting one byte past the end traps",
                fill(PAGE, 0xff, 1),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "a length of -1 traps rather than wrapping",
                fill(0, 0xff, -1),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// Examples
// ---------------------------------------------------------------------------------------

/// `f() -> i32`: the overlapping forward copy, reading back the byte a naive loop gets wrong.
fn overlap_example() -> Module {
    mem_i32("copy-overlap", ramp(0, 8).then(copy(2, 0, 8)).then(get8(4)))
}

/// `f() -> i32`: a fill whose value is wider than a byte.
fn fill_example() -> Module {
    mem_i32("fill-wide-value", fill(0, 0x1ff, 4).then(get8(0)))
}

/// Worked examples: the overlap that catches a naive loop, and the value that is masked.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("An overlapping copy", overlap_example)
            .summary(
                "write 1..8 into bytes 0..8, then `memory.copy` eight bytes from address 0 \
                 to address 2, and read byte 4",
            )
            .command("run --invoke f mod.wasm")
            .output("3")
            .note(
                "The source must be read as it was before the copy started, so byte 4 gets \
                 the original byte 2, which was 3. A forward byte loop has already written \
                 byte 2 by then and prints 1 instead.",
            ),
        ExampleSpec::module("A fill value wider than a byte", fill_example)
            .summary(
                "`memory.fill` of four bytes at address 0 with the value 0x1ff, then read byte 0",
            )
            .command("run --invoke f mod.wasm")
            .output("255")
            .note(
                "The value operand is an i32 but a fill writes bytes: only the low eight \
                 bits are used, so 0x1ff and 0xff fill identically and 0x100 fills with \
                 zeroes.",
            ),
    ]
}
