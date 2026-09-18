//! Stage 36 — `call_indirect` and its three traps.
//!
//! `call_indirect` is the only instruction whose callee is not written down in the byte
//! stream: it pops an `i32` index, looks that slot up in a table, and calls whatever
//! `funcref` it finds there. Three things can go wrong, and a runtime has to tell them apart
//! because they are three different mistakes in the program that produced the module:
//!
//! * the index is past the end of the table — `undefined element` / `out of bounds table
//!   access`, and the suite accepts either wording because `wasmtime` writes both in one
//!   sentence and the spec only fixes the first;
//! * the slot is there but empty — `uninitialized element`;
//! * the slot holds a function of another type — `indirect call type mismatch`.
//!
//! The index is an **unsigned** 32-bit number, so `i32.const -1` is not "one before the
//! start", it is index 4294967295, and it is out of range for the same reason 6 is out of
//! range in a six-slot table.
//!
//! The type immediate names a *type index*, but the check it stands for is **structural**:
//! two entries of the type section with the same parameters and results are the same type,
//! and a `funcref` declared with one of them may be called through the other. The last test
//! builds that module by hand, because [`crate::wasm::ModuleBuilder`] deduplicates types and
//! would have collapsed the two entries into one.

use crate::examples::ExampleSpec;
use crate::stages::{case_i32, case_trap, expect, run_cases_with, trap, Case, Stage, Test};
use crate::wasm::{
    const_i32, ftype, op, section, uleb, vector, wasm_name, Elem, ElemMode, Enc, Expr, Func,
    Limits, Module, ModuleBuilder, TableType, ValType,
};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 36,
        slug: "call_indirect",
        name: "call_indirect and its three traps",
        ext: false,
        hints: &[
            "`call_indirect` is 0x11 followed by two indices: the expected type, then the table — the table index is not optional padding, it selects which table to look in",
            "Pop the index as an *unsigned* i32, so -1 means 4294967295 and is out of range, not one before the end",
            "Keep the three failures apart: past the end of the table, a null slot, and a slot whose function has another type are three different traps",
            "Compare the callee's type with the immediate structurally — same parameters, same results — never by comparing type indices",
        ],
        examples,
        tests: vec![
            Test::new(
                "two different functions are reached through the same call_indirect",
                two_callees,
            ),
            Test::new(
                "the index may come from an argument, so the callee is chosen at run time",
                dynamic_index,
            ),
            Test::new(
                "arguments and results pass through call_indirect in the right order",
                arguments_and_results,
            ),
            Test::new("an index past the end of the table traps", index_past_end),
            Test::new(
                "a negative index is out of range, because the index is unsigned",
                negative_index,
            ),
            Test::new("a null table slot traps", null_slot),
            Test::new(
                "a slot whose function has another type traps",
                type_mismatch,
            ),
            Test::new(
                "two identical function types are interchangeable",
                structural_typing,
            ),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// The module every sweep starts from
// ---------------------------------------------------------------------------------------

/// The shared table layout, and the type indices a body needs to name.
///
/// Table 0 is six slots of `funcref`, filled by two active element segments:
///
/// ```text
/// 0: ten    () -> i32          3: (null)
/// 1: twenty () -> i32          4: mixed  (i32 i64) -> i32
/// 2: sub    (i32 i32) -> i32   5: (null)
/// ```
///
/// Slot 4 is the subtle mismatch: same arity as `sub`, one parameter type changed.
struct Base {
    /// The builder, with the callees, the table and the element segments already in it.
    b: ModuleBuilder,
    /// Type index of `() -> i32`.
    ret: u32,
    /// Type index of `(i32 i32) -> i32`.
    sub: u32,
}

/// Build [`Base`] under a given label.
fn base(label: &str) -> Base {
    let mut b = ModuleBuilder::new(label);
    let ret = b.add_type(ftype(&[], &[ValType::I32]));
    let sub = b.add_type(ftype(&[ValType::I32, ValType::I32], &[ValType::I32]));
    let mixed = b.add_type(ftype(&[ValType::I32, ValType::I64], &[ValType::I32]));

    let f_ten = b.add_func(ret, Func::new(Expr::new().i32_const(10)));
    let f_twenty = b.add_func(ret, Func::new(Expr::new().i32_const(20)));
    let f_sub = b.add_func(
        sub,
        Func::new(Expr::new().local_get(0).local_get(1).op(op::I32_SUB)),
    );
    let f_mixed = b.add_func(mixed, Func::new(Expr::new().local_get(0)));

    let b = b
        .table(TableType {
            elem: ValType::FuncRef,
            limits: Limits::min(6),
        })
        .elem_active(0, &[f_ten, f_twenty, f_sub])
        .elem(Elem {
            mode: ElemMode::Active {
                table: 0,
                offset: const_i32(4),
            },
            ty: ValType::FuncRef,
            funcs: vec![f_mixed],
            exprs: Vec::new(),
        });
    Base { b, ret, sub }
}

/// `i32.const index; call_indirect (type ty) (table 0)` with no arguments.
fn call_slot(ty: u32, index: i32) -> Expr {
    Expr::new().i32_const(index).call_indirect(ty, 0)
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

wasm_test!(two_callees, |ctx| {
    let base = base("call-indirect-two-callees");
    let (ret, b) = (base.ret, base.b);
    run_cases_with(
        ctx,
        b,
        vec![
            case_i32("slot 0 is `ten`", call_slot(ret, 0), 10),
            case_i32("slot 1 is `twenty`", call_slot(ret, 1), 20),
            case_i32(
                "one call_indirect per slot, in one body",
                call_slot(ret, 0).then(call_slot(ret, 1)).op(op::I32_ADD),
                30,
            ),
        ],
    )
});

wasm_test!(dynamic_index, |ctx| {
    // The index is an argument, so nothing in the module says which function is called: a
    // runtime that resolves call_indirect at validation time gets the same answer twice.
    let base = base("call-indirect-dynamic");
    let (ret, b) = (base.ret, base.b);
    let through_arg = || Expr::new().local_get(0).call_indirect(ret, 0);
    run_cases_with(
        ctx,
        b,
        vec![
            Case::new(
                "argument 0 selects `ten`",
                ftype(&[ValType::I32], &[ValType::I32]),
                through_arg(),
                &["10"],
            )
            .args(&["0"]),
            Case::new(
                "argument 1 selects `twenty`",
                ftype(&[ValType::I32], &[ValType::I32]),
                through_arg(),
                &["20"],
            )
            .args(&["1"]),
            Case::new(
                "the same body, called twice, follows the argument both times",
                ftype(&[ValType::I32], &[ValType::I32]),
                Expr::new()
                    .local_get(0)
                    .call_indirect(ret, 0)
                    .local_get(0)
                    .call_indirect(ret, 0)
                    .op(op::I32_ADD),
                &["40"],
            )
            .args(&["1"]),
        ],
    )
});

wasm_test!(arguments_and_results, |ctx| {
    // `sub`, not `add`, so the operand order is visible in the answer: 30 - 4 is 26 and
    // 4 - 30 is -26, and a runtime that pushes the arguments backwards prints the wrong one.
    let base = base("call-indirect-arguments");
    let (sub, b) = (base.sub, base.b);
    let call_sub = |a: i32, c: i32| {
        Expr::new()
            .i32_const(a)
            .i32_const(c)
            .i32_const(2)
            .call_indirect(sub, 0)
    };
    run_cases_with(
        ctx,
        b,
        vec![
            case_i32("30 - 4 through slot 2", call_sub(30, 4), 26),
            case_i32("4 - 30 through slot 2", call_sub(4, 30), -26),
            case_i32(
                "the result comes back on the stack like any other call",
                call_sub(30, 4).i32_const(1).op(op::I32_ADD),
                27,
            ),
            Case::new(
                "both arguments may come from the caller's locals",
                ftype(&[ValType::I32, ValType::I32], &[ValType::I32]),
                Expr::new()
                    .local_get(0)
                    .local_get(1)
                    .i32_const(2)
                    .call_indirect(sub, 0),
                &["-1"],
            )
            .args(&["6", "7"]),
        ],
    )
});

wasm_test!(index_past_end, |ctx| {
    let base = base("call-indirect-past-end");
    let (ret, b) = (base.ret, base.b);
    run_cases_with(
        ctx,
        b,
        vec![
            case_trap(
                "index 6 in a six-slot table",
                call_slot(ret, 6),
                trap::TABLE_INDEX_OUT_OF_RANGE,
            ),
            case_trap(
                "index 1000",
                call_slot(ret, 1000),
                trap::TABLE_INDEX_OUT_OF_RANGE,
            ),
        ],
    )
});

wasm_test!(negative_index, |ctx| {
    // -1 is 4294967295 once it is read as an unsigned index, which is past the end of every
    // table that has ever been declared.
    let base = base("call-indirect-negative");
    let (ret, b) = (base.ret, base.b);
    run_cases_with(
        ctx,
        b,
        vec![
            case_trap(
                "index -1 is index 4294967295",
                call_slot(ret, -1),
                trap::TABLE_INDEX_OUT_OF_RANGE,
            ),
            case_trap(
                "index i32::MIN is index 2147483648",
                call_slot(ret, i32::MIN),
                trap::TABLE_INDEX_OUT_OF_RANGE,
            ),
        ],
    )
});

wasm_test!(null_slot, |ctx| {
    // Slots 3 and 5 are inside the table and were never written: the table exists, the index
    // is in range, and there is still nothing to call.
    let base = base("call-indirect-null-slot");
    let (ret, b) = (base.ret, base.b);
    run_cases_with(
        ctx,
        b,
        vec![
            case_trap(
                "slot 3, between two active element segments",
                call_slot(ret, 3),
                &[trap::UNINITIALIZED_ELEMENT],
            ),
            case_trap(
                "slot 5, past the last element segment but inside the table",
                call_slot(ret, 5),
                &[trap::UNINITIALIZED_ELEMENT],
            ),
        ],
    )
});

wasm_test!(type_mismatch, |ctx| {
    // Slot 4 holds `(i32 i64) -> i32`. Calling it as `(i32 i32) -> i32` has the right arity
    // and the right result: only the second parameter differs, which is exactly the mistake
    // a runtime that compares arities instead of types will let through.
    let base = base("call-indirect-type-mismatch");
    let (sub, b) = (base.sub, base.b);
    run_cases_with(
        ctx,
        b,
        vec![case_trap(
            "slot 4 called as (i32 i32) -> i32",
            Expr::new()
                .i32_const(1)
                .i32_const(2)
                .i32_const(4)
                .call_indirect(sub, 0),
            &[trap::INDIRECT_TYPE_MISMATCH],
        )],
    )
});

wasm_test!(structural_typing, |ctx| {
    let m = twin_types();
    expect(ctx, &m, "70")?;
    Ok(())
});

// ---------------------------------------------------------------------------------------
// The hand-built module: two identical entries in the type section
// ---------------------------------------------------------------------------------------

/// One code-section entry: no local declarations, then the body.
fn code_entry(e: Expr) -> Vec<u8> {
    let mut inner = vec![0x00];
    inner.extend_from_slice(&e.into_body());
    let mut out = uleb(inner.len() as u64);
    out.extend_from_slice(&inner);
    out
}

/// A module whose type section holds `(i32) -> i32` **twice**.
///
/// The callee is declared with type 0 and the `call_indirect` names type 1. They are
/// different indices and the same type, so the call must go through: type identity in
/// WebAssembly is structural, and a runtime that caches "this slot has type index 0" and
/// compares indices refuses a module the spec accepts.
fn twin_types() -> Module {
    // (i32) -> i32, written out twice, and () -> i32 for the exported caller.
    let twin = vec![0x60, 0x01, 0x7f, 0x01, 0x7f];
    let ret = vec![0x60, 0x00, 0x01, 0x7f];

    let mut e = Enc::with_header();

    let at = e.at();
    let body = vector(&[twin.clone(), twin.clone(), ret]);
    e.section(section::TYPE, &body);
    let start = at + 1 + uleb(body.len() as u64).len() + 1;
    e.annotate(start, twin.len(), "type[0]", "(i32) -> (i32)");
    e.annotate(
        start + twin.len(),
        twin.len(),
        "type[1]",
        "(i32) -> (i32) — byte for byte the same as type[0]",
    );
    e.annotate(start + 2 * twin.len(), 4, "type[2]", "() -> (i32)");

    e.section(section::FUNCTION, &vector(&[uleb(0), uleb(2)]));
    e.annotate(e.at() - 2, 1, "function[0]", "type 0, the callee");
    e.annotate(e.at() - 1, 1, "function[1]", "type 2, the caller");

    e.section(section::TABLE, &vector(&[vec![0x70, 0x00, 0x01]]));
    e.annotate(e.at() - 3, 3, "table[0]", "funcref, min 1, no max");

    let mut export = wasm_name("f");
    export.push(0x00);
    export.extend_from_slice(&uleb(1));
    let len = export.len();
    e.section(section::EXPORT, &vector(&[export]));
    e.annotate(e.at() - len, len, "export[0]", "'f' → func 1");

    let mut elem = vec![0x00];
    elem.extend_from_slice(&const_i32(0));
    elem.extend_from_slice(&vector(&[uleb(0)]));
    let len = elem.len();
    e.section(section::ELEMENT, &vector(&[elem]));
    e.annotate(
        e.at() - len,
        len,
        "elem[0]",
        "active into table 0 at offset 0, funcs [0]",
    );

    let callee = Expr::new().local_get(0).i32_const(10).op(op::I32_MUL);
    let caller = Expr::new().i32_const(7).i32_const(0).call_indirect(1, 0);
    let callee_text = callee.text();
    let caller_text = caller.text();
    let entries = [code_entry(callee), code_entry(caller)];
    let lens = (entries[0].len(), entries[1].len());
    e.section(section::CODE, &vector(&entries));
    e.annotate(e.at() - lens.0 - lens.1, lens.0, "code[0]", callee_text);
    e.annotate(e.at() - lens.1, lens.1, "code[1]", caller_text);

    e.finish("call-indirect-twin-types")
}

/// Worked examples: the dispatch table, and the type immediate that is not an identity.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("A two-entry dispatch table", || {
            let base = base("call-indirect-two-callees");
            let ret = base.ret;
            let mut b = base.b;
            let ty = b.add_type(ftype(&[], &[ValType::I32]));
            let idx = b.add_func(
                ty,
                Func::new(
                    Expr::new()
                        .i32_const(0)
                        .call_indirect(ret, 0)
                        .i32_const(1)
                        .call_indirect(ret, 0)
                        .op(op::I32_ADD),
                ),
            );
            b.export_func("f", idx).build()
        })
        .summary(
            "a six-slot `funcref` table filled by two active element segments, and an `f` \
             that calls slot 0 and slot 1 through the same `call_indirect` and adds the \
             results",
        )
        .command("run --invoke f mod.wasm")
        .output("30")
        .note(
            "Slots 3 and 5 are never written, so calling them traps with `uninitialized \
             element`; slot 6 does not exist, so it traps with `undefined element` / `out of \
             bounds table access`; slot 4 holds `(i32 i64) -> i32`, so calling it as \
             `(i32 i32) -> i32` traps with `indirect call type mismatch`.",
        ),
        ExampleSpec::module("The same type, written twice", twin_types)
            .summary(
                "a type section holding `(i32) -> i32` twice; the callee is declared with \
                 type 0 and reached by a `call_indirect` whose immediate is type 1",
            )
            .command("run --invoke f mod.wasm")
            .output("70")
            .note(
                "The type immediate is an index into the type section, but the check is \
                 structural: compare parameters and results, never indices. Canonicalising \
                 the type section into a table of distinct types, and comparing the \
                 canonical ids, is the usual way to get this both right and fast.",
            ),
    ]
}
