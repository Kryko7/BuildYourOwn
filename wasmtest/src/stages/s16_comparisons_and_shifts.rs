//! Stage 16 — Comparisons, shifts and rotates.
//!
//! Two families that look easy and are not.
//!
//! A comparison answers an `i32` that is exactly 0 or exactly 1, whatever the width of its
//! operands: `i64.lt_s` takes two 64-bit values and gives back a 32-bit boolean. Half of
//! them are signed and half unsigned, and the pair disagree on precisely the values where it
//! matters: `-1 lt_s 1` is 1, and `-1 lt_u 1` is 0, because unsigned that -1 is 4294967295.
//! A runtime that keeps its integers in a signed register and forgets to reinterpret will
//! pass every test that uses small positive numbers.
//!
//! A shift count is taken **modulo the width** — not clamped, not a trap, not undefined.
//! `1 << 32` is 1 again, `1 << 33` is 2, and a negative count is its unsigned value modulo
//! 32 (or 64), so `1 << -1` is `1 << 31`. Only the low five bits of the count matter at
//! `i32` and the low six at `i64`. `shr_s` shifts the sign bit down into the vacated places
//! and `shr_u` shifts in zeros; `shr_s` is emphatically *not* division, because it rounds
//! towards minus infinity — `-3 shr_s 1` is -2 where `-3 div_s 2` is -1.
//!
//! `rotl` and `rotr` move the bits that fall off one end back in at the other. They are
//! inverses of each other, a rotate by the width is the identity, and their count is taken
//! modulo the width just as a shift's is.

use crate::examples::ExampleSpec;
use crate::stages::{case_i32, case_i64, run_cases, Stage, Test};
use crate::wasm::{ftype, op, Expr, Func, Module, ModuleBuilder, Op, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 16,
        slug: "comparisons_and_shifts",
        name: "Comparisons, shifts and rotates",
        ext: false,
        hints: &[
            "Every comparison returns an i32 that is 0 or 1 — never -1, never the operand — even when the operands are i64",
            "The signed and unsigned comparisons differ only in how they read the top bit: -1 lt_s 1 is 1 and -1 lt_u 1 is 0",
            "Mask the shift count: count & 31 for i32 and count & 63 for i64, so 1 shl 32 is 1 and 1 shl 33 is 2, and a negative count is its unsigned value masked the same way",
            "shr_s copies the sign bit into the top and shr_u writes zeros there; shr_s is not division, because -3 shr_s 1 is -2 while -3 div_s 2 is -1",
        ],
        examples,
        tests: vec![
            Test::new("eqz, eq and ne answer exactly 0 or exactly 1", equality),
            Test::new("lt and gt disagree about the top bit", ordering),
            Test::new("le and ge are the strict comparisons with equality allowed", ordering_or_equal),
            Test::new("the sixty-four-bit comparisons still answer a thirty-two-bit boolean", i64_comparisons),
            Test::new("shl and shr_u move bits without regard for the sign", logical_shifts),
            Test::new("shr_s copies the sign bit down from the top", arithmetic_shift),
            Test::new("the shift count is taken modulo the width", counts_wrap),
            Test::new("rotl and rotr carry the bits round the end", rotates),
            Test::new("the sixty-four-bit shifts and rotates count modulo sixty-four", i64_shifts),
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

/// `op a` with one `i32` operand.
fn un32(o: Op, a: i32) -> Expr {
    Expr::new().i32_const(a).op(o)
}

/// `op a` with one `i64` operand.
fn un64(o: Op, a: i64) -> Expr {
    Expr::new().i64_const(a).op(o)
}

/// `v shl n` the way the spec computes it: the count masked to five bits.
fn shl(v: i32, n: i32) -> i32 {
    v.wrapping_shl(n as u32)
}

/// `v shr_u n`: zeros shifted in at the top.
fn shr_u(v: i32, n: i32) -> i32 {
    (v as u32).wrapping_shr(n as u32) as i32
}

/// `v shr_s n`: the sign bit shifted in at the top.
fn shr_s(v: i32, n: i32) -> i32 {
    v.wrapping_shr(n as u32)
}

/// An arbitrary value with every nibble different, so a lost bit shows up.
const PATTERN: i32 = 0x1234_5678;
/// The same idea at sixty-four bits.
const PATTERN64: i64 = 0x0123_4567_89ab_cdef;

wasm_test!(equality, |ctx| {
    run_cases(
        ctx,
        "i32-equality",
        vec![
            case_i32(
                "0 is the only value eqz answers 1 for",
                un32(op::I32_EQZ, 0),
                1,
            ),
            case_i32("eqz of 1 is 0", un32(op::I32_EQZ, 1), 0),
            case_i32("eqz of -1 is 0", un32(op::I32_EQZ, -1), 0),
            case_i32(
                "eqz of i32::MIN is 0: the sign bit is a bit like any other",
                un32(op::I32_EQZ, i32::MIN),
                0,
            ),
            case_i32("0 eq 0 is 1", bin32(op::I32_EQ, 0, 0), 1),
            case_i32("-1 eq -1 is 1", bin32(op::I32_EQ, -1, -1), 1),
            case_i32("-1 eq 1 is 0", bin32(op::I32_EQ, -1, 1), 0),
            case_i32(
                "i32::MIN eq i32::MIN is 1",
                bin32(op::I32_EQ, i32::MIN, i32::MIN),
                1,
            ),
            case_i32(
                "i32::MIN eq i32::MAX is 0",
                bin32(op::I32_EQ, i32::MIN, i32::MAX),
                0,
            ),
            case_i32("0 ne 0 is 0", bin32(op::I32_NE, 0, 0), 0),
            case_i32("-1 ne 1 is 1", bin32(op::I32_NE, -1, 1), 1),
            case_i32("i32::MIN ne 0 is 1", bin32(op::I32_NE, i32::MIN, 0), 1),
            case_i32(
                "a true comparison is exactly 1, so two of them add up to 2 and not to -2",
                bin32(op::I32_EQ, 7, 7)
                    .i32_const(9)
                    .i32_const(9)
                    .op(op::I32_EQ)
                    .op(op::I32_ADD),
                2,
            ),
        ],
    )
});

wasm_test!(ordering, |ctx| {
    run_cases(
        ctx,
        "i32-ordering",
        vec![
            case_i32(
                "-1 lt_s 1 is 1: signed, -1 is below 1",
                bin32(op::I32_LT_S, -1, 1),
                1,
            ),
            case_i32(
                "-1 lt_u 1 is 0: unsigned, that same -1 is 4294967295",
                bin32(op::I32_LT_U, -1, 1),
                0,
            ),
            case_i32("1 lt_s -1 is 0", bin32(op::I32_LT_S, 1, -1), 0),
            case_i32("1 lt_u -1 is 1", bin32(op::I32_LT_U, 1, -1), 1),
            case_i32("-1 gt_s 1 is 0", bin32(op::I32_GT_S, -1, 1), 0),
            case_i32("-1 gt_u 1 is 1", bin32(op::I32_GT_U, -1, 1), 1),
            case_i32(
                "i32::MIN lt_s i32::MAX is 1: the range runs from MIN to MAX",
                bin32(op::I32_LT_S, i32::MIN, i32::MAX),
                1,
            ),
            case_i32(
                "i32::MIN lt_u i32::MAX is 0: unsigned it is 2147483648, the larger of the two",
                bin32(op::I32_LT_U, i32::MIN, i32::MAX),
                0,
            ),
            case_i32("i32::MIN gt_u 0 is 1", bin32(op::I32_GT_U, i32::MIN, 0), 1),
            case_i32(
                "-2 lt_s -1 is 1, and lt_u agrees for once",
                bin32(op::I32_LT_S, -2, -1),
                1,
            ),
            case_i32(
                "-2 lt_u -1 is 1 as well: 4294967294 really is below 4294967295",
                bin32(op::I32_LT_U, -2, -1),
                1,
            ),
            case_i32(
                "0 lt_s 0 is 0: the comparison is strict",
                bin32(op::I32_LT_S, 0, 0),
                0,
            ),
            case_i32(
                "0 gt_s 0 is 0 for the same reason",
                bin32(op::I32_GT_S, 0, 0),
                0,
            ),
            case_i32("7 gt_s 5 is 1", bin32(op::I32_GT_S, 7, 5), 1),
        ],
    )
});

wasm_test!(ordering_or_equal, |ctx| {
    run_cases(
        ctx,
        "i32-ordering-or-equal",
        vec![
            case_i32(
                "0 le_s 0 is 1, where lt_s was 0",
                bin32(op::I32_LE_S, 0, 0),
                1,
            ),
            case_i32("0 ge_s 0 is 1", bin32(op::I32_GE_S, 0, 0), 1),
            case_i32("-1 le_s -1 is 1", bin32(op::I32_LE_S, -1, -1), 1),
            case_i32("-1 ge_u -1 is 1", bin32(op::I32_GE_U, -1, -1), 1),
            case_i32("-1 le_s 1 is 1", bin32(op::I32_LE_S, -1, 1), 1),
            case_i32("-1 le_u 1 is 0", bin32(op::I32_LE_U, -1, 1), 0),
            case_i32("-1 ge_s 1 is 0", bin32(op::I32_GE_S, -1, 1), 0),
            case_i32("-1 ge_u 1 is 1", bin32(op::I32_GE_U, -1, 1), 1),
            case_i32(
                "i32::MAX le_s i32::MIN is 0",
                bin32(op::I32_LE_S, i32::MAX, i32::MIN),
                0,
            ),
            case_i32(
                "i32::MAX le_u i32::MIN is 1: 2147483647 is below 2147483648",
                bin32(op::I32_LE_U, i32::MAX, i32::MIN),
                1,
            ),
            case_i32(
                "i32::MIN ge_s i32::MIN is 1",
                bin32(op::I32_GE_S, i32::MIN, i32::MIN),
                1,
            ),
            case_i32("5 le_s 4 is 0", bin32(op::I32_LE_S, 5, 4), 0),
            case_i32("4 le_s 5 is 1", bin32(op::I32_LE_S, 4, 5), 1),
        ],
    )
});

wasm_test!(i64_comparisons, |ctx| {
    run_cases(
        ctx,
        "i64-comparisons",
        vec![
            case_i32("i64 eqz of 0 is 1", un64(op::I64_EQZ, 0), 1),
            case_i32("i64 eqz of i64::MIN is 0", un64(op::I64_EQZ, i64::MIN), 0),
            case_i32(
                "i64 eqz of 4294967296 is 0, though its low half is empty",
                un64(op::I64_EQZ, 4294967296),
                0,
            ),
            case_i32(
                "i64::MAX eq i64::MAX is 1",
                bin64(op::I64_EQ, i64::MAX, i64::MAX),
                1,
            ),
            case_i32(
                "i64::MAX ne i64::MIN is 1",
                bin64(op::I64_NE, i64::MAX, i64::MIN),
                1,
            ),
            case_i32(
                "-1 lt_s 1 is 1 at sixty-four bits",
                bin64(op::I64_LT_S, -1, 1),
                1,
            ),
            case_i32(
                "-1 lt_u 1 is 0: unsigned that -1 is 18446744073709551615",
                bin64(op::I64_LT_U, -1, 1),
                0,
            ),
            case_i32("-1 gt_s 1 is 0", bin64(op::I64_GT_S, -1, 1), 0),
            case_i32("-1 gt_u 1 is 1", bin64(op::I64_GT_U, -1, 1), 1),
            case_i32("-1 le_s 1 is 1", bin64(op::I64_LE_S, -1, 1), 1),
            case_i32("-1 le_u 1 is 0", bin64(op::I64_LE_U, -1, 1), 0),
            case_i32("-1 ge_s 1 is 0", bin64(op::I64_GE_S, -1, 1), 0),
            case_i32("-1 ge_u 1 is 1", bin64(op::I64_GE_U, -1, 1), 1),
            case_i32(
                "4294967296 gt_s 1 is 1, which a comparison done at thirty-two bits would miss",
                bin64(op::I64_GT_S, 4294967296, 1),
                1,
            ),
            case_i32(
                "i64::MIN lt_s 0 is 1 but lt_u 0 is 0",
                bin64(op::I64_LT_S, i64::MIN, 0)
                    .i64_const(i64::MIN)
                    .i64_const(0)
                    .op(op::I64_LT_U)
                    .op(op::I32_SUB),
                1,
            ),
        ],
    )
});

wasm_test!(logical_shifts, |ctx| {
    run_cases(
        ctx,
        "i32-logical-shifts",
        vec![
            case_i32("1 shl 0 is 1", bin32(op::I32_SHL, 1, 0), 1),
            case_i32("1 shl 1 is 2", bin32(op::I32_SHL, 1, 1), 2),
            case_i32(
                "1 shl 31 is i32::MIN: the bit lands on the sign",
                bin32(op::I32_SHL, 1, 31),
                i32::MIN,
            ),
            case_i32("-1 shl 1 is -2", bin32(op::I32_SHL, -1, 1), -2),
            case_i32(
                "-1 shl 31 is i32::MIN: every other bit has fallen off the top",
                bin32(op::I32_SHL, -1, 31),
                i32::MIN,
            ),
            case_i32(
                "i32::MIN shl 1 is 0: the only bit there was has gone",
                bin32(op::I32_SHL, i32::MIN, 1),
                0,
            ),
            case_i32(
                "0x12345678 shl 4 is 0x23456780",
                bin32(op::I32_SHL, PATTERN, 4),
                shl(PATTERN, 4),
            ),
            case_i32("1 shr_u 1 is 0", bin32(op::I32_SHR_U, 1, 1), 0),
            case_i32(
                "-1 shr_u 1 is 2147483647, not -1: shr_u brings in a zero",
                bin32(op::I32_SHR_U, -1, 1),
                2147483647,
            ),
            case_i32("-1 shr_u 31 is 1", bin32(op::I32_SHR_U, -1, 31), 1),
            case_i32(
                "i32::MIN shr_u 1 is 1073741824",
                bin32(op::I32_SHR_U, i32::MIN, 1),
                1073741824,
            ),
            case_i32(
                "i32::MIN shr_u 31 is 1: the sign bit has walked all the way down",
                bin32(op::I32_SHR_U, i32::MIN, 31),
                1,
            ),
            case_i32(
                "0x12345678 shr_u 4 is 0x01234567",
                bin32(op::I32_SHR_U, PATTERN, 4),
                shr_u(PATTERN, 4),
            ),
        ],
    )
});

wasm_test!(arithmetic_shift, |ctx| {
    run_cases(
        ctx,
        "i32-arithmetic-shift",
        vec![
            case_i32(
                "8 shr_s 2 is 2, same as shr_u",
                bin32(op::I32_SHR_S, 8, 2),
                2,
            ),
            case_i32("-8 shr_s 2 is -2", bin32(op::I32_SHR_S, -8, 2), -2),
            case_i32(
                "-1 shr_s 1 is -1: a word of ones stays a word of ones",
                bin32(op::I32_SHR_S, -1, 1),
                -1,
            ),
            case_i32("-1 shr_s 31 is still -1", bin32(op::I32_SHR_S, -1, 31), -1),
            case_i32(
                "i32::MIN shr_s 31 is -1, where shr_u gave 1",
                bin32(op::I32_SHR_S, i32::MIN, 31),
                -1,
            ),
            case_i32(
                "i32::MIN shr_s 1 is -1073741824",
                bin32(op::I32_SHR_S, i32::MIN, 1),
                -1073741824,
            ),
            case_i32(
                "i32::MAX shr_s 31 is 0: its sign bit is a zero, so zeros come in",
                bin32(op::I32_SHR_S, i32::MAX, 31),
                0,
            ),
            case_i32("-2 shr_s 1 is -1", bin32(op::I32_SHR_S, -2, 1), -1),
            case_i32(
                "-3 shr_s 1 is -2: the shift rounds towards minus infinity",
                bin32(op::I32_SHR_S, -3, 1),
                -2,
            ),
            case_i32(
                "-3 div_s 2 is -1, so the shift and the division really do differ",
                bin32(op::I32_SHR_S, -3, 1)
                    .i32_const(-3)
                    .i32_const(2)
                    .op(op::I32_DIV_S)
                    .op(op::I32_SUB),
                -1,
            ),
            case_i32(
                "0x12345678 shr_s 4 is 0x01234567, the same as shr_u for a positive value",
                bin32(op::I32_SHR_S, PATTERN, 4),
                shr_s(PATTERN, 4),
            ),
        ],
    )
});

wasm_test!(counts_wrap, |ctx| {
    run_cases(
        ctx,
        "i32-shift-counts",
        vec![
            case_i32(
                "1 shl 32 is 1 again: the count is taken modulo 32, not clamped to 31",
                bin32(op::I32_SHL, 1, 32),
                1,
            ),
            case_i32("1 shl 33 is 2", bin32(op::I32_SHL, 1, 33), 2),
            case_i32(
                "1 shl 63 is i32::MIN, because 63 modulo 32 is 31",
                bin32(op::I32_SHL, 1, 63),
                i32::MIN,
            ),
            case_i32("1 shl 64 is 1", bin32(op::I32_SHL, 1, 64), 1),
            case_i32(
                "1 shl -1 is i32::MIN: the count is unsigned, so -1 is 31 modulo 32",
                bin32(op::I32_SHL, 1, -1),
                shl(1, -1),
            ),
            case_i32(
                "1 shl -32 is 1, because 4294967264 is a multiple of 32",
                bin32(op::I32_SHL, 1, -32),
                shl(1, -32),
            ),
            case_i32(
                "-1 shr_u 32 is -1: a count of 32 shifts nothing at all",
                bin32(op::I32_SHR_U, -1, 32),
                -1,
            ),
            case_i32("-1 shr_s 32 is -1", bin32(op::I32_SHR_S, -1, 32), -1),
            case_i32(
                "i32::MIN shr_u 32 is i32::MIN, untouched",
                bin32(op::I32_SHR_U, i32::MIN, 32),
                i32::MIN,
            ),
            case_i32(
                "i32::MIN shr_u 33 is 1073741824, the same as a count of 1",
                bin32(op::I32_SHR_U, i32::MIN, 33),
                1073741824,
            ),
            case_i32(
                "0x12345678 shl 36 is 0x23456780, the same as a count of 4",
                bin32(op::I32_SHL, PATTERN, 36),
                shl(PATTERN, 4),
            ),
            case_i32(
                "a count of 31 is the largest that does anything new",
                bin32(op::I32_SHL, 1, 31)
                    .i32_const(1)
                    .i32_const(31 + 32)
                    .op(op::I32_SHL)
                    .op(op::I32_XOR),
                0,
            ),
        ],
    )
});

wasm_test!(rotates, |ctx| {
    run_cases(
        ctx,
        "i32-rotates",
        vec![
            case_i32("1 rotl 1 is 2", bin32(op::I32_ROTL, 1, 1), 2),
            case_i32(
                "i32::MIN rotl 1 is 1: the bit that fell off the top comes back at the bottom",
                bin32(op::I32_ROTL, i32::MIN, 1),
                1,
            ),
            case_i32(
                "1 rotr 1 is i32::MIN, the same journey the other way",
                bin32(op::I32_ROTR, 1, 1),
                i32::MIN,
            ),
            case_i32(
                "-1 rotl 7 is -1: a full word of ones cannot be disturbed",
                bin32(op::I32_ROTL, -1, 7),
                -1,
            ),
            case_i32(
                "0x12345678 rotl 8 is 0x34567812",
                bin32(op::I32_ROTL, PATTERN, 8),
                PATTERN.rotate_left(8),
            ),
            case_i32(
                "0x12345678 rotr 8 is 0x78123456",
                bin32(op::I32_ROTR, PATTERN, 8),
                PATTERN.rotate_right(8),
            ),
            case_i32(
                "rotating by 0 changes nothing",
                bin32(op::I32_ROTL, PATTERN, 0),
                PATTERN,
            ),
            case_i32(
                "rotating left by 32 is the identity",
                bin32(op::I32_ROTL, PATTERN, 32),
                PATTERN,
            ),
            case_i32(
                "rotating right by 32 is the identity too",
                bin32(op::I32_ROTR, PATTERN, 32),
                PATTERN,
            ),
            case_i32(
                "rotating left by 36 is the same as by 4",
                bin32(op::I32_ROTL, PATTERN, 36),
                PATTERN.rotate_left(4),
            ),
            case_i32(
                "rotl 13 followed by rotr 13 gives the value back",
                bin32(op::I32_ROTL, PATTERN, 13)
                    .i32_const(13)
                    .op(op::I32_ROTR),
                PATTERN,
            ),
            case_i32(
                "rotl 16 and rotr 16 agree, because 16 is half the width",
                bin32(op::I32_ROTL, PATTERN, 16)
                    .i32_const(PATTERN)
                    .i32_const(16)
                    .op(op::I32_ROTR)
                    .op(op::I32_XOR),
                0,
            ),
        ],
    )
});

wasm_test!(i64_shifts, |ctx| {
    run_cases(
        ctx,
        "i64-shifts-and-rotates",
        vec![
            case_i64(
                "1 shl 32 is 4294967296, where the same shift at i32 would be 1",
                bin64(op::I64_SHL, 1, 32),
                4294967296,
            ),
            case_i64(
                "1 shl 33 is 8589934592",
                bin64(op::I64_SHL, 1, 33),
                8589934592,
            ),
            case_i64("1 shl 63 is i64::MIN", bin64(op::I64_SHL, 1, 63), i64::MIN),
            case_i64(
                "1 shl 64 is 1: the count wraps at sixty-four",
                bin64(op::I64_SHL, 1, 64),
                1,
            ),
            case_i64("1 shl 65 is 2", bin64(op::I64_SHL, 1, 65), 2),
            case_i64(
                "1 shl -1 is i64::MIN, because -1 is 63 modulo 64",
                bin64(op::I64_SHL, 1, -1),
                i64::MIN,
            ),
            case_i64(
                "-1 shr_u 1 is i64::MAX",
                bin64(op::I64_SHR_U, -1, 1),
                i64::MAX,
            ),
            case_i64("-1 shr_s 1 is -1", bin64(op::I64_SHR_S, -1, 1), -1),
            case_i64(
                "i64::MIN shr_s 63 is -1",
                bin64(op::I64_SHR_S, i64::MIN, 63),
                -1,
            ),
            case_i64(
                "i64::MIN shr_u 63 is 1",
                bin64(op::I64_SHR_U, i64::MIN, 63),
                1,
            ),
            case_i64("1 rotr 1 is i64::MIN", bin64(op::I64_ROTR, 1, 1), i64::MIN),
            case_i64("i64::MIN rotl 1 is 1", bin64(op::I64_ROTL, i64::MIN, 1), 1),
            case_i64(
                "rotating left by 64 is the identity",
                bin64(op::I64_ROTL, PATTERN64, 64),
                PATTERN64,
            ),
            case_i64(
                "rotating left by 8 moves the top byte to the bottom",
                bin64(op::I64_ROTL, PATTERN64, 8),
                PATTERN64.rotate_left(8),
            ),
        ],
    )
});

/// `f() -> i32` returning `-1 lt_u 1`, the comparison that catches a signed register.
fn unsigned_compare_module() -> Module {
    let mut b = ModuleBuilder::new("i32-lt-u-minus-one");
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(
        ty,
        Func::new(Expr::new().i32_const(-1).i32_const(1).op(op::I32_LT_U)),
    );
    b.export_func("f", idx).build()
}

/// `f() -> i32` returning `1 shl 33`, the shift whose count wraps.
fn wrapped_count_module() -> Module {
    let mut b = ModuleBuilder::new("i32-shl-33");
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(
        ty,
        Func::new(Expr::new().i32_const(1).i32_const(33).op(op::I32_SHL)),
    );
    b.export_func("f", idx).build()
}

/// Worked examples: the unsigned comparison, and the shift count nobody masks first.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("An unsigned comparison of -1", unsigned_compare_module)
            .summary("`f() -> i32` returning `i32.const -1, i32.const 1, i32.lt_u`")
            .command("run --invoke f mod.wasm")
            .output("0")
            .note(
                "`i32.lt_s` of the same operands is 1. Unsigned, that -1 is 4294967295, and \
                 4294967295 is not below 1. Reinterpret both operands before comparing; do \
                 not compare signed and negate the answer.",
            ),
        ExampleSpec::module("A shift count larger than the width", wrapped_count_module)
            .summary("`f() -> i32` returning `i32.const 1, i32.const 33, i32.shl`")
            .command("run --invoke f mod.wasm")
            .output("2")
            .note(
                "The count is taken modulo 32, so 33 shifts by 1. It is not clamped to 31 \
                 (which would give i32::MIN) and it is not undefined: mask the count with 31 \
                 before you shift, and with 63 for i64.",
            ),
    ]
}
