//! Stage 37 — Tables: `get`, `set`, `size`, `grow`, `fill`, `copy`. **[ext]**
//!
//! A table is a vector of references with a `{min, max}` the same shape as a memory's, and
//! the same six operations a memory has — only the unit is one reference rather than one
//! byte, and there is no notion of a page. Everything here is the reference-types proposal:
//! a module before it had exactly one table, no `table.get`, and no way to write a slot
//! except an element segment at instantiation.
//!
//! Every module in this stage declares **two** tables, because the table index is an
//! immediate on every one of these instructions and a runtime that ignores it — or that
//! keeps a single table because a 1.0 module could only have one — passes a
//! one-table test suite and then silently reads the wrong table.
//!
//! The two that catch most implementations:
//!
//! * `table.grow` answers the size *before* it grew, and `-1` — not a trap, not 0 — when it
//!   cannot, in which case the table is exactly as it was;
//! * `table.copy` is a `memmove`, not a `memcpy`: the source and the destination may overlap
//!   in either direction and the result must look as though the whole range was read before
//!   any of it was written.

use crate::examples::ExampleSpec;
use crate::stages::{case_i32, case_trap, case_void_trap, run_cases_with, trap, Stage, Test};
use crate::wasm::{ftype, op, Expr, Func, Limits, Module, ModuleBuilder, TableType, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 37,
        slug: "table_operations",
        name: "Tables: get, set, size, grow, fill, copy",
        ext: true,
        hints: &[
            "`table.get` and `table.set` are plain opcodes (0x25, 0x26) with a table index; `size`, `grow`, `fill`, `copy` and `init` are 0xfc-prefixed and carry theirs in the second operand",
            "`table.grow` returns the size before the growth, or -1 when it refuses, and a refused growth must leave the table untouched",
            "`table.copy` may overlap in either direction: copy as though the whole source range were read before the first slot is written",
            "A module may declare more than one table, so carry the table index through every one of these instructions instead of assuming table 0",
        ],
        examples,
        tests: vec![
            Test::new(
                "table.get and table.set move a funcref between slots",
                get_and_set,
            )
            .ext(),
            Test::new("table.size answers each table's own size", size).ext(),
            Test::new(
                "table.grow returns the old size and the new slots are null",
                grow,
            )
            .ext(),
            Test::new(
                "table.grow past a declared maximum returns -1 and changes nothing",
                grow_past_max,
            )
            .ext(),
            Test::new("table.fill writes one reference across a range", fill).ext(),
            Test::new(
                "table.copy inside one table survives an overlap either way",
                copy_overlap,
            )
            .ext(),
            Test::new(
                "table.copy moves references between two tables",
                copy_between_tables,
            )
            .ext(),
            Test::new(
                "table.get and table.set past the end of a table trap",
                out_of_bounds,
            )
            .ext(),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// The module every sweep starts from
// ---------------------------------------------------------------------------------------

/// The type index of `() -> i32`, which is both the callees' type and the cases' type.
const RET: u32 = 0;

/// Two tables and three functions to put in them.
///
/// ```text
/// table 0: funcref, min 6, max 8 — slots 0,1,2 = one,two,three; 3,4,5 null
/// table 1: funcref, min 3, no max — every slot null
/// ```
///
/// `one`, `two` and `three` return 1, 2 and 3, so a slot's contents can be read out loud by
/// calling through it. They are named by the element segment, which is what makes `ref.func`
/// legal for them — see stage 39 for the rule that turns on.
fn base(label: &str) -> ModuleBuilder {
    let mut b = ModuleBuilder::new(label);
    let ret = b.add_type(ftype(&[], &[ValType::I32]));
    debug_assert_eq!(ret, RET);
    let one = b.add_func(ret, Func::new(Expr::new().i32_const(1)));
    let two = b.add_func(ret, Func::new(Expr::new().i32_const(2)));
    let three = b.add_func(ret, Func::new(Expr::new().i32_const(3)));
    b.table(TableType {
        elem: ValType::FuncRef,
        limits: Limits::range(6, 8),
    })
    .table(TableType {
        elem: ValType::FuncRef,
        limits: Limits::min(3),
    })
    .elem_active(0, &[one, two, three])
}

/// The function index of `one`, `two`, `three` — the first three functions [`base`] defines.
const ONE: u32 = 0;
/// See [`ONE`].
const THREE: u32 = 2;

/// `table.get t i; ref.is_null` — 1 when the slot is empty, 0 when it holds something.
fn is_null(table: u32, i: i32) -> Expr {
    Expr::new().i32_const(i).table_get(table).ref_is_null()
}

/// `i32.const i; call_indirect (type RET) (table t)` — the slot says which function it holds.
fn call_slot(table: u32, i: i32) -> Expr {
    Expr::new().i32_const(i).call_indirect(RET, table)
}

/// `table.set t dst (table.get t src)`.
fn move_slot(table: u32, dst: i32, src: i32) -> Expr {
    Expr::new()
        .i32_const(dst)
        .i32_const(src)
        .table_get(table)
        .table_set(table)
}

/// `table.fill t at <ref> n`.
fn fill_with(table: u32, at: i32, value: Expr, n: i32) -> Expr {
    Expr::new()
        .i32_const(at)
        .then(value)
        .i32_const(n)
        .table_fill(table)
}

/// `table.copy dst_table src_table dst src n`.
fn copy(dst_table: u32, src_table: u32, dst: i32, src: i32, n: i32) -> Expr {
    Expr::new()
        .i32_const(dst)
        .i32_const(src)
        .i32_const(n)
        .table_copy(dst_table, src_table)
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

wasm_test!(get_and_set, |ctx| {
    run_cases_with(
        ctx,
        base("table-get-set"),
        vec![
            case_i32("slot 3 starts out null", is_null(0, 3), 1),
            case_i32("slot 0 starts out full", is_null(0, 0), 0),
            case_i32(
                "a reference copied from slot 0 to slot 3 is callable through slot 3",
                move_slot(0, 3, 0).then(call_slot(0, 3)),
                1,
            ),
            case_i32(
                "table.get does not empty the slot it read",
                move_slot(0, 3, 0).then(call_slot(0, 0)),
                1,
            ),
            case_i32(
                "table.set takes any funcref, including one from ref.func",
                Expr::new()
                    .i32_const(4)
                    .ref_func(THREE)
                    .table_set(0)
                    .then(call_slot(0, 4)),
                3,
            ),
            case_i32(
                "a slot can be emptied again with ref.null",
                Expr::new()
                    .i32_const(0)
                    .ref_null(ValType::FuncRef)
                    .table_set(0)
                    .then(is_null(0, 0)),
                1,
            ),
            case_i32(
                "table 1 is a different table: writing slot 0 of it leaves table 0 alone",
                Expr::new()
                    .i32_const(0)
                    .ref_func(THREE)
                    .table_set(1)
                    .then(call_slot(0, 0)),
                1,
            ),
        ],
    )
});

wasm_test!(size, |ctx| {
    run_cases_with(
        ctx,
        base("table-size"),
        vec![
            case_i32("table 0 declares min 6", Expr::new().table_size(0), 6),
            case_i32("table 1 declares min 3", Expr::new().table_size(1), 3),
            case_i32(
                "writing a slot does not change the size",
                Expr::new()
                    .i32_const(5)
                    .ref_func(ONE)
                    .table_set(0)
                    .table_size(0),
                6,
            ),
            case_i32(
                "the two sizes differ, so the index is being read",
                Expr::new().table_size(0).table_size(1).op(op::I32_SUB),
                3,
            ),
        ],
    )
});

wasm_test!(grow, |ctx| {
    run_cases_with(
        ctx,
        base("table-grow"),
        vec![
            case_i32(
                "growing table 0 by 1 answers 6, the size before",
                Expr::new()
                    .ref_null(ValType::FuncRef)
                    .i32_const(1)
                    .table_grow(0),
                6,
            ),
            case_i32(
                "and the table is one slot longer afterwards",
                Expr::new()
                    .ref_null(ValType::FuncRef)
                    .i32_const(1)
                    .table_grow(0)
                    .drop()
                    .table_size(0),
                7,
            ),
            case_i32(
                "the slot that appeared is null",
                Expr::new()
                    .ref_null(ValType::FuncRef)
                    .i32_const(1)
                    .table_grow(0)
                    .drop()
                    .then(is_null(0, 6)),
                1,
            ),
            case_i32(
                "growing by 0 answers the current size and changes nothing",
                Expr::new()
                    .ref_null(ValType::FuncRef)
                    .i32_const(0)
                    .table_grow(0)
                    .drop()
                    .table_size(0),
                6,
            ),
            case_i32(
                "table 1 has no declared maximum, so it grows as far as it is asked",
                Expr::new()
                    .ref_null(ValType::FuncRef)
                    .i32_const(5)
                    .table_grow(1)
                    .drop()
                    .table_size(1),
                8,
            ),
            case_i32(
                "the new slots hold the reference table.grow was given, not always null",
                Expr::new()
                    .ref_func(THREE)
                    .i32_const(1)
                    .table_grow(1)
                    .drop()
                    .then(call_slot(1, 3)),
                3,
            ),
        ],
    )
});

wasm_test!(grow_past_max, |ctx| {
    run_cases_with(
        ctx,
        base("table-grow-max"),
        vec![
            case_i32(
                "6 + 3 is past the declared max of 8, so table.grow answers -1",
                Expr::new()
                    .ref_null(ValType::FuncRef)
                    .i32_const(3)
                    .table_grow(0),
                -1,
            ),
            case_i32(
                "a refused growth leaves the size where it was",
                Expr::new()
                    .ref_null(ValType::FuncRef)
                    .i32_const(3)
                    .table_grow(0)
                    .drop()
                    .table_size(0),
                6,
            ),
            case_i32(
                "growing to exactly the maximum is allowed",
                Expr::new()
                    .ref_null(ValType::FuncRef)
                    .i32_const(2)
                    .table_grow(0)
                    .drop()
                    .table_size(0),
                8,
            ),
            case_i32(
                "and one slot past it is not",
                Expr::new()
                    .ref_null(ValType::FuncRef)
                    .i32_const(2)
                    .table_grow(0)
                    .drop()
                    .ref_null(ValType::FuncRef)
                    .i32_const(1)
                    .table_grow(0),
                -1,
            ),
            case_i32(
                "a refused growth is not a trap: execution carries on",
                Expr::new()
                    .ref_null(ValType::FuncRef)
                    .i32_const(99)
                    .table_grow(0)
                    .drop()
                    .then(call_slot(0, 0)),
                1,
            ),
        ],
    )
});

wasm_test!(fill, |ctx| {
    run_cases_with(
        ctx,
        base("table-fill"),
        vec![
            case_i32(
                "filling slots 3..5 with `three` makes slot 3 callable",
                fill_with(0, 3, Expr::new().ref_func(THREE), 3).then(call_slot(0, 3)),
                3,
            ),
            case_i32(
                "and slot 5, the last one in the range",
                fill_with(0, 3, Expr::new().ref_func(THREE), 3).then(call_slot(0, 5)),
                3,
            ),
            case_i32(
                "the slot before the range is untouched",
                fill_with(0, 3, Expr::new().ref_func(THREE), 3).then(call_slot(0, 2)),
                3,
            ),
            case_i32(
                "filling with ref.null empties the range",
                fill_with(0, 0, Expr::new().ref_null(ValType::FuncRef), 2).then(is_null(0, 1)),
                1,
            ),
            case_i32(
                "a fill of length 0 is a no-op, even at the very end of the table",
                fill_with(0, 6, Expr::new().ref_null(ValType::FuncRef), 0).then(call_slot(0, 0)),
                1,
            ),
            case_i32(
                "table.fill reads its table index too",
                fill_with(1, 0, Expr::new().ref_func(THREE), 3).then(call_slot(1, 2)),
                3,
            ),
            case_trap(
                "a fill that runs off the end traps and is not clamped",
                fill_with(0, 4, Expr::new().ref_null(ValType::FuncRef), 3)
                    .then(Expr::new().i32_const(0)),
                &[trap::TABLE_OUT_OF_BOUNDS],
            ),
        ],
    )
});

wasm_test!(copy_overlap, |ctx| {
    // Slots 0,1,2 hold one,two,three. Copying 0..3 to 1..4 makes the destination overlap the
    // source from above; copying 1..3 to 0..2 makes it overlap from below. A naive forward
    // loop gets one of the two wrong — it reads a slot it has already written.
    run_cases_with(
        ctx,
        base("table-copy-overlap"),
        vec![
            case_i32(
                "copying 0..3 to 1..4 leaves `one` at slot 1",
                copy(0, 0, 1, 0, 3).then(call_slot(0, 1)),
                1,
            ),
            case_i32(
                "and `two` at slot 2",
                copy(0, 0, 1, 0, 3).then(call_slot(0, 2)),
                2,
            ),
            case_i32(
                "and `three` at slot 3",
                copy(0, 0, 1, 0, 3).then(call_slot(0, 3)),
                3,
            ),
            case_i32(
                "copying 1..3 to 0..2 leaves `two` at slot 0",
                copy(0, 0, 0, 1, 2).then(call_slot(0, 0)),
                2,
            ),
            case_i32(
                "and `three` at slot 1",
                copy(0, 0, 0, 1, 2).then(call_slot(0, 1)),
                3,
            ),
            case_i32(
                "a copy onto itself changes nothing",
                copy(0, 0, 0, 0, 3).then(call_slot(0, 1)),
                2,
            ),
            case_i32(
                "a copy of length 0 at the very end of the table is not out of bounds",
                copy(0, 0, 6, 6, 0).then(call_slot(0, 0)),
                1,
            ),
            case_trap(
                "a copy that runs off the end traps",
                copy(0, 0, 4, 0, 3).then(Expr::new().i32_const(0)),
                &[trap::TABLE_OUT_OF_BOUNDS],
            ),
        ],
    )
});

wasm_test!(copy_between_tables, |ctx| {
    run_cases_with(
        ctx,
        base("table-copy-tables"),
        vec![
            case_i32(
                "table 1 is empty until something is copied into it",
                is_null(1, 0),
                1,
            ),
            case_i32(
                "copying table 0's first three slots into table 1 makes slot 0 callable",
                copy(1, 0, 0, 0, 3).then(call_slot(1, 0)),
                1,
            ),
            case_i32("and slot 2", copy(1, 0, 0, 0, 3).then(call_slot(1, 2)), 3),
            case_i32(
                "the source table is untouched",
                copy(1, 0, 0, 0, 3).then(call_slot(0, 1)),
                2,
            ),
            case_i32(
                "copying the other way brings table 1's nulls back into table 0",
                copy(0, 1, 0, 0, 2).then(is_null(0, 1)),
                1,
            ),
            case_i32(
                "the two table indices are read in the order dst, src",
                copy(1, 0, 1, 2, 1).then(call_slot(1, 1)),
                3,
            ),
        ],
    )
});

wasm_test!(out_of_bounds, |ctx| {
    // Table 0 has six slots, so 6 is the first index that does not exist; table 1 has three.
    // The index is unsigned, so -1 is past the end rather than before the start.
    run_cases_with(
        ctx,
        base("table-bounds"),
        vec![
            case_trap(
                "table.get at index 6 of a six-slot table",
                is_null(0, 6),
                &[trap::TABLE_OUT_OF_BOUNDS],
            ),
            case_trap(
                "table.get at index -1",
                is_null(0, -1),
                &[trap::TABLE_OUT_OF_BOUNDS],
            ),
            case_trap(
                "table.get at index 3 of table 1, which has three slots",
                is_null(1, 3),
                &[trap::TABLE_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "table.set at index 6",
                Expr::new().i32_const(6).ref_func(ONE).table_set(0),
                &[trap::TABLE_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "table.set at index -1",
                Expr::new()
                    .i32_const(-1)
                    .ref_null(ValType::FuncRef)
                    .table_set(0),
                &[trap::TABLE_OUT_OF_BOUNDS],
            ),
        ],
    )
});

/// The example module: move a reference, then call through its new slot.
fn shuffle() -> Module {
    let mut b = base("table-shuffle");
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(
        ty,
        Func::new(
            move_slot(0, 4, 2)
                .then(call_slot(0, 4))
                .then(Expr::new().table_size(1))
                .op(op::I32_ADD),
        ),
    );
    b.export_func("f", idx).build()
}

/// Worked examples: moving a reference about, and the growth that is refused.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("Moving a funcref to another slot", shuffle)
            .summary(
                "two tables — six `funcref` slots with `one`, `two`, `three` in the first \
                 three, and a second table of three empty slots — and an `f` that copies \
                 slot 2 to slot 4 with `table.get`/`table.set`, calls through slot 4, and \
                 adds `table.size 1`",
            )
            .command("run --invoke f mod.wasm")
            .output("6")
            .note(
                "3 from calling `three` through its new slot, plus 3 for the size of the \
                 *other* table: every one of these instructions carries a table index, and \
                 this module has two tables so that a runtime cannot quietly ignore it.",
            ),
        ExampleSpec::module("A growth that is refused", || {
            let mut b = base("table-grow-refused");
            let ty = b.add_type(ftype(&[], &[ValType::I32, ValType::I32]));
            let idx = b.add_func(
                ty,
                Func::new(
                    Expr::new()
                        .ref_null(ValType::FuncRef)
                        .i32_const(3)
                        .table_grow(0)
                        .table_size(0),
                ),
            );
            b.export_func("f", idx).build()
        })
        .summary(
            "table 0 is declared `min 6, max 8`; `f` asks for three more slots and then \
             reports both what `table.grow` answered and what the size is now",
        )
        .command("run --invoke f mod.wasm")
        .output("-1\n6")
        .note(
            "`table.grow` answers the size *before* the growth on success and -1 when it \
             refuses — never 0, and never a trap. A refused growth must leave the table \
             exactly as it was, which is why the second line is still 6.",
        ),
    ]
}
