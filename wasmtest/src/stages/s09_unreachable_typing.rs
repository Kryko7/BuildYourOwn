//! Stage 09 — Unreachable code and the polymorphic stack. **[ext]**
//!
//! `unreachable`, `br`, `br_table` and `return` all end the current block: everything after
//! them, up to that block's `end`, can never run. The spec does not let a runtime skip it —
//! it is still decoded, and still type-checked — but it is checked against a **polymorphic**
//! stack, one that will hand over a value of any type the next instruction asks for. So
//! `unreachable` followed by `i32.add` validates with nothing pushed at all, and that is the
//! thing implementations get wrong by being too strict.
//!
//! One expectation in this stage was written the other way round at first and is wrong, so
//! it is worth spelling out. The polymorphic stack supplies values, it does not *absorb*
//! them: `unreachable; i32.const 1` in a `() -> i64` function is a validation **error**,
//! because the i32 that instruction pushes is a real, concrete type sitting where the i64
//! result must be. `wasmtime` answers `type mismatch: expected i64, found i32`, and it is
//! right. The same trap catches `br 0` in a `block (result i32)` followed by `f64.neg`: the
//! f64 is concrete and the block's `end` wants an i32. What validates is junk whose *result*
//! still fits — `i32.add`, or an `f64.neg` that is dropped again — with the operands coming
//! out of the polymorphic part. Every accepted case here is written that way.
//!
//! The other half of the stage is the limits. Unreachable code is still bytes that have to
//! decode, so an opcode no version of the format defines and an index that names nothing are
//! both rejections even though nothing could ever execute them; and the polymorphic stack
//! belongs to the block it was created in, so it does not survive the enclosing `end`.

use crate::examples::ExampleSpec;
use crate::stages::{expect, expect_rejected, expect_trap, single, trap, Stage, Test};
use crate::wasm::{ftype, op, BlockType, Expr, Func, Module, ModuleBuilder, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 9,
        slug: "unreachable_typing",
        name: "Unreachable code and the polymorphic stack",
        ext: true,
        hints: &[
            "Give each block frame an `unreachable` flag; `unreachable`, `br`, `br_table` and `return` set it, and the block's `end` clears it",
            "While the flag is set, popping an operand from an empty frame succeeds and yields whatever type was asked for — that is what makes `unreachable; i32.add` valid",
            "It supplies types, it does not swallow them: a concrete value pushed inside unreachable code still has to match the block's result type at the `end`",
            "Decode unreachable code anyway: an undefined opcode or an index that names nothing is a rejection even when the instruction can never run",
        ],
        examples,
        tests: vec![
            Test::new("unreachable gives the rest of the block a polymorphic stack", after_unreachable).ext(),
            Test::new("br in a typed block makes the rest of that block unreachable", after_br).ext(),
            Test::new("code after return and after br_table is unreachable too", after_return_and_br_table).ext(),
            Test::new("a module that reaches the unreachable instruction still traps", still_traps).ext(),
            Test::new("an undefined opcode inside unreachable code is still refused", undefined_opcode).ext(),
            Test::new("an out-of-range index inside unreachable code is still refused", bad_index).ext(),
            Test::new("the polymorphic stack does not survive the enclosing end", ends_at_end).ext(),
        ],
    }
}

wasm_test!(after_unreachable, |ctx| {
    // Four bodies that a strict-but-wrong validator refuses. Each one is accepted and then
    // traps, which is the only way to prove from outside that it validated: the runtime had
    // to reach the `unreachable` to trap on it.
    for (label, sig, body, why) in [
        (
            "poly-add",
            ftype(&[], &[ValType::I32]),
            Expr::new().unreachable().op(op::I32_ADD),
            "i32.add with an empty stack, after unreachable",
        ),
        (
            "poly-drop",
            ftype(&[], &[]),
            Expr::new().unreachable().drop().drop().drop(),
            "three drops with nothing to drop",
        ),
        (
            "poly-select",
            ftype(&[], &[ValType::I32]),
            Expr::new().unreachable().select(),
            "select, whose three operands all come out of the polymorphic stack",
        ),
        (
            "poly-wrong-type-dropped",
            ftype(&[], &[ValType::I64]),
            Expr::new().unreachable().i32_const(1).drop(),
            "an i32 pushed in a () -> i64 function and dropped again, leaving the \
             polymorphic stack to supply the i64",
        ),
    ] {
        let m = single(label, sig, body);
        expect_trap(ctx, &m, "f", &[], trap::UNREACHABLE)?;
        ctx.note(format!("accepted and trapped: {why}"));
    }
    Ok(())
});

wasm_test!(after_br, |ctx| {
    // `br 0` takes the i32 the block promises and jumps to its end; the `i32.add` behind it
    // can never run, and validates against the polymorphic stack with nothing pushed.
    let m = br_then_junk();
    expect(ctx, &m, "7")?;

    // The same, with junk whose result type is wrong — but dropped, so the block still ends
    // with the i32 it promised. (Leaving the f64 there is an error; see the module note.)
    let dropped = single(
        "br-then-f64-dropped",
        ftype(&[], &[ValType::I32]),
        Expr::new().block(
            BlockType::Value(ValType::I32),
            Expr::new()
                .i32_const(7)
                .br(0)
                .op(op::F64_NEG)
                .drop()
                .i32_const(0),
        ),
    );
    expect(ctx, &dropped, "7")?;
    Ok(())
});

wasm_test!(after_return_and_br_table, |ctx| {
    let after_return = single(
        "return-then-junk",
        ftype(&[], &[ValType::I32]),
        Expr::new().i32_const(7).return_().op(op::I32_ADD),
    );
    expect(ctx, &after_return, "7")?;

    let after_br_table = single(
        "br-table-then-junk",
        ftype(&[], &[ValType::I32]),
        Expr::new().block(
            BlockType::Value(ValType::I32),
            Expr::new()
                .i32_const(7)
                .i32_const(0)
                .br_table(&[0], 0)
                .op(op::I32_ADD),
        ),
    );
    expect(ctx, &after_br_table, "7")?;
    Ok(())
});

wasm_test!(still_traps, |ctx| {
    // Validating unreachable code leniently must not turn `unreachable` itself into a nop.
    let m = single(
        "reaches-unreachable",
        ftype(&[], &[ValType::I32]),
        Expr::new().i32_const(1).drop().unreachable(),
    );
    expect_trap(ctx, &m, "f", &[], trap::UNREACHABLE)?;
    Ok(())
});

wasm_test!(undefined_opcode, |ctx| {
    let m = undefined_opcode_module();
    expect_rejected(
        ctx,
        &m,
        "0x27 is not an opcode in any version of the format; unreachable code is still \
         decoded, so a byte that decodes to nothing is a rejection",
    )?;
    Ok(())
});

wasm_test!(bad_index, |ctx| {
    let local = single(
        "unreachable-bad-local",
        ftype(&[], &[ValType::I32]),
        Expr::new().unreachable().local_get(99),
    );
    expect_rejected(
        ctx,
        &local,
        "local 99 of a function with no parameters and no locals, behind an unreachable: \
         the polymorphic stack makes the *types* unconstrained, never the indices",
    )?;
    let call = single(
        "unreachable-bad-call",
        ftype(&[], &[ValType::I32]),
        Expr::new().unreachable().call(99),
    );
    expect_rejected(ctx, &call, "call 99 in a module with one function")?;
    Ok(())
});

wasm_test!(ends_at_end, |ctx| {
    let m = polymorphic_ends();
    expect_rejected(
        ctx,
        &m,
        "the block's `end` closes the frame the `unreachable` marked, so the `i32.add` after \
         it is ordinary reachable code with an empty stack under it",
    )?;
    Ok(())
});

// ---------------------------------------------------------------------------------------
// Modules
// ---------------------------------------------------------------------------------------

/// `block (result i32) { i32.const 7; br 0; i32.add } ` — the junk never runs, and `i32.add`
/// takes both of its operands from the polymorphic stack and pushes the i32 the block wants.
fn br_then_junk() -> Module {
    single(
        "br-then-junk",
        ftype(&[], &[ValType::I32]),
        Expr::new().block(
            BlockType::Value(ValType::I32),
            Expr::new().i32_const(7).br(0).op(op::I32_ADD),
        ),
    )
}

/// `unreachable` followed by the byte `0x27`, which no version of the format defines.
///
/// Written with [`Func::raw`] because [`Expr`] has no way to say "an opcode that does not
/// exist". `0x06` would have been the obvious choice — it is the gap between `else` and
/// `end` — but `wasmtime` answers `legacy_exceptions feature required for try instruction`
/// there, so the test would be about a disabled proposal rather than about an undefined
/// byte. `0x27`, between `table.set` and `i32.load`, is unassigned outright.
fn undefined_opcode_module() -> Module {
    let mut b = ModuleBuilder::new("unreachable-undefined-opcode");
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(
        ty,
        Func::raw(
            &[0x00, 0x27, 0x0b],
            "unreachable, 0x27 (no such opcode), end",
        ),
    );
    b.export_func("f", idx).build()
}

/// `block { unreachable } i32.add` in a `() -> i32`: valid inside the block, invalid after it.
fn polymorphic_ends() -> Module {
    single(
        "polymorphic-ends-at-end",
        ftype(&[], &[ValType::I32]),
        Expr::new()
            .block(BlockType::Empty, Expr::new().unreachable())
            .op(op::I32_ADD),
    )
}

/// Worked examples: the lenient side, and where the leniency stops.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("An i32.add that never runs", br_then_junk)
            .summary(
                "a `() -> i32` whose body is `block (result i32) { i32.const 7; br 0; \
                 i32.add }` — the `br` jumps to the block's end and the `i32.add` behind it \
                 is unreachable",
            )
            .command("run --invoke f mod.wasm")
            .output("7")
            .note(
                "The `i32.add` still has to type-check, but against a polymorphic stack that \
                 supplies both operands out of thin air. A validator that insists on two real \
                 i32s here refuses a module every compiler emits.",
            ),
        ExampleSpec::module("Where the polymorphic stack stops", polymorphic_ends)
            .summary(
                "the same idea one instruction further on: `block { unreachable } i32.add` \
                 in a `() -> i32`, with nothing on the stack",
            )
            .command("run --invoke f mod.wasm")
            .output(
                "nothing on stdout, a message on stderr, and a non-zero exit status — the \
                 module never runs",
            )
            .note(
                "The unreachable flag belongs to the block frame the `unreachable` was in, \
                 and the block's `end` pops that frame. After it the code is reachable again \
                 and the stack is empty, so the `i32.add` underflows.",
            ),
    ]
}
