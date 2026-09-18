//! Stage 30 — `memory.size` and `memory.grow`.
//!
//! Both instructions count **pages**, not bytes, and a page is 65 536 bytes. `memory.size`
//! starts at the declared minimum. `memory.grow n` takes the number of pages to add and
//! answers with the size the memory had **before** it grew — so the first successful
//! `memory.grow 1` on a one-page memory returns `1`, not `2`, and not `0`.
//!
//! When it cannot grow, `memory.grow` returns `-1` and **changes nothing**: it is not a
//! trap, execution carries straight on, the size is what it was, and every byte already in
//! memory is still there. That is the half a from-scratch runtime usually gets wrong, so
//! there is a test for each part of it.
//!
//! There are two reasons a grow can fail: the module declared a maximum and the request
//! would pass it, or the request would pass the architectural limit of 65 536 pages (4 GiB)
//! that a 32-bit memory has whether or not a maximum is written down. Asking a
//! no-maximum memory for 100 000 more pages must answer `-1`, not abort, and not try to
//! allocate it.

use crate::examples::ExampleSpec;
use crate::stages::{case_i32, case_void_trap, expect, run_cases_with, trap, Stage, Test};
use crate::wasm::{ftype, op, Expr, Func, Limits, Module, ModuleBuilder, Op, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 30,
        slug: "memory_size_and_grow",
        name: "memory.size and memory.grow",
        ext: false,
        hints: &[
            "`memory.size` answers in pages of 65 536 bytes and starts at the memory's declared minimum, so divide by the page size rather than returning a byte count",
            "`memory.grow n` returns the size *before* the growth; only the failure case returns -1, and -1 is a value, not a trap, so execution continues with it on the stack",
            "A failed grow must leave everything alone: the same size, the same bytes, no partial allocation — write the new size only once you know it fits",
            "Two limits can refuse a grow: the declared maximum, and the 65 536-page ceiling of a 32-bit memory that applies even when no maximum is declared",
        ],
        examples,
        tests: vec![
            Test::new("memory.size counts pages, not bytes", size_counts_pages),
            Test::new("memory.grow returns the size before it grew", grow_returns_old_size),
            Test::new("memory.grow 0 is legal and changes nothing", grow_zero),
            Test::new("growing past the declared maximum returns -1", past_the_max),
            Test::new("a failed grow leaves the memory and its size untouched", failure_is_inert),
            Test::new(
                "a memory with no declared maximum still refuses an implausible grow",
                no_max_still_has_a_ceiling,
            ),
            Test::new("a page that only exists after a grow", the_new_page),
            Test::new("a failed grow is not a trap", failure_is_not_a_trap),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Building blocks
// ---------------------------------------------------------------------------------------

/// A builder with a memory of the given limits and nothing else in it.
fn mem(label: &str, limits: Limits) -> ModuleBuilder {
    ModuleBuilder::new(label).memory(limits)
}

/// A module with a memory of the given limits exporting `f: () -> i32`.
fn mem_i32(label: &str, limits: Limits, body: Expr) -> Module {
    let mut b = mem(label, limits);
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(ty, Func::new(body));
    b.export_func("f", idx).build()
}

/// `memory.grow n`, leaving the old size on the stack.
fn grow(n: i32) -> Expr {
    Expr::new().i32_const(n).memory_grow()
}

/// `memory.grow n` with its answer thrown away.
fn grow_and_forget(n: i32) -> Expr {
    grow(n).drop()
}

/// A store of `value` at the literal address `addr`, at the given width.
fn store(addr: i32, o: Op, align: u32, value: Expr) -> Expr {
    Expr::new().i32_const(addr).then(value).mem(o, align, 0)
}

/// A load at the literal address `addr`, at the given width.
fn load(addr: i32, o: Op, align: u32) -> Expr {
    Expr::new().i32_const(addr).mem(o, align, 0)
}

/// `i32.store` of `value` at `addr`.
fn put32(addr: i32, value: i32) -> Expr {
    store(addr, op::I32_STORE, 2, Expr::new().i32_const(value))
}

/// `i32.load` at `addr`.
fn get32(addr: i32) -> Expr {
    load(addr, op::I32_LOAD, 2)
}

/// One page, in bytes.
const PAGE: i32 = 65_536;

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

wasm_test!(size_counts_pages, |ctx| {
    // The declaration is the only thing that changes between these four modules, so they
    // cannot share a sweep.
    ctx.note("a page is 65 536 bytes; memory.size never answers in bytes");
    let size = |label: &str, limits: Limits| mem_i32(label, limits, Expr::new().memory_size());
    expect(ctx, &size("size-1", Limits::min(1)), "1")?;
    expect(ctx, &size("size-3", Limits::min(3)), "3")?;
    expect(ctx, &size("size-2-of-5", Limits::range(2, 5)), "2")?;
    expect(ctx, &size("size-0", Limits::min(0)), "0")?;
    expect(ctx, &size("size-17", Limits::range(17, 17)), "17")?;
    Ok(())
});

wasm_test!(grow_returns_old_size, |ctx| {
    run_cases_with(
        ctx,
        mem("grow-old-size", Limits::range(1, 8)),
        vec![
            case_i32("memory.grow 1 on a one-page memory returns 1", grow(1), 1),
            case_i32("memory.grow 3 also returns the old size, 1", grow(3), 1),
            case_i32(
                "after memory.grow 1 the size is 2",
                grow_and_forget(1).memory_size(),
                2,
            ),
            case_i32(
                "a second memory.grow 1 returns 2",
                grow_and_forget(1).then(grow(1)),
                2,
            ),
            case_i32(
                "grow 1 then grow 2 leaves the size at 4",
                grow_and_forget(1).then(grow_and_forget(2)).memory_size(),
                4,
            ),
            case_i32("grow 7 straight to the maximum returns 1", grow(7), 1),
            case_i32(
                "and the size is then 8",
                grow_and_forget(7).memory_size(),
                8,
            ),
        ],
    )
});

wasm_test!(grow_zero, |ctx| {
    run_cases_with(
        ctx,
        mem("grow-zero", Limits::range(1, 4)),
        vec![
            case_i32("memory.grow 0 returns the current size", grow(0), 1),
            case_i32(
                "memory.grow 0 leaves the size where it was",
                grow_and_forget(0).memory_size(),
                1,
            ),
            case_i32(
                "memory.grow 0 after a real grow returns the new size",
                grow_and_forget(1).then(grow(0)),
                2,
            ),
            case_i32(
                "memory.grow 0 does not disturb the bytes",
                put32(0, 0x1234_5678)
                    .then(grow_and_forget(0))
                    .then(get32(0)),
                0x1234_5678,
            ),
            case_i32(
                "memory.grow 0 on a memory already at its maximum still returns the size",
                grow_and_forget(3).then(grow(0)),
                4,
            ),
        ],
    )
});

wasm_test!(past_the_max, |ctx| {
    run_cases_with(
        ctx,
        mem("grow-past-max", Limits::range(1, 2)),
        vec![
            case_i32(
                "grow 1 is within the maximum of 2 and returns 1",
                grow(1),
                1,
            ),
            case_i32(
                "grow 2 would make three pages, so it returns -1",
                grow(2),
                -1,
            ),
            case_i32(
                "grow 1 twice: the second one is refused",
                grow_and_forget(1).then(grow(1)),
                -1,
            ),
            case_i32("grow 2147483647 is refused", grow(i32::MAX), -1),
            case_i32(
                "a refused grow leaves memory.size at 1",
                grow_and_forget(2).memory_size(),
                1,
            ),
            case_i32(
                "a refused grow does not stop a later legal one",
                grow_and_forget(2).then(grow(1)),
                1,
            ),
        ],
    )
});

wasm_test!(failure_is_inert, |ctx| {
    run_cases_with(
        ctx,
        mem("grow-inert", Limits::range(1, 2)),
        vec![
            case_i32(
                "a word written before a failed grow is still there afterwards",
                put32(0, 0x1234_5678)
                    .then(grow_and_forget(5))
                    .then(get32(0)),
                0x1234_5678,
            ),
            case_i32(
                "so is a byte at the very end of the page",
                store(PAGE - 1, op::I32_STORE8, 0, Expr::new().i32_const(200))
                    .then(grow_and_forget(99))
                    .then(load(PAGE - 1, op::I32_LOAD8_U, 0)),
                200,
            ),
            case_i32(
                "memory.size is unchanged after a failed grow",
                grow_and_forget(5).memory_size(),
                1,
            ),
            case_i32("and the failed grow really answered -1", grow(5), -1),
            case_i32(
                "a failed grow does not half-grow: the page after the end still traps",
                grow_and_forget(5).memory_size().i32_const(1).op(op::I32_EQ),
                1,
            ),
            case_void_trap(
                "and storing into the page it did not allocate still traps",
                grow_and_forget(5).then(put32(PAGE, 1)),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
        ],
    )
});

wasm_test!(no_max_still_has_a_ceiling, |ctx| {
    ctx.note(
        "a 32-bit memory tops out at 65 536 pages (4 GiB) whether or not a maximum is declared",
    );
    run_cases_with(
        ctx,
        mem("grow-no-max", Limits::min(1)),
        vec![
            case_i32(
                "grow 100000 is beyond any 32-bit memory and returns -1",
                grow(100_000),
                -1,
            ),
            case_i32(
                "grow 65536 would make 65537 pages, one past the ceiling",
                grow(65_536),
                -1,
            ),
            case_i32("grow 2147483647 returns -1", grow(i32::MAX), -1),
            case_i32(
                "memory.size is still 1 after all that",
                grow_and_forget(100_000).memory_size(),
                1,
            ),
            case_i32("a modest grow of 1 still works", grow(1), 1),
            case_i32(
                "and the bytes survived the refused grows",
                put32(0, -559_038_737)
                    .then(grow_and_forget(100_000))
                    .then(get32(0)),
                -559_038_737,
            ),
        ],
    )
});

wasm_test!(the_new_page, |ctx| {
    run_cases_with(
        ctx,
        mem("new-page", Limits::min(1)),
        vec![
            case_void_trap(
                "storing at 65536 before a grow traps",
                put32(PAGE, 0x1234_5678),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_i32(
                "after memory.grow 1 the same store works and reads back",
                grow_and_forget(1)
                    .then(put32(PAGE, 0x1234_5678))
                    .then(get32(PAGE)),
                0x1234_5678,
            ),
            case_i32(
                "the new page starts as zeroes",
                grow_and_forget(1).then(get32(PAGE)),
                0,
            ),
            case_i32(
                "so does the far end of the new page",
                grow_and_forget(1).then(get32(2 * PAGE - 4)),
                0,
            ),
            case_i32(
                "the old page keeps its bytes across a grow",
                put32(0, 0x0bad_f00du32 as i32)
                    .then(grow_and_forget(1))
                    .then(get32(0)),
                0x0bad_f00du32 as i32,
            ),
            case_void_trap(
                "and the page after the new one is still out of bounds",
                grow_and_forget(1).then(put32(2 * PAGE, 1)),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
        ],
    )
});

wasm_test!(failure_is_not_a_trap, |ctx| {
    run_cases_with(
        ctx,
        mem("grow-not-a-trap", Limits::range(1, 2)),
        vec![
            case_i32(
                "execution carries on after a refused grow",
                grow_and_forget(99).i32_const(42),
                42,
            ),
            case_i32(
                "the -1 is an ordinary value that can be compared",
                grow(99).i32_const(-1).op(op::I32_EQ),
                1,
            ),
            case_i32(
                "and branched on",
                grow(99).i32_const(-1).op(op::I32_EQ).if_else(
                    crate::wasm::BlockType::Value(ValType::I32),
                    Expr::new().i32_const(7),
                    Expr::new().i32_const(9),
                ),
                7,
            ),
            case_i32(
                "two refused grows in a row are both just values",
                grow_and_forget(99).then(grow(99)),
                -1,
            ),
            case_i32(
                "a refused grow followed by a load of the old memory is fine",
                put32(16, 5).then(grow_and_forget(99)).then(get32(16)),
                5,
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// Examples
// ---------------------------------------------------------------------------------------

/// `f() -> i32`: grow a one-page memory by one and report what `memory.grow` answered.
fn grow_answer() -> Module {
    mem_i32("grow-answer", Limits::range(1, 4), grow(1))
}

/// `f() -> i32`: ask a `max 2` memory for five more pages, then report `memory.size`.
fn refused_grow() -> Module {
    mem_i32(
        "grow-refused",
        Limits::range(1, 2),
        grow_and_forget(5).memory_size(),
    )
}

/// Worked examples: what a grow answers, and what a refused one leaves behind.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("What memory.grow answers", grow_answer)
            .summary("a memory declared `min 1, max 4`, and a body of `i32.const 1; memory.grow`")
            .command("run --invoke f mod.wasm")
            .output("1")
            .note(
                "The answer is the size *before* the growth, in pages. A runtime that \
                 returns the new size prints 2 here, and one that returns a byte count \
                 prints 65536.",
            ),
        ExampleSpec::module("A grow that is refused", refused_grow)
            .summary(
                "a memory declared `min 1, max 2`, asked for five more pages, which then \
                 reports `memory.size`",
            )
            .command("run --invoke f mod.wasm")
            .output("1")
            .note(
                "`memory.grow` returned -1 — dropped here — and the memory is exactly as it \
                 was: still one page, still holding whatever it held. A refused grow is a \
                 value, not a trap, so the function runs to the end and prints a number.",
            ),
    ]
}
