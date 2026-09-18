//! Stage 18 — f32 and f64 arithmetic.
//!
//! Two things about this stage are worth saying out loud before the tests are read.
//!
//! **The printed form.** A float result comes back the way Rust's `Display` writes it —
//! `1.5`, `-0`, `inf`, `-inf`, `NaN` — and that formatter never uses exponent notation, so
//! `f32::MAX` really does arrive as thirty-nine digits and the smallest subnormal as a
//! decimal point followed by forty-four zeros and a 1. It is the shortest decimal that reads
//! back as the same float, which is exactly what `show_f32` / `show_f64` produce, so the
//! expectations here are generated rather than typed. Even so, every case in this stage was
//! chosen so that its printed form is not in doubt: the operands are exact in binary
//! floating point (halves, quarters, powers of two) and so are the answers. Where an answer
//! is not exact — `sqrt(2)`, `0.1 + 0.2` — the test compares **bits** through
//! `i32.reinterpret_f32` / `i64.reinterpret_f64`, or asks a comparison instruction for a
//! 0/1, and never trusts a decimal expansion.
//!
//! **`0.1 + 0.2` is only famous at one width.** The classic `0.1 + 0.2 != 0.3` holds for
//! `f64`, and the stage asserts it with `f64.eq` (0) and `f64.ne` (1). It does **not** hold
//! for `f32`: at single precision the rounding happens to land on the same value, and
//! `f32.eq` answers 1. That case is kept in the sweep precisely because it is the one a
//! learner would "fix" by copying the f64 expectation across.
//!
//! The one genuinely surprising rule in here is that `ceil(-0.5)` is `-0` and not `0` —
//! rounding towards positive infinity from below zero lands on the negative zero, and the
//! two are different bit patterns even though they compare equal. Because `0` and `-0`
//! compare equal, that test goes through `reinterpret` as well as through the printed form.

use crate::examples::ExampleSpec;
use crate::stages::{case_f32, case_f64, case_i32, case_i64, f_f32, run_cases, Stage, Test};
use crate::wasm::{op, Expr, Module};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 18,
        slug: "float_arithmetic",
        name: "f32 and f64 arithmetic",
        ext: false,
        hints: &[
            "f32.const and f64.const carry four and eight raw little-endian bytes, not a LEB128: read them with from_le_bytes and never sign-extend them",
            "Do every f32 operation at f32 width — computing in f64 and rounding once at the end gives a different answer for add, sub, mul, div and sqrt, and double rounding is how it shows up",
            "ceil, floor and trunc all return a float, not an integer, and ceil(-0.5) is -0: keep the sign bit rather than normalising every zero to +0",
            "Dividing a float by zero is not a trap — it is +inf, -inf or NaN depending on the signs, and only the integer divisions trap",
        ],
        examples,
        tests: vec![
            Test::new("add, sub, mul and div of exactly representable f32 values", f32_arith),
            Test::new("the same four operations at f64 width", f64_arith),
            Test::new(
                "0.1 + 0.2 is not 0.3 at f64 width, and a comparison is the only honest way to say so",
                inexactness,
            ),
            Test::new("sqrt is exact on a perfect square and has the hardware's bits on 2", sqrt),
            Test::new("abs and neg move the sign bit and nothing else", abs_and_neg),
            Test::new("ceil, floor and trunc round the way their names say", rounding),
            Test::new("ceil of a small negative is negative zero, not zero", negative_zero),
            Test::new("dividing a float by zero gives an infinity or a NaN, never a trap", div_by_zero),
            Test::new("demote loses precision, promote never does", demote_and_promote),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Arithmetic on values that are exact in binary floating point
// ---------------------------------------------------------------------------------------

wasm_test!(f32_arith, |ctx| {
    run_cases(
        ctx,
        "f32-arith",
        vec![
            case_f32(
                "0.5 + 0.25",
                Expr::new().f32_const(0.5).f32_const(0.25).op(op::F32_ADD),
                0.75,
            ),
            case_f32(
                "1.5 + 1.5",
                Expr::new().f32_const(1.5).f32_const(1.5).op(op::F32_ADD),
                3.0,
            ),
            case_f32(
                "2.5 + -1.25",
                Expr::new().f32_const(2.5).f32_const(-1.25).op(op::F32_ADD),
                1.25,
            ),
            case_f32(
                "3.25 - 0.5",
                Expr::new().f32_const(3.25).f32_const(0.5).op(op::F32_SUB),
                2.75,
            ),
            case_f32(
                "1.5 - 3.25 is negative",
                Expr::new().f32_const(1.5).f32_const(3.25).op(op::F32_SUB),
                -1.75,
            ),
            case_f32(
                "1.5 * 0.25",
                Expr::new().f32_const(1.5).f32_const(0.25).op(op::F32_MUL),
                0.375,
            ),
            case_f32(
                "3 * 0.5",
                Expr::new().f32_const(3.0).f32_const(0.5).op(op::F32_MUL),
                1.5,
            ),
            case_f32(
                "3 / 0.5 — dividing by a half doubles",
                Expr::new().f32_const(3.0).f32_const(0.5).op(op::F32_DIV),
                6.0,
            ),
            case_f32(
                "1 / 4",
                Expr::new().f32_const(1.0).f32_const(4.0).op(op::F32_DIV),
                0.25,
            ),
            case_f32(
                "16777216 + 1 stays 16777216 — 2^24 is where f32 runs out of integers",
                Expr::new()
                    .f32_const(16_777_216.0)
                    .f32_const(1.0)
                    .op(op::F32_ADD),
                16_777_216.0,
            ),
        ],
    )
});

wasm_test!(f64_arith, |ctx| {
    run_cases(
        ctx,
        "f64-arith",
        vec![
            case_f64(
                "0.5 + 0.25",
                Expr::new().f64_const(0.5).f64_const(0.25).op(op::F64_ADD),
                0.75,
            ),
            case_f64(
                "1.5 + 1.5",
                Expr::new().f64_const(1.5).f64_const(1.5).op(op::F64_ADD),
                3.0,
            ),
            case_f64(
                "1.5 - 3.25 is negative",
                Expr::new().f64_const(1.5).f64_const(3.25).op(op::F64_SUB),
                -1.75,
            ),
            case_f64(
                "3.25 - 0.5",
                Expr::new().f64_const(3.25).f64_const(0.5).op(op::F64_SUB),
                2.75,
            ),
            case_f64(
                "1.5 * 0.25",
                Expr::new().f64_const(1.5).f64_const(0.25).op(op::F64_MUL),
                0.375,
            ),
            case_f64(
                "-2.5 * 4",
                Expr::new().f64_const(-2.5).f64_const(4.0).op(op::F64_MUL),
                -10.0,
            ),
            case_f64(
                "3 / 0.5",
                Expr::new().f64_const(3.0).f64_const(0.5).op(op::F64_DIV),
                6.0,
            ),
            case_f64(
                "1 / 4",
                Expr::new().f64_const(1.0).f64_const(4.0).op(op::F64_DIV),
                0.25,
            ),
            case_f64(
                "9007199254740992 + 1 stays put — 2^53 is where f64 runs out of integers",
                Expr::new()
                    .f64_const(9_007_199_254_740_992.0)
                    .f64_const(1.0)
                    .op(op::F64_ADD),
                9_007_199_254_740_992.0,
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// Inexactness, asserted through comparisons rather than through decimal digits
// ---------------------------------------------------------------------------------------

/// `0.1 + 0.2` at f64 width, ready for a comparison instruction.
fn f64_tenth_plus_fifth() -> Expr {
    Expr::new().f64_const(0.1).f64_const(0.2).op(op::F64_ADD)
}

wasm_test!(inexactness, |ctx| {
    run_cases(
        ctx,
        "f64-inexactness",
        vec![
            case_i32(
                "f64: 0.1 + 0.2 == 0.3 is false",
                f64_tenth_plus_fifth().f64_const(0.3).op(op::F64_EQ),
                0,
            ),
            case_i32(
                "f64: 0.1 + 0.2 != 0.3 is true",
                f64_tenth_plus_fifth().f64_const(0.3).op(op::F64_NE),
                1,
            ),
            case_i32(
                "f64: the sum lands above 0.3, not below it",
                f64_tenth_plus_fifth().f64_const(0.3).op(op::F64_GT),
                1,
            ),
            case_i32(
                "f64: 0.5 + 0.25 == 0.75 is true — halves and quarters are exact",
                Expr::new()
                    .f64_const(0.5)
                    .f64_const(0.25)
                    .op(op::F64_ADD)
                    .f64_const(0.75)
                    .op(op::F64_EQ),
                1,
            ),
            case_i32(
                "f32: 0.1 + 0.2 == 0.3 is true — the famous inequality is an f64 fact",
                Expr::new()
                    .f32_const(0.1)
                    .f32_const(0.2)
                    .op(op::F32_ADD)
                    .f32_const(0.3)
                    .op(op::F32_EQ),
                1,
            ),
            case_i32(
                "f32: 16777216 + 1 == 16777216",
                Expr::new()
                    .f32_const(16_777_216.0)
                    .f32_const(1.0)
                    .op(op::F32_ADD)
                    .f32_const(16_777_216.0)
                    .op(op::F32_EQ),
                1,
            ),
            case_i32(
                "f64: 9007199254740992 + 1 == 9007199254740992",
                Expr::new()
                    .f64_const(9_007_199_254_740_992.0)
                    .f64_const(1.0)
                    .op(op::F64_ADD)
                    .f64_const(9_007_199_254_740_992.0)
                    .op(op::F64_EQ),
                1,
            ),
            case_i32(
                "f64: (1/3) * 3 == 1 — this one rounds back, unlike the last two",
                Expr::new()
                    .f64_const(1.0)
                    .f64_const(3.0)
                    .op(op::F64_DIV)
                    .f64_const(3.0)
                    .op(op::F64_MUL)
                    .f64_const(1.0)
                    .op(op::F64_EQ),
                1,
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// sqrt
// ---------------------------------------------------------------------------------------

wasm_test!(sqrt, |ctx| {
    run_cases(
        ctx,
        "sqrt",
        vec![
            case_f32(
                "f32: sqrt(6.25) is exactly 2.5",
                Expr::new().f32_const(6.25).op(op::F32_SQRT),
                2.5,
            ),
            case_f32(
                "f32: sqrt(4) is exactly 2",
                Expr::new().f32_const(4.0).op(op::F32_SQRT),
                2.0,
            ),
            case_f32(
                "f32: sqrt(0) is 0",
                Expr::new().f32_const(0.0).op(op::F32_SQRT),
                0.0,
            ),
            case_f64(
                "f64: sqrt(6.25) is exactly 2.5",
                Expr::new().f64_const(6.25).op(op::F64_SQRT),
                2.5,
            ),
            case_f64(
                "f64: sqrt(1) is 1",
                Expr::new().f64_const(1.0).op(op::F64_SQRT),
                1.0,
            ),
            case_i32(
                "f32: sqrt(2) has the bits 0x3fb504f3",
                Expr::new()
                    .f32_const(2.0)
                    .op(op::F32_SQRT)
                    .op(op::I32_REINTERPRET_F32),
                0x3fb5_04f3u32 as i32,
            ),
            case_i64(
                "f64: sqrt(2) has the bits 0x3ff6a09e667f3bcd",
                Expr::new()
                    .f64_const(2.0)
                    .op(op::F64_SQRT)
                    .op(op::I64_REINTERPRET_F64),
                0x3ff6_a09e_667f_3bcdu64 as i64,
            ),
            case_i32(
                "f64: sqrt(2) squared is not 2 again",
                Expr::new()
                    .f64_const(2.0)
                    .op(op::F64_SQRT)
                    .f64_const(2.0)
                    .op(op::F64_SQRT)
                    .op(op::F64_MUL)
                    .f64_const(2.0)
                    .op(op::F64_EQ),
                0,
            ),
            case_i32(
                "f32: sqrt(-0) is -0, sign bit and all",
                Expr::new()
                    .f32_const(-0.0)
                    .op(op::F32_SQRT)
                    .op(op::I32_REINTERPRET_F32),
                i32::MIN,
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// abs and neg
// ---------------------------------------------------------------------------------------

wasm_test!(abs_and_neg, |ctx| {
    run_cases(
        ctx,
        "abs-neg",
        vec![
            case_f32(
                "f32: abs(-3.25)",
                Expr::new().f32_const(-3.25).op(op::F32_ABS),
                3.25,
            ),
            case_f32(
                "f32: abs(3.25) leaves it alone",
                Expr::new().f32_const(3.25).op(op::F32_ABS),
                3.25,
            ),
            case_f32(
                "f32: neg(3.25)",
                Expr::new().f32_const(3.25).op(op::F32_NEG),
                -3.25,
            ),
            case_f32(
                "f32: neg(-3.25) is positive again",
                Expr::new().f32_const(-3.25).op(op::F32_NEG),
                3.25,
            ),
            case_f32(
                "f32: abs(-inf) is inf",
                Expr::new().f32_const(f32::NEG_INFINITY).op(op::F32_ABS),
                f32::INFINITY,
            ),
            case_f32(
                "f32: neg(inf) is -inf",
                Expr::new().f32_const(f32::INFINITY).op(op::F32_NEG),
                f32::NEG_INFINITY,
            ),
            case_i32(
                "f32: abs(-0) clears the sign bit",
                Expr::new()
                    .f32_const(-0.0)
                    .op(op::F32_ABS)
                    .op(op::I32_REINTERPRET_F32),
                0,
            ),
            case_i32(
                "f32: neg(0) sets it",
                Expr::new()
                    .f32_const(0.0)
                    .op(op::F32_NEG)
                    .op(op::I32_REINTERPRET_F32),
                i32::MIN,
            ),
            case_f64(
                "f64: abs(-3.25)",
                Expr::new().f64_const(-3.25).op(op::F64_ABS),
                3.25,
            ),
            case_f64(
                "f64: neg(3.25)",
                Expr::new().f64_const(3.25).op(op::F64_NEG),
                -3.25,
            ),
            case_i64(
                "f64: abs(-0) clears the sign bit",
                Expr::new()
                    .f64_const(-0.0)
                    .op(op::F64_ABS)
                    .op(op::I64_REINTERPRET_F64),
                0,
            ),
            case_i64(
                "f64: neg(0) sets it",
                Expr::new()
                    .f64_const(0.0)
                    .op(op::F64_NEG)
                    .op(op::I64_REINTERPRET_F64),
                i64::MIN,
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// ceil, floor, trunc
// ---------------------------------------------------------------------------------------

wasm_test!(rounding, |ctx| {
    run_cases(
        ctx,
        "ceil-floor-trunc",
        vec![
            case_f32(
                "f32: ceil(1.7) is 2",
                Expr::new().f32_const(1.7).op(op::F32_CEIL),
                2.0,
            ),
            case_f32(
                "f32: ceil(-1.7) is -1, towards positive infinity",
                Expr::new().f32_const(-1.7).op(op::F32_CEIL),
                -1.0,
            ),
            case_f32(
                "f32: floor(1.7) is 1",
                Expr::new().f32_const(1.7).op(op::F32_FLOOR),
                1.0,
            ),
            case_f32(
                "f32: floor(-1.7) is -2, towards negative infinity",
                Expr::new().f32_const(-1.7).op(op::F32_FLOOR),
                -2.0,
            ),
            case_f32(
                "f32: trunc(1.7) is 1",
                Expr::new().f32_const(1.7).op(op::F32_TRUNC),
                1.0,
            ),
            case_f32(
                "f32: trunc(-1.7) is -1, towards zero",
                Expr::new().f32_const(-1.7).op(op::F32_TRUNC),
                -1.0,
            ),
            case_f32(
                "f32: ceil(3) leaves an integral value alone",
                Expr::new().f32_const(3.0).op(op::F32_CEIL),
                3.0,
            ),
            case_f32(
                "f32: floor(-3) leaves an integral value alone",
                Expr::new().f32_const(-3.0).op(op::F32_FLOOR),
                -3.0,
            ),
            case_f64(
                "f64: ceil(1.7) is 2",
                Expr::new().f64_const(1.7).op(op::F64_CEIL),
                2.0,
            ),
            case_f64(
                "f64: ceil(-1.7) is -1",
                Expr::new().f64_const(-1.7).op(op::F64_CEIL),
                -1.0,
            ),
            case_f64(
                "f64: floor(-1.7) is -2",
                Expr::new().f64_const(-1.7).op(op::F64_FLOOR),
                -2.0,
            ),
            case_f64(
                "f64: trunc(-1.7) is -1",
                Expr::new().f64_const(-1.7).op(op::F64_TRUNC),
                -1.0,
            ),
            case_f64(
                "f64: trunc(3) leaves an integral value alone",
                Expr::new().f64_const(3.0).op(op::F64_TRUNC),
                3.0,
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// The negative zero the rounding operations can produce
// ---------------------------------------------------------------------------------------

wasm_test!(negative_zero, |ctx| {
    run_cases(
        ctx,
        "negative-zero",
        vec![
            case_f32("f32: ceil(-0.5) prints -0, not 0", ceil_minus_half(), -0.0),
            case_i32(
                "f32: ceil(-0.5) has the sign bit set",
                ceil_minus_half().op(op::I32_REINTERPRET_F32),
                i32::MIN,
            ),
            case_i32(
                "f32: trunc(-0.5) is -0 as well",
                Expr::new()
                    .f32_const(-0.5)
                    .op(op::F32_TRUNC)
                    .op(op::I32_REINTERPRET_F32),
                i32::MIN,
            ),
            case_f32(
                "f32: floor(-0.5) is -1, so the two do differ here",
                Expr::new().f32_const(-0.5).op(op::F32_FLOOR),
                -1.0,
            ),
            case_i32(
                "f32: ceil(-0) keeps the sign it was given",
                Expr::new()
                    .f32_const(-0.0)
                    .op(op::F32_CEIL)
                    .op(op::I32_REINTERPRET_F32),
                i32::MIN,
            ),
            case_i64(
                "f64: ceil(-0.5) is -0",
                Expr::new()
                    .f64_const(-0.5)
                    .op(op::F64_CEIL)
                    .op(op::I64_REINTERPRET_F64),
                i64::MIN,
            ),
            case_i64(
                "f64: trunc(-0.9) is -0",
                Expr::new()
                    .f64_const(-0.9)
                    .op(op::F64_TRUNC)
                    .op(op::I64_REINTERPRET_F64),
                i64::MIN,
            ),
            case_i32(
                "f32: -0 + -0 stays -0",
                Expr::new()
                    .f32_const(-0.0)
                    .f32_const(-0.0)
                    .op(op::F32_ADD)
                    .op(op::I32_REINTERPRET_F32),
                i32::MIN,
            ),
            case_i32(
                "f32: -0 + 0 is +0 — a sum of opposite zeros rounds to positive",
                Expr::new()
                    .f32_const(-0.0)
                    .f32_const(0.0)
                    .op(op::F32_ADD)
                    .op(op::I32_REINTERPRET_F32),
                0,
            ),
            case_i32(
                "f32: 1.5 - 1.5 is +0, not -0",
                Expr::new()
                    .f32_const(1.5)
                    .f32_const(1.5)
                    .op(op::F32_SUB)
                    .op(op::I32_REINTERPRET_F32),
                0,
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// Division by zero
// ---------------------------------------------------------------------------------------

wasm_test!(div_by_zero, |ctx| {
    run_cases(
        ctx,
        "float-div-by-zero",
        vec![
            case_f32(
                "f32: 1 / 0 is inf, and nothing traps",
                Expr::new().f32_const(1.0).f32_const(0.0).op(op::F32_DIV),
                f32::INFINITY,
            ),
            case_f32(
                "f32: -1 / 0 is -inf",
                Expr::new().f32_const(-1.0).f32_const(0.0).op(op::F32_DIV),
                f32::NEG_INFINITY,
            ),
            case_f32(
                "f32: 1 / -0 is -inf — the divisor's sign counts",
                Expr::new().f32_const(1.0).f32_const(-0.0).op(op::F32_DIV),
                f32::NEG_INFINITY,
            ),
            case_f32(
                "f32: -1 / -0 is inf",
                Expr::new().f32_const(-1.0).f32_const(-0.0).op(op::F32_DIV),
                f32::INFINITY,
            ),
            case_f32(
                "f32: 0 / 0 is NaN",
                Expr::new().f32_const(0.0).f32_const(0.0).op(op::F32_DIV),
                f32::NAN,
            ),
            case_f32(
                "f32: -0 / 0 is NaN too",
                Expr::new().f32_const(-0.0).f32_const(0.0).op(op::F32_DIV),
                f32::NAN,
            ),
            case_f64(
                "f64: 3.25 / 0 is inf",
                Expr::new().f64_const(3.25).f64_const(0.0).op(op::F64_DIV),
                f64::INFINITY,
            ),
            case_f64(
                "f64: -1 / 0 is -inf",
                Expr::new().f64_const(-1.0).f64_const(0.0).op(op::F64_DIV),
                f64::NEG_INFINITY,
            ),
            case_f64(
                "f64: 0 / 0 is NaN",
                Expr::new().f64_const(0.0).f64_const(0.0).op(op::F64_DIV),
                f64::NAN,
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// demote and promote
// ---------------------------------------------------------------------------------------

wasm_test!(demote_and_promote, |ctx| {
    run_cases(
        ctx,
        "demote-promote",
        vec![
            case_f32(
                "demote(1.5) is 1.5 — exact both ways",
                Expr::new().f64_const(1.5).op(op::F32_DEMOTE_F64),
                1.5,
            ),
            case_f32(
                "demote(3.25) is 3.25",
                Expr::new().f64_const(3.25).op(op::F32_DEMOTE_F64),
                3.25,
            ),
            case_f64(
                "promote(1.5) is 1.5",
                Expr::new().f32_const(1.5).op(op::F64_PROMOTE_F32),
                1.5,
            ),
            case_i32(
                "demote then promote of 3.25 is still 3.25",
                Expr::new()
                    .f64_const(3.25)
                    .op(op::F32_DEMOTE_F64)
                    .op(op::F64_PROMOTE_F32)
                    .f64_const(3.25)
                    .op(op::F64_EQ),
                1,
            ),
            case_i32(
                "demote then promote of 0.1 is not 0.1 — the trip down loses bits",
                Expr::new()
                    .f64_const(0.1)
                    .op(op::F32_DEMOTE_F64)
                    .op(op::F64_PROMOTE_F32)
                    .f64_const(0.1)
                    .op(op::F64_EQ),
                0,
            ),
            case_i32(
                "demote(0.1) has the bits of the f32 nearest 0.1",
                Expr::new()
                    .f64_const(0.1)
                    .op(op::F32_DEMOTE_F64)
                    .op(op::I32_REINTERPRET_F32),
                0x3dcc_cccdu32 as i32,
            ),
            case_i64(
                "promote(0.1f32) has the bits of that f32, not of 0.1",
                Expr::new()
                    .f32_const(0.1)
                    .op(op::F64_PROMOTE_F32)
                    .op(op::I64_REINTERPRET_F64),
                0x3fb9_9999_a000_0000u64 as i64,
            ),
            case_f32(
                "demote(1e300) overflows to inf rather than trapping",
                Expr::new().f64_const(1e300).op(op::F32_DEMOTE_F64),
                f32::INFINITY,
            ),
            case_i32(
                "demote(1e-300) underflows to +0, sign bit clear",
                Expr::new()
                    .f64_const(1e-300)
                    .op(op::F32_DEMOTE_F64)
                    .op(op::I32_REINTERPRET_F32),
                0,
            ),
            case_i32(
                "promote is exact, so promoting sqrt(6.25) still gives 2.5",
                Expr::new()
                    .f32_const(6.25)
                    .op(op::F32_SQRT)
                    .op(op::F64_PROMOTE_F32)
                    .f64_const(2.5)
                    .op(op::F64_EQ),
                1,
            ),
        ],
    )
});

/// `f32.const -0.5` followed by `f32.ceil` — the body two cases share.
fn ceil_minus_half() -> Expr {
    Expr::new().f32_const(-0.5).op(op::F32_CEIL)
}

// ---------------------------------------------------------------------------------------
// Worked examples
// ---------------------------------------------------------------------------------------

/// `f() -> f32 { ceil(-0.5) }` — the module that prints `-0`.
fn ceil_example() -> Module {
    f_f32("ceil-neg-half", ceil_minus_half())
}

/// `f() -> f32 { 1 / 0 }` — an infinity, not a trap.
fn div_example() -> Module {
    f_f32(
        "one-over-zero",
        Expr::new().f32_const(1.0).f32_const(0.0).op(op::F32_DIV),
    )
}

/// Worked examples: the negative zero, and the division that does not trap.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("ceil of a small negative number", ceil_example)
            .summary("one function, `f() -> f32`, whose body is `f32.const -0.5` then `f32.ceil`")
            .command("run --invoke f mod.wasm")
            .output("-0")
            .note(
                "Rounding towards positive infinity from below zero lands on negative zero, \
                 and a runtime that normalises every zero to +0 prints `0` here. The two \
                 compare equal, so the only way to tell them apart is the sign bit — \
                 `i32.reinterpret_f32` gives 0x80000000 for one and 0 for the other.",
            ),
        ExampleSpec::module("One divided by zero", div_example)
            .summary("`f() -> f32` whose body is `f32.const 1`, `f32.const 0`, `f32.div`")
            .command("run --invoke f mod.wasm")
            .output("inf")
            .note(
                "Only the integer divisions trap. A float division by zero follows IEEE 754: \
                 +inf, -inf or NaN according to the signs of the two operands, printed as \
                 `inf`, `-inf` and `NaN`.",
            ),
    ]
}
