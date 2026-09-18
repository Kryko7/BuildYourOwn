//! Stage 08 — Operand stack underflow and leftovers.
//!
//! Every instruction has a fixed shape: how many values it takes off the operand stack, of
//! what types, and what it pushes back. The validator walks a body once with a stack of
//! *types* and either pops what an instruction needs or refuses the module. Nothing here is
//! a runtime check — by the time a body executes, the runtime already knows the stack is
//! deep enough, which is exactly why a real runtime can compile `i32.add` to one machine
//! instruction with no guard in front of it.
//!
//! Note where the error lands. `wasmtime` reports the underflow at the offset of the
//! instruction that could not be satisfied — `expected i32 but nothing on stack` — not at
//! the end of the body, and a runtime that only compares the final stack height would let
//! `i32.const 1, i32.add` through because the height happens to work out.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::stages::{expect_rejected, single, single_with_locals, Stage, Test};
use crate::wasm::{ftype, op, BlockType, Expr, Func, Module, ModuleBuilder, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 8,
        slug: "stack_discipline",
        name: "Operand stack underflow and leftovers",
        ext: false,
        hints: &[
            "Validate with a stack of types, not values: push the result types of each instruction and pop the operand types it needs, refusing the module the moment a pop finds nothing",
            "Every instruction that takes operands can underflow — `drop`, `local.set`, `if`'s condition and a `call`'s arguments as much as `i32.add`",
            "A `return` in the middle of a body must supply the function's results there and then; it does not inherit them from the end of the body",
            "Check the types as well as the depth: `i32.add` on an i64 is as wrong as `i32.add` on an empty stack, and a stack that is merely the right height is not the right stack",
        ],
        examples,
        tests: vec![
            Test::new("i32.add with fewer than two operands is refused", add_underflow),
            Test::new("drop on an empty stack is refused", drop_underflow),
            Test::new("local.set with nothing to set is refused", local_set_underflow),
            Test::new("a call whose arguments are not on the stack is refused", call_underflow),
            Test::new("a return that does not supply the results is refused", return_underflow),
            Test::new("an if with no condition on the stack is refused", if_underflow),
            Test::new("a body that stacks sixty-four values and folds them is accepted", deep_but_correct),
        ],
    }
}

wasm_test!(add_underflow, |ctx| {
    // One operand and none. The first is the interesting one: the *height* at the end of the
    // body is right — one i32 — so only a validator that pops per instruction catches it.
    let one = single(
        "add-one-operand",
        ftype(&[], &[ValType::I32]),
        Expr::new().i32_const(1).op(op::I32_ADD),
    );
    expect_rejected(
        ctx,
        &one,
        "i32.add takes two operands and only one was pushed, even though the body would end \
         one value deep either way",
    )?;
    let none = single(
        "add-no-operands",
        ftype(&[], &[ValType::I32]),
        Expr::new().op(op::I32_ADD),
    );
    expect_rejected(ctx, &none, "i32.add with an empty operand stack")?;
    Ok(())
});

wasm_test!(drop_underflow, |ctx| {
    let m = single("drop-empty", ftype(&[], &[]), Expr::new().drop());
    expect_rejected(
        ctx,
        &m,
        "drop takes one value of any type, and 'any type' still means there has to be one",
    )?;
    Ok(())
});

wasm_test!(local_set_underflow, |ctx| {
    let m = single_with_locals(
        "local-set-empty",
        ftype(&[], &[]),
        &[(1, ValType::I32)],
        Expr::new().local_set(0),
    );
    expect_rejected(
        ctx,
        &m,
        "local.set pops the value it stores; the local exists, the value does not",
    )?;
    Ok(())
});

wasm_test!(call_underflow, |ctx| {
    let m = call_without_arguments();
    expect_rejected(
        ctx,
        &m,
        "the callee takes two i32 parameters and the caller pushes neither: a call pops one \
         value per parameter, right to left, before it pushes the results",
    )?;
    Ok(())
});

wasm_test!(return_underflow, |ctx| {
    let m = single(
        "return-no-result",
        ftype(&[], &[ValType::I32]),
        Expr::new().return_(),
    );
    expect_rejected(
        ctx,
        &m,
        "return leaves the function then and there, so the results have to be on the stack \
         at the return, not merely somewhere later in the body",
    )?;
    Ok(())
});

wasm_test!(if_underflow, |ctx| {
    let m = single(
        "if-no-condition",
        ftype(&[], &[]),
        Expr::new().if_(BlockType::Empty, Expr::new().nop()),
    );
    expect_rejected(
        ctx,
        &m,
        "if pops an i32 condition before either arm is even looked at",
    )?;
    Ok(())
});

wasm_test!(deep_but_correct, |ctx| {
    // The balancing case: sixty-four values pushed, then folded back to one with sixty-three
    // adds. The stack is used hard and used correctly, and the answer is 1 + 2 + ... + 64.
    let m = deep_sum();
    let run = ctx.invoke(&m, "f", &[])?;
    let mut c = Check::new("a body that uses the operand stack to its limit", &run);
    c.module(&m);
    c.eq("stdout", "2080", run.first_line().as_str());
    c.that(
        "exit",
        "exit status 0: a deep operand stack is ordinary, not an error",
        run.exit.success(),
        run.exit.label(),
    );
    c.finish()?;
    ctx.note("64 values on the operand stack at once, folded with 63 i32.add");
    Ok(())
});

// ---------------------------------------------------------------------------------------
// Modules
// ---------------------------------------------------------------------------------------

/// How many values [`deep_sum`] stacks before it starts folding them.
const DEPTH: i32 = 64;

/// `() -> i32` that pushes `1..=64` and adds them all up: 2080.
fn deep_sum() -> Module {
    let mut e = Expr::new();
    for i in 1..=DEPTH {
        e = e.i32_const(i);
    }
    for _ in 1..DEPTH {
        e = e.op(op::I32_ADD);
    }
    single("deep-but-correct", ftype(&[], &[ValType::I32]), e)
}

/// A module whose exported function calls a two-parameter function with an empty stack.
fn call_without_arguments() -> Module {
    let mut b = ModuleBuilder::new("call-without-arguments");
    let add_ty = b.add_type(ftype(&[ValType::I32, ValType::I32], &[ValType::I32]));
    let add = b.add_func(
        add_ty,
        Func::new(Expr::new().local_get(0).local_get(1).op(op::I32_ADD)),
    );
    let f_ty = b.add_type(ftype(&[], &[ValType::I32]));
    let f = b.add_func(f_ty, Func::new(Expr::new().call(add)));
    b.export_func("f", f).build()
}

/// The one-operand `i32.add`, as a named builder for the catalog.
fn add_with_one_operand() -> Module {
    single(
        "add-one-operand",
        ftype(&[], &[ValType::I32]),
        Expr::new().i32_const(1).op(op::I32_ADD),
    )
}

/// Worked examples: the underflow that ends at the right height, and a stack used hard.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("An i32.add with one operand", add_with_one_operand)
            .summary(
                "a `() -> i32` whose body is `i32.const 1`, `i32.add`, `end` — one value \
                 pushed, two needed",
            )
            .command("run --invoke f mod.wasm")
            .output(
                "nothing on stdout, a message on stderr, and a non-zero exit status — the \
                 module never runs",
            )
            .note(
                "The body would finish one value deep, which is exactly what the signature \
                 asks for, so a validator that only compares stack heights at the end lets \
                 this through. Pop the operands of every instruction as you meet it.",
            ),
        ExampleSpec::module("Sixty-four values, then sixty-three adds", deep_sum)
            .summary(
                "a `() -> i32` that pushes the constants 1 to 64 and folds them with \
                 `i32.add` until one value is left",
            )
            .command("run --invoke f mod.wasm")
            .output("2080")
            .note(
                "There is no limit here worth enforcing: the operand stack of a function is \
                 as deep as its body makes it, and the validator already knows the maximum \
                 depth once it has walked the body once.",
            ),
    ]
}
