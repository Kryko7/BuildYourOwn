//! Stage 46 — v128: constants, lanes and splats.
//!
//! A `v128` is 128 bits with no type of its own: the same sixteen bytes are read as sixteen
//! i8 lanes, eight i16, four i32, two i64, four f32 or two f64 depending only on which
//! instruction touches them. Everything in this stage follows from that and from one more
//! rule: **lane 0 is the lowest-addressed byte**, so `v128.const` writes its bytes in memory
//! order and a runtime that prints a `v128` prints lane 0 at the *bottom* of the number.
//!
//! The extract instructions are where the shapes stop being interchangeable. `i32x4` and
//! `i64x2` extract exactly; `i8x16` and `i16x8` come in signed and unsigned forms because
//! the lane is narrower than the i32 it is extracted into, so the runtime has to choose
//! between sign- and zero-extension. A runtime that implements only one of the two passes
//! half of this stage.

use crate::examples::ExampleSpec;
use crate::stages::{
    case_i32, case_i64, case_v128, run_cases, v128_of_i16x8, v128_of_i32x4, v128_of_i8x16, Stage,
    Test,
};
use crate::wasm::{ftype, opv, Expr, Func, Limits, Module, ModuleBuilder, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 46,
        slug: "v128_lanes",
        name: "v128: constants, lanes and splats",
        ext: true,
        hints: &[
            "A v128 value is sixteen bytes and nothing else; the shape (i8x16, i32x4, f64x2) lives in the instruction, not in the value",
            "v128.const carries its sixteen bytes as immediates in memory order, so lane 0 is the first byte and the low end of the printed number",
            "extract_lane on i8x16 and i16x8 has _s and _u forms because the lane is narrower than the i32 it lands in; i32x4 and i64x2 need no such choice",
            "splat copies one scalar into every lane; i8x16.splat takes an i32 and keeps only its low eight bits",
        ],
        examples,
        tests: vec![
            Test::new("a v128 constant comes back byte for byte", const_round_trips).ext(),
            Test::new("lane 0 is the lowest byte of the value", lane_zero_is_lowest).ext(),
            Test::new("i32x4 lanes are extracted exactly", i32x4_extract).ext(),
            Test::new("i64x2 halves the vector", i64x2_extract).ext(),
            Test::new("a narrow lane is sign- or zero-extended on the way out", narrow_extract_signedness).ext(),
            Test::new("replace_lane changes one lane and leaves the rest", replace_lane).ext(),
            Test::new("splat fills every lane with the same scalar", splat_fills).ext(),
            Test::new("splat of a wide scalar keeps only the low bits of each lane", splat_truncates).ext(),
            Test::new("float lanes are the same bytes read differently", float_lanes_are_bytes).ext(),
            Test::new("a v128 survives a round trip through memory", memory_round_trip).ext(),
        ],
    }
}

/// `v128.const` of four i32 lanes.
fn c4(lanes: [i32; 4]) -> Expr {
    Expr::new().v128_const_i32x4(lanes)
}

const MIXED: [i32; 4] = [1, -2, 3, -4];

wasm_test!(const_round_trips, |ctx| {
    run_cases(
        ctx,
        "v128-const",
        vec![
            case_v128("all zero lanes are zero", c4([0, 0, 0, 0]), 0),
            case_v128(
                "four small positive lanes",
                c4([1, 2, 3, 4]),
                v128_of_i32x4([1, 2, 3, 4]),
            ),
            case_v128(
                "negative lanes are the two's complement bits, not a sign anywhere else",
                c4([-1, 0, 0, 0]),
                v128_of_i32x4([-1, 0, 0, 0]),
            ),
            case_v128(
                "every bit set is the largest u128",
                c4([-1, -1, -1, -1]),
                u128::MAX,
            ),
            case_v128("a mixed vector", c4(MIXED), v128_of_i32x4(MIXED)),
        ],
    )
});

wasm_test!(lane_zero_is_lowest, |ctx| {
    run_cases(
        ctx,
        "lane-order",
        vec![
            case_v128("a 1 in lane 0 is the value 1", c4([1, 0, 0, 0]), 1),
            case_v128(
                "a 1 in lane 3 is 1 shifted up by 96 bits",
                c4([0, 0, 0, 1]),
                1u128 << 96,
            ),
            case_i32(
                "extracting lane 0 of that gives 0, not 1",
                c4([0, 0, 0, 1]).opv_lane(opv::I32X4_EXTRACT_LANE, 0),
                0,
            ),
            case_i32(
                "extracting lane 3 gives the 1",
                c4([0, 0, 0, 1]).opv_lane(opv::I32X4_EXTRACT_LANE, 3),
                1,
            ),
        ],
    )
});

wasm_test!(i32x4_extract, |ctx| {
    let cases = (0..4)
        .map(|i| {
            case_i32(
                format!("lane {i} of the mixed vector"),
                c4(MIXED).opv_lane(opv::I32X4_EXTRACT_LANE, i as u8),
                MIXED[i],
            )
        })
        .chain([
            case_i32(
                "a lane holding i32::MIN comes back as i32::MIN",
                c4([i32::MIN, 0, 0, 0]).opv_lane(opv::I32X4_EXTRACT_LANE, 0),
                i32::MIN,
            ),
            case_i32(
                "a lane holding i32::MAX comes back as i32::MAX",
                c4([0, i32::MAX, 0, 0]).opv_lane(opv::I32X4_EXTRACT_LANE, 1),
                i32::MAX,
            ),
        ])
        .collect();
    run_cases(ctx, "i32x4-extract", cases)
});

wasm_test!(i64x2_extract, |ctx| {
    run_cases(
        ctx,
        "i64x2-extract",
        vec![
            case_i64(
                "the low half of a vector whose lanes 0 and 1 are 1 and 0",
                c4([1, 0, 0, 0]).opv_lane(opv::I64X2_EXTRACT_LANE, 0),
                1,
            ),
            case_i64(
                "two i32 lanes make one i64 lane, low lane first",
                c4([0, 1, 0, 0]).opv_lane(opv::I64X2_EXTRACT_LANE, 0),
                1i64 << 32,
            ),
            case_i64(
                "the high half is lanes 2 and 3",
                c4([0, 0, -1, -1]).opv_lane(opv::I64X2_EXTRACT_LANE, 1),
                -1,
            ),
        ],
    )
});

wasm_test!(narrow_extract_signedness, |ctx| {
    // Lane 0 holds 0xff: -1 read signed, 255 read unsigned.
    let byte_high = Expr::new().v128_const([0xff; 16]);
    let half_high = Expr::new().v128_const_i32x4([-1, -1, -1, -1]);
    run_cases(
        ctx,
        "narrow-extract",
        vec![
            case_i32(
                "i8x16.extract_lane_s sign-extends 0xff to -1",
                byte_high.clone().opv_lane(opv::I8X16_EXTRACT_LANE_S, 0),
                -1,
            ),
            case_i32(
                "i8x16.extract_lane_u zero-extends the same byte to 255",
                byte_high.clone().opv_lane(opv::I8X16_EXTRACT_LANE_U, 0),
                255,
            ),
            case_i32(
                "i16x8.extract_lane_s sign-extends 0xffff to -1",
                half_high.clone().opv_lane(opv::I16X8_EXTRACT_LANE_S, 0),
                -1,
            ),
            case_i32(
                "i16x8.extract_lane_u zero-extends it to 65535",
                half_high.opv_lane(opv::I16X8_EXTRACT_LANE_U, 0),
                65535,
            ),
            case_i32(
                "a byte below 0x80 is the same either way",
                Expr::new()
                    .v128_const([0x7f; 16])
                    .opv_lane(opv::I8X16_EXTRACT_LANE_S, 5),
                127,
            ),
        ],
    )
});

wasm_test!(replace_lane, |ctx| {
    run_cases(
        ctx,
        "replace-lane",
        vec![
            case_v128(
                "replacing lane 0 leaves the other three",
                c4([1, 2, 3, 4])
                    .i32_const(99)
                    .opv_lane(opv::I32X4_REPLACE_LANE, 0),
                v128_of_i32x4([99, 2, 3, 4]),
            ),
            case_v128(
                "replacing lane 3 touches only the top",
                c4([1, 2, 3, 4])
                    .i32_const(-1)
                    .opv_lane(opv::I32X4_REPLACE_LANE, 3),
                v128_of_i32x4([1, 2, 3, -1]),
            ),
            case_v128(
                "replacing one byte of an all-zero vector",
                Expr::new()
                    .v128_const([0; 16])
                    .i32_const(0xff)
                    .opv_lane(opv::I8X16_REPLACE_LANE, 1),
                v128_of_i8x16([0, -1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            ),
            case_i32(
                "replace then extract gives back what was put in",
                c4([0; 4])
                    .i32_const(-12345)
                    .opv_lane(opv::I32X4_REPLACE_LANE, 2)
                    .opv_lane(opv::I32X4_EXTRACT_LANE, 2),
                -12345,
            ),
        ],
    )
});

wasm_test!(splat_fills, |ctx| {
    run_cases(
        ctx,
        "splat",
        vec![
            case_v128(
                "i32x4.splat of 7 puts 7 in all four lanes",
                Expr::new().i32_const(7).opv(opv::I32X4_SPLAT),
                v128_of_i32x4([7, 7, 7, 7]),
            ),
            case_v128(
                "i32x4.splat of -1 sets every bit",
                Expr::new().i32_const(-1).opv(opv::I32X4_SPLAT),
                u128::MAX,
            ),
            case_v128(
                "i8x16.splat of 1 puts a 1 in every byte",
                Expr::new().i32_const(1).opv(opv::I8X16_SPLAT),
                v128_of_i8x16([1; 16]),
            ),
            case_v128(
                "i16x8.splat of -2 fills eight half-words",
                Expr::new().i32_const(-2).opv(opv::I16X8_SPLAT),
                v128_of_i16x8([-2; 8]),
            ),
            case_v128(
                "i64x2.splat of 1 fills both halves",
                Expr::new().i64_const(1).opv(opv::I64X2_SPLAT),
                v128_of_i32x4([1, 0, 1, 0]),
            ),
        ],
    )
});

wasm_test!(splat_truncates, |ctx| {
    run_cases(
        ctx,
        "splat-truncate",
        vec![
            case_v128(
                "i8x16.splat keeps only the low eight bits of its i32",
                Expr::new().i32_const(0x1234_5601).opv(opv::I8X16_SPLAT),
                v128_of_i8x16([1; 16]),
            ),
            case_v128(
                "i16x8.splat keeps only the low sixteen",
                Expr::new().i32_const(0x1234_0001).opv(opv::I16X8_SPLAT),
                v128_of_i16x8([1; 8]),
            ),
            case_i32(
                "so a byte splat of 256 is a vector of zeroes",
                Expr::new()
                    .i32_const(256)
                    .opv(opv::I8X16_SPLAT)
                    .opv_lane(opv::I8X16_EXTRACT_LANE_U, 0),
                0,
            ),
        ],
    )
});

wasm_test!(float_lanes_are_bytes, |ctx| {
    run_cases(
        ctx,
        "float-lanes",
        vec![
            case_v128(
                "f32x4.splat of 1.0 is four copies of 0x3f800000",
                Expr::new().f32_const(1.0).opv(opv::F32X4_SPLAT),
                v128_of_i32x4([0x3f80_0000; 4]),
            ),
            case_v128(
                "f64x2.splat of 1.0 is two copies of 0x3ff0000000000000",
                Expr::new().f64_const(1.0).opv(opv::F64X2_SPLAT),
                v128_of_i32x4([0, 0x3ff0_0000, 0, 0x3ff0_0000]),
            ),
            case_i32(
                "the same vector read as i32 lanes gives the bit pattern",
                Expr::new()
                    .f32_const(1.0)
                    .opv(opv::F32X4_SPLAT)
                    .opv_lane(opv::I32X4_EXTRACT_LANE, 0),
                0x3f80_0000,
            ),
            case_v128(
                "negative zero differs from zero in exactly one bit",
                Expr::new().f32_const(-0.0).opv(opv::F32X4_SPLAT),
                v128_of_i32x4([i32::MIN; 4]),
            ),
        ],
    )
});

wasm_test!(memory_round_trip, |ctx| {
    let base = ModuleBuilder::new("v128-memory").memory(Limits::min(1));
    crate::stages::run_cases_with(
        ctx,
        base,
        vec![
            case_v128(
                "stored at offset 0 and loaded back",
                Expr::new()
                    .i32_const(0)
                    .v128_const_i32x4(MIXED)
                    .v128_store(0)
                    .i32_const(0)
                    .v128_load(0),
                v128_of_i32x4(MIXED),
            ),
            case_i32(
                "a v128 store writes sixteen bytes, readable as i32 loads",
                Expr::new()
                    .i32_const(0)
                    .v128_const_i32x4([10, 20, 30, 40])
                    .v128_store(0)
                    .i32_const(8)
                    .i32_load(0),
                30,
            ),
            case_v128(
                "an unaligned address is allowed, not a trap",
                Expr::new()
                    .i32_const(1)
                    .v128_const_i32x4([5, 6, 7, 8])
                    .v128_store(0)
                    .i32_const(1)
                    .v128_load(0),
                v128_of_i32x4([5, 6, 7, 8]),
            ),
        ],
    )
});

/// A splat, as its own module.
fn splat_module() -> Module {
    let mut b = ModuleBuilder::new("v128-splat");
    let ty = b.add_type(ftype(&[], &[ValType::V128]));
    let idx = b.add_func(
        ty,
        Func::new(Expr::new().i32_const(7).opv(opv::I32X4_SPLAT)),
    );
    b.export_func("f", idx).build()
}

/// One lane read out of a constant, as its own module.
fn extract_module() -> Module {
    let mut b = ModuleBuilder::new("v128-extract");
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(
        ty,
        Func::new(
            Expr::new()
                .v128_const_i32x4([1, 2, 3, 4])
                .opv_lane(opv::I32X4_EXTRACT_LANE, 2),
        ),
    );
    b.export_func("f", idx).build()
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("Splatting a scalar into four lanes", splat_module)
            .summary("`f() -> v128` returning `i32.const 7, i32x4.splat`")
            .command("run --invoke f mod.wasm")
            .output("554597137728977571700839284743")
            .note(
                "A v128 result is printed as one unsigned 128-bit decimal with lane 0 at the \
                 low end, so four lanes of 7 are 7 + (7<<32) + (7<<64) + (7<<96). The \
                 instruction is 0xfd 0x11 — every vector opcode is 0xfd followed by a uLEB128 \
                 sub-index, so a decoder that reads one byte per opcode stops here.",
            ),
        ExampleSpec::module("Reading one lane back out", extract_module)
            .summary("`f() -> i32` returning `v128.const i32x4 1 2 3 4, i32x4.extract_lane 2`")
            .command("run --invoke f mod.wasm")
            .output("3")
            .note(
                "v128.const carries sixteen immediate bytes in memory order, and the lane \
                 index is one more immediate byte after the opcode. Lane 2 of 1 2 3 4 is 3: \
                 lane 0 is the first four bytes, not the last.",
            ),
    ]
}
