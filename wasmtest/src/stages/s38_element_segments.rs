//! Stage 38 — Element segments: active, passive, declarative. **[ext]**
//!
//! The element section is how functions get into a table, and it has three modes that do
//! three different things:
//!
//! * **active** — copied into a table at instantiation, before the start function and before
//!   any export can be called;
//! * **passive** — copied nowhere until a `table.init` asks for it, and gone once
//!   `elem.drop` says so;
//! * **declarative** — copied nowhere ever. It exists only to put its functions into the set
//!   `ref.func` is allowed to name (stage 39 is where that rule bites).
//!
//! The one worth slowing down for is **an active segment that does not fit**. It is *not* a
//! validation error, and a runtime that refuses such a module at load time is refusing it in
//! the wrong place. The table's size is not knowable while validating — its minimum is, but
//! an imported table may be larger, and the offset may be a `global.get` — so the spec makes
//! the range check part of instantiation. The module decodes, validates, and then **traps**:
//! `out of bounds table access`, with nothing on stdout, exactly as an out-of-range data
//! segment traps with `out of bounds memory access`. The difference is visible from the
//! outside, which is why this stage asserts the trap wording here and only here.
//!
//! The segments are also applied in order, and the check for one happens as it is applied —
//! so a module may have written part of a table before a later segment brings it down.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::stages::{
    case_i32, case_trap, expect_lines, run_cases_with, trap, Stage, Test, TRAP_MARKER,
};
use crate::wasm::{
    const_i32, ftype, Elem, ElemMode, Expr, Func, Limits, Module, ModuleBuilder, TableType, ValType,
};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 38,
        slug: "element_segments",
        name: "Element segments: active, passive, declarative",
        ext: true,
        hints: &[
            "The first byte of a segment is a bitfield, not an enum: bit 0 says passive-or-declarative, bit 1 says 'an explicit table index or element type follows', bit 2 says 'element expressions rather than function indices'",
            "Apply active segments in order at instantiation, before the start function runs; a passive one waits for `table.init` and a declarative one is never applied at all",
            "An active segment that does not fit its table is an instantiation *trap*, not a validation error — the module must decode and validate first",
            "`elem.drop` makes the segment empty rather than removing it, so a later `table.init` of a non-zero length is out of bounds",
        ],
        examples,
        tests: vec![
            Test::new(
                "an active segment is in the table before any code runs",
                active_before_anything,
            )
            .ext(),
            Test::new(
                "two active segments fill different parts of one table",
                two_active_segments,
            )
            .ext(),
            Test::new(
                "an active segment past the end of the table stops instantiation",
                active_past_end,
            )
            .ext(),
            Test::new(
                "an active segment ending exactly at the end of the table is accepted",
                active_exactly_at_end,
            )
            .ext(),
            Test::new(
                "a passive segment reaches the table only through table.init",
                passive_needs_init,
            )
            .ext(),
            Test::new("elem.drop makes a later table.init trap", drop_then_init).ext(),
            Test::new(
                "a declarative segment puts nothing in any table",
                declarative_is_empty,
            )
            .ext(),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Building blocks
// ---------------------------------------------------------------------------------------

/// The type index of `() -> i32`.
const RET: u32 = 0;
/// The function index of `one`; `two` and `three` follow it.
const ONE: u32 = 0;
/// See [`ONE`].
const THREE: u32 = 2;

/// A builder holding `one`, `two`, `three` (returning 1, 2, 3) and one `funcref` table.
///
/// No element segment yet: each test adds the segments it is about.
fn base(label: &str, slots: u32) -> ModuleBuilder {
    let mut b = ModuleBuilder::new(label);
    let ret = b.add_type(ftype(&[], &[ValType::I32]));
    debug_assert_eq!(ret, RET);
    b.add_func(ret, Func::new(Expr::new().i32_const(1)));
    b.add_func(ret, Func::new(Expr::new().i32_const(2)));
    b.add_func(ret, Func::new(Expr::new().i32_const(3)));
    b.table(TableType {
        elem: ValType::FuncRef,
        limits: Limits::min(slots),
    })
}

/// An active segment putting `funcs` into table 0 at `offset`.
fn active_at(offset: i32, funcs: &[u32]) -> Elem {
    Elem {
        mode: ElemMode::Active {
            table: 0,
            offset: const_i32(offset),
        },
        ty: ValType::FuncRef,
        funcs: funcs.to_vec(),
        exprs: Vec::new(),
    }
}

/// A passive segment holding `funcs`, waiting for a `table.init`.
fn passive(funcs: &[u32]) -> Elem {
    Elem {
        mode: ElemMode::Passive,
        ty: ValType::FuncRef,
        funcs: funcs.to_vec(),
        exprs: Vec::new(),
    }
}

/// A declarative segment: it names `funcs` and puts them nowhere.
fn declarative(funcs: &[u32]) -> Elem {
    Elem {
        mode: ElemMode::Declarative,
        ty: ValType::FuncRef,
        funcs: funcs.to_vec(),
        exprs: Vec::new(),
    }
}

/// `table.get 0 i; ref.is_null`.
fn is_null(i: i32) -> Expr {
    Expr::new().i32_const(i).table_get(0).ref_is_null()
}

/// `i32.const i; call_indirect (type RET) (table 0)`.
fn call_slot(i: i32) -> Expr {
    Expr::new().i32_const(i).call_indirect(RET, 0)
}

/// `table.init seg 0` copying `n` entries from `src` to `dst`.
fn init(seg: u32, dst: i32, src: i32, n: i32) -> Expr {
    Expr::new()
        .i32_const(dst)
        .i32_const(src)
        .i32_const(n)
        .table_init(seg, 0)
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

wasm_test!(active_before_anything, |ctx| {
    // The first thing any of these cases does is call through the table. Nothing in the
    // module has written a slot, so if the call works the segment was applied at
    // instantiation, before a single instruction of the export ran.
    let b = base("elem-active", 6).elem(active_at(0, &[ONE, ONE + 1, THREE]));
    run_cases_with(
        ctx,
        b,
        vec![
            case_i32("slot 0 holds `one`", call_slot(0), 1),
            case_i32("slot 1 holds `two`", call_slot(1), 2),
            case_i32("slot 2 holds `three`", call_slot(2), 3),
            case_i32("slot 3 was not written", is_null(3), 1),
            case_i32("slot 0 was", is_null(0), 0),
        ],
    )
});

wasm_test!(two_active_segments, |ctx| {
    // Two segments, one table, different offsets: the gap between them stays null, which is
    // how you can tell a runtime applied both rather than concatenating them at 0.
    let b = base("elem-two-active", 6)
        .elem(active_at(0, &[ONE]))
        .elem(active_at(4, &[ONE + 1, THREE]));
    run_cases_with(
        ctx,
        b,
        vec![
            case_i32("the first segment landed at 0", call_slot(0), 1),
            case_i32("the second landed at 4", call_slot(4), 2),
            case_i32("and ran on into 5", call_slot(5), 3),
            case_i32("slot 1 is in neither segment", is_null(1), 1),
            case_i32("nor is slot 3", is_null(3), 1),
        ],
    )
});

wasm_test!(active_past_end, |ctx| {
    // Four slots, and a segment asking for slots 3 and 4. The module is well-formed and
    // valid; it is the *instance* that cannot be built, so the runtime traps.
    let mut b = base("elem-past-end", 4).elem(active_at(3, &[ONE, ONE + 1]));
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(ty, Func::new(call_slot(0)));
    let m = b.export_func("f", idx).build();

    let run = ctx.invoke(&m, "f", &[])?;
    let mut c = Check::new("an active element segment that does not fit", &run);
    c.module(&m);
    c.that(
        "exit",
        "a non-zero exit status",
        run.exit.failed(),
        run.exit.label(),
    );
    c.that(
        "stdout",
        "nothing: instantiation failed, so `f` never ran",
        run.stdout.is_empty(),
        run.stdout.clone(),
    );
    c.that(
        "stderr",
        "an out-of-bounds table access reported as a trap, not as a decode or validation \
         error — the range check belongs to instantiation",
        run.stderr_has(trap::TABLE_OUT_OF_BOUNDS) && run.stderr_has(TRAP_MARKER),
        crate::stages::first_meaningful_line(&run.stderr),
    );
    c.note(
        "the same module with the offset one lower is accepted and runs, so nothing about \
         the bytes is malformed; only the arithmetic against the table's size fails",
    );
    c.finish()
});

wasm_test!(active_exactly_at_end, |ctx| {
    // Offset 1 and three entries fills slots 1, 2 and 3 of a four-slot table: the last
    // element lands on the last slot, which is the off-by-one a bounds check gets wrong.
    let b = base("elem-exact-fit", 4).elem(active_at(1, &[ONE, ONE + 1, THREE]));
    run_cases_with(
        ctx,
        b,
        vec![
            case_i32("the segment starts at slot 1", call_slot(1), 1),
            case_i32("and ends on the last slot", call_slot(3), 3),
            case_i32("slot 0 is before the segment and stays null", is_null(0), 1),
        ],
    )
});

wasm_test!(passive_needs_init, |ctx| {
    // Segment 0 is passive. Nothing in the table until an instruction asks for it.
    let b = base("elem-passive", 6).elem(passive(&[ONE, ONE + 1, THREE]));
    run_cases_with(
        ctx,
        b,
        vec![
            case_i32("slot 0 is null before table.init", is_null(0), 1),
            case_i32("so is slot 2", is_null(2), 1),
            case_i32(
                "table.init copies the whole segment to slot 0",
                init(0, 0, 0, 3).then(call_slot(0)),
                1,
            ),
            case_i32(
                "and the third entry to slot 2",
                init(0, 0, 0, 3).then(call_slot(2)),
                3,
            ),
            case_i32(
                "table.init can copy a sub-range to any offset",
                init(0, 4, 1, 2).then(call_slot(4)),
                2,
            ),
            case_i32(
                "and the slots outside the range are untouched",
                init(0, 4, 1, 2).then(is_null(3)),
                1,
            ),
            case_i32(
                "a table.init of length 0 copies nothing and is not an error",
                init(0, 0, 0, 0).then(is_null(0)),
                1,
            ),
            case_trap(
                "a table.init that runs off the end of the table traps",
                init(0, 4, 0, 3).then(Expr::new().i32_const(0)),
                &[trap::TABLE_OUT_OF_BOUNDS],
            ),
        ],
    )
});

wasm_test!(drop_then_init, |ctx| {
    // `elem.drop` does not remove the segment, it empties it: the index stays valid and a
    // later `table.init` of a non-zero length is asking for entries that are no longer there.
    let b = base("elem-drop", 6).elem(passive(&[ONE, ONE + 1, THREE]));
    run_cases_with(
        ctx,
        b,
        vec![
            case_i32(
                "table.init works before the drop",
                init(0, 0, 0, 3).then(call_slot(0)),
                1,
            ),
            case_trap(
                "and traps after it",
                Expr::new().elem_drop(0).then(init(0, 0, 0, 3)).i32_const(0),
                &[trap::TABLE_OUT_OF_BOUNDS],
            ),
            case_trap(
                "dropping twice is allowed, and the second init still traps",
                Expr::new()
                    .elem_drop(0)
                    .elem_drop(0)
                    .then(init(0, 0, 0, 1))
                    .i32_const(0),
                &[trap::TABLE_OUT_OF_BOUNDS],
            ),
            case_i32(
                "a table.init of length 0 from a dropped segment is still fine",
                Expr::new()
                    .elem_drop(0)
                    .then(init(0, 0, 0, 0))
                    .then(is_null(0)),
                1,
            ),
            case_i32(
                "what was copied before the drop stays in the table",
                init(0, 0, 0, 3).elem_drop(0).then(call_slot(2)),
                3,
            ),
        ],
    )
});

wasm_test!(declarative_is_empty, |ctx| {
    // A declarative segment is a promise, not a copy: it says "`ref.func` may name these",
    // and puts nothing anywhere. The module is accepted and the table is untouched.
    let m = declarative_module();
    expect_lines(ctx, &m, "f", &[], &["1", "0", "3"])?;
    Ok(())
});

/// A module whose only element segment is declarative.
///
/// `f` returns three numbers: whether table slot 0 is empty (it is), whether the `funcref`
/// that `ref.func` produced is null (it is not), and the result of calling that function
/// after putting it into the table by hand.
fn declarative_module() -> Module {
    let mut b = base("elem-declarative", 4).elem(declarative(&[THREE]));
    let ty = b.add_type(ftype(&[], &[ValType::I32, ValType::I32, ValType::I32]));
    let idx = b.add_func(
        ty,
        Func::new(
            is_null(0)
                .ref_func(THREE)
                .ref_is_null()
                .i32_const(0)
                .ref_func(THREE)
                .table_set(0)
                .then(call_slot(0)),
        ),
    );
    b.export_func("f", idx).build()
}

/// Worked examples: the three modes side by side, and the segment that does not fit.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("Active at instantiation, passive on demand", || {
            let mut b = base("elem-active-and-passive", 6)
                .elem(active_at(0, &[ONE]))
                .elem(passive(&[ONE + 1, THREE]));
            let ty = b.add_type(ftype(&[], &[ValType::I32, ValType::I32, ValType::I32]));
            let idx = b.add_func(
                ty,
                Func::new(
                    call_slot(0)
                        .then(is_null(4))
                        .then(init(1, 4, 0, 2))
                        .then(call_slot(5)),
                ),
            );
            b.export_func("f", idx).build()
        })
        .summary(
            "one active segment putting `one` at slot 0 and one passive segment holding \
             `two` and `three`; `f` calls slot 0, asks whether slot 4 is empty, runs \
             `table.init 1 0` to copy the passive segment to slot 4, and calls slot 5",
        )
        .command("run --invoke f mod.wasm")
        .output("1\n1\n3")
        .note(
            "The active segment was applied before `f` was callable; the passive one was \
             still sitting in the module, which is why slot 4 answered 1 (null) until \
             `table.init` ran. Segment indices count active, passive and declarative \
             segments alike, so the passive one here is segment 1.",
        ),
        ExampleSpec::module("An active segment that does not fit", || {
            let mut b = base("elem-past-end", 4).elem(active_at(3, &[ONE, ONE + 1]));
            let ty = b.add_type(ftype(&[], &[ValType::I32]));
            let idx = b.add_func(ty, Func::new(call_slot(0)));
            b.export_func("f", idx).build()
        })
        .summary(
            "a four-slot table and an active segment of two entries at offset 3, so the \
             second entry would land on a slot that does not exist",
        )
        .command("run --invoke f mod.wasm")
        .output(
            "nothing on stdout, `wasm trap: out of bounds table access` on stderr, and a \
             non-zero exit",
        )
        .note(
            "This is a trap at instantiation, not a validation error: the module is \
             well-formed and type-correct, and the check that fails is arithmetic against a \
             size the validator does not necessarily know. Refusing it at load time is the \
             usual mistake, and it is visible from outside — the wording says `trap`.",
        ),
    ]
}
