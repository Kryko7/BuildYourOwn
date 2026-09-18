//! Stage 19 — NaN, signed zeros, infinities, min/max and nearest.
//!
//! These are the rules nobody gets right the first time, and most of them cannot be seen in
//! the printed result at all. `0` and `-0` print differently but compare equal; every NaN,
//! whatever its sign and whatever its payload, prints as the bare word `NaN`. So wherever
//! the question is "which zero" or "which NaN", the test reinterprets the float as an
//! integer and compares bit patterns.
//!
//! **What the NaN tests check, and what they deliberately do not.** The spec pins the result
//! of an arithmetic operation with a NaN operand only loosely: it must be *a* NaN, and if it
//! comes from nowhere in particular it must be the canonical one, but a runtime is free to
//! pass an operand's payload through, and free to choose the sign. So the propagation tests
//! reinterpret the result and assert exactly two things by masking with `0x7fc00000` (f32)
//! or `0x7ff8000000000000` (f64):
//!
//! * every exponent bit is set — the value is an infinity or a NaN; and
//! * the top mantissa bit is set — it is a *quiet* NaN, not a signalling one.
//!
//! The remaining 22 (or 51) payload bits and the sign bit are the runtime's own business and
//! are never compared. For the record, `wasmtime` 48 keeps the operand's payload and only
//! forces the quiet bit on: `nan:0x1 + 1.0` comes back as `0x7fc00001`, and `-nan + 1.0`
//! keeps its sign. A runtime that answers the canonical `0x7fc00000` every time passes these
//! tests just as well, which is the point of the mask.
//!
//! `abs`, `neg` and `copysign` are the exception: the spec defines all three as pure sign-bit
//! operations, NaNs included, so those results *are* compared exactly.
//!
//! One more thing worth knowing before implementing `min`/`max`: they are not C's `fmin` and
//! `fmax`. C returns the non-NaN operand; WebAssembly returns a NaN if *either* operand is a
//! NaN. And on the two zeros they are ordered — `min(+0, -0)` is `-0` and `max(+0, -0)` is
//! `+0` — even though `+0 == -0`, so a naive `if a < b { a } else { b }` gets it wrong in
//! both directions.

use crate::examples::ExampleSpec;
use crate::stages::{case_f32, case_f64, case_i32, case_i64, f_f32, run_cases, Case, Stage, Test};
use crate::wasm::{ftype, op, Expr, Module, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 19,
        slug: "float_special_values",
        name: "NaN, signed zeros, infinities, min/max and nearest",
        ext: false,
        hints: &[
            "f32.min and f32.max are not fmin and fmax: if either operand is a NaN the result is a NaN, and min(+0, -0) is -0 while max(+0, -0) is +0",
            "f32.nearest rounds a tie to the even neighbour — 0.5 and 2.5 both become 2's neighbours 0 and 2 — so it is neither round() nor trunc(x + 0.5)",
            "copysign takes the magnitude of its first operand and the sign bit of its second, whatever that second operand is: a negative zero and a negative NaN both make the result negative",
            "Every comparison with a NaN is false except ne, which is true; keep that rule out of your min/max and sorting code",
        ],
        examples,
        tests: vec![
            Test::new("min and max answer a NaN when either operand is a NaN", min_max_nan),
            Test::new("min and max tell the two zeros apart", min_max_zero),
            Test::new("nearest rounds a tie to the even neighbour", nearest),
            Test::new("copysign takes the sign of its second operand", copysign),
            Test::new("abs and neg are sign-bit operations, on a NaN too", abs_neg_nan),
            Test::new("arithmetic on infinities", infinities),
            Test::new("an operation with a NaN operand produces a quiet NaN", nan_propagation),
            Test::new("every comparison with a NaN is false except ne", nan_comparisons),
            Test::new("the two zeros compare equal and have different bits", signed_zeros),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Bit patterns this stage names more than once
// ---------------------------------------------------------------------------------------

/// The f32 bits that are set in every quiet NaN: the eight exponent bits and the top
/// mantissa bit. Masking with this and comparing keeps the payload out of the assertion.
const F32_QUIET_MASK: i32 = 0x7fc0_0000u32 as i32;

/// The same for f64: eleven exponent bits and the top mantissa bit.
const F64_QUIET_MASK: i64 = 0x7ff8_0000_0000_0000u64 as i64;

/// A signalling f32 NaN: exponent all ones, payload 1, quiet bit **clear**.
const F32_SIGNALLING: u32 = 0x7f80_0001;

/// A signalling f64 NaN.
const F64_SIGNALLING: u64 = 0x7ff0_0000_0000_0001;

/// An f32 quiet NaN carrying a payload no runtime would invent on its own.
const F32_PAYLOAD_NAN: u32 = 0x7fe0_0001;

/// `<expr> ; i32.reinterpret_f32 ; i32.and MASK ; i32.const MASK ; i32.eq` — 1 when the
/// result is a quiet NaN, whatever its sign and payload.
fn is_quiet_f32(name: &str, value: Expr) -> Case {
    Case::new(
        name,
        ftype(&[], &[ValType::I32]),
        value
            .op(op::I32_REINTERPRET_F32)
            .i32_const(F32_QUIET_MASK)
            .op(op::I32_AND)
            .i32_const(F32_QUIET_MASK)
            .op(op::I32_EQ),
        &["1"],
    )
}

/// The f64 form of [`is_quiet_f32`].
fn is_quiet_f64(name: &str, value: Expr) -> Case {
    Case::new(
        name,
        ftype(&[], &[ValType::I32]),
        value
            .op(op::I64_REINTERPRET_F64)
            .i64_const(F64_QUIET_MASK)
            .op(op::I64_AND)
            .i64_const(F64_QUIET_MASK)
            .op(op::I64_EQ),
        &["1"],
    )
}

// ---------------------------------------------------------------------------------------
// min and max
// ---------------------------------------------------------------------------------------

wasm_test!(min_max_nan, |ctx| {
    run_cases(
        ctx,
        "min-max-nan",
        vec![
            case_f32(
                "f32: min(1.5, 3.25) is the smaller one",
                Expr::new().f32_const(1.5).f32_const(3.25).op(op::F32_MIN),
                1.5,
            ),
            case_f32(
                "f32: max(1.5, 3.25) is the larger one",
                Expr::new().f32_const(1.5).f32_const(3.25).op(op::F32_MAX),
                3.25,
            ),
            case_f32(
                "f32: min(-inf, 1.5) is -inf",
                Expr::new()
                    .f32_const(f32::NEG_INFINITY)
                    .f32_const(1.5)
                    .op(op::F32_MIN),
                f32::NEG_INFINITY,
            ),
            is_quiet_f32(
                "f32: min(NaN, 1.5) is a NaN, not 1.5 — this is where fmin differs",
                Expr::new()
                    .f32_bits(F32_PAYLOAD_NAN)
                    .f32_const(1.5)
                    .op(op::F32_MIN),
            ),
            is_quiet_f32(
                "f32: min(1.5, NaN) is a NaN whichever side it is on",
                Expr::new()
                    .f32_const(1.5)
                    .f32_bits(F32_PAYLOAD_NAN)
                    .op(op::F32_MIN),
            ),
            is_quiet_f32(
                "f32: max(1.5, NaN) is a NaN",
                Expr::new()
                    .f32_const(1.5)
                    .f32_bits(F32_PAYLOAD_NAN)
                    .op(op::F32_MAX),
            ),
            is_quiet_f32(
                "f32: max(NaN, -inf) is a NaN, not the only number in sight",
                Expr::new()
                    .f32_bits(F32_PAYLOAD_NAN)
                    .f32_const(f32::NEG_INFINITY)
                    .op(op::F32_MAX),
            ),
            case_f64(
                "f64: max(-2.5, -10) is -2.5",
                Expr::new().f64_const(-2.5).f64_const(-10.0).op(op::F64_MAX),
                -2.5,
            ),
            is_quiet_f64(
                "f64: min(NaN, 1.5) is a NaN",
                Expr::new()
                    .f64_bits(f64::NAN.to_bits())
                    .f64_const(1.5)
                    .op(op::F64_MIN),
            ),
            is_quiet_f64(
                "f64: max(1.5, NaN) is a NaN",
                Expr::new()
                    .f64_const(1.5)
                    .f64_bits(f64::NAN.to_bits())
                    .op(op::F64_MAX),
            ),
        ],
    )
});

wasm_test!(min_max_zero, |ctx| {
    run_cases(
        ctx,
        "min-max-zero",
        vec![
            case_i32(
                "f32: min(+0, -0) is -0",
                Expr::new()
                    .f32_const(0.0)
                    .f32_const(-0.0)
                    .op(op::F32_MIN)
                    .op(op::I32_REINTERPRET_F32),
                i32::MIN,
            ),
            case_i32(
                "f32: min(-0, +0) is -0 whichever way round they come",
                Expr::new()
                    .f32_const(-0.0)
                    .f32_const(0.0)
                    .op(op::F32_MIN)
                    .op(op::I32_REINTERPRET_F32),
                i32::MIN,
            ),
            case_i32(
                "f32: max(+0, -0) is +0",
                Expr::new()
                    .f32_const(0.0)
                    .f32_const(-0.0)
                    .op(op::F32_MAX)
                    .op(op::I32_REINTERPRET_F32),
                0,
            ),
            case_i32(
                "f32: max(-0, +0) is +0",
                Expr::new()
                    .f32_const(-0.0)
                    .f32_const(0.0)
                    .op(op::F32_MAX)
                    .op(op::I32_REINTERPRET_F32),
                0,
            ),
            case_i32(
                "f32: min(-0, -0) is -0",
                Expr::new()
                    .f32_const(-0.0)
                    .f32_const(-0.0)
                    .op(op::F32_MIN)
                    .op(op::I32_REINTERPRET_F32),
                i32::MIN,
            ),
            case_i32(
                "f32: the naive 'a < b ? a : b' would answer +0 here, because -0 < +0 is false",
                Expr::new().f32_const(-0.0).f32_const(0.0).op(op::F32_LT),
                0,
            ),
            case_i64(
                "f64: min(+0, -0) is -0",
                Expr::new()
                    .f64_const(0.0)
                    .f64_const(-0.0)
                    .op(op::F64_MIN)
                    .op(op::I64_REINTERPRET_F64),
                i64::MIN,
            ),
            case_i64(
                "f64: max(+0, -0) is +0",
                Expr::new()
                    .f64_const(0.0)
                    .f64_const(-0.0)
                    .op(op::F64_MAX)
                    .op(op::I64_REINTERPRET_F64),
                0,
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// nearest
// ---------------------------------------------------------------------------------------

wasm_test!(nearest, |ctx| {
    run_cases(
        ctx,
        "nearest",
        vec![
            case_i32(
                "f32: nearest(0.5) is +0 — the tie goes to the even neighbour, not up",
                Expr::new()
                    .f32_const(0.5)
                    .op(op::F32_NEAREST)
                    .op(op::I32_REINTERPRET_F32),
                0,
            ),
            case_f32(
                "f32: nearest(1.5) is 2",
                Expr::new().f32_const(1.5).op(op::F32_NEAREST),
                2.0,
            ),
            case_f32(
                "f32: nearest(2.5) is 2, not 3",
                Expr::new().f32_const(2.5).op(op::F32_NEAREST),
                2.0,
            ),
            case_f32(
                "f32: nearest(3.5) is 4",
                Expr::new().f32_const(3.5).op(op::F32_NEAREST),
                4.0,
            ),
            case_i32(
                "f32: nearest(-0.5) is -0, so trunc(x + 0.5) is wrong twice over",
                Expr::new()
                    .f32_const(-0.5)
                    .op(op::F32_NEAREST)
                    .op(op::I32_REINTERPRET_F32),
                i32::MIN,
            ),
            case_f32(
                "f32: nearest(-1.5) is -2",
                Expr::new().f32_const(-1.5).op(op::F32_NEAREST),
                -2.0,
            ),
            case_f32(
                "f32: nearest(-2.5) is -2",
                Expr::new().f32_const(-2.5).op(op::F32_NEAREST),
                -2.0,
            ),
            case_i32(
                "f32: nearest(0.49) is +0 — only an exact tie is decided by evenness",
                Expr::new()
                    .f32_const(0.49)
                    .op(op::F32_NEAREST)
                    .op(op::I32_REINTERPRET_F32),
                0,
            ),
            case_f32(
                "f32: nearest(0.51) is 1",
                Expr::new().f32_const(0.51).op(op::F32_NEAREST),
                1.0,
            ),
            case_f64(
                "f64: nearest(2.5) is 2",
                Expr::new().f64_const(2.5).op(op::F64_NEAREST),
                2.0,
            ),
            case_f64(
                "f64: nearest(-1.5) is -2",
                Expr::new().f64_const(-1.5).op(op::F64_NEAREST),
                -2.0,
            ),
            case_i64(
                "f64: nearest(-0.5) is -0",
                Expr::new()
                    .f64_const(-0.5)
                    .op(op::F64_NEAREST)
                    .op(op::I64_REINTERPRET_F64),
                i64::MIN,
            ),
            case_f64(
                "f64: nearest(3) leaves an integral value alone",
                Expr::new().f64_const(3.0).op(op::F64_NEAREST),
                3.0,
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// copysign
// ---------------------------------------------------------------------------------------

wasm_test!(copysign, |ctx| {
    run_cases(
        ctx,
        "copysign",
        vec![
            case_f32(
                "f32: copysign(1, -2) is -1 — the magnitude comes from the left",
                Expr::new()
                    .f32_const(1.0)
                    .f32_const(-2.0)
                    .op(op::F32_COPYSIGN),
                -1.0,
            ),
            case_f32(
                "f32: copysign(-1, 2) is 1",
                Expr::new()
                    .f32_const(-1.0)
                    .f32_const(2.0)
                    .op(op::F32_COPYSIGN),
                1.0,
            ),
            case_f32(
                "f32: copysign(-3.25, -0.5) stays negative",
                Expr::new()
                    .f32_const(-3.25)
                    .f32_const(-0.5)
                    .op(op::F32_COPYSIGN),
                -3.25,
            ),
            case_f32(
                "f32: copysign(1, -0) is -1 — a negative zero carries a sign like anything else",
                Expr::new()
                    .f32_const(1.0)
                    .f32_const(-0.0)
                    .op(op::F32_COPYSIGN),
                -1.0,
            ),
            case_f32(
                "f32: copysign(-1, +0) is 1",
                Expr::new()
                    .f32_const(-1.0)
                    .f32_const(0.0)
                    .op(op::F32_COPYSIGN),
                1.0,
            ),
            case_f32(
                "f32: copysign(1, -NaN) is -1 — the sign of a NaN is a real bit",
                Expr::new()
                    .f32_const(1.0)
                    .f32_bits(0xffc0_0000)
                    .op(op::F32_COPYSIGN),
                -1.0,
            ),
            case_f32(
                "f32: copysign(-1, +NaN) is 1",
                Expr::new()
                    .f32_const(-1.0)
                    .f32_bits(0x7fc0_0000)
                    .op(op::F32_COPYSIGN),
                1.0,
            ),
            case_i32(
                "f32: copysign(+0, -1) is -0",
                Expr::new()
                    .f32_const(0.0)
                    .f32_const(-1.0)
                    .op(op::F32_COPYSIGN)
                    .op(op::I32_REINTERPRET_F32),
                i32::MIN,
            ),
            case_f32(
                "f32: copysign(inf, -1) is -inf",
                Expr::new()
                    .f32_const(f32::INFINITY)
                    .f32_const(-1.0)
                    .op(op::F32_COPYSIGN),
                f32::NEG_INFINITY,
            ),
            case_f64(
                "f64: copysign(3.25, -1) is -3.25",
                Expr::new()
                    .f64_const(3.25)
                    .f64_const(-1.0)
                    .op(op::F64_COPYSIGN),
                -3.25,
            ),
            case_f64(
                "f64: copysign(-3.25, -0) stays negative",
                Expr::new()
                    .f64_const(-3.25)
                    .f64_const(-0.0)
                    .op(op::F64_COPYSIGN),
                -3.25,
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// abs and neg on a NaN — the one place a NaN's bits are pinned exactly
// ---------------------------------------------------------------------------------------

wasm_test!(abs_neg_nan, |ctx| {
    run_cases(
        ctx,
        "abs-neg-nan",
        vec![
            case_i32(
                "f32: abs of a negative NaN clears the sign and keeps every payload bit",
                Expr::new()
                    .f32_bits(0xffe0_0001)
                    .op(op::F32_ABS)
                    .op(op::I32_REINTERPRET_F32),
                0x7fe0_0001u32 as i32,
            ),
            case_i32(
                "f32: neg of a positive NaN sets the sign and keeps every payload bit",
                Expr::new()
                    .f32_bits(F32_PAYLOAD_NAN)
                    .op(op::F32_NEG)
                    .op(op::I32_REINTERPRET_F32),
                0xffe0_0001u32 as i32,
            ),
            case_i32(
                "f32: neg does not quieten a signalling NaN — it is a sign flip, not arithmetic",
                Expr::new()
                    .f32_bits(F32_SIGNALLING)
                    .op(op::F32_NEG)
                    .op(op::I32_REINTERPRET_F32),
                0xff80_0001u32 as i32,
            ),
            case_i32(
                "f32: abs does not quieten a signalling NaN either",
                Expr::new()
                    .f32_bits(0xff80_0001)
                    .op(op::F32_ABS)
                    .op(op::I32_REINTERPRET_F32),
                F32_SIGNALLING as i32,
            ),
            case_i32(
                "f32: neg twice is the identity, payload included",
                Expr::new()
                    .f32_bits(F32_PAYLOAD_NAN)
                    .op(op::F32_NEG)
                    .op(op::F32_NEG)
                    .op(op::I32_REINTERPRET_F32),
                F32_PAYLOAD_NAN as i32,
            ),
            case_i64(
                "f64: abs of a negative NaN clears the sign",
                Expr::new()
                    .f64_bits(0xfff8_0000_0000_0001)
                    .op(op::F64_ABS)
                    .op(op::I64_REINTERPRET_F64),
                0x7ff8_0000_0000_0001u64 as i64,
            ),
            case_i64(
                "f64: neg of a positive NaN sets it",
                Expr::new()
                    .f64_bits(0x7ff8_0000_0000_0001)
                    .op(op::F64_NEG)
                    .op(op::I64_REINTERPRET_F64),
                0xfff8_0000_0000_0001u64 as i64,
            ),
            case_i32(
                "f32: copysign is the third sign-bit operation, and it keeps the payload too",
                Expr::new()
                    .f32_bits(F32_PAYLOAD_NAN)
                    .f32_const(-1.0)
                    .op(op::F32_COPYSIGN)
                    .op(op::I32_REINTERPRET_F32),
                0xffe0_0001u32 as i32,
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// Infinities
// ---------------------------------------------------------------------------------------

wasm_test!(infinities, |ctx| {
    run_cases(
        ctx,
        "infinities",
        vec![
            is_quiet_f32(
                "f32: inf - inf is a NaN",
                Expr::new()
                    .f32_const(f32::INFINITY)
                    .f32_const(f32::INFINITY)
                    .op(op::F32_SUB),
            ),
            is_quiet_f32(
                "f32: inf * 0 is a NaN",
                Expr::new()
                    .f32_const(f32::INFINITY)
                    .f32_const(0.0)
                    .op(op::F32_MUL),
            ),
            is_quiet_f32(
                "f32: inf / inf is a NaN",
                Expr::new()
                    .f32_const(f32::INFINITY)
                    .f32_const(f32::INFINITY)
                    .op(op::F32_DIV),
            ),
            case_f32(
                "f32: inf + inf is still inf",
                Expr::new()
                    .f32_const(f32::INFINITY)
                    .f32_const(f32::INFINITY)
                    .op(op::F32_ADD),
                f32::INFINITY,
            ),
            case_f32(
                "f32: inf + 1 is inf — no finite number moves it",
                Expr::new()
                    .f32_const(f32::INFINITY)
                    .f32_const(1.0)
                    .op(op::F32_ADD),
                f32::INFINITY,
            ),
            case_i32(
                "f32: 1 / inf is +0",
                Expr::new()
                    .f32_const(1.0)
                    .f32_const(f32::INFINITY)
                    .op(op::F32_DIV)
                    .op(op::I32_REINTERPRET_F32),
                0,
            ),
            case_i32(
                "f32: -1 / inf is -0, and only the bits say so",
                Expr::new()
                    .f32_const(-1.0)
                    .f32_const(f32::INFINITY)
                    .op(op::F32_DIV)
                    .op(op::I32_REINTERPRET_F32),
                i32::MIN,
            ),
            case_i32(
                "f32: inf has the bits 0x7f800000 — every exponent bit, no mantissa bit",
                Expr::new()
                    .f32_const(f32::INFINITY)
                    .op(op::I32_REINTERPRET_F32),
                0x7f80_0000,
            ),
            case_f32(
                "f32: sqrt(inf) is inf",
                Expr::new().f32_const(f32::INFINITY).op(op::F32_SQRT),
                f32::INFINITY,
            ),
            is_quiet_f64(
                "f64: inf - inf is a NaN",
                Expr::new()
                    .f64_const(f64::INFINITY)
                    .f64_const(f64::INFINITY)
                    .op(op::F64_SUB),
            ),
            is_quiet_f64(
                "f64: inf * 0 is a NaN",
                Expr::new()
                    .f64_const(f64::INFINITY)
                    .f64_const(0.0)
                    .op(op::F64_MUL),
            ),
            case_i64(
                "f64: -1 / inf is -0",
                Expr::new()
                    .f64_const(-1.0)
                    .f64_const(f64::INFINITY)
                    .op(op::F64_DIV)
                    .op(op::I64_REINTERPRET_F64),
                i64::MIN,
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// NaN propagation
// ---------------------------------------------------------------------------------------

wasm_test!(nan_propagation, |ctx| {
    run_cases(
        ctx,
        "nan-propagation",
        vec![
            is_quiet_f32(
                "f32: NaN + 1 is a quiet NaN",
                Expr::new()
                    .f32_bits(F32_PAYLOAD_NAN)
                    .f32_const(1.0)
                    .op(op::F32_ADD),
            ),
            is_quiet_f32(
                "f32: 1 - NaN is a quiet NaN",
                Expr::new()
                    .f32_const(1.0)
                    .f32_bits(F32_PAYLOAD_NAN)
                    .op(op::F32_SUB),
            ),
            is_quiet_f32(
                "f32: NaN * 2 is a quiet NaN",
                Expr::new()
                    .f32_bits(F32_PAYLOAD_NAN)
                    .f32_const(2.0)
                    .op(op::F32_MUL),
            ),
            is_quiet_f32(
                "f32: NaN / 2 is a quiet NaN",
                Expr::new()
                    .f32_bits(F32_PAYLOAD_NAN)
                    .f32_const(2.0)
                    .op(op::F32_DIV),
            ),
            is_quiet_f32(
                "f32: a signalling NaN comes back quiet — the top mantissa bit is forced on",
                Expr::new()
                    .f32_bits(F32_SIGNALLING)
                    .f32_const(1.0)
                    .op(op::F32_ADD),
            ),
            is_quiet_f32(
                "f32: sqrt(-1) is a quiet NaN made out of nothing",
                Expr::new().f32_const(-1.0).op(op::F32_SQRT),
            ),
            is_quiet_f32(
                "f32: ceil of a NaN is a quiet NaN, not an integer",
                Expr::new().f32_bits(F32_SIGNALLING).op(op::F32_CEIL),
            ),
            is_quiet_f32(
                "f32: nearest of a NaN is a quiet NaN",
                Expr::new().f32_bits(F32_SIGNALLING).op(op::F32_NEAREST),
            ),
            is_quiet_f64(
                "f64: NaN + 1 is a quiet NaN",
                Expr::new()
                    .f64_bits(F64_SIGNALLING)
                    .f64_const(1.0)
                    .op(op::F64_ADD),
            ),
            is_quiet_f64(
                "f64: sqrt(-1) is a quiet NaN",
                Expr::new().f64_const(-1.0).op(op::F64_SQRT),
            ),
            is_quiet_f64(
                "f64: trunc of a NaN is a quiet NaN",
                Expr::new().f64_bits(F64_SIGNALLING).op(op::F64_TRUNC),
            ),
            case_i32(
                "f32: an arithmetic NaN is never a zero, an infinity or a number",
                Expr::new()
                    .f32_bits(F32_SIGNALLING)
                    .f32_const(1.0)
                    .op(op::F32_ADD)
                    .op(op::I32_REINTERPRET_F32)
                    .i32_const(0x7fff_ffff)
                    .op(op::I32_AND)
                    .i32_const(0x7f80_0000)
                    .op(op::I32_GT_S),
                1,
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// Comparisons
// ---------------------------------------------------------------------------------------

wasm_test!(nan_comparisons, |ctx| {
    run_cases(
        ctx,
        "nan-comparisons",
        vec![
            case_i32(
                "f32: NaN == NaN is false — a NaN is not even equal to itself",
                Expr::new()
                    .f32_bits(F32_PAYLOAD_NAN)
                    .f32_bits(F32_PAYLOAD_NAN)
                    .op(op::F32_EQ),
                0,
            ),
            case_i32(
                "f32: NaN != NaN is true — the one comparison that answers yes",
                Expr::new()
                    .f32_bits(F32_PAYLOAD_NAN)
                    .f32_bits(F32_PAYLOAD_NAN)
                    .op(op::F32_NE),
                1,
            ),
            case_i32(
                "f32: NaN == 1.5 is false",
                Expr::new()
                    .f32_bits(F32_PAYLOAD_NAN)
                    .f32_const(1.5)
                    .op(op::F32_EQ),
                0,
            ),
            case_i32(
                "f32: NaN != 1.5 is true",
                Expr::new()
                    .f32_bits(F32_PAYLOAD_NAN)
                    .f32_const(1.5)
                    .op(op::F32_NE),
                1,
            ),
            case_i32(
                "f32: NaN < 1.5 is false",
                Expr::new()
                    .f32_bits(F32_PAYLOAD_NAN)
                    .f32_const(1.5)
                    .op(op::F32_LT),
                0,
            ),
            case_i32(
                "f32: NaN > 1.5 is false, so it is neither below nor above",
                Expr::new()
                    .f32_bits(F32_PAYLOAD_NAN)
                    .f32_const(1.5)
                    .op(op::F32_GT),
                0,
            ),
            case_i32(
                "f32: NaN <= 1.5 is false — le is not the negation of gt here",
                Expr::new()
                    .f32_bits(F32_PAYLOAD_NAN)
                    .f32_const(1.5)
                    .op(op::F32_LE),
                0,
            ),
            case_i32(
                "f32: NaN >= 1.5 is false",
                Expr::new()
                    .f32_bits(F32_PAYLOAD_NAN)
                    .f32_const(1.5)
                    .op(op::F32_GE),
                0,
            ),
            case_i32(
                "f32: 1.5 < NaN is false with the NaN on the right as well",
                Expr::new()
                    .f32_const(1.5)
                    .f32_bits(F32_PAYLOAD_NAN)
                    .op(op::F32_LT),
                0,
            ),
            case_i32(
                "f64: NaN == NaN is false",
                Expr::new()
                    .f64_bits(f64::NAN.to_bits())
                    .f64_bits(f64::NAN.to_bits())
                    .op(op::F64_EQ),
                0,
            ),
            case_i32(
                "f64: NaN != NaN is true",
                Expr::new()
                    .f64_bits(f64::NAN.to_bits())
                    .f64_bits(f64::NAN.to_bits())
                    .op(op::F64_NE),
                1,
            ),
            case_i32(
                "f64: NaN >= NaN is false",
                Expr::new()
                    .f64_bits(f64::NAN.to_bits())
                    .f64_bits(f64::NAN.to_bits())
                    .op(op::F64_GE),
                0,
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// The two zeros
// ---------------------------------------------------------------------------------------

wasm_test!(signed_zeros, |ctx| {
    run_cases(
        ctx,
        "signed-zeros",
        vec![
            case_i32(
                "f32: -0 == +0 is true",
                Expr::new().f32_const(-0.0).f32_const(0.0).op(op::F32_EQ),
                1,
            ),
            case_i32(
                "f32: -0 != +0 is false",
                Expr::new().f32_const(-0.0).f32_const(0.0).op(op::F32_NE),
                0,
            ),
            case_i32(
                "f32: -0 < +0 is false; neither is below the other",
                Expr::new().f32_const(-0.0).f32_const(0.0).op(op::F32_LT),
                0,
            ),
            case_i32(
                "f32: -0 <= +0 is true",
                Expr::new().f32_const(-0.0).f32_const(0.0).op(op::F32_LE),
                1,
            ),
            case_i32(
                "f32: their bit patterns are not equal, which is the whole point",
                Expr::new()
                    .f32_const(-0.0)
                    .op(op::I32_REINTERPRET_F32)
                    .f32_const(0.0)
                    .op(op::I32_REINTERPRET_F32)
                    .op(op::I32_EQ),
                0,
            ),
            case_i32(
                "f32: +0 is all zero bits",
                Expr::new().f32_const(0.0).op(op::I32_REINTERPRET_F32),
                0,
            ),
            case_i32(
                "f32: -0 is the sign bit and nothing else",
                Expr::new().f32_const(-0.0).op(op::I32_REINTERPRET_F32),
                i32::MIN,
            ),
            case_i32(
                "f64: -0 == +0 is true here too",
                Expr::new().f64_const(-0.0).f64_const(0.0).op(op::F64_EQ),
                1,
            ),
            case_i64(
                "f64: -0 is the sign bit and nothing else",
                Expr::new().f64_const(-0.0).op(op::I64_REINTERPRET_F64),
                i64::MIN,
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// Worked examples
// ---------------------------------------------------------------------------------------

/// `f() -> f32 { min(NaN, 1.5) }` — a NaN, not the number.
fn min_nan_example() -> Module {
    f_f32(
        "min-nan",
        Expr::new()
            .f32_bits(F32_PAYLOAD_NAN)
            .f32_const(1.5)
            .op(op::F32_MIN),
    )
}

/// `f() -> f32 { nearest(2.5) }` — the tie that rounds down.
fn nearest_example() -> Module {
    f_f32(
        "nearest-two-and-a-half",
        Expr::new().f32_const(2.5).op(op::F32_NEAREST),
    )
}

/// Worked examples: the min that is not `fmin`, and the tie that rounds to even.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("min with a NaN operand", min_nan_example)
            .summary(
                "`f() -> f32` whose body is a NaN with payload 0x7fe00001, then `f32.const 1.5`, \
                 then `f32.min`",
            )
            .command("run --invoke f mod.wasm")
            .output("NaN")
            .note(
                "C's fmin(NaN, 1.5) is 1.5; WebAssembly's f32.min is a NaN. Every NaN prints as \
                 the bare word `NaN` whatever its sign and payload, so a test that cares which \
                 NaN came back has to reinterpret it as an i32 — and then only the exponent \
                 bits and the quiet bit are pinned by the spec.",
            ),
        ExampleSpec::module("nearest of an exact tie", nearest_example)
            .summary("`f() -> f32` whose body is `f32.const 2.5` then `f32.nearest`")
            .command("run --invoke f mod.wasm")
            .output("2")
            .note(
                "Round-half-to-even, not round-half-up: 0.5 becomes 0, 1.5 becomes 2, 2.5 \
                 becomes 2 and 3.5 becomes 4. `trunc(x + 0.5)` gets 2.5 wrong and gets -0.5 \
                 wrong twice, because nearest(-0.5) is -0.",
            ),
    ]
}
