//! Stage 15 — Division, remainder and the two integer traps.
//!
//! Division is the first instruction in the numeric section that can refuse to produce a
//! value at all, and it can do so for two quite different reasons. A zero divisor is
//! `integer divide by zero`, for all eight of `i32`/`i64` × `div_s`/`div_u`/`rem_s`/`rem_u`.
//! `i32::MIN / -1` and `i64::MIN / -1` are `integer overflow`, because the true quotient is
//! one past the top of the range — and *only* the signed division does this: `div_u` of the
//! same two bit patterns is a perfectly ordinary 0.
//!
//! Then there is the case everybody gets wrong. `i32::MIN rem_s -1` is **0**, not a trap.
//! The remainder is well defined even though the quotient is not, and a runtime that
//! implements `rem_s` as "divide, multiply back, subtract" — or that leans on a host CPU
//! instruction which computes both at once — will trap here when it must not. It has a test
//! of its own, and so does its `i64` twin.
//!
//! The rounding is the other half of the stage. `div_s` truncates **towards zero**, so
//! `-7 / 2` is -3 and not the -4 that a floor division would give, and `rem_s` takes its
//! sign from the **dividend**, so `-7 % 2` is -1 while `7 % -2` is 1. The two agree: for
//! every pair that does not trap, `(a / b) * b + (a % b)` is `a` again, and the last sweep
//! checks exactly that.
//!
//! Finally, a trap aborts the program. Nothing the function computed before it reaches
//! stdout, because a result is only printed once the call has returned, and it never does.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::stages::{
    case_i32, case_i64, case_trap, expect_trap, run_cases, trap, Case, Stage, Test,
};
use crate::wasm::{ftype, op, Expr, Func, Module, ModuleBuilder, Op, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 15,
        slug: "division_traps",
        name: "Division, remainder and the two integer traps",
        ext: false,
        hints: &[
            "div_s truncates towards zero, so -7 / 2 is -3; rem_s takes the sign of the dividend, so -7 % 2 is -1 and 7 % -2 is 1",
            "Check the divisor for zero before dividing anything: all eight of div_s, div_u, rem_s and rem_u at both widths trap with 'integer divide by zero'",
            "i32::MIN / -1 and i64::MIN / -1 trap with 'integer overflow' — but div_u of the same bit patterns is 0, and it must not trap",
            "i32::MIN rem_s -1 is 0 and is NOT a trap, so do not compute the remainder by dividing first; most hardware divide instructions fault on exactly this pair",
        ],
        examples,
        tests: vec![
            Test::new("div_s truncates towards zero, not towards minus infinity", div_s_truncates),
            Test::new("rem_s takes its sign from the dividend", rem_s_sign),
            Test::new("div_u and rem_u read both operands as unsigned", unsigned_division),
            Test::new("every division and remainder by zero traps", divide_by_zero),
            Test::new("the most negative value divided by -1 traps with integer overflow", overflow_trap),
            Test::new("the most negative value remaindered by -1 is 0, not a trap", rem_of_min_is_not_a_trap),
            Test::new("quotient and remainder reconstruct the dividend", reconstruction),
            Test::new("a trap leaves nothing on stdout, whatever ran before it", nothing_is_printed),
        ],
    }
}

/// `a op b` with two `i32` operands.
fn bin32(o: Op, a: i32, b: i32) -> Expr {
    Expr::new().i32_const(a).i32_const(b).op(o)
}

/// `a op b` with two `i64` operands.
fn bin64(o: Op, a: i64, b: i64) -> Expr {
    Expr::new().i64_const(a).i64_const(b).op(o)
}

/// A `() -> i64` case that must trap: `case_trap` only builds the `i32` shape.
fn trap64(name: &str, body: Expr, reasons: &[&str]) -> Case {
    Case::new(name, ftype(&[], &[ValType::I64]), body, &[]).traps(reasons)
}

wasm_test!(div_s_truncates, |ctx| {
    run_cases(
        ctx,
        "div-s",
        vec![
            case_i32("7 / 2 is 3", bin32(op::I32_DIV_S, 7, 2), 3),
            case_i32(
                "-7 / 2 is -3: the quotient is truncated towards zero, not floored to -4",
                bin32(op::I32_DIV_S, -7, 2),
                -3,
            ),
            case_i32("7 / -2 is -3 as well", bin32(op::I32_DIV_S, 7, -2), -3),
            case_i32(
                "-7 / -2 is 3: two negatives make a positive quotient",
                bin32(op::I32_DIV_S, -7, -2),
                3,
            ),
            case_i32("1 / 2 is 0", bin32(op::I32_DIV_S, 1, 2), 0),
            case_i32(
                "-1 / 2 is 0, not -1: truncation towards zero again",
                bin32(op::I32_DIV_S, -1, 2),
                0,
            ),
            case_i32("-1 / -1 is 1", bin32(op::I32_DIV_S, -1, -1), 1),
            case_i32("0 / -5 is 0", bin32(op::I32_DIV_S, 0, -5), 0),
            case_i32(
                "i32::MIN / 2 is -1073741824",
                bin32(op::I32_DIV_S, i32::MIN, 2),
                -1073741824,
            ),
            case_i32(
                "i32::MIN / -2 is 1073741824, which still fits",
                bin32(op::I32_DIV_S, i32::MIN, -2),
                1073741824,
            ),
            case_i32(
                "i32::MAX / -1 is -2147483647 and does not trap",
                bin32(op::I32_DIV_S, i32::MAX, -1),
                -2147483647,
            ),
            case_i64(
                "-7 / 2 is -3 at sixty-four bits too",
                bin64(op::I64_DIV_S, -7, 2),
                -3,
            ),
            case_i64(
                "i64::MIN / 2 is -4611686018427387904",
                bin64(op::I64_DIV_S, i64::MIN, 2),
                -4611686018427387904,
            ),
        ],
    )
});

wasm_test!(rem_s_sign, |ctx| {
    run_cases(
        ctx,
        "rem-s",
        vec![
            case_i32("7 % 2 is 1", bin32(op::I32_REM_S, 7, 2), 1),
            case_i32(
                "-7 % 2 is -1: the remainder follows the dividend, not the divisor",
                bin32(op::I32_REM_S, -7, 2),
                -1,
            ),
            case_i32(
                "7 % -2 is 1, for the same reason the other way round",
                bin32(op::I32_REM_S, 7, -2),
                1,
            ),
            case_i32("-7 % -2 is -1", bin32(op::I32_REM_S, -7, -2), -1),
            case_i32("5 % 5 is 0", bin32(op::I32_REM_S, 5, 5), 0),
            case_i32(
                "-5 % 5 is 0, with no sign to carry",
                bin32(op::I32_REM_S, -5, 5),
                0,
            ),
            case_i32("1 % 2 is 1", bin32(op::I32_REM_S, 1, 2), 1),
            case_i32("-1 % 2 is -1", bin32(op::I32_REM_S, -1, 2), -1),
            case_i32(
                "i32::MIN % 3 is -2, and a floored remainder would say 1",
                bin32(op::I32_REM_S, i32::MIN, 3),
                i32::MIN % 3,
            ),
            case_i32("i32::MIN % 2 is 0", bin32(op::I32_REM_S, i32::MIN, 2), 0),
            case_i32("i32::MAX % -1 is 0", bin32(op::I32_REM_S, i32::MAX, -1), 0),
            case_i64(
                "-7 % 2 is -1 at sixty-four bits too",
                bin64(op::I64_REM_S, -7, 2),
                -1,
            ),
            case_i64(
                "i64::MIN % 3 is -2",
                bin64(op::I64_REM_S, i64::MIN, 3),
                i64::MIN % 3,
            ),
        ],
    )
});

wasm_test!(unsigned_division, |ctx| {
    run_cases(
        ctx,
        "div-u",
        vec![
            case_i32(
                "7 div_u 2 is 3, the same as the signed answer",
                bin32(op::I32_DIV_U, 7, 2),
                3,
            ),
            case_i32(
                "-1 div_u 2 is 2147483647: the dividend is read as 4294967295",
                bin32(op::I32_DIV_U, -1, 2),
                2147483647,
            ),
            case_i32(
                "-2 div_u 2 is 2147483647 as well",
                bin32(op::I32_DIV_U, -2, 2),
                2147483647,
            ),
            case_i32(
                "-1 div_u -1 is 1: both operands are the same large unsigned number",
                bin32(op::I32_DIV_U, -1, -1),
                1,
            ),
            case_i32(
                "-1 rem_u 2 is 1, because 4294967295 is odd",
                bin32(op::I32_REM_U, -1, 2),
                1,
            ),
            case_i32("-1 rem_u -1 is 0", bin32(op::I32_REM_U, -1, -1), 0),
            case_i32(
                "-7 div_u 2 is 2147483644, nothing like the signed -3",
                bin32(op::I32_DIV_U, -7, 2),
                2147483644,
            ),
            case_i32("-7 rem_u 2 is 1", bin32(op::I32_REM_U, -7, 2), 1),
            case_i32(
                "i32::MIN div_u 2 is 1073741824: unsigned, it is 2147483648",
                bin32(op::I32_DIV_U, i32::MIN, 2),
                1073741824,
            ),
            case_i32(
                "i32::MIN div_u -1 is 0, because 2147483648 is less than 4294967295",
                bin32(op::I32_DIV_U, i32::MIN, -1),
                0,
            ),
            case_i32(
                "i32::MIN rem_u -1 is i32::MIN, the whole dividend left over",
                bin32(op::I32_REM_U, i32::MIN, -1),
                i32::MIN,
            ),
            case_i64(
                "-1 div_u 2 is 9223372036854775807 at sixty-four bits",
                bin64(op::I64_DIV_U, -1, 2),
                9223372036854775807,
            ),
            case_i64(
                "i64::MIN div_u -1 is 0",
                bin64(op::I64_DIV_U, i64::MIN, -1),
                0,
            ),
            case_i64(
                "i64::MIN rem_u -1 is i64::MIN",
                bin64(op::I64_REM_U, i64::MIN, -1),
                i64::MIN,
            ),
        ],
    )
});

wasm_test!(divide_by_zero, |ctx| {
    run_cases(
        ctx,
        "divide-by-zero",
        vec![
            case_trap(
                "1 div_s 0 traps",
                bin32(op::I32_DIV_S, 1, 0),
                &[trap::DIVIDE_BY_ZERO],
            ),
            case_trap(
                "1 div_u 0 traps",
                bin32(op::I32_DIV_U, 1, 0),
                &[trap::DIVIDE_BY_ZERO],
            ),
            case_trap(
                "1 rem_s 0 traps — a remainder by zero is no more defined than a quotient",
                bin32(op::I32_REM_S, 1, 0),
                &[trap::DIVIDE_BY_ZERO],
            ),
            case_trap(
                "1 rem_u 0 traps",
                bin32(op::I32_REM_U, 1, 0),
                &[trap::DIVIDE_BY_ZERO],
            ),
            case_trap(
                "0 div_s 0 traps: zero over zero is not zero",
                bin32(op::I32_DIV_S, 0, 0),
                &[trap::DIVIDE_BY_ZERO],
            ),
            case_trap(
                "i32::MIN div_s 0 traps for the divisor, not for the overflow",
                bin32(op::I32_DIV_S, i32::MIN, 0),
                &[trap::DIVIDE_BY_ZERO],
            ),
            trap64(
                "1 div_s 0 traps at sixty-four bits",
                bin64(op::I64_DIV_S, 1, 0),
                &[trap::DIVIDE_BY_ZERO],
            ),
            trap64(
                "1 div_u 0 traps at sixty-four bits",
                bin64(op::I64_DIV_U, 1, 0),
                &[trap::DIVIDE_BY_ZERO],
            ),
            trap64(
                "1 rem_s 0 traps at sixty-four bits",
                bin64(op::I64_REM_S, 1, 0),
                &[trap::DIVIDE_BY_ZERO],
            ),
            trap64(
                "1 rem_u 0 traps at sixty-four bits",
                bin64(op::I64_REM_U, 1, 0),
                &[trap::DIVIDE_BY_ZERO],
            ),
        ],
    )
});

wasm_test!(overflow_trap, |ctx| {
    run_cases(
        ctx,
        "division-overflow",
        vec![
            case_trap(
                "i32::MIN div_s -1 traps: the true quotient is 2147483648, one past the end",
                bin32(op::I32_DIV_S, i32::MIN, -1),
                &[trap::INTEGER_OVERFLOW],
            ),
            trap64(
                "i64::MIN div_s -1 traps for the same reason",
                bin64(op::I64_DIV_S, i64::MIN, -1),
                &[trap::INTEGER_OVERFLOW],
            ),
            case_i32(
                "i32::MIN div_u -1 is 0 and does not trap: unsigned division has no overflow",
                bin32(op::I32_DIV_U, i32::MIN, -1),
                0,
            ),
            case_i64(
                "i64::MIN div_u -1 is 0 and does not trap either",
                bin64(op::I64_DIV_U, i64::MIN, -1),
                0,
            ),
            case_i32(
                "i32::MIN div_s 1 is i32::MIN: only the -1 divisor overflows",
                bin32(op::I32_DIV_S, i32::MIN, 1),
                i32::MIN,
            ),
            case_i32(
                "(i32::MIN + 1) div_s -1 is 2147483647, the largest quotient that fits",
                bin32(op::I32_DIV_S, i32::MIN + 1, -1),
                i32::MAX,
            ),
            case_i32(
                "i32::MAX div_s -1 is -2147483647 and does not trap",
                bin32(op::I32_DIV_S, i32::MAX, -1),
                -2147483647,
            ),
            case_i64(
                "(i64::MIN + 1) div_s -1 is i64::MAX",
                bin64(op::I64_DIV_S, i64::MIN + 1, -1),
                i64::MAX,
            ),
        ],
    )
});

wasm_test!(rem_of_min_is_not_a_trap, |ctx| {
    run_cases(
        ctx,
        "rem-of-min",
        vec![
            case_i32(
                "i32::MIN rem_s -1 is 0 — the remainder is defined even where the quotient is not",
                bin32(op::I32_REM_S, i32::MIN, -1),
                0,
            ),
            case_i64(
                "i64::MIN rem_s -1 is 0 as well",
                bin64(op::I64_REM_S, i64::MIN, -1),
                0,
            ),
            case_trap(
                "i32::MIN div_s -1 does trap, which is the whole point of the pair",
                bin32(op::I32_DIV_S, i32::MIN, -1),
                &[trap::INTEGER_OVERFLOW],
            ),
            case_i32(
                "i32::MIN rem_u -1 is i32::MIN, and does not trap either",
                bin32(op::I32_REM_U, i32::MIN, -1),
                i32::MIN,
            ),
            case_i64(
                "i64::MIN rem_u -1 is i64::MIN",
                bin64(op::I64_REM_U, i64::MIN, -1),
                i64::MIN,
            ),
            case_i32(
                "(i32::MIN + 1) rem_s -1 is 0",
                bin32(op::I32_REM_S, i32::MIN + 1, -1),
                0,
            ),
            case_i32(
                "i32::MAX rem_s -1 is 0",
                bin32(op::I32_REM_S, i32::MAX, -1),
                0,
            ),
            case_i32(
                "i32::MIN rem_s -1 can be computed twice running, so it left no fault behind",
                bin32(op::I32_REM_S, i32::MIN, -1)
                    .i32_const(i32::MIN)
                    .i32_const(-1)
                    .op(op::I32_REM_S)
                    .op(op::I32_ADD),
                0,
            ),
        ],
    )
});

/// `(a / b) * b + (a % b)`, which must be `a` again for every pair that does not trap.
fn reconstruct32(div: Op, rem: Op, a: i32, b: i32) -> Expr {
    Expr::new()
        .i32_const(a)
        .i32_const(b)
        .op(div)
        .i32_const(b)
        .op(op::I32_MUL)
        .i32_const(a)
        .i32_const(b)
        .op(rem)
        .op(op::I32_ADD)
}

/// The same identity at sixty-four bits.
fn reconstruct64(div: Op, rem: Op, a: i64, b: i64) -> Expr {
    Expr::new()
        .i64_const(a)
        .i64_const(b)
        .op(div)
        .i64_const(b)
        .op(op::I64_MUL)
        .i64_const(a)
        .i64_const(b)
        .op(rem)
        .op(op::I64_ADD)
}

wasm_test!(reconstruction, |ctx| {
    run_cases(
        ctx,
        "division-identity",
        vec![
            case_i32(
                "(7 / 2) * 2 + (7 % 2) is 7",
                reconstruct32(op::I32_DIV_S, op::I32_REM_S, 7, 2),
                7,
            ),
            case_i32(
                "the identity holds for -7 / 2, which is where truncation and sign meet",
                reconstruct32(op::I32_DIV_S, op::I32_REM_S, -7, 2),
                -7,
            ),
            case_i32(
                "and for 7 / -2",
                reconstruct32(op::I32_DIV_S, op::I32_REM_S, 7, -2),
                7,
            ),
            case_i32(
                "and for -7 / -2",
                reconstruct32(op::I32_DIV_S, op::I32_REM_S, -7, -2),
                -7,
            ),
            case_i32(
                "it holds at i32::MIN over 3",
                reconstruct32(op::I32_DIV_S, op::I32_REM_S, i32::MIN, 3),
                i32::MIN,
            ),
            case_i32(
                "it holds at i32::MAX over 7",
                reconstruct32(op::I32_DIV_S, op::I32_REM_S, i32::MAX, 7),
                i32::MAX,
            ),
            case_i32(
                "the unsigned pair reconstructs -1 read as 4294967295",
                reconstruct32(op::I32_DIV_U, op::I32_REM_U, -1, 3),
                -1,
            ),
            case_i32(
                "and reconstructs i32::MIN read as 2147483648",
                reconstruct32(op::I32_DIV_U, op::I32_REM_U, i32::MIN, 7),
                i32::MIN,
            ),
            case_i64(
                "the signed identity holds at sixty-four bits",
                reconstruct64(op::I64_DIV_S, op::I64_REM_S, i64::MIN, 3),
                i64::MIN,
            ),
            case_i64(
                "and the unsigned one does too",
                reconstruct64(op::I64_DIV_U, op::I64_REM_U, -1, 3),
                -1,
            ),
        ],
    )
});

/// A `() -> i32` function that does some real arithmetic, throws it away, and then divides
/// by `divisor`. Nothing it computed can ever be seen: the result is printed by the caller
/// once the call returns, and a trap means it does not.
fn traps_after_work(label: &str, dividend: i32, divisor: i32) -> Module {
    let mut b = ModuleBuilder::new(label);
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(
        ty,
        Func::new(
            Expr::new()
                .i32_const(111)
                .i32_const(222)
                .op(op::I32_ADD)
                .drop()
                .i32_const(dividend)
                .i32_const(divisor)
                .op(op::I32_DIV_S),
        ),
    );
    b.export_func("f", idx).build()
}

wasm_test!(nothing_is_printed, |ctx| {
    for (label, dividend, divisor, reason) in [
        ("div-by-zero-after-work", 1, 0, trap::DIVIDE_BY_ZERO),
        ("overflow-after-work", i32::MIN, -1, trap::INTEGER_OVERFLOW),
    ] {
        let m = traps_after_work(label, dividend, divisor);
        let run = expect_trap(ctx, &m, "f", &[], reason)?;
        let mut c = Check::new(
            format!("that '{}' printed nothing before it trapped", m.label),
            &run,
        );
        c.module(&m);
        c.eq("stdout", "", run.stdout.as_str());
        c.note(
            "the function added 111 and 222 successfully first; a trap is not a return, so \
             the caller has nothing to print",
        );
        c.finish()?;
    }
    ctx.note("both traps left stdout completely empty");
    Ok(())
});

/// The module behind the "rem_s of the most negative value" example.
fn min_rem_module() -> Module {
    let mut b = ModuleBuilder::new("i32-min-rem-minus-one");
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(
        ty,
        Func::new(
            Expr::new()
                .i32_const(i32::MIN)
                .i32_const(-1)
                .op(op::I32_REM_S),
        ),
    );
    b.export_func("f", idx).build()
}

/// The module behind the "div_s of the most negative value" example.
fn min_div_module() -> Module {
    let mut b = ModuleBuilder::new("i32-min-div-minus-one");
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(
        ty,
        Func::new(
            Expr::new()
                .i32_const(i32::MIN)
                .i32_const(-1)
                .op(op::I32_DIV_S),
        ),
    );
    b.export_func("f", idx).build()
}

/// Worked examples: the pair that differ, side by side.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("The division that traps", min_div_module)
            .summary("`f() -> i32` returning `i32.const -2147483648, i32.const -1, i32.div_s`")
            .command("run --invoke f mod.wasm")
            .output(
                "nothing on stdout, `wasm trap: integer overflow` on stderr, and a non-zero \
                 exit status",
            )
            .note(
                "The quotient would be 2147483648, one past i32::MAX. This is the only \
                 overflow trap integer arithmetic has; add, sub and mul all wrap instead.",
            ),
        ExampleSpec::module("The remainder that does not", min_rem_module)
            .summary("`f() -> i32` returning `i32.const -2147483648, i32.const -1, i32.rem_s`")
            .command("run --invoke f mod.wasm")
            .output("0")
            .note(
                "Same two operands, same width, no trap: the remainder is 0. Implement \
                 rem_s without going through div_s, and special-case a divisor of -1 before \
                 you hand the pair to a hardware divide instruction, which would fault.",
            ),
    ]
}
