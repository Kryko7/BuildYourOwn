//! Stage 31 — out-of-bounds memory traps, at the byte.
//!
//! A one-page memory is 65 536 bytes, addressed 0 to 65 535. The bound is on the **whole
//! access**, not on its first byte, so the last address an `i32.load` may name is 65 532 and
//! 65 533 traps; the last an `i64.load` may name is 65 528 and 65 529 traps. Every width has
//! its own boundary and this stage walks all of them, for loads and for stores, one byte
//! either side. A runtime that checks only the starting address passes half of these and
//! fails the other half, which is exactly the point.
//!
//! Addresses are unsigned. `i32.const -1` as an address is 4 294 967 295, not "one byte back
//! from the end", so it traps like any other address past the memory.
//!
//! One thing cannot be observed from inside the instance: that a trapping store wrote
//! nothing before it trapped. A trap ends the invocation, so no later instruction in the
//! same function can read the bytes, and the next process gets a fresh memory. What is
//! observable is the boundary itself — the store one byte inside works and writes exactly
//! what it should, the store one byte outside traps — and that is what these tests pin down.
//!
//! The zero-length case lives here rather than in stage 34: a load or a store always touches
//! at least one byte, so "zero length" is not a thing they can do, but `memory.fill` of
//! length 0 **is** legal right at the end of memory, and traps one byte beyond it. That one
//! test carries the `ext` tag because `memory.fill` belongs to the bulk-memory family.

use crate::examples::ExampleSpec;
use crate::stages::{
    case_i32, case_i64, case_trap, case_void_trap, run_cases_with, trap, Stage, Test,
};
use crate::wasm::{ftype, op, Expr, Func, Limits, Module, ModuleBuilder, Op, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 31,
        slug: "memory_bounds",
        name: "Out-of-bounds memory traps at the byte boundary",
        ext: false,
        hints: &[
            "Bound the whole access, not its first byte: an access at address `a` of `w` bytes is in bounds when `a + w <= size_in_bytes`, computed wide enough not to overflow",
            "That makes the last legal address width-dependent — 65 535 for one byte, 65 532 for four, 65 528 for eight on a one-page memory — and one past it must trap",
            "Addresses are unsigned: `i32.const -1` means 4 294 967 295, so it is far outside the memory rather than one byte back from the end",
            "The bound follows the current size, so recompute it after every `memory.grow` rather than caching the limit at instantiation",
        ],
        examples,
        tests: vec![
            Test::new("the last valid address of a load depends on its width", last_valid_load),
            Test::new("one byte past a load's last valid address traps", first_invalid_load),
            Test::new("the last valid address of a store depends on its width", last_valid_store),
            Test::new("one byte past a store's last valid address traps", first_invalid_store),
            Test::new("an address of -1 is 4294967295 and traps", minus_one),
            Test::new("the boundary follows the declared size", boundary_follows_size),
            Test::new("a grow moves the boundary with it", grow_moves_the_boundary),
            Test::new("memory.fill of length zero is legal at the very end of memory", zero_length_fill)
                .ext(),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Building blocks
// ---------------------------------------------------------------------------------------

/// One page, in bytes.
const PAGE: i32 = 65_536;

/// A builder with a memory of the given size in pages and nothing else in it.
fn mem(label: &str, pages: u32) -> ModuleBuilder {
    ModuleBuilder::new(label).memory(Limits::min(pages))
}

/// A module with one page of memory exporting `f: () -> i32`.
fn mem_i32(label: &str, body: Expr) -> Module {
    let mut b = mem(label, 1);
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(ty, Func::new(body));
    b.export_func("f", idx).build()
}

/// A load at the literal address `addr`, at the given width.
fn load(addr: i32, o: Op, align: u32) -> Expr {
    Expr::new().i32_const(addr).mem(o, align, 0)
}

/// A store of `value` at the literal address `addr`, at the given width.
fn store(addr: i32, o: Op, align: u32, value: Expr) -> Expr {
    Expr::new().i32_const(addr).then(value).mem(o, align, 0)
}

/// An `i32`-typed store of `value` at `addr`.
fn store_i32(addr: i32, o: Op, align: u32, value: i32) -> Expr {
    store(addr, o, align, Expr::new().i32_const(value))
}

/// `memory.fill d value n`.
fn fill(dest: i32, value: i32, len: i32) -> Expr {
    Expr::new()
        .i32_const(dest)
        .i32_const(value)
        .i32_const(len)
        .memory_fill()
}

/// `memory.grow n`, answer dropped.
fn grow(n: i32) -> Expr {
    Expr::new().i32_const(n).memory_grow().drop()
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

wasm_test!(last_valid_load, |ctx| {
    ctx.note("one page is 65 536 bytes, addressed 0 to 65 535");
    run_cases_with(
        ctx,
        mem("last-valid-load", 1),
        vec![
            case_i32(
                "i32.load8_u at 65535 is the last one-byte load",
                load(PAGE - 1, op::I32_LOAD8_U, 0),
                0,
            ),
            case_i32(
                "i32.load8_s at 65535 too",
                load(PAGE - 1, op::I32_LOAD8_S, 0),
                0,
            ),
            case_i32(
                "i32.load16_u at 65534 is the last two-byte load",
                load(PAGE - 2, op::I32_LOAD16_U, 1),
                0,
            ),
            case_i32(
                "i32.load at 65532 is the last four-byte load",
                load(PAGE - 4, op::I32_LOAD, 2),
                0,
            ),
            case_i64(
                "i64.load32_u at 65532 is a four-byte load too",
                load(PAGE - 4, op::I64_LOAD32_U, 2),
                0,
            ),
            case_i64(
                "i64.load at 65528 is the last eight-byte load",
                load(PAGE - 8, op::I64_LOAD, 3),
                0,
            ),
            case_i32(
                "f32.load at 65532 reads the last four bytes",
                load(PAGE - 4, op::F32_LOAD, 2).op(op::I32_REINTERPRET_F32),
                0,
            ),
            case_i64(
                "f64.load at 65528 reads the last eight bytes",
                load(PAGE - 8, op::F64_LOAD, 3).op(op::I64_REINTERPRET_F64),
                0,
            ),
        ],
    )
});

wasm_test!(first_invalid_load, |ctx| {
    run_cases_with(
        ctx,
        mem("first-invalid-load", 1),
        vec![
            case_trap(
                "i32.load8_u at 65536 is one byte past the page",
                load(PAGE, op::I32_LOAD8_U, 0),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_trap(
                "i32.load16_u at 65535 needs a byte that is not there",
                load(PAGE - 1, op::I32_LOAD16_U, 1),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_trap(
                "i32.load at 65533 runs one byte off the end",
                load(PAGE - 3, op::I32_LOAD, 2),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_trap(
                "i64.load32_u at 65533 does the same",
                load(PAGE - 3, op::I64_LOAD32_U, 2).op(op::I32_WRAP_I64),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_trap(
                "i64.load at 65529 runs one byte off the end",
                load(PAGE - 7, op::I64_LOAD, 3).op(op::I32_WRAP_I64),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_trap(
                "f32.load at 65533 traps",
                load(PAGE - 3, op::F32_LOAD, 2).op(op::I32_REINTERPRET_F32),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_trap(
                "f64.load at 65529 traps",
                load(PAGE - 7, op::F64_LOAD, 3)
                    .op(op::I64_REINTERPRET_F64)
                    .op(op::I32_WRAP_I64),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_trap(
                "an address well past the page traps too",
                load(PAGE * 4, op::I32_LOAD, 2),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
        ],
    )
});

wasm_test!(last_valid_store, |ctx| {
    // Each case stores at the last address its width allows and reads the same bytes back,
    // so a store that silently did nothing fails as loudly as one that trapped.
    run_cases_with(
        ctx,
        mem("last-valid-store", 1),
        vec![
            case_i32(
                "i32.store8 at 65535 writes the last byte",
                store_i32(PAGE - 1, op::I32_STORE8, 0, 0xab).then(load(
                    PAGE - 1,
                    op::I32_LOAD8_U,
                    0,
                )),
                0xab,
            ),
            case_i32(
                "i32.store16 at 65534 writes the last two bytes",
                store_i32(PAGE - 2, op::I32_STORE16, 1, 0xabcd).then(load(
                    PAGE - 2,
                    op::I32_LOAD16_U,
                    1,
                )),
                0xabcd,
            ),
            case_i32(
                "i32.store at 65532 writes the last four bytes",
                store_i32(PAGE - 4, op::I32_STORE, 2, 0x1234_5678).then(load(
                    PAGE - 4,
                    op::I32_LOAD,
                    2,
                )),
                0x1234_5678,
            ),
            case_i64(
                "i64.store32 at 65532 writes four bytes there",
                store(
                    PAGE - 4,
                    op::I64_STORE32,
                    2,
                    Expr::new().i64_const(0xdead_beef),
                )
                .then(load(PAGE - 4, op::I64_LOAD32_U, 2)),
                0xdead_beef,
            ),
            case_i64(
                "i64.store at 65528 writes the last eight bytes",
                store(
                    PAGE - 8,
                    op::I64_STORE,
                    3,
                    Expr::new().i64_const(0x0102_0304_0506_0708),
                )
                .then(load(PAGE - 8, op::I64_LOAD, 3)),
                0x0102_0304_0506_0708,
            ),
            case_i32(
                "f32.store at 65532 writes four bytes there",
                store(PAGE - 4, op::F32_STORE, 2, Expr::new().f32_const(1.5)).then(load(
                    PAGE - 4,
                    op::I32_LOAD,
                    2,
                )),
                1.5f32.to_bits() as i32,
            ),
            case_i64(
                "f64.store at 65528 writes eight bytes there",
                store(PAGE - 8, op::F64_STORE, 3, Expr::new().f64_const(1.5)).then(load(
                    PAGE - 8,
                    op::I64_LOAD,
                    3,
                )),
                1.5f64.to_bits() as i64,
            ),
            case_i32(
                "and the byte before the last four is still zero",
                store_i32(PAGE - 4, op::I32_STORE, 2, -1).then(load(PAGE - 5, op::I32_LOAD8_U, 0)),
                0,
            ),
        ],
    )
});

wasm_test!(first_invalid_store, |ctx| {
    run_cases_with(
        ctx,
        mem("first-invalid-store", 1),
        vec![
            case_void_trap(
                "i32.store8 at 65536 traps",
                store_i32(PAGE, op::I32_STORE8, 0, 1),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "i32.store16 at 65535 traps",
                store_i32(PAGE - 1, op::I32_STORE16, 1, 1),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "i32.store at 65533 traps",
                store_i32(PAGE - 3, op::I32_STORE, 2, 1),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "i64.store32 at 65533 traps",
                store(PAGE - 3, op::I64_STORE32, 2, Expr::new().i64_const(1)),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "i64.store at 65529 traps",
                store(PAGE - 7, op::I64_STORE, 3, Expr::new().i64_const(1)),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "f32.store at 65533 traps",
                store(PAGE - 3, op::F32_STORE, 2, Expr::new().f32_const(1.5)),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "f64.store at 65529 traps",
                store(PAGE - 7, op::F64_STORE, 3, Expr::new().f64_const(1.5)),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "a store well past the page traps too",
                store_i32(PAGE * 4, op::I32_STORE, 2, 1),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
        ],
    )
});

wasm_test!(minus_one, |ctx| {
    ctx.note("i32.const -1 as an address means 4294967295, not one byte back from the end");
    run_cases_with(
        ctx,
        mem("address-minus-one", 1),
        vec![
            case_trap(
                "i32.load8_u at -1 traps",
                load(-1, op::I32_LOAD8_U, 0),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_trap(
                "i32.load at -1 traps",
                load(-1, op::I32_LOAD, 2),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_trap(
                "i64.load at -1 traps",
                load(-1, op::I64_LOAD, 3).op(op::I32_WRAP_I64),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_trap(
                "f64.load at -1 traps",
                load(-1, op::F64_LOAD, 3)
                    .op(op::I64_REINTERPRET_F64)
                    .op(op::I32_WRAP_I64),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "i32.store8 at -1 traps",
                store_i32(-1, op::I32_STORE8, 0, 1),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "i32.store at -1 traps",
                store_i32(-1, op::I32_STORE, 2, 1),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_trap(
                "and -4 does not mean the last four bytes either",
                load(-4, op::I32_LOAD, 2),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_trap(
                "INT_MIN as an address is 2147483648 and traps",
                load(i32::MIN, op::I32_LOAD8_U, 0),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
        ],
    )
});

wasm_test!(boundary_follows_size, |ctx| {
    // Three pages, then none at all: the arithmetic is the same, only the size changes.
    run_cases_with(
        ctx,
        mem("bounds-3-pages", 3),
        vec![
            case_i32(
                "with three pages i32.load at 196604 is the last four-byte load",
                load(3 * PAGE - 4, op::I32_LOAD, 2),
                0,
            ),
            case_trap(
                "and 196605 traps",
                load(3 * PAGE - 3, op::I32_LOAD, 2),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_i32(
                "i32.load8_u at 196607 is the last byte",
                load(3 * PAGE - 1, op::I32_LOAD8_U, 0),
                0,
            ),
            case_trap(
                "and 196608 traps",
                load(3 * PAGE, op::I32_LOAD8_U, 0),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_i64(
                "i64.load at 196600 is the last eight-byte load",
                load(3 * PAGE - 8, op::I64_LOAD, 3),
                0,
            ),
            case_i32(
                "an address inside the second page is unremarkable",
                store_i32(PAGE + 16, op::I32_STORE, 2, 7).then(load(PAGE + 16, op::I32_LOAD, 2)),
                7,
            ),
        ],
    )?;
    run_cases_with(
        ctx,
        mem("bounds-0-pages", 0),
        vec![
            case_trap(
                "a zero-page memory has no valid address at all, not even 0",
                load(0, op::I32_LOAD8_U, 0),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "and nothing can be stored into it",
                store_i32(0, op::I32_STORE8, 0, 1),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_i32(
                "but memory.size is happy to say 0",
                Expr::new().memory_size(),
                0,
            ),
        ],
    )
});

wasm_test!(grow_moves_the_boundary, |ctx| {
    run_cases_with(
        ctx,
        mem("bounds-after-grow", 1),
        vec![
            case_trap(
                "i32.load at 65533 traps before the grow",
                load(PAGE - 3, op::I32_LOAD, 2),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_i32(
                "after memory.grow 1 the same address reads a zero",
                grow(1).then(load(PAGE - 3, op::I32_LOAD, 2)),
                0,
            ),
            case_i32(
                "the last four-byte load is now at 131068",
                grow(1).then(load(2 * PAGE - 4, op::I32_LOAD, 2)),
                0,
            ),
            case_trap(
                "and 131069 traps",
                grow(1).then(load(2 * PAGE - 3, op::I32_LOAD, 2)),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_i32(
                "the last byte is now 131071",
                grow(1).then(load(2 * PAGE - 1, op::I32_LOAD8_U, 0)),
                0,
            ),
            case_trap(
                "and 131072 traps",
                grow(1).then(load(2 * PAGE, op::I32_LOAD8_U, 0)),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_i32(
                "a store at the new last address works and reads back",
                grow(1)
                    .then(store_i32(2 * PAGE - 4, op::I32_STORE, 2, 0x1234_5678))
                    .then(load(2 * PAGE - 4, op::I32_LOAD, 2)),
                0x1234_5678,
            ),
        ],
    )
});

wasm_test!(zero_length_fill, |ctx| {
    ctx.note(
        "a load or a store always touches at least one byte, so only a bulk operation can \
         have length 0 — this is the one bulk case that belongs to the boundary",
    );
    run_cases_with(
        ctx,
        mem("zero-length-fill", 1),
        vec![
            case_i32(
                "memory.fill of length 0 at 65536, one past the last byte, is legal",
                fill(PAGE, 0xff, 0).i32_const(1),
                1,
            ),
            case_i32(
                "memory.fill of length 0 at 0 is legal and changes nothing",
                store_i32(0, op::I32_STORE8, 0, 7)
                    .then(fill(0, 0xff, 0))
                    .then(load(0, op::I32_LOAD8_U, 0)),
                7,
            ),
            case_i32(
                "memory.fill of length 1 at 65535 is the last legal fill",
                fill(PAGE - 1, 0xaa, 1).then(load(PAGE - 1, op::I32_LOAD8_U, 0)),
                0xaa,
            ),
            case_void_trap(
                "memory.fill of length 1 at 65536 traps",
                fill(PAGE, 0xff, 1),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "memory.fill of length 0 at 65537 traps: the destination itself is outside",
                fill(PAGE + 1, 0xff, 0),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "memory.fill of length 0 at -1 traps for the same reason",
                fill(-1, 0xff, 0),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// Examples
// ---------------------------------------------------------------------------------------

/// `f() -> i32`: the last `i32.load` a one-page memory allows.
fn last_word() -> Module {
    mem_i32("last-word", load(PAGE - 4, op::I32_LOAD, 2))
}

/// `f() -> i32`: one byte further on, which must trap.
fn one_past() -> Module {
    mem_i32("one-past", load(PAGE - 3, op::I32_LOAD, 2))
}

/// Worked examples: the two addresses either side of the line.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("The last four bytes of a page", last_word)
            .summary("one page of memory and a body of `i32.const 65532; i32.load`")
            .command("run --invoke f mod.wasm")
            .output("0")
            .note(
                "65 532 + 4 is exactly 65 536, the size of the memory, so the access fits. \
                 The answer is 0 because nothing was ever written there.",
            ),
        ExampleSpec::module("One byte too far", one_past)
            .summary("the same module with the address changed to 65 533")
            .command("run --invoke f mod.wasm")
            .output(
                "nothing on stdout, `wasm trap: out of bounds memory access` on stderr, and \
                 a non-zero exit status",
            )
            .note(
                "65 533 is a perfectly good address; the fourth byte of the load is not. \
                 Bound `address + width`, not `address`, or this one prints 0.",
            ),
    ]
}
