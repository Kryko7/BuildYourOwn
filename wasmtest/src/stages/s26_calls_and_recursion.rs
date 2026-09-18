//! Stage 26 — Calls, recursion and call stack exhausted.
//!
//! A `call` names a function *index*, not a position in the file: imports come first, then
//! everything the module defines, and the whole index space exists before a single body
//! runs. So a function may call one defined after it, may call itself, and two functions
//! may call each other — the mutual-recursion test here is the one that catches a decoder
//! that resolves calls as it goes.
//!
//! Arguments are the callee's first locals, in order, and the callee's frame is fresh: its
//! remaining locals start at zero and its operand stack starts empty however full the
//! caller's was.
//!
//! The last test recurses without a base case. **How deep** a runtime gets before it gives
//! up is entirely its own business — it depends on the frame size, on whether the
//! interpreter uses the host stack, and on the platform's stack limit — and this suite
//! never asserts a number. What it does assert is that the answer is a clean trap saying
//! `call stack exhausted`, with a non-zero exit status, rather than a segfault, a killed
//! process or a host-language stack overflow. That is the difference between a runtime and
//! a program that happens to run WebAssembly.

use crate::examples::ExampleSpec;
use crate::stages::{case_i32, case_trap, run_cases_with, trap, Case, Stage, Test};
use crate::wasm::{ftype, op, BlockType, Expr, Func, Module, ModuleBuilder, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 26,
        slug: "calls_and_recursion",
        name: "Calls, recursion and call stack exhausted",
        ext: false,
        hints: &[
            "`call n` names a function index — imports first, then the module's own functions — and pops one argument per parameter, the first parameter being the value pushed first",
            "The index space is complete before any body runs, so a function may call one defined later, may call itself, and two functions may call each other",
            "The callee gets a fresh frame: the arguments become locals 0..n-1, the declared locals after them start at zero, and the operand stack starts empty",
            "Count the call depth yourself and trap with 'call stack exhausted' when your own limit is reached; running out of the host's stack and dying is not a trap",
        ],
        examples,
        tests: vec![
            Test::new("a call returns the callee's result to the caller", a_call_returns),
            Test::new("arguments arrive in the order they were pushed", argument_order),
            Test::new("a chain of five calls returns through all five", a_chain_of_five),
            Test::new("factorial by recursion matches the same factorial in Rust", factorial),
            Test::new("fibonacci by recursion matches the same fibonacci in Rust", fibonacci),
            Test::new("two mutually recursive functions agree on which numbers are even", mutual_recursion),
            Test::new("unbounded recursion is a clean trap, not a crash", stack_exhausted)
                .min_timeout_ms(30_000),
        ],
    }
}

/// `(i32) -> i32`, the signature almost every helper in this stage has.
fn unary() -> crate::wasm::FuncType {
    ftype(&[ValType::I32], &[ValType::I32])
}

wasm_test!(a_call_returns, |ctx| {
    let mut b = ModuleBuilder::new("a-call-returns");
    let t = b.add_type(unary());
    let add7 = b.add_func(
        t,
        Func::new(Expr::new().local_get(0).i32_const(7).op(op::I32_ADD)),
    );
    let double = b.add_func(
        t,
        Func::new(Expr::new().local_get(0).i32_const(2).op(op::I32_MUL)),
    );
    run_cases_with(
        ctx,
        b,
        vec![
            case_i32(
                "the callee's result is the caller's value",
                Expr::new().i32_const(3).call(add7),
                10,
            ),
            // The result of one call is the argument of the next, with nothing in between.
            case_i32(
                "the result of one call feeds straight into another",
                Expr::new().i32_const(3).call(add7).call(double),
                20,
            ),
            case_i32(
                "the other way round, which is a different number",
                Expr::new().i32_const(3).call(double).call(add7),
                13,
            ),
            case_i32(
                "two calls whose results are added",
                Expr::new()
                    .i32_const(1)
                    .call(add7)
                    .i32_const(2)
                    .call(add7)
                    .op(op::I32_ADD),
                17,
            ),
            // The callee's locals start at zero however full the caller's stack is.
            case_i32(
                "a call leaves the caller's stack below the arguments alone",
                Expr::new()
                    .i32_const(100)
                    .i32_const(3)
                    .call(add7)
                    .op(op::I32_ADD),
                110,
            ),
        ],
    )
});

wasm_test!(argument_order, |ctx| {
    let mut b = ModuleBuilder::new("argument-order");
    // `sub` is the point: `add` would pass this test with the arguments the wrong way round.
    let t2 = b.add_type(ftype(&[ValType::I32, ValType::I32], &[ValType::I32]));
    let sub = b.add_func(
        t2,
        Func::new(Expr::new().local_get(0).local_get(1).op(op::I32_SUB)),
    );
    let t3 = b.add_type(ftype(
        &[ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ));
    let digits = b.add_func(
        t3,
        Func::new(
            Expr::new()
                .local_get(0)
                .i32_const(100)
                .op(op::I32_MUL)
                .local_get(1)
                .i32_const(10)
                .op(op::I32_MUL)
                .op(op::I32_ADD)
                .local_get(2)
                .op(op::I32_ADD),
        ),
    );
    run_cases_with(
        ctx,
        b,
        vec![
            case_i32(
                "the first value pushed is the first parameter",
                Expr::new().i32_const(10).i32_const(3).call(sub),
                7,
            ),
            case_i32(
                "swapping the operands changes the sign",
                Expr::new().i32_const(3).i32_const(10).call(sub),
                -7,
            ),
            case_i32(
                "three parameters keep their order",
                Expr::new()
                    .i32_const(1)
                    .i32_const(2)
                    .i32_const(3)
                    .call(digits),
                123,
            ),
            case_i32(
                "the same three values in another order",
                Expr::new()
                    .i32_const(3)
                    .i32_const(2)
                    .i32_const(1)
                    .call(digits),
                321,
            ),
            // The arguments are themselves calls: they are evaluated left to right and the
            // outer call sees them in that order.
            case_i32(
                "arguments that are themselves calls keep their order",
                Expr::new()
                    .i32_const(10)
                    .i32_const(1)
                    .call(sub)
                    .i32_const(5)
                    .i32_const(3)
                    .call(sub)
                    .call(sub),
                7,
            ),
        ],
    )
});

wasm_test!(a_chain_of_five, |ctx| {
    let mut b = ModuleBuilder::new("a-chain-of-five");
    let t = b.add_type(unary());
    // Added innermost first, so each link can name the one it calls. c1 doubles; c2..c5 add
    // one and pass it down, so c5(n) is 2 * (n + 4).
    let c1 = b.add_func(
        t,
        Func::new(Expr::new().local_get(0).i32_const(2).op(op::I32_MUL)),
    );
    let link = |next: u32| {
        Func::new(
            Expr::new()
                .local_get(0)
                .i32_const(1)
                .op(op::I32_ADD)
                .call(next),
        )
    };
    let c2 = b.add_func(t, link(c1));
    let c3 = b.add_func(t, link(c2));
    let c4 = b.add_func(t, link(c3));
    let c5 = b.add_func(t, link(c4));
    run_cases_with(
        ctx,
        b,
        vec![
            case_i32(
                "the whole chain of five, from 3",
                Expr::new().i32_const(3).call(c5),
                14,
            ),
            case_i32(
                "the whole chain of five, from 0",
                Expr::new().i32_const(0).call(c5),
                8,
            ),
            case_i32("four links", Expr::new().i32_const(0).call(c4), 6),
            case_i32("three links", Expr::new().i32_const(0).call(c3), 4),
            case_i32("one link", Expr::new().i32_const(5).call(c1), 10),
            // The chain runs inside an expression, so every return value has to land back
            // on the caller's stack in the right place.
            case_i32(
                "two chains added together",
                Expr::new()
                    .i32_const(3)
                    .call(c5)
                    .i32_const(0)
                    .call(c5)
                    .op(op::I32_SUB),
                6,
            ),
        ],
    )
});

/// `n!`, computed here so the expectation is not a table somebody typed. The empty product
/// is 1, which is also the right answer for `0!`.
fn factorial_of(n: i32) -> i32 {
    (1..=n).product()
}

wasm_test!(factorial, |ctx| {
    let mut b = ModuleBuilder::new("factorial");
    let t = b.add_type(unary());
    // fact is function 0 and names itself: `call 0` inside the body it is being built from.
    let fact = b.add_func(
        t,
        Func::new(
            Expr::new()
                .local_get(0)
                .i32_const(2)
                .op(op::I32_LT_S)
                .if_else(
                    BlockType::Value(ValType::I32),
                    Expr::new().i32_const(1),
                    Expr::new()
                        .local_get(0)
                        .local_get(0)
                        .i32_const(1)
                        .op(op::I32_SUB)
                        .call(0)
                        .op(op::I32_MUL),
                ),
        ),
    );
    // 12! is the largest factorial that fits in an i32; 13! wraps, which is a different
    // stage's problem.
    let cases = (0..=12)
        .map(|n| {
            case_i32(
                format!("{n}! is {}", factorial_of(n)),
                Expr::new().i32_const(n).call(fact),
                factorial_of(n),
            )
        })
        .collect();
    run_cases_with(ctx, b, cases)
});

/// The n-th Fibonacci number, iteratively, to check the recursive one against.
fn fibonacci_of(n: i32) -> i32 {
    let (mut a, mut b) = (0i32, 1i32);
    for _ in 0..n {
        let next = a + b;
        a = b;
        b = next;
    }
    a
}

wasm_test!(fibonacci, |ctx| {
    let mut b = ModuleBuilder::new("fibonacci");
    let t = b.add_type(unary());
    // The doubly recursive definition: fib(n) = fib(n-1) + fib(n-2). fib(24) is 46 368 and
    // costs about 75 000 calls, which is the point — a runtime with a per-call allocation
    // somewhere will notice.
    let fib = b.add_func(
        t,
        Func::new(
            Expr::new()
                .local_get(0)
                .i32_const(2)
                .op(op::I32_LT_S)
                .if_else(
                    BlockType::Value(ValType::I32),
                    Expr::new().local_get(0),
                    Expr::new()
                        .local_get(0)
                        .i32_const(1)
                        .op(op::I32_SUB)
                        .call(0)
                        .local_get(0)
                        .i32_const(2)
                        .op(op::I32_SUB)
                        .call(0)
                        .op(op::I32_ADD),
                ),
        ),
    );
    let mut cases: Vec<Case> = (0..=12)
        .map(|n| {
            case_i32(
                format!("fib({n}) is {}", fibonacci_of(n)),
                Expr::new().i32_const(n).call(fib),
                fibonacci_of(n),
            )
        })
        .collect();
    for n in [18, 24] {
        cases.push(case_i32(
            format!("fib({n}) is {}", fibonacci_of(n)),
            Expr::new().i32_const(n).call(fib),
            fibonacci_of(n),
        ));
    }
    run_cases_with(ctx, b, cases)
});

wasm_test!(mutual_recursion, |ctx| {
    let mut b = ModuleBuilder::new("mutual-recursion");
    let t = b.add_type(unary());
    // is_even is function 0 and is_odd function 1. One of the two indices has to be written
    // down before the function it names exists, which is the whole point: a call names an
    // index, and the index space is complete before any body runs.
    let (even_idx, odd_idx) = (0u32, 1u32);
    let base = |zero: i32, other: u32| {
        Func::new(
            Expr::new().local_get(0).op(op::I32_EQZ).if_else(
                BlockType::Value(ValType::I32),
                Expr::new().i32_const(zero),
                Expr::new()
                    .local_get(0)
                    .i32_const(1)
                    .op(op::I32_SUB)
                    .call(other),
            ),
        )
    };
    let is_even = b.add_func(t, base(1, odd_idx));
    let is_odd = b.add_func(t, base(0, even_idx));
    let mut cases: Vec<Case> = Vec::new();
    for n in 0..=6 {
        cases.push(case_i32(
            format!("is_even({n}) is {}", 1 - n % 2),
            Expr::new().i32_const(n).call(is_even),
            1 - n % 2,
        ));
        cases.push(case_i32(
            format!("is_odd({n}) is {}", n % 2),
            Expr::new().i32_const(n).call(is_odd),
            n % 2,
        ));
    }
    // A thousand alternations deep, to show the two really do take turns rather than one of
    // them quietly answering on its own.
    cases.push(case_i32(
        "is_even(1000) is 1, a thousand alternating calls deep",
        Expr::new().i32_const(1000).call(is_even),
        1,
    ));
    cases.push(case_i32(
        "is_odd(1001) is 1",
        Expr::new().i32_const(1001).call(is_odd),
        1,
    ));
    run_cases_with(ctx, b, cases)
});

/// A function with no base case: it calls itself, and the `i32.add` after the call is what
/// stops a runtime turning it into a loop.
fn runaway() -> Func {
    Func::new(Expr::new().call(0).i32_const(1).op(op::I32_ADD))
}

wasm_test!(stack_exhausted, |ctx| {
    let mut b = ModuleBuilder::new("stack-exhausted");
    let nullary = b.add_type(ftype(&[], &[ValType::I32]));
    // Function 0 calls function 0. The `i32.const 1  i32.add` after it means the call is
    // not in tail position, so nothing may rewrite it into a jump.
    let blow = b.add_func(nullary, runaway());
    let t = b.add_type(unary());
    // A bounded recursion of a thousand frames, for contrast: deep is not the same as
    // unbounded, and a runtime whose limit is too mean fails this one instead.
    let sum_to = b.add_func(
        t,
        Func::new(
            Expr::new().local_get(0).op(op::I32_EQZ).if_else(
                BlockType::Value(ValType::I32),
                Expr::new().i32_const(0),
                Expr::new()
                    .local_get(0)
                    .local_get(0)
                    .i32_const(1)
                    .op(op::I32_SUB)
                    .call(1)
                    .op(op::I32_ADD),
            ),
        ),
    );
    run_cases_with(
        ctx,
        b,
        vec![
            case_i32(
                "a bounded recursion a thousand frames deep returns normally",
                Expr::new().i32_const(1000).call(sum_to),
                500_500,
            ),
            case_trap(
                "a function that calls itself with no base case traps",
                Expr::new().call(blow),
                &[trap::STACK_EXHAUSTED],
            ),
            case_trap(
                "the same runaway reached through another call",
                Expr::new().i32_const(0).call(sum_to).drop().call(blow),
                &[trap::STACK_EXHAUSTED],
            ),
        ],
    )?;
    ctx.note(
        "the depth at which a runtime gives up is its own business; the suite only asks for \
         a trap saying 'call stack exhausted' rather than a crash",
    );
    Ok(())
});

/// `fact(n)` on its own, as a module a learner can run.
fn factorial_module() -> Module {
    let mut b = ModuleBuilder::new("factorial");
    let t = b.add_type(unary());
    let fact = b.add_func(
        t,
        Func::new(
            Expr::new()
                .local_get(0)
                .i32_const(2)
                .op(op::I32_LT_S)
                .if_else(
                    BlockType::Value(ValType::I32),
                    Expr::new().i32_const(1),
                    Expr::new()
                        .local_get(0)
                        .local_get(0)
                        .i32_const(1)
                        .op(op::I32_SUB)
                        .call(0)
                        .op(op::I32_MUL),
                ),
        ),
    );
    b.export_func("f", fact).build()
}

/// The function that calls itself for ever.
fn runaway_module() -> Module {
    let mut b = ModuleBuilder::new("runaway");
    let t = b.add_type(ftype(&[], &[ValType::I32]));
    let blow = b.add_func(t, runaway());
    b.export_func("f", blow).build()
}

/// Worked examples: recursion that ends, and recursion that does not.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("Factorial, by recursion", factorial_module)
            .summary(
                "one function: `if n < 2 then 1 else n * f(n - 1)`, written with an \
                 `if (result i32)` and a `call 0` that names the function being defined",
            )
            .command("run --invoke f mod.wasm 10")
            .output("3628800")
            .note(
                "`call 0` inside function 0 is not a forward reference problem: the index \
                 space is complete before any body runs. 12! is the largest factorial that \
                 fits in an i32.",
            ),
        ExampleSpec::module("Recursion with no base case", runaway_module)
            .summary(
                "`call 0  i32.const 1  i32.add` — function 0 calling function 0, with the \
                 add after it so the call is not in tail position",
            )
            .command("run --invoke f mod.wasm")
            .output(
                "nothing on stdout, `wasm trap: call stack exhausted` on stderr, and a \
                 non-zero exit status",
            )
            .note(
                "How deep it gets first is the runtime's business. What is not negotiable \
                 is that it ends in a trap: an interpreter that recurses on its own host \
                 stack will segfault here instead, and a segfault is not a trap.",
            ),
    ]
}
