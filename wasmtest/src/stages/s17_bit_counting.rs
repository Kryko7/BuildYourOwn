//! Stage 17 — clz, ctz, popcnt and sign extension. **[ext]**
//!
//! Six unary operators that a runtime can put off until everything else works, which is why
//! the whole stage is beyond the core track. None of them traps and none of them has an
//! interesting failure mode — except at the two ends of their range, where nearly every
//! implementation gets it wrong at least once.
//!
//! `clz` and `ctz` of **zero** are the full width, 32 or 64. There is no highest set bit to
//! count down from and no lowest one to count up to, so the answer is "all of them", and a
//! host intrinsic (`__builtin_clz`, `bsr`, `lzcnt` on older hardware) is very often
//! *undefined* for that input rather than 32. Test the zero case first.
//!
//! The sign-extension operators are the other half. `i32.extend8_s` reads the low eight bits
//! of its operand as a signed byte and widens that to the full width: `0x7f` stays 127 and
//! `0x80` becomes -128. Everything above the low bits is thrown away before the sign is
//! read, so `0x12345680` also becomes -128 — the high bytes are not consulted and are not an
//! error. `i64.extend32_s` is the odd one out in name only: it is the `i64` instruction that
//! does to thirty-two bits what `i64.extend_i32_s` (stage 14) does to an `i32` operand.

use crate::examples::ExampleSpec;
use crate::stages::{case_i32, case_i64, run_cases, Stage, Test};
use crate::wasm::{ftype, op, BlockType, Expr, Func, Module, ModuleBuilder, Op, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 17,
        slug: "bit_counting",
        name: "clz, ctz, popcnt and sign extension",
        ext: true,
        hints: &[
            "clz and ctz of 0 are the width of the type — 32 for i32 and 64 for i64 — and that is the case a host intrinsic most often leaves undefined, so handle it before you call one",
            "i32.clz, i32.ctz and i32.popcnt return an i32; their i64 counterparts return an i64, so i64.popcnt of -1 is the i64 value 64",
            "i32.extend8_s takes the low eight bits, reads them as a signed byte and widens that: 0x7f is 127, 0x80 is -128, and whatever sat in the high twenty-four bits is discarded first",
            "i64.extend32_s is the same idea at thirty-two bits and is a different instruction from i64.extend_i32_s, which takes an i32 operand rather than an i64 one",
        ],
        examples,
        tests: vec![
            Test::new("clz counts the zeros above the highest set bit", clz_i32).ext(),
            Test::new("ctz counts the zeros below the lowest set bit", ctz_i32).ext(),
            Test::new("popcnt counts the ones and nothing else", popcnt_i32).ext(),
            Test::new("the sixty-four-bit counters answer up to sixty-four", counters_i64).ext(),
            Test::new("i32.extend8_s and i32.extend16_s widen a signed narrow value", extend_i32).ext(),
            Test::new("the i64 sign extensions cover eight, sixteen and thirty-two bits", extend_i64).ext(),
            Test::new("sign extension reads the low bits and discards everything above them", extend_ignores_the_top).ext(),
            Test::new("clz, ctz and popcnt agree with each other and with a loop", combined).ext(),
        ],
    }
}

/// `op a` with one `i32` operand.
fn un32(o: Op, a: i32) -> Expr {
    Expr::new().i32_const(a).op(o)
}

/// `op a` with one `i64` operand.
fn un64(o: Op, a: i64) -> Expr {
    Expr::new().i64_const(a).op(o)
}

/// An arbitrary value with every nibble different.
const PATTERN: i32 = 0x1234_5678;
/// The same idea at sixty-four bits.
const PATTERN64: i64 = 0x0123_4567_89ab_cdef;

wasm_test!(clz_i32, |ctx| {
    run_cases(
        ctx,
        "i32-clz",
        vec![
            case_i32(
                "clz of 0 is 32: there is no set bit, so every bit is a leading zero",
                un32(op::I32_CLZ, 0),
                32,
            ),
            case_i32("clz of 1 is 31", un32(op::I32_CLZ, 1), 31),
            case_i32("clz of 2 is 30", un32(op::I32_CLZ, 2), 30),
            case_i32(
                "clz of 3 is 30 as well: only the highest set bit counts",
                un32(op::I32_CLZ, 3),
                30,
            ),
            case_i32(
                "clz of i32::MIN is 0: the sign bit is the highest bit there is",
                un32(op::I32_CLZ, i32::MIN),
                0,
            ),
            case_i32("clz of -1 is 0", un32(op::I32_CLZ, -1), 0),
            case_i32(
                "clz of i32::MAX is 1: everything but the sign bit",
                un32(op::I32_CLZ, i32::MAX),
                1,
            ),
            case_i32(
                "clz of 0x80000001 is 0, though the lowest bit is set too",
                un32(op::I32_CLZ, 0x8000_0001u32 as i32),
                0,
            ),
            case_i32(
                "clz of 0x00ff00ff is 8",
                un32(op::I32_CLZ, 0x00ff_00ff),
                0x00ff_00ffi32.leading_zeros() as i32,
            ),
            case_i32(
                "clz of 0x00010000 is 15",
                un32(op::I32_CLZ, 0x0001_0000),
                15,
            ),
            case_i32(
                "clz of 0x12345678 is 3",
                un32(op::I32_CLZ, PATTERN),
                PATTERN.leading_zeros() as i32,
            ),
        ],
    )
});

wasm_test!(ctz_i32, |ctx| {
    run_cases(
        ctx,
        "i32-ctz",
        vec![
            case_i32(
                "ctz of 0 is 32, the same whole-width answer clz gives",
                un32(op::I32_CTZ, 0),
                32,
            ),
            case_i32("ctz of 1 is 0", un32(op::I32_CTZ, 1), 0),
            case_i32("ctz of 2 is 1", un32(op::I32_CTZ, 2), 1),
            case_i32(
                "ctz of 3 is 0: only the lowest set bit counts",
                un32(op::I32_CTZ, 3),
                0,
            ),
            case_i32(
                "ctz of i32::MIN is 31: its one set bit is at the very top",
                un32(op::I32_CTZ, i32::MIN),
                31,
            ),
            case_i32("ctz of -1 is 0", un32(op::I32_CTZ, -1), 0),
            case_i32(
                "ctz of 0x80000001 is 0, though the highest bit is set too",
                un32(op::I32_CTZ, 0x8000_0001u32 as i32),
                0,
            ),
            case_i32(
                "ctz of 0x00ff0000 is 16",
                un32(op::I32_CTZ, 0x00ff_0000),
                16,
            ),
            case_i32(
                "ctz of 0x10000000 is 28",
                un32(op::I32_CTZ, 0x1000_0000),
                28,
            ),
            case_i32(
                "ctz of 0x12345678 is 3",
                un32(op::I32_CTZ, PATTERN),
                PATTERN.trailing_zeros() as i32,
            ),
            case_i32(
                "clz and ctz of a single bit add up to 31",
                un32(op::I32_CLZ, 1 << 13)
                    .i32_const(1 << 13)
                    .op(op::I32_CTZ)
                    .op(op::I32_ADD),
                31,
            ),
        ],
    )
});

wasm_test!(popcnt_i32, |ctx| {
    run_cases(
        ctx,
        "i32-popcnt",
        vec![
            case_i32("popcnt of 0 is 0", un32(op::I32_POPCNT, 0), 0),
            case_i32("popcnt of 1 is 1", un32(op::I32_POPCNT, 1), 1),
            case_i32(
                "popcnt of -1 is 32: every bit is set",
                un32(op::I32_POPCNT, -1),
                32,
            ),
            case_i32(
                "popcnt of i32::MIN is 1: the sign bit counts like any other",
                un32(op::I32_POPCNT, i32::MIN),
                1,
            ),
            case_i32(
                "popcnt of i32::MAX is 31",
                un32(op::I32_POPCNT, i32::MAX),
                31,
            ),
            case_i32(
                "popcnt of 0xaaaaaaaa is 16",
                un32(op::I32_POPCNT, 0xaaaa_aaaau32 as i32),
                16,
            ),
            case_i32(
                "popcnt of 0x55555555 is 16 as well",
                un32(op::I32_POPCNT, 0x5555_5555),
                16,
            ),
            case_i32(
                "popcnt of 0x80000001 is 2: a bit at each end",
                un32(op::I32_POPCNT, 0x8000_0001u32 as i32),
                2,
            ),
            case_i32(
                "popcnt of 0x0f0f0f0f is 16",
                un32(op::I32_POPCNT, 0x0f0f_0f0f),
                16,
            ),
            case_i32(
                "popcnt of 0x12345678 is 13",
                un32(op::I32_POPCNT, PATTERN),
                PATTERN.count_ones() as i32,
            ),
            case_i32(
                "popcnt of a value and of its complement add up to 32",
                un32(op::I32_POPCNT, PATTERN)
                    .i32_const(!PATTERN)
                    .op(op::I32_POPCNT)
                    .op(op::I32_ADD),
                32,
            ),
        ],
    )
});

wasm_test!(counters_i64, |ctx| {
    run_cases(
        ctx,
        "i64-counters",
        vec![
            case_i64("i64 clz of 0 is 64", un64(op::I64_CLZ, 0), 64),
            case_i64("i64 ctz of 0 is 64", un64(op::I64_CTZ, 0), 64),
            case_i64("i64 popcnt of 0 is 0", un64(op::I64_POPCNT, 0), 0),
            case_i64("i64 clz of 1 is 63", un64(op::I64_CLZ, 1), 63),
            case_i64("i64 clz of i64::MIN is 0", un64(op::I64_CLZ, i64::MIN), 0),
            case_i64("i64 ctz of i64::MIN is 63", un64(op::I64_CTZ, i64::MIN), 63),
            case_i64(
                "i64 clz of 4294967296 is 31, which no thirty-two-bit count could say",
                un64(op::I64_CLZ, 4294967296),
                4294967296i64.leading_zeros() as i64,
            ),
            case_i64(
                "i64 ctz of 4294967296 is 32",
                un64(op::I64_CTZ, 4294967296),
                32,
            ),
            case_i64("i64 clz of i64::MAX is 1", un64(op::I64_CLZ, i64::MAX), 1),
            case_i64("i64 popcnt of -1 is 64", un64(op::I64_POPCNT, -1), 64),
            case_i64(
                "i64 popcnt of i64::MAX is 63",
                un64(op::I64_POPCNT, i64::MAX),
                63,
            ),
            case_i64(
                "i64 popcnt of 0x0123456789abcdef is 32",
                un64(op::I64_POPCNT, PATTERN64),
                PATTERN64.count_ones() as i64,
            ),
            case_i64(
                "a bit at each end gives clz 0, ctz 0 and popcnt 2",
                un64(op::I64_POPCNT, i64::MIN + 1)
                    .i64_const(i64::MIN + 1)
                    .op(op::I64_CLZ)
                    .op(op::I64_ADD)
                    .i64_const(i64::MIN + 1)
                    .op(op::I64_CTZ)
                    .op(op::I64_ADD),
                2,
            ),
        ],
    )
});

wasm_test!(extend_i32, |ctx| {
    run_cases(
        ctx,
        "i32-extend",
        vec![
            case_i32("extend8_s of 0 is 0", un32(op::I32_EXTEND8_S, 0), 0),
            case_i32("extend8_s of 1 is 1", un32(op::I32_EXTEND8_S, 1), 1),
            case_i32(
                "extend8_s of 0x7f is 127: the largest byte that is still positive",
                un32(op::I32_EXTEND8_S, 0x7f),
                127,
            ),
            case_i32(
                "extend8_s of 0x80 is -128: one more, and the sign bit of the byte is set",
                un32(op::I32_EXTEND8_S, 0x80),
                -128,
            ),
            case_i32("extend8_s of 0xff is -1", un32(op::I32_EXTEND8_S, 0xff), -1),
            case_i32(
                "extend8_s of 0x100 is 0: the low byte is empty",
                un32(op::I32_EXTEND8_S, 0x100),
                0,
            ),
            case_i32(
                "extend16_s of 0x7fff is 32767",
                un32(op::I32_EXTEND16_S, 0x7fff),
                32767,
            ),
            case_i32(
                "extend16_s of 0x8000 is -32768",
                un32(op::I32_EXTEND16_S, 0x8000),
                -32768,
            ),
            case_i32(
                "extend16_s of 0xffff is -1",
                un32(op::I32_EXTEND16_S, 0xffff),
                -1,
            ),
            case_i32(
                "extend16_s of 0x1234 leaves a positive value alone",
                un32(op::I32_EXTEND16_S, 0x1234),
                0x1234,
            ),
            case_i32(
                "extend16_s of 0x00ff is 255, where extend8_s of the same value is -1",
                un32(op::I32_EXTEND16_S, 0x00ff),
                255,
            ),
        ],
    )
});

wasm_test!(extend_i64, |ctx| {
    run_cases(
        ctx,
        "i64-extend",
        vec![
            case_i64(
                "i64 extend8_s of 0x7f is 127",
                un64(op::I64_EXTEND8_S, 0x7f),
                127,
            ),
            case_i64(
                "i64 extend8_s of 0x80 is -128",
                un64(op::I64_EXTEND8_S, 0x80),
                -128,
            ),
            case_i64(
                "i64 extend8_s of 0xff is -1",
                un64(op::I64_EXTEND8_S, 0xff),
                -1,
            ),
            case_i64(
                "i64 extend16_s of 0x7fff is 32767",
                un64(op::I64_EXTEND16_S, 0x7fff),
                32767,
            ),
            case_i64(
                "i64 extend16_s of 0x8000 is -32768",
                un64(op::I64_EXTEND16_S, 0x8000),
                -32768,
            ),
            case_i64(
                "i64 extend16_s of 0xffff is -1",
                un64(op::I64_EXTEND16_S, 0xffff),
                -1,
            ),
            case_i64(
                "i64 extend32_s of 0x7fffffff is 2147483647",
                un64(op::I64_EXTEND32_S, 0x7fff_ffff),
                2147483647,
            ),
            case_i64(
                "i64 extend32_s of 0x80000000 is -2147483648: the same step one width up",
                un64(op::I64_EXTEND32_S, 0x8000_0000),
                -2147483648,
            ),
            case_i64(
                "i64 extend32_s of 0xffffffff is -1",
                un64(op::I64_EXTEND32_S, 0xffff_ffff),
                -1,
            ),
            case_i64("i64 extend32_s of 0 is 0", un64(op::I64_EXTEND32_S, 0), 0),
            case_i64(
                "i64 extend32_s of -1 is -1: it was already sign-extended",
                un64(op::I64_EXTEND32_S, -1),
                -1,
            ),
        ],
    )
});

wasm_test!(extend_ignores_the_top, |ctx| {
    run_cases(
        ctx,
        "extend-discards-the-top",
        vec![
            case_i32(
                "extend8_s of 0x12345680 is -128: the high three bytes are simply dropped",
                un32(op::I32_EXTEND8_S, 0x1234_5680),
                -128,
            ),
            case_i32(
                "extend8_s of 0x7fffff7f is 127, positive despite the noise above it",
                un32(op::I32_EXTEND8_S, 0x7fff_ff7f),
                127,
            ),
            case_i32(
                "extend8_s of -1 is -1, because the low byte is 0xff",
                un32(op::I32_EXTEND8_S, -1),
                -1,
            ),
            case_i32(
                "extend8_s of i32::MIN is 0: its low byte is empty",
                un32(op::I32_EXTEND8_S, i32::MIN),
                0,
            ),
            case_i32(
                "extend16_s of 0xdead8000 is -32768",
                un32(op::I32_EXTEND16_S, 0xdead_8000u32 as i32),
                -32768,
            ),
            case_i32(
                "extend16_s of i32::MIN is 0",
                un32(op::I32_EXTEND16_S, i32::MIN),
                0,
            ),
            case_i64(
                "i64 extend32_s of 0x1234567880000000 is -2147483648",
                un64(op::I64_EXTEND32_S, 0x1234_5678_8000_0000),
                -2147483648,
            ),
            case_i64(
                "i64 extend32_s of i64::MIN is 0: its low half is empty",
                un64(op::I64_EXTEND32_S, i64::MIN),
                0,
            ),
            case_i64(
                "i64 extend8_s of i64::MAX is -1, because its low byte is 0xff",
                un64(op::I64_EXTEND8_S, i64::MAX),
                -1,
            ),
            case_i64(
                "i64 extend16_s of 0x0123456789abcdef is 0xcdef read as a signed half-word",
                un64(op::I64_EXTEND16_S, PATTERN64),
                (PATTERN64 as u16) as i16 as i64,
            ),
        ],
    )
});

/// Count the set bits of `x` the slow way: clear the lowest one, `x & (x - 1)`, until the
/// word is empty, counting the rounds. Local 0 holds the word, local 1 the count.
fn popcnt_loop(x: i32) -> Expr {
    Expr::new()
        .i32_const(x)
        .local_set(0)
        .block(
            BlockType::Empty,
            Expr::new().loop_(
                BlockType::Empty,
                Expr::new()
                    .local_get(0)
                    .op(op::I32_EQZ)
                    .br_if(1)
                    .local_get(1)
                    .i32_const(1)
                    .op(op::I32_ADD)
                    .local_set(1)
                    .local_get(0)
                    .local_get(0)
                    .i32_const(1)
                    .op(op::I32_SUB)
                    .op(op::I32_AND)
                    .local_set(0)
                    .br(0),
            ),
        )
        .local_get(1)
}

/// `clz(x) + ctz(x) + popcnt(x)`, which is 32 exactly when `x` has one bit set.
fn clz_ctz_popcnt(x: i32) -> Expr {
    Expr::new()
        .i32_const(x)
        .op(op::I32_CLZ)
        .i32_const(x)
        .op(op::I32_CTZ)
        .op(op::I32_ADD)
        .i32_const(x)
        .op(op::I32_POPCNT)
        .op(op::I32_ADD)
}

wasm_test!(combined, |ctx| {
    run_cases(
        ctx,
        "bit-counting-together",
        vec![
            case_i32(
                "clearing the lowest set bit 13 times empties 0x12345678, which is its popcnt",
                popcnt_loop(PATTERN),
                PATTERN.count_ones() as i32,
            )
            .locals(&[(2, ValType::I32)]),
            case_i32("the same loop needs 32 rounds for -1", popcnt_loop(-1), 32)
                .locals(&[(2, ValType::I32)]),
            case_i32(
                "and one round for i32::MIN, whose single bit is the sign",
                popcnt_loop(i32::MIN),
                1,
            )
            .locals(&[(2, ValType::I32)]),
            case_i32(
                "and no rounds at all for 0, where ctz would have answered 32",
                popcnt_loop(0),
                0,
            )
            .locals(&[(2, ValType::I32)]),
            case_i32(
                "clz plus ctz plus popcnt is 32 for the single bit at position 13",
                clz_ctz_popcnt(1 << 13),
                32,
            ),
            case_i32(
                "and 32 again for the single bit at position 0",
                clz_ctz_popcnt(1),
                32,
            ),
            case_i32(
                "and for the sign bit, the only one where clz is 0 and ctz is 31",
                clz_ctz_popcnt(i32::MIN),
                32,
            ),
            case_i32(
                "clz plus ctz plus popcnt of 0 is 64, not 32: both counters answer the width",
                clz_ctz_popcnt(0),
                64,
            ),
            case_i32(
                "31 minus clz is the index of the highest set bit, 28 for 0x12345678",
                Expr::new()
                    .i32_const(31)
                    .i32_const(PATTERN)
                    .op(op::I32_CLZ)
                    .op(op::I32_SUB),
                28,
            ),
            case_i32(
                "x & -x isolates the lowest set bit, and ctz cannot tell the difference",
                Expr::new()
                    .i32_const(PATTERN)
                    .i32_const(0)
                    .i32_const(PATTERN)
                    .op(op::I32_SUB)
                    .op(op::I32_AND)
                    .op(op::I32_CTZ)
                    .i32_const(PATTERN)
                    .op(op::I32_CTZ)
                    .op(op::I32_SUB),
                0,
            ),
        ],
    )
});

/// `f() -> i32` returning `i32.clz` of zero, the answer an intrinsic often leaves undefined.
fn clz_of_zero_module() -> Module {
    let mut b = ModuleBuilder::new("i32-clz-zero");
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(ty, Func::new(Expr::new().i32_const(0).op(op::I32_CLZ)));
    b.export_func("f", idx).build()
}

/// `f() -> i32` returning `i32.extend8_s` of a value whose high bytes are noise.
fn extend8_module() -> Module {
    let mut b = ModuleBuilder::new("i32-extend8-s");
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(
        ty,
        Func::new(Expr::new().i32_const(0x1234_5680).op(op::I32_EXTEND8_S)),
    );
    b.export_func("f", idx).build()
}

/// Worked examples: the counter's zero case, and the extension's discarded high bits.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("Counting the leading zeros of zero", clz_of_zero_module)
            .summary("`f() -> i32` returning `i32.const 0, i32.clz`")
            .command("run --invoke f mod.wasm")
            .output("32")
            .note(
                "There is no set bit, so every one of the thirty-two is a leading zero. \
                 `i32.ctz` of 0 is 32 as well, and the i64 pair answer 64. Most hardware \
                 count-leading-zeros instructions are undefined for this input, so check for \
                 zero before you reach for one.",
            ),
        ExampleSpec::module("Sign-extending a byte out of a larger word", extend8_module)
            .summary("`f() -> i32` returning `i32.const 0x12345680, i32.extend8_s`")
            .command("run --invoke f mod.wasm")
            .output("-128")
            .note(
                "Only the low eight bits are read. 0x80 as a signed byte is -128, and the \
                 0x123456 above it is discarded before the sign is looked at — it is not an \
                 error and it does not change the answer.",
            ),
    ]
}
