//! Stage 20 — Truncation traps, trunc_sat and reinterpret. **[ext]**
//!
//! Three families that look alike and behave nothing alike.
//!
//! `i32.trunc_f32_s` and its seven siblings round **towards zero** and then insist the
//! answer fits. When it does not, they trap — and there are two different traps, which is
//! the point of half this stage:
//!
//! * a **NaN** is `invalid conversion to integer`;
//! * an **infinity**, and any finite value outside the range, is `integer overflow`.
//!
//! The second of those catches people out. An infinity looks like the clearest possible case
//! of "this is not an integer", so a first draft of this stage expected `invalid conversion
//! to integer` for it. That is wrong in `wasmtime` and wrong in the spec: the official
//! `conversions.wast` asserts `integer overflow` for `(f32.const inf)`, and a NaN is the only
//! input that gives the other reason. The expectation was corrected rather than loosened,
//! because telling the two apart is exactly what this stage is for.
//!
//! The boundaries are worth staring at, because "the biggest i32" is not a float. For
//! `i32.trunc_f64_s`, `2147483647.5` truncates cleanly to `2147483647` while `2147483648.0`
//! traps, so the test is whether the *truncated* value fits, not whether the float does. At
//! f32 width there is no float between `2147483520.0` and `2147483648.0` at all, so the
//! largest accepted input is the first of those. The unsigned forms are not simply
//! "non-negative only": they accept anything that truncates to zero, so `-0.5` is fine and
//! `-1.0` traps.
//!
//! `i32.trunc_sat_f32_s` and its siblings are the same conversion with the trap replaced by
//! a clamp: a NaN becomes 0, anything too large becomes the type's maximum, anything too
//! small its minimum. They never trap, which is why they are the ones a compiler emits for a
//! language whose cast is defined on every input.
//!
//! One expectation in here was corrected against the runtime rather than against intuition.
//! `0.0 / 0.0` produces a NaN out of nothing, and the spec calls that a *canonical* NaN — but
//! canonical pins the payload only, not the sign, and `wasmtime` on x86-64 hands back
//! `0xffc00000`, the negative one, because that is what the hardware's default NaN is. So the
//! two cases that name the canonical bits mask the sign bit off first and compare the
//! remaining 31 (or 63) bits.
//!
//! `reinterpret` is not a conversion at all. It hands the same 32 or 64 bits to the other
//! type, so `f32.const 1.0` becomes `1065353216` and `i32.const 1` becomes the smallest
//! positive subnormal. That printed subnormal is a decimal point followed by forty-four
//! zeros and a 1, because a float result is written the way Rust's `Display` writes it and
//! that formatter never uses exponent notation. Every expectation in this stage that goes
//! near such a value is therefore an integer compared through `reinterpret`, not a decimal.

use crate::examples::ExampleSpec;
use crate::stages::{
    case_f32, case_f64, case_i32, case_i64, case_trap, f_i32, run_cases, trap, Case, Stage, Test,
};
use crate::wasm::{ftype, op, op2, Expr, Module, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 20,
        slug: "conversions",
        name: "Truncation traps, trunc_sat and reinterpret",
        ext: true,
        hints: &[
            "i32.trunc_f32_s rounds towards zero first and then checks the range, so -1.9 becomes -1 and 2147483647.5 becomes 2147483647 — test the truncated value, not the float",
            "A NaN traps with 'invalid conversion to integer'; an infinity and any out-of-range finite value trap with 'integer overflow'. They are two different traps and a learner's runtime usually conflates them",
            "The unsigned truncations accept everything that truncates to zero, so -0.5 is fine and -1.0 is an overflow; do not reject on the sign bit",
            "The 0xfc-prefixed trunc_sat family never traps: NaN gives 0, too large gives the type's maximum, too small its minimum — and reinterpret moves bits without touching them at all",
        ],
        examples,
        tests: vec![
            Test::new("truncation rounds towards zero at every width", towards_zero).ext(),
            Test::new("the signed truncations accept the values that just fit", signed_boundaries)
                .ext(),
            Test::new("the unsigned truncations accept -0.5 and refuse -1", unsigned_boundaries)
                .ext(),
            Test::new("a NaN is an invalid conversion, an infinity is an integer overflow", nan_and_infinity_traps)
                .ext(),
            Test::new("a finite value one step past the range is an integer overflow", overflow_traps)
                .ext(),
            Test::new("the saturating truncations never trap on a NaN or an infinity", saturating_specials)
                .ext(),
            Test::new("the saturating truncations clamp to the type's own extremes", saturating_clamp)
                .ext(),
            Test::new("reinterpret moves the bits in all four directions", reinterpret_directions)
                .ext(),
            Test::new("reinterpret is not convert, and it names the exact bits", reinterpret_bits)
                .ext(),
        ],
    }
}

/// A `() -> i64` case that must trap — [`case_trap`] only covers the `i32` shape, and half
/// the truncations in this stage leave an `i64` on the stack.
fn case_trap_i64(name: &str, body: Expr, reasons: &[&str]) -> Case {
    Case::new(name, ftype(&[], &[ValType::I64]), body, &[]).traps(reasons)
}

// ---------------------------------------------------------------------------------------
// The happy path
// ---------------------------------------------------------------------------------------

wasm_test!(towards_zero, |ctx| {
    run_cases(
        ctx,
        "trunc-towards-zero",
        vec![
            case_i32(
                "i32.trunc_f32_s(1.9) is 1",
                Expr::new().f32_const(1.9).op(op::I32_TRUNC_F32_S),
                1,
            ),
            case_i32(
                "i32.trunc_f32_s(-1.9) is -1, not -2 — towards zero, not towards -inf",
                Expr::new().f32_const(-1.9).op(op::I32_TRUNC_F32_S),
                -1,
            ),
            case_i32(
                "i32.trunc_f32_s(0.9) is 0",
                Expr::new().f32_const(0.9).op(op::I32_TRUNC_F32_S),
                0,
            ),
            case_i32(
                "i32.trunc_f64_s(-1.9) is -1",
                Expr::new().f64_const(-1.9).op(op::I32_TRUNC_F64_S),
                -1,
            ),
            case_i32(
                "i32.trunc_f32_u(3.75) is 3",
                Expr::new().f32_const(3.75).op(op::I32_TRUNC_F32_U),
                3,
            ),
            case_i32(
                "i32.trunc_f64_u(0.9) is 0",
                Expr::new().f64_const(0.9).op(op::I32_TRUNC_F64_U),
                0,
            ),
            case_i64(
                "i64.trunc_f32_s(-1.9) is -1",
                Expr::new().f32_const(-1.9).op(op::I64_TRUNC_F32_S),
                -1,
            ),
            case_i64(
                "i64.trunc_f64_s(-1.9) is -1",
                Expr::new().f64_const(-1.9).op(op::I64_TRUNC_F64_S),
                -1,
            ),
            case_i64(
                "i64.trunc_f32_s(1e10) is 10000000000 — beyond i32, still exact in f32",
                Expr::new().f32_const(1e10).op(op::I64_TRUNC_F32_S),
                10_000_000_000,
            ),
            case_i64(
                "i64.trunc_f32_u(1e10) agrees with the signed form on a positive value",
                Expr::new().f32_const(1e10).op(op::I64_TRUNC_F32_U),
                10_000_000_000,
            ),
            case_i64(
                "i64.trunc_f64_u(4294967296.5) is 4294967296",
                Expr::new()
                    .f64_const(4_294_967_296.5)
                    .op(op::I64_TRUNC_F64_U),
                4_294_967_296,
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// Boundaries
// ---------------------------------------------------------------------------------------

wasm_test!(signed_boundaries, |ctx| {
    run_cases(
        ctx,
        "trunc-signed-boundaries",
        vec![
            case_i32(
                "i32.trunc_f64_s(2147483647.5) fits, because the truncated value fits",
                Expr::new()
                    .f64_const(2_147_483_647.5)
                    .op(op::I32_TRUNC_F64_S),
                i32::MAX,
            ),
            case_i32(
                "i32.trunc_f64_s(-2147483648.0) is exactly i32::MIN",
                Expr::new()
                    .f64_const(-2_147_483_648.0)
                    .op(op::I32_TRUNC_F64_S),
                i32::MIN,
            ),
            case_i32(
                "i32.trunc_f64_s(-2147483648.9) truncates up to i32::MIN and fits",
                Expr::new()
                    .f64_const(-2_147_483_648.9)
                    .op(op::I32_TRUNC_F64_S),
                i32::MIN,
            ),
            case_i32(
                "i32.trunc_f32_s(2147483520.0) is the largest f32 that fits — there is no f32 between it and 2^31",
                Expr::new()
                    .f32_const(2_147_483_520.0)
                    .op(op::I32_TRUNC_F32_S),
                2_147_483_520,
            ),
            case_i32(
                "i32.trunc_f32_s(-2147483648.0) fits exactly",
                Expr::new()
                    .f32_const(-2_147_483_648.0)
                    .op(op::I32_TRUNC_F32_S),
                i32::MIN,
            ),
            case_i64(
                "i64.trunc_f64_s(9223372036854774784.0) is the largest f64 below 2^63",
                Expr::new()
                    .f64_const(9_223_372_036_854_774_784.0)
                    .op(op::I64_TRUNC_F64_S),
                9_223_372_036_854_774_784,
            ),
            case_i64(
                "i64.trunc_f64_s(-9223372036854775808.0) is exactly i64::MIN",
                Expr::new()
                    .f64_const(-9_223_372_036_854_775_808.0)
                    .op(op::I64_TRUNC_F64_S),
                i64::MIN,
            ),
            case_i64(
                "i64.trunc_f32_s(9223371487098961920.0) is the largest f32 below 2^63",
                Expr::new()
                    .f32_const(9_223_371_487_098_961_920.0)
                    .op(op::I64_TRUNC_F32_S),
                9_223_371_487_098_961_920,
            ),
        ],
    )
});

wasm_test!(unsigned_boundaries, |ctx| {
    run_cases(
        ctx,
        "trunc-unsigned-boundaries",
        vec![
            case_i32(
                "i32.trunc_f32_u(-0.5) is 0 — it truncates into range before the check",
                Expr::new().f32_const(-0.5).op(op::I32_TRUNC_F32_U),
                0,
            ),
            case_i32(
                "i32.trunc_f64_u(-0.9) is 0 as well",
                Expr::new().f64_const(-0.9).op(op::I32_TRUNC_F64_U),
                0,
            ),
            case_i32(
                "i32.trunc_f64_u(-0) is 0 and the sign bit is not an error",
                Expr::new().f64_const(-0.0).op(op::I32_TRUNC_F64_U),
                0,
            ),
            case_trap(
                "i32.trunc_f32_u(-1.0) overflows: it truncates to -1, which no u32 holds",
                Expr::new().f32_const(-1.0).op(op::I32_TRUNC_F32_U),
                &[trap::INTEGER_OVERFLOW],
            ),
            case_i32(
                "i32.trunc_f64_u(4294967295.0) is u32::MAX, printed as the i32 -1",
                Expr::new()
                    .f64_const(4_294_967_295.0)
                    .op(op::I32_TRUNC_F64_U),
                -1,
            ),
            case_i32(
                "i32.trunc_f32_u(4294967040.0) is the largest f32 below 2^32",
                Expr::new()
                    .f32_const(4_294_967_040.0)
                    .op(op::I32_TRUNC_F32_U),
                -256,
            ),
            case_i64(
                "i64.trunc_f32_u(-0.5) is 0",
                Expr::new().f32_const(-0.5).op(op::I64_TRUNC_F32_U),
                0,
            ),
            case_trap_i64(
                "i64.trunc_f64_u(-1.0) overflows",
                Expr::new().f64_const(-1.0).op(op::I64_TRUNC_F64_U),
                &[trap::INTEGER_OVERFLOW],
            ),
            case_i64(
                "i64.trunc_f64_u(18446744073709549568.0) is the largest f64 below 2^64, printed as -2048",
                Expr::new()
                    .f64_const(18_446_744_073_709_549_568.0)
                    .op(op::I64_TRUNC_F64_U),
                -2048,
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// The two traps
// ---------------------------------------------------------------------------------------

wasm_test!(nan_and_infinity_traps, |ctx| {
    run_cases(
        ctx,
        "trunc-nan-and-infinity",
        vec![
            case_trap(
                "i32.trunc_f32_s(NaN) is an invalid conversion",
                Expr::new()
                    .f32_bits(f32::NAN.to_bits())
                    .op(op::I32_TRUNC_F32_S),
                &[trap::INVALID_CONVERSION],
            ),
            case_trap(
                "i32.trunc_f32_u(NaN) is an invalid conversion",
                Expr::new()
                    .f32_bits(f32::NAN.to_bits())
                    .op(op::I32_TRUNC_F32_U),
                &[trap::INVALID_CONVERSION],
            ),
            case_trap(
                "i32.trunc_f64_s(NaN) is an invalid conversion",
                Expr::new()
                    .f64_bits(f64::NAN.to_bits())
                    .op(op::I32_TRUNC_F64_S),
                &[trap::INVALID_CONVERSION],
            ),
            case_trap_i64(
                "i64.trunc_f64_s(NaN) is an invalid conversion",
                Expr::new()
                    .f64_bits(f64::NAN.to_bits())
                    .op(op::I64_TRUNC_F64_S),
                &[trap::INVALID_CONVERSION],
            ),
            case_trap_i64(
                "i64.trunc_f64_u(NaN) is an invalid conversion",
                Expr::new()
                    .f64_bits(f64::NAN.to_bits())
                    .op(op::I64_TRUNC_F64_U),
                &[trap::INVALID_CONVERSION],
            ),
            case_trap(
                "i32.trunc_f32_s(inf) is an integer overflow, not an invalid conversion",
                Expr::new().f32_const(f32::INFINITY).op(op::I32_TRUNC_F32_S),
                &[trap::INTEGER_OVERFLOW],
            ),
            case_trap(
                "i32.trunc_f32_s(-inf) is an integer overflow",
                Expr::new()
                    .f32_const(f32::NEG_INFINITY)
                    .op(op::I32_TRUNC_F32_S),
                &[trap::INTEGER_OVERFLOW],
            ),
            case_trap(
                "i32.trunc_f32_u(inf) is an integer overflow",
                Expr::new().f32_const(f32::INFINITY).op(op::I32_TRUNC_F32_U),
                &[trap::INTEGER_OVERFLOW],
            ),
            case_trap_i64(
                "i64.trunc_f64_s(-inf) is an integer overflow",
                Expr::new()
                    .f64_const(f64::NEG_INFINITY)
                    .op(op::I64_TRUNC_F64_S),
                &[trap::INTEGER_OVERFLOW],
            ),
            case_trap_i64(
                "i64.trunc_f32_u(inf) is an integer overflow",
                Expr::new().f32_const(f32::INFINITY).op(op::I64_TRUNC_F32_U),
                &[trap::INTEGER_OVERFLOW],
            ),
        ],
    )
});

wasm_test!(overflow_traps, |ctx| {
    run_cases(
        ctx,
        "trunc-overflow",
        vec![
            case_trap(
                "i32.trunc_f64_s(2147483648.0) — one past i32::MAX",
                Expr::new()
                    .f64_const(2_147_483_648.0)
                    .op(op::I32_TRUNC_F64_S),
                &[trap::INTEGER_OVERFLOW],
            ),
            case_trap(
                "i32.trunc_f64_s(-2147483649.0) — one past i32::MIN",
                Expr::new()
                    .f64_const(-2_147_483_649.0)
                    .op(op::I32_TRUNC_F64_S),
                &[trap::INTEGER_OVERFLOW],
            ),
            case_trap(
                "i32.trunc_f32_s(2147483648.0) — the first f32 above the range",
                Expr::new()
                    .f32_const(2_147_483_648.0)
                    .op(op::I32_TRUNC_F32_S),
                &[trap::INTEGER_OVERFLOW],
            ),
            case_trap(
                "i32.trunc_f32_s(-2147483904.0) — the first f32 below it",
                Expr::new()
                    .f32_const(-2_147_483_904.0)
                    .op(op::I32_TRUNC_F32_S),
                &[trap::INTEGER_OVERFLOW],
            ),
            case_trap(
                "i32.trunc_f64_u(4294967296.0) — one past u32::MAX",
                Expr::new()
                    .f64_const(4_294_967_296.0)
                    .op(op::I32_TRUNC_F64_U),
                &[trap::INTEGER_OVERFLOW],
            ),
            case_trap(
                "i32.trunc_f64_s(1e300) — a long way past",
                Expr::new().f64_const(1e300).op(op::I32_TRUNC_F64_S),
                &[trap::INTEGER_OVERFLOW],
            ),
            case_trap_i64(
                "i64.trunc_f64_s(9223372036854775808.0) — 2^63 exactly, one past i64::MAX",
                Expr::new()
                    .f64_const(9_223_372_036_854_775_808.0)
                    .op(op::I64_TRUNC_F64_S),
                &[trap::INTEGER_OVERFLOW],
            ),
            case_trap_i64(
                "i64.trunc_f32_s(9223372036854775808.0) — the same boundary at f32 width",
                Expr::new()
                    .f32_const(9_223_372_036_854_775_808.0)
                    .op(op::I64_TRUNC_F32_S),
                &[trap::INTEGER_OVERFLOW],
            ),
            case_trap_i64(
                "i64.trunc_f64_u(18446744073709551616.0) — 2^64 exactly",
                Expr::new()
                    .f64_const(18_446_744_073_709_551_616.0)
                    .op(op::I64_TRUNC_F64_U),
                &[trap::INTEGER_OVERFLOW],
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// The saturating family
// ---------------------------------------------------------------------------------------

wasm_test!(saturating_specials, |ctx| {
    run_cases(
        ctx,
        "trunc-sat-specials",
        vec![
            case_i32(
                "i32.trunc_sat_f32_s(NaN) is 0",
                Expr::new()
                    .f32_bits(f32::NAN.to_bits())
                    .op2(op2::I32_TRUNC_SAT_F32_S),
                0,
            ),
            case_i32(
                "i32.trunc_sat_f32_u(NaN) is 0, not u32::MAX",
                Expr::new()
                    .f32_bits(f32::NAN.to_bits())
                    .op2(op2::I32_TRUNC_SAT_F32_U),
                0,
            ),
            case_i32(
                "i32.trunc_sat_f64_s(NaN) is 0",
                Expr::new()
                    .f64_bits(f64::NAN.to_bits())
                    .op2(op2::I32_TRUNC_SAT_F64_S),
                0,
            ),
            case_i64(
                "i64.trunc_sat_f64_s(NaN) is 0",
                Expr::new()
                    .f64_bits(f64::NAN.to_bits())
                    .op2(op2::I64_TRUNC_SAT_F64_S),
                0,
            ),
            case_i64(
                "i64.trunc_sat_f32_u(NaN) is 0",
                Expr::new()
                    .f32_bits(f32::NAN.to_bits())
                    .op2(op2::I64_TRUNC_SAT_F32_U),
                0,
            ),
            case_i32(
                "i32.trunc_sat_f32_s(-1.9) still rounds towards zero: -1",
                Expr::new().f32_const(-1.9).op2(op2::I32_TRUNC_SAT_F32_S),
                -1,
            ),
            case_i32(
                "i32.trunc_sat_f32_u(-1.0) clamps to 0 where the trapping form overflows",
                Expr::new().f32_const(-1.0).op2(op2::I32_TRUNC_SAT_F32_U),
                0,
            ),
            case_i32(
                "i32.trunc_sat_f64_s(2147483648.0) clamps to i32::MAX where the trapping form traps",
                Expr::new()
                    .f64_const(2_147_483_648.0)
                    .op2(op2::I32_TRUNC_SAT_F64_S),
                i32::MAX,
            ),
        ],
    )
});

wasm_test!(saturating_clamp, |ctx| {
    run_cases(
        ctx,
        "trunc-sat-clamp",
        vec![
            case_i32(
                "i32.trunc_sat_f32_s(inf) is i32::MAX",
                Expr::new()
                    .f32_const(f32::INFINITY)
                    .op2(op2::I32_TRUNC_SAT_F32_S),
                i32::MAX,
            ),
            case_i32(
                "i32.trunc_sat_f32_s(-inf) is i32::MIN",
                Expr::new()
                    .f32_const(f32::NEG_INFINITY)
                    .op2(op2::I32_TRUNC_SAT_F32_S),
                i32::MIN,
            ),
            case_i32(
                "i32.trunc_sat_f32_u(inf) is u32::MAX, printed as -1",
                Expr::new()
                    .f32_const(f32::INFINITY)
                    .op2(op2::I32_TRUNC_SAT_F32_U),
                -1,
            ),
            case_i32(
                "i32.trunc_sat_f32_u(-inf) is 0 — the unsigned minimum, not i32::MIN",
                Expr::new()
                    .f32_const(f32::NEG_INFINITY)
                    .op2(op2::I32_TRUNC_SAT_F32_U),
                0,
            ),
            case_i32(
                "i32.trunc_sat_f64_s(1e300) clamps to i32::MAX",
                Expr::new().f64_const(1e300).op2(op2::I32_TRUNC_SAT_F64_S),
                i32::MAX,
            ),
            case_i32(
                "i32.trunc_sat_f64_s(-1e300) clamps to i32::MIN",
                Expr::new().f64_const(-1e300).op2(op2::I32_TRUNC_SAT_F64_S),
                i32::MIN,
            ),
            case_i32(
                "i32.trunc_sat_f64_u(1e300) clamps to u32::MAX",
                Expr::new().f64_const(1e300).op2(op2::I32_TRUNC_SAT_F64_U),
                -1,
            ),
            case_i64(
                "i64.trunc_sat_f64_s(inf) is i64::MAX",
                Expr::new()
                    .f64_const(f64::INFINITY)
                    .op2(op2::I64_TRUNC_SAT_F64_S),
                i64::MAX,
            ),
            case_i64(
                "i64.trunc_sat_f64_s(-inf) is i64::MIN",
                Expr::new()
                    .f64_const(f64::NEG_INFINITY)
                    .op2(op2::I64_TRUNC_SAT_F64_S),
                i64::MIN,
            ),
            case_i64(
                "i64.trunc_sat_f64_u(inf) is u64::MAX, printed as -1",
                Expr::new()
                    .f64_const(f64::INFINITY)
                    .op2(op2::I64_TRUNC_SAT_F64_U),
                -1,
            ),
            case_i64(
                "i64.trunc_sat_f64_u(-inf) is 0",
                Expr::new()
                    .f64_const(f64::NEG_INFINITY)
                    .op2(op2::I64_TRUNC_SAT_F64_U),
                0,
            ),
            case_i64(
                "i64.trunc_sat_f32_s(inf) is i64::MAX",
                Expr::new()
                    .f32_const(f32::INFINITY)
                    .op2(op2::I64_TRUNC_SAT_F32_S),
                i64::MAX,
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// reinterpret
// ---------------------------------------------------------------------------------------

wasm_test!(reinterpret_directions, |ctx| {
    run_cases(
        ctx,
        "reinterpret-directions",
        vec![
            case_i32(
                "i32.reinterpret_f32(1.0) is 1065353216",
                Expr::new().f32_const(1.0).op(op::I32_REINTERPRET_F32),
                0x3f80_0000,
            ),
            case_f32(
                "f32.reinterpret_i32(1065353216) is 1.0, the other way round",
                Expr::new()
                    .i32_const(0x3f80_0000)
                    .op(op::F32_REINTERPRET_I32),
                1.0,
            ),
            case_i64(
                "i64.reinterpret_f64(1.0) is 4607182418800017408",
                Expr::new().f64_const(1.0).op(op::I64_REINTERPRET_F64),
                0x3ff0_0000_0000_0000,
            ),
            case_f64(
                "f64.reinterpret_i64(4607182418800017408) is 1.0",
                Expr::new()
                    .i64_const(0x3ff0_0000_0000_0000)
                    .op(op::F64_REINTERPRET_I64),
                1.0,
            ),
            case_i32(
                "f32 round-trips through i32 unchanged, -3.25 included",
                Expr::new()
                    .f32_const(-3.25)
                    .op(op::I32_REINTERPRET_F32)
                    .op(op::F32_REINTERPRET_I32)
                    .f32_const(-3.25)
                    .op(op::F32_EQ),
                1,
            ),
            case_i64(
                "i64 round-trips through f64 unchanged, bit for bit",
                Expr::new()
                    .i64_const(-1)
                    .op(op::F64_REINTERPRET_I64)
                    .op(op::I64_REINTERPRET_F64),
                -1,
            ),
            case_i32(
                "i32.reinterpret_f32(-0) is the sign bit alone, so it is i32::MIN",
                Expr::new().f32_const(-0.0).op(op::I32_REINTERPRET_F32),
                i32::MIN,
            ),
            case_i32(
                "i32.reinterpret_f32(inf) is 0x7f800000",
                Expr::new()
                    .f32_const(f32::INFINITY)
                    .op(op::I32_REINTERPRET_F32),
                0x7f80_0000,
            ),
            case_f32(
                "f32.reinterpret_i32(-1) is a NaN: every exponent and mantissa bit set",
                Expr::new().i32_const(-1).op(op::F32_REINTERPRET_I32),
                f32::NAN,
            ),
        ],
    )
});

wasm_test!(reinterpret_bits, |ctx| {
    run_cases(
        ctx,
        "reinterpret-vs-convert",
        vec![
            case_i32(
                "1.0 reinterpreted is 1065353216; truncating it would have given 1",
                Expr::new().f32_const(1.0).op(op::I32_REINTERPRET_F32),
                0x3f80_0000,
            ),
            case_i32(
                "1.0 truncated is 1, so the two instructions really do differ",
                Expr::new().f32_const(1.0).op(op::I32_TRUNC_F32_S),
                1,
            ),
            case_f32(
                "1065353216 converted is the float 1065353216, not 1.0",
                Expr::new()
                    .i32_const(0x3f80_0000)
                    .op(op::F32_CONVERT_I32_S),
                1_065_353_216.0,
            ),
            case_i32(
                "the smallest positive f32 subnormal has the bits 0x00000001",
                Expr::new()
                    .i32_const(1)
                    .op(op::F32_REINTERPRET_I32)
                    .op(op::I32_REINTERPRET_F32),
                1,
            ),
            case_i32(
                "that subnormal is greater than zero, small as it is",
                Expr::new()
                    .i32_const(1)
                    .op(op::F32_REINTERPRET_I32)
                    .f32_const(0.0)
                    .op(op::F32_GT),
                1,
            ),
            case_i32(
                "converting 1 to f32 gives 1.0, whose bits are 0x3f800000 — nothing like the subnormal",
                Expr::new()
                    .i32_const(1)
                    .op(op::F32_CONVERT_I32_S)
                    .op(op::I32_REINTERPRET_F32),
                0x3f80_0000,
            ),
            case_i64(
                "the smallest positive f64 subnormal has the bits 0x0000000000000001",
                Expr::new()
                    .i64_const(1)
                    .op(op::F64_REINTERPRET_I64)
                    .op(op::I64_REINTERPRET_F64),
                1,
            ),
            case_i32(
                "a NaN made out of nothing carries the canonical payload 0x7fc00000, sign aside",
                Expr::new()
                    .f32_const(0.0)
                    .f32_const(0.0)
                    .op(op::F32_DIV)
                    .op(op::I32_REINTERPRET_F32)
                    .i32_const(0x7fff_ffff)
                    .op(op::I32_AND),
                0x7fc0_0000,
            ),
            case_i64(
                "and at f64 width the canonical payload is 0x7ff8000000000000",
                Expr::new()
                    .f64_const(0.0)
                    .f64_const(0.0)
                    .op(op::F64_DIV)
                    .op(op::I64_REINTERPRET_F64)
                    .i64_const(0x7fff_ffff_ffff_ffff)
                    .op(op::I64_AND),
                0x7ff8_0000_0000_0000,
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// Worked examples
// ---------------------------------------------------------------------------------------

/// `f() -> i32 { i32.trunc_f32_s(inf) }` — the trap that is not an invalid conversion.
fn infinity_trap_example() -> Module {
    f_i32(
        "trunc-infinity",
        Expr::new().f32_const(f32::INFINITY).op(op::I32_TRUNC_F32_S),
    )
}

/// `f() -> i32 { i32.reinterpret_f32(1.0) }` — the bits of 1.0.
fn reinterpret_example() -> Module {
    f_i32(
        "reinterpret-one",
        Expr::new().f32_const(1.0).op(op::I32_REINTERPRET_F32),
    )
}

/// Worked examples: the trap that is easy to mislabel, and the instruction that moves bits.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("Truncating an infinity", infinity_trap_example)
            .summary("`f() -> i32` whose body is `f32.const inf` then `i32.trunc_f32_s`")
            .command("run --invoke f mod.wasm")
            .output(
                "nothing on stdout; `wasm trap: integer overflow` on stderr and a non-zero exit",
            )
            .note(
                "Swap the `inf` for a NaN and the same instruction traps with `invalid \
                 conversion to integer` instead. Two inputs, two different trap reasons — a \
                 runtime that answers one message for both is the usual bug here.",
            ),
        ExampleSpec::module("The bits of 1.0", reinterpret_example)
            .summary("`f() -> i32` whose body is `f32.const 1.0` then `i32.reinterpret_f32`")
            .command("run --invoke f mod.wasm")
            .output("1065353216")
            .note(
                "0x3f800000: sign 0, exponent 127, mantissa 0. `i32.trunc_f32_s` on the same \
                 input would answer 1 — reinterpret moves the bits across, convert and \
                 truncate compute a new value.",
            ),
    ]
}
