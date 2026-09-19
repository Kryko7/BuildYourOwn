//! Stage 47 — Vector arithmetic, shape by shape.
//!
//! The same sixteen bytes, four different integer shapes, and one rule that decides every
//! expectation here: a lanewise operation is the scalar operation done independently in each
//! lane, with **no carry between lanes**. `i8x16.add` of `0xff` and `0x01` is `0x00` in that
//! byte and leaves its neighbour alone; the scalar `i32` addition that would have carried is
//! simply not what the instruction does.
//!
//! Integer add, sub and mul wrap within the lane exactly as their scalar counterparts wrap
//! within a register. The saturating forms are the ones with no scalar equivalent in
//! WebAssembly at all: `add_sat_s` clamps to the lane's signed range instead of wrapping, and
//! `add_sat_u` to its unsigned range, which is the whole reason the vector instruction set
//! has them.
//!
//! The shifts take their count as an ordinary `i32` operand, not an immediate, and that count
//! is taken **modulo the lane width** — so shifting an i32x4 by 32 is a shift by zero, not a
//! wipe.

use crate::examples::ExampleSpec;
use crate::stages::{
    case_v128, run_cases, v128_of_i16x8, v128_of_i32x4, v128_of_i8x16, Stage, Test,
};
use crate::wasm::{ftype, opv, Expr, Func, Module, ModuleBuilder, OpV, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 47,
        slug: "v128_arithmetic",
        name: "Vector arithmetic, shape by shape",
        ext: true,
        hints: &[
            "Every lane is independent: nothing carries from one lane into the next, which is the whole point of the shape",
            "add, sub and mul wrap within the lane — i8x16.add of 127 and 1 is -128, not 128 and not a trap",
            "add_sat_s clamps to the lane's signed range and add_sat_u to its unsigned range; these have no scalar equivalent in WebAssembly",
            "A shift count is an i32 operand and is taken modulo the lane width, so i32x4.shl by 32 shifts by nothing at all",
        ],
        examples,
        tests: vec![
            Test::new("i32x4 add, sub and mul work lane by lane", i32x4_arithmetic).ext(),
            Test::new("nothing carries from one lane into the next", no_carry_between_lanes).ext(),
            Test::new("byte lanes wrap inside the byte", i8x16_wrapping).ext(),
            Test::new("the saturating adds clamp instead of wrapping", saturating_add).ext(),
            Test::new("the saturating subtracts clamp at the other end", saturating_sub).ext(),
            Test::new("i16x8 and i64x2 do the same in their own widths", other_int_shapes).ext(),
            Test::new("neg and abs are lanewise too", neg_and_abs).ext(),
            Test::new("min_s and max_s pick per lane", min_and_max).ext(),
            Test::new("a shift count is taken modulo the lane width", shifts).ext(),
            Test::new("the logical and arithmetic right shifts differ on a negative lane", shift_signedness).ext(),
            Test::new("f32x4 arithmetic is four f32 operations", f32x4_arithmetic).ext(),
            Test::new("a NaN lane stays NaN through min and max", float_nan_lanes).ext(),
        ],
    }
}

/// `a op b` for two i32x4 constants.
fn bin4(o: OpV, a: [i32; 4], b: [i32; 4]) -> Expr {
    Expr::new().v128_const_i32x4(a).v128_const_i32x4(b).opv(o)
}

/// `a op b` for two raw byte vectors.
fn binb(o: OpV, a: [u8; 16], b: [u8; 16]) -> Expr {
    Expr::new().v128_const(a).v128_const(b).opv(o)
}

/// Sixteen bytes all the same.
fn bytes(v: u8) -> [u8; 16] {
    [v; 16]
}

wasm_test!(i32x4_arithmetic, |ctx| {
    run_cases(
        ctx,
        "i32x4-arith",
        vec![
            case_v128(
                "add is four independent additions",
                bin4(opv::I32X4_ADD, [1, 2, 3, 4], [10, 20, 30, 40]),
                v128_of_i32x4([11, 22, 33, 44]),
            ),
            case_v128(
                "sub does not commute, lane by lane",
                bin4(opv::I32X4_SUB, [10, 20, 30, 40], [1, 2, 3, 4]),
                v128_of_i32x4([9, 18, 27, 36]),
            ),
            case_v128(
                "mul keeps the low thirty-two bits of each lane",
                bin4(opv::I32X4_MUL, [2, 3, -4, 0], [5, 7, 11, 99]),
                v128_of_i32x4([10, 21, -44, 0]),
            ),
            case_v128(
                "a lane that overflows wraps and stays in its lane",
                bin4(opv::I32X4_ADD, [i32::MAX, 0, 0, 0], [1, 0, 0, 0]),
                v128_of_i32x4([i32::MIN, 0, 0, 0]),
            ),
            case_v128(
                "mul of two large lanes keeps the low half",
                bin4(opv::I32X4_MUL, [65536, 0, 0, 0], [65536, 0, 0, 0]),
                v128_of_i32x4([0, 0, 0, 0]),
            ),
        ],
    )
});

wasm_test!(no_carry_between_lanes, |ctx| {
    run_cases(
        ctx,
        "no-carry",
        vec![
            case_v128(
                "i32x4: lane 0 overflowing leaves lane 1 untouched",
                bin4(opv::I32X4_ADD, [-1, 0, 0, 0], [1, 0, 0, 0]),
                0,
            ),
            case_v128(
                "the same bytes added as i8x16 wrap sixteen times over",
                binb(opv::I8X16_ADD, bytes(0xff), bytes(0x01)),
                0,
            ),
            case_v128(
                "the same two operands under i64x2 give a different answer entirely",
                binb(opv::I64X2_ADD, bytes(0xff), bytes(0x01)),
                // Each 64-bit lane is 0xffff_ffff_ffff_ffff + 0x0101_0101_0101_0101, which
                // is 0x0101_0101_0101_0100 once the carry off the top of the lane is dropped.
                // The byte shape gave zero for these very bytes: the shape is the operation.
                v128_of_i32x4([0x0101_0100, 0x0101_0101, 0x0101_0100, 0x0101_0101]),
            ),
            case_v128(
                "a carry out of the low i64 lane does not reach the high one",
                Expr::new()
                    .v128_const_i32x4([-1, -1, 0, 0])
                    .v128_const_i32x4([1, 0, 0, 0])
                    .opv(opv::I64X2_ADD),
                0,
            ),
        ],
    )
});

wasm_test!(i8x16_wrapping, |ctx| {
    run_cases(
        ctx,
        "i8x16-wrap",
        vec![
            case_v128(
                "127 + 1 is -128 in every byte",
                binb(opv::I8X16_ADD, bytes(0x7f), bytes(0x01)),
                v128_of_i8x16([i8::MIN; 16]),
            ),
            case_v128(
                "0 - 1 is -1 in every byte",
                binb(opv::I8X16_SUB, bytes(0), bytes(0x01)),
                u128::MAX,
            ),
            case_v128(
                "adding zero changes nothing",
                binb(opv::I8X16_ADD, bytes(0x42), bytes(0)),
                v128_of_i8x16([0x42; 16]),
            ),
        ],
    )
});

wasm_test!(saturating_add, |ctx| {
    run_cases(
        ctx,
        "add-sat",
        vec![
            case_v128(
                "signed: 127 + 1 clamps to 127 instead of wrapping to -128",
                binb(opv::I8X16_ADD_SAT_S, bytes(0x7f), bytes(0x01)),
                v128_of_i8x16([i8::MAX; 16]),
            ),
            case_v128(
                "signed: -128 + -1 clamps to -128",
                binb(opv::I8X16_ADD_SAT_S, bytes(0x80), bytes(0xff)),
                v128_of_i8x16([i8::MIN; 16]),
            ),
            case_v128(
                "unsigned: 255 + 1 clamps to 255",
                binb(opv::I8X16_ADD_SAT_U, bytes(0xff), bytes(0x01)),
                u128::MAX,
            ),
            case_v128(
                "unsigned reads the same bytes differently: 0x7f + 0x01 is 0x80, not clamped",
                binb(opv::I8X16_ADD_SAT_U, bytes(0x7f), bytes(0x01)),
                v128_of_i8x16([i8::MIN; 16]),
            ),
            case_v128(
                "below the limit the saturating add is an ordinary add",
                binb(opv::I8X16_ADD_SAT_S, bytes(0x10), bytes(0x0f)),
                v128_of_i8x16([0x1f; 16]),
            ),
        ],
    )
});

wasm_test!(saturating_sub, |ctx| {
    run_cases(
        ctx,
        "sub-sat",
        vec![
            case_v128(
                "signed: -128 - 1 clamps to -128",
                binb(opv::I8X16_SUB_SAT_S, bytes(0x80), bytes(0x01)),
                v128_of_i8x16([i8::MIN; 16]),
            ),
            case_v128(
                "unsigned: 0 - 1 clamps to 0 rather than wrapping to 255",
                binb(opv::I8X16_SUB_SAT_U, bytes(0), bytes(0x01)),
                0,
            ),
            case_v128(
                "the wrapping sub of the same operands gives 255",
                binb(opv::I8X16_SUB, bytes(0), bytes(0x01)),
                u128::MAX,
            ),
        ],
    )
});

wasm_test!(other_int_shapes, |ctx| {
    run_cases(
        ctx,
        "other-shapes",
        vec![
            case_v128(
                "i16x8.add works in eight half-words",
                Expr::new()
                    .i32_const(1000)
                    .opv(opv::I16X8_SPLAT)
                    .i32_const(2000)
                    .opv(opv::I16X8_SPLAT)
                    .opv(opv::I16X8_ADD),
                v128_of_i16x8([3000; 8]),
            ),
            case_v128(
                "i16x8.add wraps at 32767",
                Expr::new()
                    .i32_const(i16::MAX as i32)
                    .opv(opv::I16X8_SPLAT)
                    .i32_const(1)
                    .opv(opv::I16X8_SPLAT)
                    .opv(opv::I16X8_ADD),
                v128_of_i16x8([i16::MIN; 8]),
            ),
            case_v128(
                "i16x8.mul keeps the low sixteen bits",
                Expr::new()
                    .i32_const(300)
                    .opv(opv::I16X8_SPLAT)
                    .i32_const(300)
                    .opv(opv::I16X8_SPLAT)
                    .opv(opv::I16X8_MUL),
                v128_of_i16x8([(90000u32 as u16) as i16; 8]),
            ),
            case_v128(
                "i64x2.add adds two 64-bit lanes",
                Expr::new()
                    .i64_const(1)
                    .opv(opv::I64X2_SPLAT)
                    .i64_const(2)
                    .opv(opv::I64X2_SPLAT)
                    .opv(opv::I64X2_ADD),
                v128_of_i32x4([3, 0, 3, 0]),
            ),
            case_v128(
                "i64x2.mul is a full 64-bit multiply per lane",
                Expr::new()
                    .i64_const(1 << 20)
                    .opv(opv::I64X2_SPLAT)
                    .i64_const(1 << 20)
                    .opv(opv::I64X2_SPLAT)
                    .opv(opv::I64X2_MUL),
                v128_of_i32x4([0, 1 << 8, 0, 1 << 8]),
            ),
        ],
    )
});

wasm_test!(neg_and_abs, |ctx| {
    run_cases(
        ctx,
        "neg-abs",
        vec![
            case_v128(
                "i8x16.neg negates every byte",
                Expr::new().v128_const(bytes(0x01)).opv(opv::I8X16_NEG),
                u128::MAX,
            ),
            case_v128(
                "i8x16.abs of -1 is 1",
                Expr::new().v128_const(bytes(0xff)).opv(opv::I8X16_ABS),
                v128_of_i8x16([1; 16]),
            ),
            case_v128(
                "abs of the most negative byte is itself, the one value with no positive twin",
                Expr::new().v128_const(bytes(0x80)).opv(opv::I8X16_ABS),
                v128_of_i8x16([i8::MIN; 16]),
            ),
        ],
    )
});

wasm_test!(min_and_max, |ctx| {
    run_cases(
        ctx,
        "min-max",
        vec![
            case_v128(
                "i32x4.min_s picks the smaller of each pair",
                bin4(opv::I32X4_MIN_S, [1, 20, -3, 0], [10, 2, 3, 0]),
                v128_of_i32x4([1, 2, -3, 0]),
            ),
            case_v128(
                "i32x4.max_s picks the larger",
                bin4(opv::I32X4_MAX_S, [1, 20, -3, 0], [10, 2, 3, 0]),
                v128_of_i32x4([10, 20, 3, 0]),
            ),
            case_v128(
                "signed min treats the high bit as a sign",
                bin4(opv::I32X4_MIN_S, [-1, 0, 0, 0], [1, 0, 0, 0]),
                v128_of_i32x4([-1, 0, 0, 0]),
            ),
            case_v128(
                "i8x16.max_s in every byte",
                binb(opv::I8X16_MAX_S, bytes(0xff), bytes(0x01)),
                v128_of_i8x16([1; 16]),
            ),
            case_v128(
                "i8x16.min_s of the same pair",
                binb(opv::I8X16_MIN_S, bytes(0xff), bytes(0x01)),
                u128::MAX,
            ),
        ],
    )
});

wasm_test!(shifts, |ctx| {
    let shl = |lanes: [i32; 4], by: i32| {
        Expr::new()
            .v128_const_i32x4(lanes)
            .i32_const(by)
            .opv(opv::I32X4_SHL)
    };
    run_cases(
        ctx,
        "shifts",
        vec![
            case_v128(
                "shifting left by one doubles every lane",
                shl([1, 2, 3, 4], 1),
                v128_of_i32x4([2, 4, 6, 8]),
            ),
            case_v128(
                "a shift of zero changes nothing",
                shl([1, 2, 3, 4], 0),
                v128_of_i32x4([1, 2, 3, 4]),
            ),
            case_v128(
                "a shift of 32 is a shift of 0, because the count is modulo the lane width",
                shl([1, 2, 3, 4], 32),
                v128_of_i32x4([1, 2, 3, 4]),
            ),
            case_v128(
                "a shift of 33 is a shift of 1",
                shl([1, 2, 3, 4], 33),
                v128_of_i32x4([2, 4, 6, 8]),
            ),
            case_v128(
                "bits shifted past the top of a lane are lost, not carried up",
                shl([i32::MIN, 0, 0, 0], 1),
                0,
            ),
        ],
    )
});

wasm_test!(shift_signedness, |ctx| {
    let shr =
        |o: OpV, lanes: [i32; 4], by: i32| Expr::new().v128_const_i32x4(lanes).i32_const(by).opv(o);
    run_cases(
        ctx,
        "shift-signedness",
        vec![
            case_v128(
                "shr_s keeps the sign, so -8 >> 1 is -4",
                shr(opv::I32X4_SHR_S, [-8, -8, -8, -8], 1),
                v128_of_i32x4([-4; 4]),
            ),
            case_v128(
                "shr_u shifts a zero in, so the same lane becomes a large positive",
                shr(opv::I32X4_SHR_U, [-8, -8, -8, -8], 1),
                v128_of_i32x4([((-8i32 as u32) >> 1) as i32; 4]),
            ),
            case_v128(
                "on a positive lane the two agree",
                shr(opv::I32X4_SHR_S, [8, 8, 8, 8], 1),
                v128_of_i32x4([4; 4]),
            ),
            case_v128(
                "shr_s of -1 by anything is still -1",
                shr(opv::I32X4_SHR_S, [-1; 4], 31),
                u128::MAX,
            ),
        ],
    )
});

wasm_test!(f32x4_arithmetic, |ctx| {
    let f4 = |v: f32| Expr::new().f32_const(v).opv(opv::F32X4_SPLAT);
    let bits = |v: f32| v128_of_i32x4([v.to_bits() as i32; 4]);
    run_cases(
        ctx,
        "f32x4",
        vec![
            case_v128(
                "add",
                f4(1.5)
                    .f32_const(2.25)
                    .opv(opv::F32X4_SPLAT)
                    .opv(opv::F32X4_ADD),
                bits(3.75),
            ),
            case_v128(
                "sub",
                f4(1.5)
                    .f32_const(2.25)
                    .opv(opv::F32X4_SPLAT)
                    .opv(opv::F32X4_SUB),
                bits(-0.75),
            ),
            case_v128(
                "mul",
                f4(1.5)
                    .f32_const(2.0)
                    .opv(opv::F32X4_SPLAT)
                    .opv(opv::F32X4_MUL),
                bits(3.0),
            ),
            case_v128(
                "div",
                f4(3.0)
                    .f32_const(2.0)
                    .opv(opv::F32X4_SPLAT)
                    .opv(opv::F32X4_DIV),
                bits(1.5),
            ),
            case_v128(
                "dividing by zero gives an infinity, not a trap",
                f4(1.0)
                    .f32_const(0.0)
                    .opv(opv::F32X4_SPLAT)
                    .opv(opv::F32X4_DIV),
                bits(f32::INFINITY),
            ),
            case_v128(
                "f64x2 works the same in two lanes",
                Expr::new()
                    .f64_const(1.5)
                    .opv(opv::F64X2_SPLAT)
                    .f64_const(2.5)
                    .opv(opv::F64X2_SPLAT)
                    .opv(opv::F64X2_ADD),
                {
                    let b = 4.0f64.to_bits();
                    v128_of_i32x4([b as i32, (b >> 32) as i32, b as i32, (b >> 32) as i32])
                },
            ),
        ],
    )
});

wasm_test!(float_nan_lanes, |ctx| {
    let splat = |v: f32| Expr::new().f32_const(v).opv(opv::F32X4_SPLAT);
    run_cases(
        ctx,
        "f32x4-nan",
        vec![
            case_v128(
                "min of 1.0 and 2.0 is 1.0 in every lane",
                splat(1.0)
                    .f32_const(2.0)
                    .opv(opv::F32X4_SPLAT)
                    .opv(opv::F32X4_MIN),
                v128_of_i32x4([1.0f32.to_bits() as i32; 4]),
            ),
            case_v128(
                "max of the same pair is 2.0",
                splat(1.0)
                    .f32_const(2.0)
                    .opv(opv::F32X4_SPLAT)
                    .opv(opv::F32X4_MAX),
                v128_of_i32x4([2.0f32.to_bits() as i32; 4]),
            ),
            case_v128(
                "min of -0.0 and 0.0 is -0.0, which is a different bit pattern",
                splat(-0.0)
                    .f32_const(0.0)
                    .opv(opv::F32X4_SPLAT)
                    .opv(opv::F32X4_MIN),
                v128_of_i32x4([i32::MIN; 4]),
            ),
            case_v128(
                "max of the same two is +0.0",
                splat(-0.0)
                    .f32_const(0.0)
                    .opv(opv::F32X4_SPLAT)
                    .opv(opv::F32X4_MAX),
                0,
            ),
        ],
    )
});

/// A lanewise add, as its own module.
fn add_module() -> Module {
    let mut b = ModuleBuilder::new("i32x4-add");
    let ty = b.add_type(ftype(&[], &[ValType::V128]));
    let idx = b.add_func(
        ty,
        Func::new(bin4(opv::I32X4_ADD, [1, 2, 3, 4], [10, 20, 30, 40])),
    );
    b.export_func("f", idx).build()
}

/// Saturating against wrapping, as its own module.
fn saturate_module() -> Module {
    let mut b = ModuleBuilder::new("i8x16-add-sat");
    let ty = b.add_type(ftype(&[], &[ValType::V128]));
    let idx = b.add_func(
        ty,
        Func::new(binb(opv::I8X16_ADD_SAT_S, bytes(0x7f), bytes(0x01))),
    );
    b.export_func("f", idx).build()
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("Four additions in one instruction", add_module)
            .summary("`f() -> v128` adding i32x4 1 2 3 4 to 10 20 30 40")
            .command("run --invoke f mod.wasm")
            .output("3486039151236373408642838298635")
            .note(
                "11, 22, 33 and 44 packed low lane first: 11 + (22<<32) + (33<<64) + (44<<96). \
                 Nothing carries between the lanes, so a runtime that implements this as one \
                 128-bit addition gets the same answer here and the wrong one the moment a \
                 lane overflows.",
            ),
        ExampleSpec::module("Saturating where wrapping would be wrong", saturate_module)
            .summary("`f() -> v128` computing `i8x16.add_sat_s` of 0x7f and 0x01 in every byte")
            .command("run --invoke f mod.wasm")
            .output("169473963133173273960190490760135540607")
            .note(
                "Each byte clamps at 127 rather than wrapping to -128. The unsigned form \
                 clamps at 255 on the same bytes, and the plain `i8x16.add` wraps — three \
                 instructions, three answers, one pair of operands.",
            ),
    ]
}
