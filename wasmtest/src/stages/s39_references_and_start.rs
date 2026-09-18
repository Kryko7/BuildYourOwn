//! Stage 39 — References and the start section. **[ext]**
//!
//! Two small features that are easy to half-implement.
//!
//! **References.** There are two reference types — `funcref` and `externref` — and three
//! instructions: `ref.null t` makes a null one, `ref.is_null` asks whether a reference is
//! null, and `ref.func n` makes a `funcref` for a function of the module. Only `ref.func`
//! has a rule worth remembering, and it is the surprising one: **the function must be
//! declared.** A function index counts as declared when it appears somewhere outside a
//! function body — in an export, in the start section, in a global's initialiser, or in an
//! element segment, including a *declarative* segment that exists for no other purpose. A
//! function that is only ever named by `ref.func` inside a body is not declared, and the
//! module fails validation, however obviously the index is in range. The rule exists so a
//! runtime knows, before it compiles anything, which functions may have their address taken.
//!
//! **The start section.** One function index, no arguments, no results, run at the end of
//! instantiation — after the globals are initialised and after every active data and element
//! segment has been applied, and before the first export can be called. So a start function
//! may write a global and read the memory and tables the segments filled, and if it traps,
//! instantiation fails: there is no instance, and no export is callable at all. That last
//! part is what separates the start section from "a function the host happens to call
//! first".

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::stages::{
    case_i32, expect, expect_rejected, expect_trap, run_cases_with, trap, Stage, Test,
};
use crate::wasm::{
    ftype, global_i32, op, Elem, ElemMode, Expr, Func, Limits, Module, ModuleBuilder, TableType,
    ValType,
};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 39,
        slug: "references_and_start",
        name: "References and the start section",
        ext: true,
        hints: &[
            "`ref.null t` (0xd0) carries the reference type as an immediate, `ref.is_null` (0xd1) takes any reference, and `ref.func n` (0xd2) names a function",
            "Collect the set of function indices that appear anywhere outside a function body — exports, the start section, global initialisers, every element segment — and refuse a `ref.func` naming anything else",
            "Run the start function at the end of instantiation: after the globals, after every active segment, and before the first export is callable",
            "A start function that traps aborts instantiation — there is no instance afterwards, so no export can be called and nothing reaches stdout",
        ],
        examples,
        tests: vec![
            Test::new(
                "ref.null funcref and ref.null externref are both null",
                null_references,
            )
            .ext(),
            Test::new(
                "ref.is_null tells a null apart from a ref.func",
                is_null_discriminates,
            )
            .ext(),
            Test::new(
                "a ref.func put into a table survives the round trip and is callable",
                ref_func_round_trip,
            )
            .ext(),
            Test::new(
                "ref.func may only name a function the module declared",
                ref_func_must_be_declared,
            )
            .ext(),
            Test::new(
                "the start function has already run when an export is called",
                start_runs_first,
            )
            .ext(),
            Test::new(
                "a start function that traps stops the module before any export runs",
                start_traps,
            )
            .ext(),
            Test::new(
                "the segments are in place before the start function runs",
                start_sees_segments,
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

/// `one`, `two`, `three`, a six-slot `funcref` table holding them, and a two-slot
/// `externref` table holding nothing.
///
/// The active element segment is also what makes `ref.func` legal for these three: see
/// [`ref_func_must_be_declared`] for what happens without it.
fn base(label: &str) -> ModuleBuilder {
    let mut b = ModuleBuilder::new(label);
    let ret = b.add_type(ftype(&[], &[ValType::I32]));
    debug_assert_eq!(ret, RET);
    b.add_func(ret, Func::new(Expr::new().i32_const(1)));
    b.add_func(ret, Func::new(Expr::new().i32_const(2)));
    b.add_func(ret, Func::new(Expr::new().i32_const(3)));
    b.table(TableType {
        elem: ValType::FuncRef,
        limits: Limits::min(6),
    })
    .table(TableType {
        elem: ValType::ExternRef,
        limits: Limits::min(2),
    })
    .elem_active(0, &[ONE, ONE + 1, THREE])
}

/// `i32.const i; call_indirect (type RET) (table 0)`.
fn call_slot(i: i32) -> Expr {
    Expr::new().i32_const(i).call_indirect(RET, 0)
}

/// `table.get t i; ref.is_null`.
fn slot_is_null(table: u32, i: i32) -> Expr {
    Expr::new().i32_const(i).table_get(table).ref_is_null()
}

// ---------------------------------------------------------------------------------------
// References
// ---------------------------------------------------------------------------------------

wasm_test!(null_references, |ctx| {
    run_cases_with(
        ctx,
        base("ref-null"),
        vec![
            case_i32(
                "a null funcref is null",
                Expr::new().ref_null(ValType::FuncRef).ref_is_null(),
                1,
            ),
            case_i32(
                "a null externref is null too",
                Expr::new().ref_null(ValType::ExternRef).ref_is_null(),
                1,
            ),
            case_i32(
                "an externref table starts out full of nulls",
                slot_is_null(1, 0),
                1,
            ),
            case_i32(
                "and a null externref written into it reads back null",
                Expr::new()
                    .i32_const(1)
                    .ref_null(ValType::ExternRef)
                    .table_set(1)
                    .then(slot_is_null(1, 1)),
                1,
            ),
            case_i32(
                "the two reference types are separate: table 0 is untouched by table 1",
                Expr::new()
                    .i32_const(0)
                    .ref_null(ValType::ExternRef)
                    .table_set(1)
                    .then(call_slot(0)),
                1,
            ),
        ],
    )
});

wasm_test!(is_null_discriminates, |ctx| {
    run_cases_with(
        ctx,
        base("ref-is-null"),
        vec![
            case_i32(
                "ref.func is not null",
                Expr::new().ref_func(THREE).ref_is_null(),
                0,
            ),
            case_i32(
                "ref.null funcref is",
                Expr::new().ref_null(ValType::FuncRef).ref_is_null(),
                1,
            ),
            case_i32(
                "a reference read out of a filled table slot is not null",
                slot_is_null(0, 2),
                0,
            ),
            case_i32("one read out of an empty slot is", slot_is_null(0, 5), 1),
            case_i32(
                "ref.is_null consumes the reference and leaves an i32 behind",
                Expr::new()
                    .ref_func(ONE)
                    .ref_is_null()
                    .ref_null(ValType::FuncRef)
                    .ref_is_null()
                    .op(op::I32_ADD),
                1,
            ),
        ],
    )
});

wasm_test!(ref_func_round_trip, |ctx| {
    // A funcref goes into a slot, comes back out into another slot, and is still the same
    // function: a reference is a value, not a name that has to be resolved again.
    run_cases_with(
        ctx,
        base("ref-func-round-trip"),
        vec![
            case_i32(
                "ref.func written into an empty slot is callable through it",
                Expr::new()
                    .i32_const(4)
                    .ref_func(THREE)
                    .table_set(0)
                    .then(call_slot(4)),
                3,
            ),
            case_i32(
                "and out again into a second slot",
                Expr::new()
                    .i32_const(4)
                    .ref_func(THREE)
                    .table_set(0)
                    .i32_const(5)
                    .i32_const(4)
                    .table_get(0)
                    .table_set(0)
                    .then(call_slot(5)),
                3,
            ),
            case_i32(
                "the slot it came from still holds it",
                Expr::new()
                    .i32_const(4)
                    .ref_func(THREE)
                    .table_set(0)
                    .i32_const(5)
                    .i32_const(4)
                    .table_get(0)
                    .table_set(0)
                    .then(call_slot(4)),
                3,
            ),
            case_i32(
                "a reference taken from a slot the element segment filled works the same way",
                Expr::new()
                    .i32_const(5)
                    .i32_const(1)
                    .table_get(0)
                    .table_set(0)
                    .then(call_slot(5)),
                2,
            ),
        ],
    )
});

wasm_test!(ref_func_must_be_declared, |ctx| {
    // The same two functions twice. The only difference is a declarative element segment
    // naming the hidden one — a segment that puts nothing anywhere and exists purely to say
    // "this function may have its address taken".
    expect_rejected(
        ctx,
        &ref_func_module(false),
        "function 0 is named by `ref.func` inside a body and nowhere else, so it is not \
         declared and the module must not validate",
    )?;
    expect(ctx, &ref_func_module(true), "0")?;
    Ok(())
});

/// Two functions: a hidden one, and an exported `f` that takes its address.
///
/// With `declare`, a declarative element segment names the hidden function and the module is
/// valid; without it, the same bytes minus that segment are not.
fn ref_func_module(declare: bool) -> Module {
    let label = if declare {
        "ref-func-declared"
    } else {
        "ref-func-undeclared"
    };
    let mut b = ModuleBuilder::new(label);
    let ret = b.add_type(ftype(&[], &[ValType::I32]));
    let hidden = b.add_func(ret, Func::new(Expr::new().i32_const(9)));
    let f = b.add_func(ret, Func::new(Expr::new().ref_func(hidden).ref_is_null()));
    let mut b = b.export_func("f", f);
    if declare {
        b = b.elem(Elem {
            mode: ElemMode::Declarative,
            ty: ValType::FuncRef,
            funcs: vec![hidden],
            exprs: Vec::new(),
        });
    }
    b.build()
}

// ---------------------------------------------------------------------------------------
// The start section
// ---------------------------------------------------------------------------------------

/// A mutable global initialised to 0, a start function that writes `v` into it, and an `f`
/// that reads it. With `with_start` false the start section is left out, so `f` sees 0.
fn start_writes_global(with_start: bool, v: i32) -> Module {
    let label = if with_start {
        "start-writes-global"
    } else {
        "start-absent"
    };
    let mut b = ModuleBuilder::new(label).global(global_i32(0, true));
    let ret = b.add_type(ftype(&[], &[ValType::I32]));
    let void = b.add_type(ftype(&[], &[]));
    let s = b.add_func(void, Func::new(Expr::new().i32_const(v).global_set(0)));
    let f = b.add_func(ret, Func::new(Expr::new().global_get(0)));
    let b = b.export_func("f", f);
    if with_start {
        b.start(s).build()
    } else {
        b.build()
    }
}

/// A start function that writes a global and then executes `unreachable`.
fn start_that_traps() -> Module {
    let mut b = ModuleBuilder::new("start-traps").global(global_i32(0, true));
    let ret = b.add_type(ftype(&[], &[ValType::I32]));
    let void = b.add_type(ftype(&[], &[]));
    let s = b.add_func(
        void,
        Func::new(Expr::new().i32_const(7).global_set(0).unreachable()),
    );
    let f = b.add_func(ret, Func::new(Expr::new().global_get(0)));
    b.export_func("f", f).start(s).build()
}

/// A start function that reads what the active data and element segments put in place.
///
/// It calls table slot 2 — `three`, put there by an active element segment — multiplies the
/// answer by 100, adds the byte an active data segment wrote at address 0, and stores the
/// total in a global that `f` returns.
fn start_reads_segments() -> Module {
    let mut b = ModuleBuilder::new("start-after-segments").global(global_i32(0, true));
    let ret = b.add_type(ftype(&[], &[ValType::I32]));
    let void = b.add_type(ftype(&[], &[]));
    let one = b.add_func(ret, Func::new(Expr::new().i32_const(1)));
    let two = b.add_func(ret, Func::new(Expr::new().i32_const(2)));
    let three = b.add_func(ret, Func::new(Expr::new().i32_const(3)));
    let s = b.add_func(
        void,
        Func::new(
            call_slot(2)
                .i32_const(100)
                .op(op::I32_MUL)
                .i32_const(0)
                .mem(op::I32_LOAD8_U, 0, 0)
                .op(op::I32_ADD)
                .global_set(0),
        ),
    );
    let f = b.add_func(ret, Func::new(Expr::new().global_get(0)));
    b.table(TableType {
        elem: ValType::FuncRef,
        limits: Limits::min(4),
    })
    .memory(Limits::min(1))
    .elem_active(0, &[one, two, three])
    .data_active(0, &[9])
    .export_func("f", f)
    .start(s)
    .build()
}

wasm_test!(start_runs_first, |ctx| {
    // Two modules that differ only in the start section. The exported function is the same
    // `global.get 0` in both, so the 7 can only have come from the start function.
    expect(ctx, &start_writes_global(true, 7), "7")?;
    expect(ctx, &start_writes_global(false, 7), "0")?;
    Ok(())
});

wasm_test!(start_traps, |ctx| {
    // The start function writes the global and then traps. If instantiation carried on
    // regardless, `f` would print 7; if it aborted but the trap were swallowed, `f` would
    // print something. Neither happens: there is no instance, so there is no `f` to call.
    let m = start_that_traps();
    let run = expect_trap(ctx, &m, "f", &[], trap::UNREACHABLE)?;
    let mut c = Check::new("what a trapping start function leaves behind", &run);
    c.module(&m);
    c.that(
        "stdout",
        "nothing at all — instantiation failed, so the export was never callable",
        run.stdout.is_empty(),
        run.stdout.clone(),
    );
    c.finish()
});

wasm_test!(start_sees_segments, |ctx| {
    // 3 from the element segment, 9 from the data segment: both were applied before the
    // start function ran, which is the order instantiation fixes.
    expect(ctx, &start_reads_segments(), "309")?;
    Ok(())
});

/// Worked examples: the declaration rule, and the start function that reads the segments.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("A ref.func nobody declared", || ref_func_module(false))
            .summary(
                "two functions — a hidden one that returns 9 and an exported `f` whose whole \
                 body is `ref.func 0; ref.is_null` — and no element segment, no second \
                 export, nothing else naming function 0",
            )
            .command("run --invoke f mod.wasm")
            .output(
                "nothing on stdout, a validation error on stderr, and exit 1 — adding a \
                 declarative element segment naming function 0 makes the same module print 0",
            )
            .note(
                "`ref.func` may only name a function that appears somewhere outside a \
                 function body: an export, the start section, a global initialiser or an \
                 element segment. Gather that set while decoding, before you validate any \
                 body — this is the rule a from-scratch validator forgets.",
            ),
        ExampleSpec::module(
            "A start function reading its segments",
            start_reads_segments,
        )
        .summary(
            "an active element segment putting `three` in table slot 2, an active data \
                 segment putting the byte 9 at address 0, and a start function that calls \
                 slot 2, multiplies by 100, adds the byte, and stores the total in a mutable \
                 global that `f` returns",
        )
        .command("run --invoke f mod.wasm")
        .output("309")
        .note(
            "Instantiation has an order: globals, then active data and element segments, \
                 then the start function, and only then is an export callable. Change the \
                 start function to `unreachable` and nothing reaches stdout at all — a \
                 trapping start aborts instantiation, so there is no instance to call into.",
        ),
    ]
}
