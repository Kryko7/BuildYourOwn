//! Stage 29 — the memory immediate: `offset`, `align`, and the byte order underneath.
//!
//! Every load and store carries two immediates before its operands. They are very different
//! things and it is worth being blunt about which is which.
//!
//! `offset` is part of the address. The effective address is `i + offset` computed as
//! **unsigned 33-bit** arithmetic, so it cannot wrap: `i32.load offset=0xffffffff` at
//! address 1 is address 4 294 967 296, which is out of bounds, and never address 0. A
//! runtime that adds the two as `u32` gets a valid access where the spec demands a trap,
//! which is the bug this stage is built around.
//!
//! `align` is a hint and nothing else. It never changes the answer and it never causes a
//! trap; the only thing it can do is make the module **invalid**, because the spec requires
//! the declared alignment to be no larger than the natural one for the width. So
//! `i32.load align=1` at an odd address is a perfectly good instruction that reads the same
//! four bytes as `i32.load align=4`, while `i32.load align=8` is a module that never runs.
//! The immediate is the log2 of the alignment: 0, 1, 2, 3 mean 1, 2, 4, 8 bytes.
//!
//! The byte-order half of the stage writes one wide value and reads it back a byte at a
//! time. WebAssembly memory is little-endian for every type, floats included, whatever the
//! host underneath happens to be.

use crate::examples::ExampleSpec;
use crate::stages::{
    case_i32, case_i64, case_trap, case_void_trap, expect_rejected, run_cases_with, trap, Stage,
    Test,
};
use crate::wasm::{ftype, op, Expr, Func, Limits, Module, ModuleBuilder, Op, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 29,
        slug: "offset_and_alignment",
        name: "offset, align and little-endian byte order",
        ext: false,
        hints: &[
            "The effective address is the dynamic operand plus the static `offset` immediate, added as unsigned 33-bit arithmetic — compute it in a u64 so a huge offset traps instead of wrapping back into the page",
            "`align` is only a hint: it must never change the result and never trap, but a declared alignment larger than the width's natural one makes the module invalid, so check it while validating",
            "The alignment immediate is a log2: 0, 1, 2, 3 stand for 1, 2, 4 and 8 bytes, and the natural alignment of a load is its width in bytes",
            "Memory is little-endian for every type: the low byte of an i64 goes at the lowest address, and reading it back with eight `i32.load8_u` must give the bytes in that order",
        ],
        examples,
        tests: vec![
            Test::new("the offset immediate adds to the dynamic address", offset_adds),
            Test::new(
                "address and offset add as unsigned 33-bit arithmetic, so a huge offset traps",
                offset_cannot_wrap,
            ),
            Test::new("a smaller declared alignment is legal and changes nothing", align_is_a_hint),
            Test::new(
                "an alignment larger than the natural one is a validation error",
                align_too_large,
            ),
            Test::new("an unaligned access works at every width", unaligned_works),
            Test::new("an i64 is stored little-endian, low byte first", i64_byte_order),
            Test::new("an i32 and a float are stored little-endian too", other_byte_orders),
            Test::new("a value written byte by byte reads back as one wide load", built_by_hand),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Building blocks
// ---------------------------------------------------------------------------------------

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

/// A store at `addr + offset`, with the declared alignment `align` (a log2).
fn store_at(addr: i32, o: Op, align: u32, offset: u32, value: Expr) -> Expr {
    Expr::new()
        .i32_const(addr)
        .then(value)
        .mem(o, align, offset)
}

/// A load at `addr + offset`, with the declared alignment `align` (a log2).
fn load_at(addr: i32, o: Op, align: u32, offset: u32) -> Expr {
    Expr::new().i32_const(addr).mem(o, align, offset)
}

/// `i32.store8` of `value` at `addr`.
fn put8(addr: i32, value: i32) -> Expr {
    store_at(addr, op::I32_STORE8, 0, 0, Expr::new().i32_const(value))
}

/// `i32.load8_u` at `addr`.
fn get8(addr: i32) -> Expr {
    load_at(addr, op::I32_LOAD8_U, 0, 0)
}

/// Write `value` as a plain `i32.store` at `addr`.
fn put32(addr: i32, value: i32) -> Expr {
    store_at(addr, op::I32_STORE, 2, 0, Expr::new().i32_const(value))
}

/// Write `value` as a plain `i64.store` at `addr`.
fn put64(addr: i32, value: i64) -> Expr {
    store_at(addr, op::I64_STORE, 3, 0, Expr::new().i64_const(value))
}

/// A module whose body is one load with a deliberately over-declared alignment.
fn over_aligned(label: &str, o: Op, align: u32) -> Module {
    mem_i32(
        label,
        Expr::new()
            .i32_const(0)
            .mem(o, align, 0)
            .drop()
            .i32_const(0),
    )
}

/// The value used wherever the stage needs a byte pattern that is obvious in hex.
const PATTERN: i64 = 0x0102_0304_0506_0708;

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

wasm_test!(offset_adds, |ctx| {
    // 0x11223344 is written at address 4; every case reads it back by a different split of
    // the same effective address.
    let seed = put32(4, 0x1122_3344);
    run_cases_with(
        ctx,
        one_page("offset-adds"),
        vec![
            case_i32(
                "offset=0 at address 4",
                seed.clone().then(load_at(4, op::I32_LOAD, 2, 0)),
                0x1122_3344,
            ),
            case_i32(
                "offset=4 at address 0 reads the same four bytes",
                seed.clone().then(load_at(0, op::I32_LOAD, 2, 4)),
                0x1122_3344,
            ),
            case_i32(
                "offset=2 at address 2 reads the same four bytes",
                seed.clone().then(load_at(2, op::I32_LOAD, 2, 2)),
                0x1122_3344,
            ),
            case_i32(
                "offset=3 at address 1 reads the same four bytes",
                seed.clone().then(load_at(1, op::I32_LOAD, 2, 3)),
                0x1122_3344,
            ),
            case_i32(
                "offset=5 at address 0 is one byte further on",
                seed.clone().then(load_at(0, op::I32_LOAD8_U, 0, 5)),
                0x33,
            ),
            case_i32(
                "a store carries an offset too",
                store_at(0, op::I32_STORE, 2, 16, Expr::new().i32_const(0x55aa)).then(load_at(
                    16,
                    op::I32_LOAD,
                    2,
                    0,
                )),
                0x55aa,
            ),
            case_i32(
                "a store's offset and a load's offset meet in the middle",
                store_at(8, op::I32_STORE, 2, 8, Expr::new().i32_const(0x99)).then(load_at(
                    4,
                    op::I32_LOAD,
                    2,
                    12,
                )),
                0x99,
            ),
            case_i64(
                "an offset works at the i64 width as well",
                put64(24, PATTERN).then(load_at(16, op::I64_LOAD, 3, 8)),
                PATTERN,
            ),
        ],
    )
});

wasm_test!(offset_cannot_wrap, |ctx| {
    ctx.note("the effective address is i + offset in 33 bits: 1 + 0xffffffff is 4294967296, not 0");
    run_cases_with(
        ctx,
        one_page("offset-wrap"),
        vec![
            case_i32(
                "offset=65532 at address 0 is the last four bytes of the page",
                load_at(0, op::I32_LOAD, 2, 65532),
                0,
            ),
            case_trap(
                "offset=65533 at address 0 runs one byte off the end",
                load_at(0, op::I32_LOAD, 2, 65533),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_trap(
                "offset=0xffffffff at address 0 is far outside the page",
                load_at(0, op::I32_LOAD, 2, 0xffff_ffff),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_trap(
                "offset=0xffffffff at address 1 does not wrap to address 0",
                load_at(1, op::I32_LOAD, 2, 0xffff_ffff),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_trap(
                "offset=0x80000000 at address 0x80000000 does not wrap either",
                load_at(i32::MIN, op::I32_LOAD, 2, 0x8000_0000),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_trap(
                "offset=0xffffffff at address 1 traps for an i32.load8_u too",
                load_at(1, op::I32_LOAD8_U, 0, 0xffff_ffff),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "a store cannot wrap into the page either",
                store_at(1, op::I32_STORE, 2, 0xffff_ffff, Expr::new().i32_const(1)),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_i32(
                "offset=65535 at address 0 is the last byte, and still in bounds",
                load_at(0, op::I32_LOAD8_U, 0, 65535),
                0,
            ),
        ],
    )
});

wasm_test!(align_is_a_hint, |ctx| {
    // Address 3 is aligned for nothing, so every one of these declares an alignment it does
    // not have. The spec allows that; only an over-declared alignment is an error.
    let seed32 = put32(3, 0x1234_5678);
    let seed64 = put64(3, PATTERN);
    run_cases_with(
        ctx,
        one_page("align-hint"),
        vec![
            case_i32(
                "i32.load align=1 at an odd address",
                seed32.clone().then(load_at(3, op::I32_LOAD, 0, 0)),
                0x1234_5678,
            ),
            case_i32(
                "i32.load align=2 at the same address gives the same answer",
                seed32.clone().then(load_at(3, op::I32_LOAD, 1, 0)),
                0x1234_5678,
            ),
            case_i32(
                "i32.load align=4, the natural one, gives the same answer",
                seed32.clone().then(load_at(3, op::I32_LOAD, 2, 0)),
                0x1234_5678,
            ),
            case_i32(
                "an i32.store align=1 is readable by an i32.load align=4",
                store_at(
                    5,
                    op::I32_STORE,
                    0,
                    0,
                    Expr::new().i32_const(0x0bad_f00du32 as i32),
                )
                .then(load_at(5, op::I32_LOAD, 2, 0)),
                0x0bad_f00du32 as i32,
            ),
            case_i64(
                "i64.load align=1 at an odd address",
                seed64.clone().then(load_at(3, op::I64_LOAD, 0, 0)),
                PATTERN,
            ),
            case_i64(
                "i64.load align=8 at the same odd address gives the same answer",
                seed64.clone().then(load_at(3, op::I64_LOAD, 3, 0)),
                PATTERN,
            ),
            case_i32(
                "i32.load16_u align=2, the natural alignment for two bytes",
                put8(9, 0xcd)
                    .then(put8(10, 0xab))
                    .then(load_at(9, op::I32_LOAD16_U, 1, 0)),
                0xabcd,
            ),
            case_i32(
                "i32.load16_u align=1, under-declared, reads exactly the same",
                put8(9, 0xcd)
                    .then(put8(10, 0xab))
                    .then(load_at(9, op::I32_LOAD16_U, 0, 0)),
                0xabcd,
            ),
        ],
    )
});

wasm_test!(align_too_large, |ctx| {
    // Each of these is a module that decodes perfectly and must still be refused, with
    // nothing on stdout.
    expect_rejected(
        ctx,
        &over_aligned("align-i32-8", op::I32_LOAD, 3),
        "i32.load declares align=8, larger than the natural 4",
    )?;
    expect_rejected(
        ctx,
        &over_aligned("align-i64-16", op::I64_LOAD, 4),
        "i64.load declares align=16, larger than the natural 8",
    )?;
    expect_rejected(
        ctx,
        &over_aligned("align-load8-2", op::I32_LOAD8_U, 1),
        "i32.load8_u declares align=2, and one byte has a natural alignment of 1",
    )?;
    expect_rejected(
        ctx,
        &over_aligned("align-load16-4", op::I32_LOAD16_S, 2),
        "i32.load16_s declares align=4, larger than the natural 2",
    )?;
    expect_rejected(
        ctx,
        &over_aligned("align-f32-8", op::F32_LOAD, 3),
        "f32.load declares align=8, larger than the natural 4",
    )?;
    let store = mem_i32(
        "align-store-8",
        store_at(0, op::I32_STORE, 3, 0, Expr::new().i32_const(1)).then(Expr::new().i32_const(0)),
    );
    expect_rejected(ctx, &store, "i32.store declares align=8 as well")?;
    Ok(())
});

wasm_test!(unaligned_works, |ctx| {
    run_cases_with(
        ctx,
        one_page("unaligned"),
        vec![
            case_i32(
                "an i32 written and read at address 1",
                put32(1, 0x1234_5678).then(load_at(1, op::I32_LOAD, 2, 0)),
                0x1234_5678,
            ),
            case_i32(
                "an i32 written and read at address 3",
                put32(3, -1).then(load_at(3, op::I32_LOAD, 2, 0)),
                -1,
            ),
            case_i64(
                "an i64 written and read at address 3",
                put64(3, PATTERN).then(load_at(3, op::I64_LOAD, 3, 0)),
                PATTERN,
            ),
            case_i64(
                "an i64 written and read at address 65527, one short of aligned",
                put64(65527, PATTERN).then(load_at(65527, op::I64_LOAD, 3, 0)),
                PATTERN,
            ),
            case_i32(
                "a 16-bit value at an odd address",
                store_at(7, op::I32_STORE16, 1, 0, Expr::new().i32_const(0xbeef)).then(load_at(
                    7,
                    op::I32_LOAD16_U,
                    1,
                    0,
                )),
                0xbeef,
            ),
            case_i64(
                "an f64 at address 5 comes back bit for bit",
                store_at(5, op::F64_STORE, 3, 0, Expr::new().f64_const(-2.25))
                    .then(load_at(5, op::F64_LOAD, 3, 0))
                    .op(op::I64_REINTERPRET_F64),
                (-2.25f64).to_bits() as i64,
            ),
            case_i32(
                "an f32 at address 7 comes back bit for bit",
                store_at(7, op::F32_STORE, 2, 0, Expr::new().f32_const(1.5))
                    .then(load_at(7, op::F32_LOAD, 2, 0))
                    .op(op::I32_REINTERPRET_F32),
                1.5f32.to_bits() as i32,
            ),
            case_i32(
                "an unaligned store does not disturb the byte before it",
                put8(0, 0x5a).then(put32(1, -1)).then(get8(0)),
                0x5a,
            ),
        ],
    )
});

wasm_test!(i64_byte_order, |ctx| {
    // 0x0102030405060708 written at address 0, then each of its eight bytes read back on its
    // own. Little-endian means byte 0 is the *low* byte, 0x08.
    let mut cases = Vec::new();
    for i in 0..8u32 {
        let want = (PATTERN >> (8 * i)) & 0xff;
        cases.push(case_i32(
            format!("byte {i} of 0x0102030405060708 is 0x{want:02x}"),
            put64(0, PATTERN).then(get8(i as i32)),
            want as i32,
        ));
    }
    run_cases_with(ctx, one_page("i64-endian"), cases)
});

wasm_test!(other_byte_orders, |ctx| {
    let word = 0x0102_0304i32;
    let dbl = 0x0102_0304_0506_0708u64;
    run_cases_with(
        ctx,
        one_page("endian"),
        vec![
            case_i32(
                "byte 0 of the i32 0x01020304 is 0x04",
                put32(0, word).then(get8(0)),
                0x04,
            ),
            case_i32(
                "byte 1 of the i32 0x01020304 is 0x03",
                put32(0, word).then(get8(1)),
                0x03,
            ),
            case_i32(
                "byte 2 of the i32 0x01020304 is 0x02",
                put32(0, word).then(get8(2)),
                0x02,
            ),
            case_i32(
                "byte 3 of the i32 0x01020304 is 0x01",
                put32(0, word).then(get8(3)),
                0x01,
            ),
            case_i32(
                "byte 0 of an f64 with those bits is 0x08",
                store_at(8, op::F64_STORE, 3, 0, Expr::new().f64_bits(dbl)).then(get8(8)),
                0x08,
            ),
            case_i32(
                "byte 7 of an f64 with those bits is 0x01",
                store_at(8, op::F64_STORE, 3, 0, Expr::new().f64_bits(dbl)).then(get8(15)),
                0x01,
            ),
            case_i32(
                "byte 0 of an f32 with the bits 0x01020304 is 0x04",
                store_at(16, op::F32_STORE, 2, 0, Expr::new().f32_bits(0x0102_0304)).then(get8(16)),
                0x04,
            ),
            case_i32(
                "byte 3 of an f32 with the bits 0x01020304 is 0x01",
                store_at(16, op::F32_STORE, 2, 0, Expr::new().f32_bits(0x0102_0304)).then(get8(19)),
                0x01,
            ),
        ],
    )
});

wasm_test!(built_by_hand, |ctx| {
    // The other direction: write the bytes individually, then read the whole thing at once.
    let mut bytes = Expr::new();
    for i in 0..8i32 {
        bytes = bytes.then(put8(i, 8 - i));
    }
    run_cases_with(
        ctx,
        one_page("by-hand"),
        vec![
            case_i64(
                "eight i32.store8 of 08 07 06 05 04 03 02 01 read back as one i64",
                bytes.clone().then(load_at(0, op::I64_LOAD, 3, 0)),
                PATTERN,
            ),
            case_i32(
                "the low four of those bytes read back as one i32",
                bytes.clone().then(load_at(0, op::I32_LOAD, 2, 0)),
                0x0506_0708,
            ),
            case_i32(
                "the high four read back as one i32 at address 4",
                bytes.clone().then(load_at(4, op::I32_LOAD, 2, 0)),
                0x0102_0304,
            ),
            case_i32(
                "two bytes 00 80 read back as i32.load16_u are 32768",
                put8(16, 0x00)
                    .then(put8(17, 0x80))
                    .then(load_at(16, op::I32_LOAD16_U, 1, 0)),
                32768,
            ),
            case_i32(
                "the same two bytes read as i32.load16_s are -32768",
                put8(16, 0x00)
                    .then(put8(17, 0x80))
                    .then(load_at(16, op::I32_LOAD16_S, 1, 0)),
                -32768,
            ),
            case_i32(
                "four bytes 00 00 c0 3f read back as an f32 are 1.5",
                put8(24, 0x00)
                    .then(put8(25, 0x00))
                    .then(put8(26, 0xc0))
                    .then(put8(27, 0x3f))
                    .then(load_at(24, op::F32_LOAD, 2, 0))
                    .op(op::I32_REINTERPRET_F32),
                1.5f32.to_bits() as i32,
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// Examples
// ---------------------------------------------------------------------------------------

/// `f() -> i32`: the same four bytes reached two ways, subtracted.
fn offset_example() -> Module {
    mem_i32(
        "offset-two-ways",
        put32(4, 0x1122_3344)
            .then(load_at(0, op::I32_LOAD, 2, 4))
            .then(load_at(4, op::I32_LOAD, 2, 0))
            .op(op::I32_SUB),
    )
}

/// `f() -> i32`: `i32.load offset=0xffffffff` at address 1, which must trap.
fn wrapping_offset() -> Module {
    mem_i32("offset-wraps", load_at(1, op::I32_LOAD, 2, 0xffff_ffff))
}

/// Worked examples: the offset that adds, and the offset that must not wrap.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("Two ways to name the same address", offset_example)
            .summary(
                "store 0x11223344 at address 4, then subtract `i32.load offset=0` at \
                 address 4 from `i32.load offset=4` at address 0",
            )
            .command("run --invoke f mod.wasm")
            .output("0")
            .note(
                "The static offset is simply added to the dynamic address, so the two loads \
                 read the same four bytes and their difference is zero.",
            ),
        ExampleSpec::module("An offset that must not wrap", wrapping_offset)
            .summary("`i32.load align=4 offset=4294967295` at address 1, on a one-page memory")
            .command("run --invoke f mod.wasm")
            .output(
                "nothing on stdout, `wasm trap: out of bounds memory access` on stderr, and \
                 a non-zero exit status",
            )
            .note(
                "1 + 0xffffffff is 4 294 967 296, not 0: the two are added as unsigned \
                 33-bit numbers. A runtime that adds them in a u32 reads address 0 instead \
                 and prints a number here.",
            ),
    ]
}
