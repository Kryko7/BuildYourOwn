//! Stage 48 — Lanewise comparison, bitwise logic and the reductions.
//!
//! A vector comparison does not produce a boolean. It produces a **mask**: every lane of the
//! result is all-ones where the comparison held and all-zeros where it did not. That is what
//! makes `v128.bitselect` useful — a mask and its two candidate vectors pick a value per lane
//! with no branching anywhere — and it is why a runtime that writes 1 instead of all-ones
//! passes a naive test and then produces nonsense the moment the mask is used.
//!
//! The reductions are the way back out to a scalar. `v128.any_true` asks whether any bit in
//! the whole vector is set; `i32x4.all_true` asks whether every *lane* is non-zero — which is
//! a different question, and a vector of `[1, 0, 1, 1]` answers yes to the first and no to
//! the second. `bitmask` gathers one bit per lane, taken from the lane's sign bit, into an
//! ordinary i32.
//!
//! `i8x16.shuffle` takes its sixteen lane indices as immediates and reads from the two input
//! vectors as one 32-byte array; `i8x16.swizzle` takes them from a second vector at runtime
//! and answers zero for an index of 16 or more, rather than trapping.

use crate::examples::ExampleSpec;
use crate::stages::{case_i32, case_v128, run_cases, v128_of_i32x4, v128_of_i8x16, Stage, Test};
use crate::wasm::{ftype, opv, Expr, Func, Module, ModuleBuilder, OpV, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 48,
        slug: "v128_compare_and_bitwise",
        name: "Lanewise comparison, bitwise logic and the reductions",
        ext: true,
        hints: &[
            "A comparison writes all-ones or all-zeros per lane, never 1 and 0 — the result is a mask meant to be used with bitselect",
            "v128.bitselect(a, b, mask) takes a bit from a where the mask bit is 1 and from b where it is 0",
            "any_true asks about the whole vector's bits; all_true asks about each lane being non-zero — different questions with different answers",
            "i8x16.shuffle indexes the two operands as one 32-byte array with sixteen immediate indices; swizzle takes its indices at runtime and gives zero for any index past the end",
        ],
        examples,
        tests: vec![
            Test::new("a comparison produces all-ones, not one", compare_makes_a_mask).ext(),
            Test::new("every integer comparison agrees on that shape", comparison_family).ext(),
            Test::new("float comparison masks are integer masks", float_compare).ext(),
            Test::new("and, or, xor and not ignore lanes entirely", bitwise_ops).ext(),
            Test::new("andnot subtracts one vector's bits from another", andnot).ext(),
            Test::new("bitselect picks per bit from the mask", bitselect).ext(),
            Test::new("a comparison and a bitselect together are a lanewise if", mask_then_select).ext(),
            Test::new("any_true looks at the whole vector", any_true).ext(),
            Test::new("all_true looks at each lane in turn", all_true).ext(),
            Test::new("bitmask gathers one sign bit per lane", bitmask).ext(),
            Test::new("shuffle reads its immediates from both vectors", shuffle).ext(),
            Test::new("swizzle takes its indices at run time and zeroes the rest", swizzle).ext(),
        ],
    }
}

fn c4(lanes: [i32; 4]) -> Expr {
    Expr::new().v128_const_i32x4(lanes)
}

/// `a cmp b` for two i32x4 constants.
fn cmp4(o: OpV, a: [i32; 4], b: [i32; 4]) -> Expr {
    Expr::new().v128_const_i32x4(a).v128_const_i32x4(b).opv(o)
}

/// A lane that is all ones.
const ONES: i32 = -1;

wasm_test!(compare_makes_a_mask, |ctx| {
    run_cases(
        ctx,
        "mask-shape",
        vec![
            case_v128(
                "eq writes all-ones where the lanes match and all-zeros where they do not",
                cmp4(opv::I32X4_EQ, [1, 2, 3, 4], [1, 0, 3, 0]),
                v128_of_i32x4([ONES, 0, ONES, 0]),
            ),
            case_i32(
                "so one matching lane extracts as -1, not as 1",
                cmp4(opv::I32X4_EQ, [5, 0, 0, 0], [5, 0, 0, 0])
                    .opv_lane(opv::I32X4_EXTRACT_LANE, 0),
                -1,
            ),
            case_v128(
                "all four matching is every bit set",
                cmp4(opv::I32X4_EQ, [1, 2, 3, 4], [1, 2, 3, 4]),
                u128::MAX,
            ),
            case_v128(
                "none matching is zero",
                cmp4(opv::I32X4_EQ, [1, 2, 3, 4], [9, 9, 9, 9]),
                0,
            ),
        ],
    )
});

wasm_test!(comparison_family, |ctx| {
    run_cases(
        ctx,
        "cmp-family",
        vec![
            case_v128(
                "ne is the complement of eq",
                cmp4(opv::I32X4_NE, [1, 2, 3, 4], [1, 0, 3, 0]),
                v128_of_i32x4([0, ONES, 0, ONES]),
            ),
            case_v128(
                "lt_s compares as signed, so -1 is less than 1",
                cmp4(opv::I32X4_LT_S, [-1, 1, 0, 5], [1, -1, 0, 5]),
                v128_of_i32x4([ONES, 0, 0, 0]),
            ),
            case_v128(
                "gt_s is its mirror",
                cmp4(opv::I32X4_GT_S, [-1, 1, 0, 5], [1, -1, 0, 5]),
                v128_of_i32x4([0, ONES, 0, 0]),
            ),
            case_v128(
                "i8x16.eq works byte by byte",
                Expr::new()
                    .v128_const([0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1])
                    .v128_const([0; 16])
                    .opv(opv::I8X16_EQ),
                v128_of_i8x16([-1, 0, -1, 0, -1, 0, -1, 0, -1, 0, -1, 0, -1, 0, -1, 0]),
            ),
            case_v128(
                "i8x16.lt_s reads the byte as signed, so 0xff is below 0x01",
                Expr::new()
                    .v128_const([0xff; 16])
                    .v128_const([0x01; 16])
                    .opv(opv::I8X16_LT_S),
                u128::MAX,
            ),
        ],
    )
});

wasm_test!(float_compare, |ctx| {
    let splat = |v: f32| Expr::new().f32_const(v).opv(opv::F32X4_SPLAT);
    run_cases(
        ctx,
        "float-cmp",
        vec![
            case_v128(
                "f32x4.eq of equal lanes is an all-ones mask, not a float",
                splat(1.5)
                    .f32_const(1.5)
                    .opv(opv::F32X4_SPLAT)
                    .opv(opv::F32X4_EQ),
                u128::MAX,
            ),
            case_v128(
                "f32x4.lt of 1.0 against 2.0",
                splat(1.0)
                    .f32_const(2.0)
                    .opv(opv::F32X4_SPLAT)
                    .opv(opv::F32X4_LT),
                u128::MAX,
            ),
            case_v128(
                "-0.0 and 0.0 compare equal even though their bits differ",
                splat(-0.0)
                    .f32_const(0.0)
                    .opv(opv::F32X4_SPLAT)
                    .opv(opv::F32X4_EQ),
                u128::MAX,
            ),
            case_v128(
                "a NaN lane is equal to nothing, itself included",
                splat(f32::NAN)
                    .f32_const(f32::NAN)
                    .opv(opv::F32X4_SPLAT)
                    .opv(opv::F32X4_EQ),
                0,
            ),
        ],
    )
});

wasm_test!(bitwise_ops, |ctx| {
    let a = [0x0f0f_0f0fu32 as i32; 4];
    let b = [0x00ff_00ffu32 as i32; 4];
    run_cases(
        ctx,
        "bitwise",
        vec![
            case_v128(
                "and",
                Expr::new()
                    .v128_const_i32x4(a)
                    .v128_const_i32x4(b)
                    .opv(opv::V128_AND),
                v128_of_i32x4([0x000f_000f; 4]),
            ),
            case_v128(
                "or",
                Expr::new()
                    .v128_const_i32x4(a)
                    .v128_const_i32x4(b)
                    .opv(opv::V128_OR),
                v128_of_i32x4([0x0fff_0fffu32 as i32; 4]),
            ),
            case_v128(
                "xor",
                Expr::new()
                    .v128_const_i32x4(a)
                    .v128_const_i32x4(b)
                    .opv(opv::V128_XOR),
                v128_of_i32x4([0x0ff0_0ff0u32 as i32; 4]),
            ),
            case_v128(
                "not flips all 128 bits",
                Expr::new().v128_const_i32x4([0; 4]).opv(opv::V128_NOT),
                u128::MAX,
            ),
            case_v128(
                "xor with itself is zero, whatever the lanes held",
                Expr::new()
                    .v128_const_i32x4([1, -2, 3, -4])
                    .v128_const_i32x4([1, -2, 3, -4])
                    .opv(opv::V128_XOR),
                0,
            ),
        ],
    )
});

wasm_test!(andnot, |ctx| {
    run_cases(
        ctx,
        "andnot",
        vec![
            case_v128(
                "andnot(a, b) is a AND NOT b, in that order",
                Expr::new()
                    .v128_const_i32x4([0x0f0f_0f0f; 4])
                    .v128_const_i32x4([0x00ff_00ff; 4])
                    .opv(opv::V128_ANDNOT),
                v128_of_i32x4([0x0f00_0f00; 4]),
            ),
            case_v128(
                "and it does not commute: the operands the other way round differ",
                Expr::new()
                    .v128_const_i32x4([0x00ff_00ff; 4])
                    .v128_const_i32x4([0x0f0f_0f0f; 4])
                    .opv(opv::V128_ANDNOT),
                v128_of_i32x4([0x00f0_00f0; 4]),
            ),
            case_v128(
                "anything andnot itself is zero",
                Expr::new()
                    .v128_const_i32x4([-1; 4])
                    .v128_const_i32x4([-1; 4])
                    .opv(opv::V128_ANDNOT),
                0,
            ),
        ],
    )
});

wasm_test!(bitselect, |ctx| {
    // bitselect(a, b, mask): a where the mask bit is 1, b where it is 0.
    let sel = |a: [i32; 4], b: [i32; 4], m: [i32; 4]| {
        Expr::new()
            .v128_const_i32x4(a)
            .v128_const_i32x4(b)
            .v128_const_i32x4(m)
            .opv(opv::V128_BITSELECT)
    };
    run_cases(
        ctx,
        "bitselect",
        vec![
            case_v128(
                "an all-ones mask takes everything from the first vector",
                sel([1, 2, 3, 4], [9, 9, 9, 9], [-1; 4]),
                v128_of_i32x4([1, 2, 3, 4]),
            ),
            case_v128(
                "an all-zero mask takes everything from the second",
                sel([1, 2, 3, 4], [9, 9, 9, 9], [0; 4]),
                v128_of_i32x4([9, 9, 9, 9]),
            ),
            case_v128(
                "a per-lane mask picks per lane",
                sel([1, 2, 3, 4], [9, 9, 9, 9], [-1, 0, -1, 0]),
                v128_of_i32x4([1, 9, 3, 9]),
            ),
            case_v128(
                "the choice is per bit, not per lane",
                sel([-1; 4], [0; 4], [0x00ff_00ff; 4]),
                v128_of_i32x4([0x00ff_00ff; 4]),
            ),
        ],
    )
});

wasm_test!(mask_then_select, |ctx| {
    // The lanewise `max` written by hand: where a > b take a, else take b.
    let a = [5, 1, 9, -3];
    let b = [2, 7, 9, 4];
    run_cases(
        ctx,
        "mask-select",
        vec![
            case_v128(
                "compare, then select: a hand-written lanewise max",
                Expr::new()
                    .v128_const_i32x4(a)
                    .v128_const_i32x4(b)
                    .v128_const_i32x4(a)
                    .v128_const_i32x4(b)
                    .opv(opv::I32X4_GT_S)
                    .opv(opv::V128_BITSELECT),
                v128_of_i32x4([5, 7, 9, 4]),
            ),
            case_v128(
                "which is what i32x4.max_s does in one instruction",
                Expr::new()
                    .v128_const_i32x4(a)
                    .v128_const_i32x4(b)
                    .opv(opv::I32X4_MAX_S),
                v128_of_i32x4([5, 7, 9, 4]),
            ),
        ],
    )
});

wasm_test!(any_true, |ctx| {
    run_cases(
        ctx,
        "any-true",
        vec![
            case_i32(
                "a zero vector has no bit set",
                c4([0; 4]).opv(opv::V128_ANY_TRUE),
                0,
            ),
            case_i32(
                "one bit anywhere is enough",
                c4([0, 0, 1, 0]).opv(opv::V128_ANY_TRUE),
                1,
            ),
            case_i32(
                "an all-ones vector says yes",
                c4([-1; 4]).opv(opv::V128_ANY_TRUE),
                1,
            ),
            case_i32(
                "the answer is 1 or 0, not a mask",
                c4([1, 1, 1, 1]).opv(opv::V128_ANY_TRUE),
                1,
            ),
        ],
    )
});

wasm_test!(all_true, |ctx| {
    run_cases(
        ctx,
        "all-true",
        vec![
            case_i32(
                "every lane non-zero says yes",
                c4([1, 2, 3, 4]).opv(opv::I32X4_ALL_TRUE),
                1,
            ),
            case_i32(
                "one zero lane says no, even though other bits are set",
                c4([1, 0, 1, 1]).opv(opv::I32X4_ALL_TRUE),
                0,
            ),
            case_i32(
                "and any_true says yes to that very vector — they are different questions",
                c4([1, 0, 1, 1]).opv(opv::V128_ANY_TRUE),
                1,
            ),
            case_i32(
                "the shape decides what a lane is: as bytes, that vector has zero bytes in it",
                c4([1, 0, 1, 1]).opv(opv::I8X16_ALL_TRUE),
                0,
            ),
            case_i32(
                "a vector with no zero byte answers yes at every shape",
                Expr::new().v128_const([0x01; 16]).opv(opv::I8X16_ALL_TRUE),
                1,
            ),
        ],
    )
});

wasm_test!(bitmask, |ctx| {
    run_cases(
        ctx,
        "bitmask",
        vec![
            case_i32(
                "no lane has its sign bit set",
                c4([1, 2, 3, 4]).opv(opv::I32X4_BITMASK),
                0,
            ),
            case_i32(
                "lane 0 negative sets bit 0",
                c4([-1, 0, 0, 0]).opv(opv::I32X4_BITMASK),
                0b0001,
            ),
            case_i32(
                "lane 3 negative sets bit 3",
                c4([0, 0, 0, -1]).opv(opv::I32X4_BITMASK),
                0b1000,
            ),
            case_i32(
                "all four negative set all four bits",
                c4([-1; 4]).opv(opv::I32X4_BITMASK),
                0b1111,
            ),
            case_i32(
                "it is the sign bit that counts, not the whole lane",
                c4([i32::MIN, 1, i32::MIN, 1]).opv(opv::I32X4_BITMASK),
                0b0101,
            ),
            case_i32(
                "i8x16.bitmask gathers sixteen of them",
                Expr::new().v128_const([0x80; 16]).opv(opv::I8X16_BITMASK),
                0xffff,
            ),
        ],
    )
});

wasm_test!(shuffle, |ctx| {
    let a = Expr::new().v128_const([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);
    let both = |lanes: [u8; 16]| {
        a.clone()
            .v128_const([
                16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
            ])
            .i8x16_shuffle(lanes)
    };
    run_cases(
        ctx,
        "shuffle",
        vec![
            case_v128(
                "the identity shuffle gives the first vector back",
                both([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]),
                v128_of_i8x16([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]),
            ),
            case_v128(
                "indices 16..31 read from the second vector",
                both([
                    16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
                ]),
                v128_of_i8x16([
                    16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
                ]),
            ),
            case_v128(
                "the two can be interleaved",
                both([0, 16, 1, 17, 2, 18, 3, 19, 4, 20, 5, 21, 6, 22, 7, 23]),
                v128_of_i8x16([0, 16, 1, 17, 2, 18, 3, 19, 4, 20, 5, 21, 6, 22, 7, 23]),
            ),
            case_v128("a lane can be taken more than once", both([0; 16]), 0),
            case_v128(
                "and the order can be reversed",
                both([15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0]),
                v128_of_i8x16([15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0]),
            ),
        ],
    )
});

wasm_test!(swizzle, |ctx| {
    let data = Expr::new().v128_const([
        100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115,
    ]);
    run_cases(
        ctx,
        "swizzle",
        vec![
            case_v128(
                "each index picks a byte of the first vector",
                data.clone().v128_const([0; 16]).opv(opv::I8X16_SWIZZLE),
                v128_of_i8x16([100; 16]),
            ),
            case_v128(
                "the identity indices give the vector back",
                data.clone()
                    .v128_const([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15])
                    .opv(opv::I8X16_SWIZZLE),
                v128_of_i8x16([
                    100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115,
                ]),
            ),
            case_v128(
                "an index of 16 or more gives zero rather than trapping",
                data.clone().v128_const([16; 16]).opv(opv::I8X16_SWIZZLE),
                0,
            ),
            case_v128(
                "including 0xff, which is out of range and not a negative index",
                data.v128_const([0xff; 16]).opv(opv::I8X16_SWIZZLE),
                0,
            ),
        ],
    )
});

/// A comparison mask, as its own module.
fn mask_module() -> Module {
    let mut b = ModuleBuilder::new("i32x4-eq-mask");
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(
        ty,
        Func::new(
            cmp4(opv::I32X4_EQ, [5, 0, 0, 0], [5, 0, 0, 0]).opv_lane(opv::I32X4_EXTRACT_LANE, 0),
        ),
    );
    b.export_func("f", idx).build()
}

/// any_true against all_true on one vector, as its own module.
fn all_true_module() -> Module {
    let mut b = ModuleBuilder::new("i32x4-all-true");
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(ty, Func::new(c4([1, 0, 1, 1]).opv(opv::I32X4_ALL_TRUE)));
    b.export_func("f", idx).build()
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("A comparison is a mask, not a boolean", mask_module)
            .summary("`f() -> i32` returning lane 0 of `i32x4.eq` on two equal vectors")
            .command("run --invoke f mod.wasm")
            .output("-1")
            .note(
                "A lane that compares equal is set to all ones, which read back as an i32 is \
                 -1. A runtime that writes 1 here passes any test that only asks 'is it \
                 non-zero' and then produces nonsense the first time the mask reaches \
                 v128.bitselect.",
            ),
        ExampleSpec::module("all_true is not any_true", all_true_module)
            .summary("`f() -> i32` returning `i32x4.all_true` of the vector 1 0 1 1")
            .command("run --invoke f mod.wasm")
            .output("0")
            .note(
                "One zero lane makes all_true say no. v128.any_true on the very same vector \
                 says yes, because it asks whether any bit in the 128 is set. Two reductions, \
                 two questions; mixing them up is the usual bug.",
            ),
    ]
}
