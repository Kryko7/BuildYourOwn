//! Stage 25 — select, typed and untyped.
//!
//! `select` pops three values: the condition first, then the second operand, then the
//! first. It yields the **first** operand when the condition is non-zero. That order is
//! what people invert — reading `a b c select` as "if c then b else a" gives the right
//! answer for a symmetric test and the wrong one for everything else — so the first test
//! here pushes two different constants and checks both directions.
//!
//! There is nothing lazy about it. Both operands are ordinary values that were computed
//! before `select` ran, so a `select` between a constant and a division by zero traps
//! whichever way the condition points; it is not a conditional expression in the sense a C
//! programmer means.
//!
//! Two encodings exist. `0x1b` is the untyped form and takes numeric operands only; `0x1c`
//! is the typed form, which writes out a vector of result types and is the only one a
//! reference type may go through. The last test proves the untyped form over an
//! `externref` is refused before anything runs.

use crate::examples::ExampleSpec;
use crate::stages::{
    case_f32, case_f64, case_i32, case_i64, case_trap, expect_rejected, f_i32, run_cases, trap,
    Case, Stage, Test,
};
use crate::wasm::{ftype, op, Expr, Module, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 25,
        slug: "select",
        name: "select, typed and untyped",
        ext: false,
        hints: &[
            "`select` pops the condition, then the second operand, then the first, and yields the **first** operand when the condition is non-zero",
            "Both operands are already on the stack when it runs, so nothing is skipped: a trap in the operand that loses still happens",
            "The two operands must have the same type, and that type is the result type — there is no promotion and no mixing",
            "The untyped 0x1b form only accepts numeric operands; a funcref or externref needs the typed 0x1c form, which writes the type out",
        ],
        examples,
        tests: vec![
            Test::new("select yields the first operand when the condition is true", which_operand),
            Test::new("select works on all four numeric types", every_numeric_type),
            Test::new("every non-zero condition picks the first operand", what_counts_as_true),
            Test::new("select implements a branchless max", branchless_max),
            Test::new("both operands are evaluated before select runs", nothing_is_lazy),
            Test::new("the typed form behaves like the untyped one on numbers", typed_on_numbers),
            Test::new("a reference operand needs the typed form", reference_operands),
        ],
    }
}

/// `first second cond select` over i32 constants.
fn pick(first: i32, second: i32, cond: i32) -> Expr {
    Expr::new()
        .i32_const(first)
        .i32_const(second)
        .i32_const(cond)
        .select()
}

wasm_test!(which_operand, |ctx| {
    run_cases(
        ctx,
        "which-operand",
        vec![
            case_i32(
                "a true condition yields the first operand",
                pick(10, 20, 1),
                10,
            ),
            case_i32(
                "a false condition yields the second operand",
                pick(10, 20, 0),
                20,
            ),
            // The same two numbers pushed the other way round. A runtime with the order
            // inverted passes the pair above only by accident and fails both of these.
            case_i32(
                "the first operand is the one pushed first",
                pick(20, 10, 1),
                20,
            ),
            case_i32(
                "the second operand is the one pushed second",
                pick(20, 10, 0),
                10,
            ),
            case_i32(
                "select leaves exactly one value behind",
                Expr::new()
                    .i32_const(5)
                    .i32_const(10)
                    .i32_const(20)
                    .i32_const(1)
                    .select()
                    .op(op::I32_SUB),
                -5,
            ),
        ],
    )
});

wasm_test!(every_numeric_type, |ctx| {
    run_cases(
        ctx,
        "every-numeric-type",
        vec![
            case_i32("i32, the first operand", pick(10, 20, 1), 10),
            case_i32("i32, the second operand", pick(10, 20, 0), 20),
            case_i64(
                "i64, the first operand",
                Expr::new()
                    .i64_const(9_000_000_000)
                    .i64_const(-9_000_000_000)
                    .i32_const(1)
                    .select(),
                9_000_000_000,
            ),
            case_i64(
                "i64, the second operand",
                Expr::new()
                    .i64_const(9_000_000_000)
                    .i64_const(-9_000_000_000)
                    .i32_const(0)
                    .select(),
                -9_000_000_000,
            ),
            case_f32(
                "f32, the first operand",
                Expr::new()
                    .f32_const(1.5)
                    .f32_const(-2.5)
                    .i32_const(1)
                    .select(),
                1.5,
            ),
            case_f32(
                "f32, the second operand",
                Expr::new()
                    .f32_const(1.5)
                    .f32_const(-2.5)
                    .i32_const(0)
                    .select(),
                -2.5,
            ),
            case_f64(
                "f64, the first operand",
                Expr::new()
                    .f64_const(1.5)
                    .f64_const(-2.5)
                    .i32_const(1)
                    .select(),
                1.5,
            ),
            // The condition never looks at the value, so a select over floats hands back
            // the exact bits it was given, negative zero and infinities included.
            case_f64(
                "f64 negative zero comes back as negative zero",
                Expr::new()
                    .f64_const(-0.0)
                    .f64_const(1.0)
                    .i32_const(1)
                    .select(),
                -0.0,
            ),
            case_f64(
                "f64 infinity comes back as infinity",
                Expr::new()
                    .f64_const(1.0)
                    .f64_const(f64::INFINITY)
                    .i32_const(0)
                    .select(),
                f64::INFINITY,
            ),
        ],
    )
});

wasm_test!(what_counts_as_true, |ctx| {
    run_cases(
        ctx,
        "what-counts-as-true",
        vec![
            case_i32("condition 0 is the only false one", pick(10, 20, 0), 20),
            case_i32("condition 1 is true", pick(10, 20, 1), 10),
            case_i32("condition -1 is true", pick(10, 20, -1), 10),
            case_i32("condition 2 is true", pick(10, 20, 2), 10),
            case_i32("condition INT_MAX is true", pick(10, 20, i32::MAX), 10),
            case_i32("condition INT_MIN is true", pick(10, 20, i32::MIN), 10),
            // 256 is true and its low byte is zero: an implementation that looks only at the
            // bottom eight bits of the condition fails this case and nothing else.
            case_i32(
                "condition 256 is true, low byte and all",
                pick(10, 20, 256),
                10,
            ),
        ],
    )
});

/// `max(a, b)` with no branch in it: `a b (a > b) select`.
fn max_body() -> Expr {
    Expr::new()
        .local_get(0)
        .local_get(1)
        .local_get(0)
        .local_get(1)
        .op(op::I32_GT_S)
        .select()
}

wasm_test!(branchless_max, |ctx| {
    let inputs: &[([&str; 2], i32)] = &[
        (["3", "4"], 4),
        (["4", "3"], 4),
        (["7", "7"], 7),
        (["-1", "-9"], -1),
        (["-9", "-1"], -1),
        (["-2147483648", "2147483647"], 2_147_483_647),
        (["0", "-2147483648"], 0),
    ];
    let cases = inputs
        .iter()
        .map(|(args, want)| {
            Case::new(
                format!("max({}, {}) is {want}", args[0], args[1]),
                ftype(&[ValType::I32, ValType::I32], &[ValType::I32]),
                max_body(),
                &[&want.to_string()],
            )
            .args(args)
        })
        .collect();
    run_cases(ctx, "branchless-max", cases)
});

wasm_test!(nothing_is_lazy, |ctx| {
    run_cases(
        ctx,
        "nothing-is-lazy",
        vec![
            // The condition picks the 7, but 1/0 was evaluated on the way to the select and
            // the program is already over. An implementation that treats select as a
            // conditional expression returns 7 here.
            case_trap(
                "a trapping second operand traps even when the first is chosen",
                Expr::new()
                    .i32_const(7)
                    .i32_const(1)
                    .i32_const(0)
                    .op(op::I32_DIV_S)
                    .i32_const(1)
                    .select(),
                &[trap::DIVIDE_BY_ZERO],
            ),
            case_trap(
                "a trapping first operand traps even when the second is chosen",
                Expr::new()
                    .i32_const(1)
                    .i32_const(0)
                    .op(op::I32_DIV_S)
                    .i32_const(7)
                    .i32_const(0)
                    .select(),
                &[trap::DIVIDE_BY_ZERO],
            ),
            case_trap(
                "a trapping condition traps before the select is reached",
                Expr::new()
                    .i32_const(10)
                    .i32_const(20)
                    .i32_const(1)
                    .i32_const(0)
                    .op(op::I32_DIV_S)
                    .select(),
                &[trap::DIVIDE_BY_ZERO],
            ),
            // The same shape without the zero divisor, to show that the trap above really
            // is the division and not the select.
            case_i32(
                "the same shape with a divisor that is not zero",
                Expr::new()
                    .i32_const(7)
                    .i32_const(6)
                    .i32_const(2)
                    .op(op::I32_DIV_S)
                    .i32_const(1)
                    .select(),
                7,
            ),
        ],
    )
});

wasm_test!(typed_on_numbers, |ctx| {
    run_cases(
        ctx,
        "typed-on-numbers",
        vec![
            case_i32(
                "select (i32), the first operand",
                Expr::new()
                    .i32_const(10)
                    .i32_const(20)
                    .i32_const(1)
                    .select_typed(&[ValType::I32]),
                10,
            ),
            case_i32(
                "select (i32), the second operand",
                Expr::new()
                    .i32_const(10)
                    .i32_const(20)
                    .i32_const(0)
                    .select_typed(&[ValType::I32]),
                20,
            ),
            case_i64(
                "select (i64)",
                Expr::new()
                    .i64_const(10)
                    .i64_const(20)
                    .i32_const(0)
                    .select_typed(&[ValType::I64]),
                20,
            ),
            case_f32(
                "select (f32)",
                Expr::new()
                    .f32_const(1.5)
                    .f32_const(-2.5)
                    .i32_const(1)
                    .select_typed(&[ValType::F32]),
                1.5,
            ),
            case_f64(
                "select (f64)",
                Expr::new()
                    .f64_const(1.5)
                    .f64_const(-2.5)
                    .i32_const(0)
                    .select_typed(&[ValType::F64]),
                -2.5,
            ),
            case_trap(
                "select (i32) is no lazier than the untyped form",
                Expr::new()
                    .i32_const(7)
                    .i32_const(1)
                    .i32_const(0)
                    .op(op::I32_DIV_S)
                    .i32_const(1)
                    .select_typed(&[ValType::I32]),
                &[trap::DIVIDE_BY_ZERO],
            ),
        ],
    )
});

/// `select (t)` between a null reference and a null reference, reported by `ref.is_null`.
///
/// There is nothing to print about a reference, so every case here ends in `ref.is_null`
/// and answers 1 for "the null one was chosen" and 0 for "the other one was".
fn ref_select(t: ValType, first: Expr, second: Expr, cond: i32) -> Expr {
    first
        .then(second)
        .i32_const(cond)
        .select_typed(&[t])
        .ref_is_null()
}

wasm_test!(reference_operands, |ctx| {
    run_cases(
        ctx,
        "reference-operands",
        vec![
            case_i32(
                "select (externref) between two nulls",
                ref_select(
                    ValType::ExternRef,
                    Expr::new().ref_null(ValType::ExternRef),
                    Expr::new().ref_null(ValType::ExternRef),
                    1,
                ),
                1,
            ),
            // `ref.func 0` names this very module's first function; a case's own function is
            // exported, which is what lets `ref.func` name it.
            case_i32(
                "select (funcref) picks the function when the condition is true",
                ref_select(
                    ValType::FuncRef,
                    Expr::new().ref_func(0),
                    Expr::new().ref_null(ValType::FuncRef),
                    1,
                ),
                0,
            ),
            case_i32(
                "select (funcref) picks the null when the condition is false",
                ref_select(
                    ValType::FuncRef,
                    Expr::new().ref_func(0),
                    Expr::new().ref_null(ValType::FuncRef),
                    0,
                ),
                1,
            ),
            case_i32(
                "select (funcref) the other way round",
                ref_select(
                    ValType::FuncRef,
                    Expr::new().ref_null(ValType::FuncRef),
                    Expr::new().ref_func(0),
                    0,
                ),
                0,
            ),
        ],
    )?;

    // The untyped form is defined only for numeric operands. This module is well formed in
    // every other respect, so a runtime that lets it through has skipped the type check
    // rather than failed it.
    expect_rejected(
        ctx,
        &untyped_select_over_a_reference(),
        "the untyped select (0x1b) is defined only for numeric operands, and both of these \
         are externref",
    )?;
    Ok(())
});

/// `ref.null externref  ref.null externref  i32.const 1  select` — the form that must not
/// validate.
fn untyped_select_over_a_reference() -> Module {
    f_i32(
        "untyped-select-over-externref",
        Expr::new()
            .ref_null(ValType::ExternRef)
            .ref_null(ValType::ExternRef)
            .i32_const(1)
            .select()
            .ref_is_null(),
    )
}

/// `select` deciding between two constants on a condition supplied at run time.
fn chooser() -> Module {
    crate::stages::single(
        "chooser",
        ftype(&[ValType::I32], &[ValType::I32]),
        Expr::new()
            .i32_const(10)
            .i32_const(20)
            .local_get(0)
            .select(),
    )
}

/// A `select` whose losing operand traps anyway.
fn select_still_traps() -> Module {
    f_i32(
        "select-still-traps",
        Expr::new()
            .i32_const(7)
            .i32_const(1)
            .i32_const(0)
            .op(op::I32_DIV_S)
            .i32_const(1)
            .select(),
    )
}

/// Worked examples: the order of the operands, and the thing select is not.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("select and the order of its operands", chooser)
            .summary(
                "`i32.const 10  i32.const 20  local.get 0  select` — the condition is the \
                 function's argument",
            )
            .command("run --invoke f mod.wasm 1")
            .output("10")
            .note(
                "Pass 0 and the answer is 20. The condition is popped first, then the \
                 second operand, then the first, and a non-zero condition keeps the **first** \
                 — the one that was pushed earliest and is deepest on the stack.",
            ),
        ExampleSpec::module("select is not a conditional expression", select_still_traps)
            .summary(
                "a select between `i32.const 7` and `1 / 0` with a condition of 1, which \
                 chooses the 7",
            )
            .command("run --invoke f mod.wasm")
            .output(
                "nothing on stdout, `wasm trap: integer divide by zero` on stderr, and a \
                 non-zero exit status",
            )
            .note(
                "Both operands are computed before `select` sees them, so the division \
                 happens whatever the condition says. If you want laziness, use `if`.",
            ),
    ]
}
