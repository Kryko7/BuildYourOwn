//! Stage 14 — i64 arithmetic, wrapping and extension.
//!
//! The arithmetic is stage 13's, one width up: modulo 2^64, never trapping, printed as a
//! signed 64-bit decimal. What is new is the traffic between the two widths, and that is
//! where the bugs live.
//!
//! `i32.wrap_i64` throws the top thirty-two bits away and keeps the bottom thirty-two —
//! including their sign bit, so the positive `i64` value 2147483648 wraps to the negative
//! `i32` value -2147483648. Going the other way there are *two* instructions, not one:
//! `i64.extend_i32_s` copies the sign bit into the new top half and `i64.extend_i32_u` fills
//! it with zeros. They agree on every non-negative `i32` and disagree on every negative one,
//! which is why a runtime that implements only one of them passes most of a test suite and
//! then quietly returns 4294967295 where -1 was wanted. The pair gets a test of its own.
//!
//! One more thing this stage is watching for: a 64-bit multiply computed in a 32-bit
//! register. `65536 * 65536` is 0 at thirty-two bits and 4294967296 at sixty-four, and a
//! runtime that truncates will look perfectly correct on every small value.

use crate::examples::ExampleSpec;
use crate::stages::{case_i32, case_i64, run_cases, Case, Stage, Test};
use crate::wasm::{ftype, op, Expr, Func, Module, ModuleBuilder, Op, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 14,
        slug: "i64_arithmetic",
        name: "i64 arithmetic, wrapping and extension",
        ext: false,
        hints: &[
            "Keep i64 values in a real 64-bit register: a multiply done at thirty-two bits gives 0 for 65536 * 65536 where the answer is 4294967296",
            "i32.wrap_i64 keeps the low thirty-two bits and drops the rest, sign bit and all, so the i64 2147483648 becomes the i32 -2147483648",
            "i64.extend_i32_s copies the argument's sign bit into the top half and i64.extend_i32_u writes zeros there; they differ for every negative i32, and -1 becomes -1 or 4294967295 accordingly",
            "An i64 constant is a signed LEB128 up to ten bytes long, so i64::MIN and i64::MAX need the full ten and a decoder that stops at five will silently truncate them",
        ],
        examples,
        tests: vec![
            Test::new("add and sub wrap at the sixty-four-bit edges", add_sub_wraps),
            Test::new("mul keeps the low sixty-four bits", mul_wraps),
            Test::new("a product that overflows thirty-two bits is not truncated", wide_products),
            Test::new("and, or and xor work on all sixty-four bits", bitwise),
            Test::new("i32.wrap_i64 keeps the low thirty-two bits and drops the rest", wrapping),
            Test::new("extend_i32_s and extend_i32_u disagree about a negative i32", the_extend_pair),
            Test::new("narrowing then widening only round trips when the value fits", round_trips),
            Test::new("a sixty-four-bit constant survives its ten-byte encoding", wide_constants),
            Test::new("arguments given on the command line reach a sixty-four-bit function", from_the_command_line),
        ],
    }
}

/// `a op b` as a whole body, both operands `i64`.
fn bin(o: Op, a: i64, b: i64) -> Expr {
    Expr::new().i64_const(a).i64_const(b).op(o)
}

/// `op a` as a whole body, the operand an `i64`.
fn un(o: Op, a: i64) -> Expr {
    Expr::new().i64_const(a).op(o)
}

/// `op a` as a whole body, the operand an `i32`.
fn un32(o: Op, a: i32) -> Expr {
    Expr::new().i32_const(a).op(o)
}

/// Alternating bits, high of each pair set.
const ALT_HIGH: i64 = 0xaaaa_aaaa_aaaa_aaaau64 as i64;
/// Alternating bits, low of each pair set.
const ALT_LOW: i64 = 0x5555_5555_5555_5555;
/// An arbitrary value with every nibble different, so a dropped byte shows up.
const PATTERN: i64 = 0x0123_4567_89ab_cdef;
/// 2^32, the smallest value a 32-bit register cannot hold.
const FOUR_GIG: i64 = 0x1_0000_0000;

wasm_test!(add_sub_wraps, |ctx| {
    run_cases(
        ctx,
        "i64-add-sub",
        vec![
            case_i64("0 + 0 is 0", bin(op::I64_ADD, 0, 0), 0),
            case_i64("-1 + 1 is 0", bin(op::I64_ADD, -1, 1), 0),
            case_i64(
                "2147483647 + 1 is 2147483648, not an i32 that wrapped",
                bin(op::I64_ADD, 2147483647, 1),
                2147483648,
            ),
            case_i64(
                "-2147483648 - 1 is -2147483649, again with no thirty-two-bit wrap",
                bin(op::I64_SUB, -2147483648, 1),
                -2147483649,
            ),
            case_i64(
                "2^32 + 2^32 is 2^33",
                bin(op::I64_ADD, FOUR_GIG, FOUR_GIG),
                8589934592,
            ),
            case_i64(
                "i64::MAX + 1 wraps round to i64::MIN",
                bin(op::I64_ADD, i64::MAX, 1),
                i64::MIN,
            ),
            case_i64(
                "i64::MIN - 1 wraps the other way, to i64::MAX",
                bin(op::I64_SUB, i64::MIN, 1),
                i64::MAX,
            ),
            case_i64(
                "i64::MAX + i64::MAX is -2",
                bin(op::I64_ADD, i64::MAX, i64::MAX),
                i64::MAX.wrapping_add(i64::MAX),
            ),
            case_i64(
                "i64::MIN + i64::MIN is 0",
                bin(op::I64_ADD, i64::MIN, i64::MIN),
                0,
            ),
            case_i64(
                "i64::MAX - i64::MIN is -1",
                bin(op::I64_SUB, i64::MAX, i64::MIN),
                -1,
            ),
            case_i64(
                "0 - i64::MIN is i64::MIN again, the value that is its own negation",
                bin(op::I64_SUB, 0, i64::MIN),
                i64::MIN,
            ),
        ],
    )
});

wasm_test!(mul_wraps, |ctx| {
    run_cases(
        ctx,
        "i64-mul",
        vec![
            case_i64("0 * i64::MIN is 0", bin(op::I64_MUL, 0, i64::MIN), 0),
            case_i64(
                "1 * i64::MIN leaves it alone",
                bin(op::I64_MUL, 1, i64::MIN),
                i64::MIN,
            ),
            case_i64("-1 * -1 is 1", bin(op::I64_MUL, -1, -1), 1),
            case_i64("3 * -7 is -21", bin(op::I64_MUL, 3, -7), -21),
            case_i64(
                "i64::MIN * -1 is i64::MIN again: mul never traps",
                bin(op::I64_MUL, i64::MIN, -1),
                i64::MIN,
            ),
            case_i64(
                "2^32 * 2^32 is 0: the product is exactly 2^64",
                bin(op::I64_MUL, FOUR_GIG, FOUR_GIG),
                0,
            ),
            case_i64(
                "(2^32 - 1) * (2^32 + 1) is -1: 2^64 - 1 read back as a signed integer",
                bin(op::I64_MUL, 4294967295, 4294967297),
                -1,
            ),
            case_i64(
                "i64::MAX * 2 is -2",
                bin(op::I64_MUL, i64::MAX, 2),
                i64::MAX.wrapping_mul(2),
            ),
            case_i64(
                "i64::MAX * i64::MAX is 1",
                bin(op::I64_MUL, i64::MAX, i64::MAX),
                i64::MAX.wrapping_mul(i64::MAX),
            ),
            case_i64(
                "i64::MIN * i64::MIN is 0",
                bin(op::I64_MUL, i64::MIN, i64::MIN),
                0,
            ),
        ],
    )
});

wasm_test!(wide_products, |ctx| {
    run_cases(
        ctx,
        "i64-wide-products",
        vec![
            case_i64(
                "65536 * 65536 is 4294967296 — it is 0 only at thirty-two bits",
                bin(op::I64_MUL, 65536, 65536),
                4294967296,
            ),
            case_i64(
                "2147483647 * 2147483647 is 4611686014132420609",
                bin(op::I64_MUL, 2147483647, 2147483647),
                4611686014132420609,
            ),
            case_i64(
                "-2147483648 * -2147483648 is 2^62, and 0 at thirty-two bits",
                bin(op::I64_MUL, -2147483648, -2147483648),
                4611686018427387904,
            ),
            case_i64(
                "3037000499 squared is 9223372030926249001, just inside i64::MAX",
                bin(op::I64_MUL, 3037000499, 3037000499),
                9223372030926249001,
            ),
            case_i64(
                "1000000 * 1000000 is a million million",
                bin(op::I64_MUL, 1000000, 1000000),
                1000000000000,
            ),
            case_i64(
                "2^32 * 2 is 2^33, not 0",
                bin(op::I64_MUL, FOUR_GIG, 2),
                8589934592,
            ),
            case_i64(
                "-1 * 2^32 is -4294967296",
                bin(op::I64_MUL, -1, FOUR_GIG),
                -4294967296,
            ),
            case_i64(
                "the product of two wide values still wraps once it passes 2^64",
                bin(op::I64_MUL, 4611686018427387904, 3),
                4611686018427387904i64.wrapping_mul(3),
            ),
        ],
    )
});

wasm_test!(bitwise, |ctx| {
    run_cases(
        ctx,
        "i64-bitwise",
        vec![
            case_i64(
                "the two alternating patterns share no bit, so and is 0",
                bin(op::I64_AND, ALT_HIGH, ALT_LOW),
                0,
            ),
            case_i64(
                "between them they are every bit, so or is -1",
                bin(op::I64_OR, ALT_HIGH, ALT_LOW),
                -1,
            ),
            case_i64(
                "and xor is -1 as well",
                bin(op::I64_XOR, ALT_HIGH, ALT_LOW),
                -1,
            ),
            case_i64(
                "xor of a value with itself is 0, all sixty-four bits of it",
                bin(op::I64_XOR, PATTERN, PATTERN),
                0,
            ),
            case_i64(
                "xor with -1 is the one's complement",
                bin(op::I64_XOR, PATTERN, -1),
                !PATTERN,
            ),
            case_i64(
                "and with 0xffffffff keeps the low half and clears the high half",
                bin(op::I64_AND, PATTERN, 0xffff_ffff),
                0x89ab_cdef,
            ),
            case_i64(
                "and with a high-half mask keeps the top thirty-two bits where they are",
                bin(op::I64_AND, PATTERN, 0xffff_ffff_0000_0000u64 as i64),
                0x0123_4567_0000_0000,
            ),
            case_i64(
                "i64::MIN and i64::MAX is 0: the sign bit is in neither of the other's word",
                bin(op::I64_AND, i64::MIN, i64::MAX),
                0,
            ),
            case_i64(
                "i64::MIN or i64::MAX is -1",
                bin(op::I64_OR, i64::MIN, i64::MAX),
                -1,
            ),
            case_i64(
                "or with the sign bit turns 1 into the most negative odd number",
                bin(op::I64_OR, 1, i64::MIN),
                i64::MIN + 1,
            ),
        ],
    )
});

wasm_test!(wrapping, |ctx| {
    run_cases(
        ctx,
        "i32-wrap-i64",
        vec![
            case_i32("wrapping 0 gives 0", un(op::I32_WRAP_I64, 0), 0),
            case_i32("wrapping 42 gives 42", un(op::I32_WRAP_I64, 42), 42),
            case_i32(
                "wrapping 2^32 gives 0: the low half is empty",
                un(op::I32_WRAP_I64, FOUR_GIG),
                0,
            ),
            case_i32(
                "wrapping 4294967303 gives 7, not 4294967303",
                un(op::I32_WRAP_I64, FOUR_GIG + 7),
                7,
            ),
            case_i32(
                "wrapping -4294967254 gives 42: the high half goes, whatever it held",
                un(op::I32_WRAP_I64, -4294967296 + 42),
                42,
            ),
            case_i32(
                "wrapping the positive 2147483648 gives the negative -2147483648",
                un(op::I32_WRAP_I64, 2147483648),
                i32::MIN,
            ),
            case_i32(
                "wrapping 2147483647 leaves it alone: it already fits",
                un(op::I32_WRAP_I64, 2147483647),
                i32::MAX,
            ),
            case_i32("wrapping -1 gives -1", un(op::I32_WRAP_I64, -1), -1),
            case_i32(
                "wrapping i64::MAX gives -1: its low thirty-two bits are all ones",
                un(op::I32_WRAP_I64, i64::MAX),
                -1,
            ),
            case_i32(
                "wrapping i64::MIN gives 0: its low thirty-two bits are all zeros",
                un(op::I32_WRAP_I64, i64::MIN),
                0,
            ),
            case_i32(
                "wrapping 0x0123456789abcdef gives 0x89abcdef, which prints negative",
                un(op::I32_WRAP_I64, PATTERN),
                0x89ab_cdefu32 as i32,
            ),
        ],
    )
});

wasm_test!(the_extend_pair, |ctx| {
    run_cases(
        ctx,
        "i64-extend-i32",
        vec![
            case_i64("extend_i32_s of 0 is 0", un32(op::I64_EXTEND_I32_S, 0), 0),
            case_i64("extend_i32_u of 0 is 0", un32(op::I64_EXTEND_I32_U, 0), 0),
            case_i64(
                "extend_i32_s of 2147483647 is 2147483647",
                un32(op::I64_EXTEND_I32_S, i32::MAX),
                2147483647,
            ),
            case_i64(
                "extend_i32_u of 2147483647 is the same: the two agree on every positive i32",
                un32(op::I64_EXTEND_I32_U, i32::MAX),
                2147483647,
            ),
            case_i64(
                "extend_i32_s of -1 is -1",
                un32(op::I64_EXTEND_I32_S, -1),
                -1,
            ),
            case_i64(
                "extend_i32_u of -1 is 4294967295 — the same bits, a different number",
                un32(op::I64_EXTEND_I32_U, -1),
                4294967295,
            ),
            case_i64(
                "extend_i32_s of -2 is -2",
                un32(op::I64_EXTEND_I32_S, -2),
                -2,
            ),
            case_i64(
                "extend_i32_u of -2 is 4294967294",
                un32(op::I64_EXTEND_I32_U, -2),
                4294967294,
            ),
            case_i64(
                "extend_i32_s of -2147483648 is -2147483648",
                un32(op::I64_EXTEND_I32_S, i32::MIN),
                -2147483648,
            ),
            case_i64(
                "extend_i32_u of -2147483648 is 2147483648",
                un32(op::I64_EXTEND_I32_U, i32::MIN),
                2147483648,
            ),
            case_i64(
                "the difference between the two is exactly 2^32 for any negative i32",
                Expr::new()
                    .i32_const(-1)
                    .op(op::I64_EXTEND_I32_U)
                    .i32_const(-1)
                    .op(op::I64_EXTEND_I32_S)
                    .op(op::I64_SUB),
                4294967296,
            ),
        ],
    )
});

wasm_test!(round_trips, |ctx| {
    run_cases(
        ctx,
        "i64-round-trips",
        vec![
            case_i64(
                "i64 7 survives wrap then extend_i32_s",
                un(op::I32_WRAP_I64, 7).op(op::I64_EXTEND_I32_S),
                7,
            ),
            case_i64(
                "i64 -1 survives wrap then extend_i32_s",
                un(op::I32_WRAP_I64, -1).op(op::I64_EXTEND_I32_S),
                -1,
            ),
            case_i64(
                "4294967303 does not survive: wrap keeps 7 and extend gives back 7",
                un(op::I32_WRAP_I64, FOUR_GIG + 7).op(op::I64_EXTEND_I32_S),
                7,
            ),
            case_i64(
                "4294967295 comes back as -1 through extend_i32_s",
                un(op::I32_WRAP_I64, 4294967295).op(op::I64_EXTEND_I32_S),
                -1,
            ),
            case_i64(
                "4294967295 comes back unchanged through extend_i32_u",
                un(op::I32_WRAP_I64, 4294967295).op(op::I64_EXTEND_I32_U),
                4294967295,
            ),
            case_i64(
                "i64::MAX comes back as 4294967295 through extend_i32_u, never as itself",
                un(op::I32_WRAP_I64, i64::MAX).op(op::I64_EXTEND_I32_U),
                4294967295,
            ),
            case_i32(
                "i32::MIN survives extend_i32_s then wrap exactly",
                un32(op::I64_EXTEND_I32_S, i32::MIN).op(op::I32_WRAP_I64),
                i32::MIN,
            ),
            case_i32(
                "i32::MIN survives extend_i32_u then wrap exactly as well",
                un32(op::I64_EXTEND_I32_U, i32::MIN).op(op::I32_WRAP_I64),
                i32::MIN,
            ),
            case_i32(
                "-1 survives the i32 -> i64 -> i32 trip whichever extension is used",
                un32(op::I64_EXTEND_I32_U, -1).op(op::I32_WRAP_I64),
                -1,
            ),
        ],
    )
});

wasm_test!(wide_constants, |ctx| {
    run_cases(
        ctx,
        "i64-constants",
        vec![
            case_i64("0 is one byte", Expr::new().i64_const(0), 0),
            case_i64("-1 is one byte too", Expr::new().i64_const(-1), -1),
            case_i64("63 fits in one byte", Expr::new().i64_const(63), 63),
            case_i64(
                "64 needs two, because the sign bit of the first is taken",
                Expr::new().i64_const(64),
                64,
            ),
            case_i64(
                "2147483648 is past anything an i32 constant could say",
                Expr::new().i64_const(2147483648),
                2147483648,
            ),
            case_i64(
                "-2147483649 is past the other end",
                Expr::new().i64_const(-2147483649),
                -2147483649,
            ),
            case_i64(
                "0x00ff00ff00ff00ff keeps every one of its bytes",
                Expr::new().i64_const(0x00ff_00ff_00ff_00ff),
                0x00ff_00ff_00ff_00ff,
            ),
            case_i64(
                "i64::MAX needs the full ten bytes of LEB128",
                Expr::new().i64_const(i64::MAX),
                i64::MAX,
            ),
            case_i64(
                "i64::MIN needs them too",
                Expr::new().i64_const(i64::MIN),
                i64::MIN,
            ),
            case_i64(
                "a ten-byte constant still adds like any other",
                bin(op::I64_ADD, i64::MIN, i64::MAX),
                -1,
            ),
        ],
    )
});

/// An `(i64, i64) -> i64` case applying one operator to the two arguments.
fn arg_case(name: &str, o: Op, args: &[&str], want: i64) -> Case {
    Case::new(
        name,
        ftype(&[ValType::I64, ValType::I64], &[ValType::I64]),
        Expr::new().local_get(0).local_get(1).op(o),
        &[&want.to_string()],
    )
    .args(args)
}

wasm_test!(from_the_command_line, |ctx| {
    run_cases(
        ctx,
        "i64-args",
        vec![
            arg_case("add of 3 and 4 is 7", op::I64_ADD, &["3", "4"], 7),
            arg_case(
                "add of -5 and 2 is -3",
                op::I64_ADD,
                &["-5", "2"],
                -3,
            ),
            arg_case(
                "add of i64::MAX and 1 wraps to i64::MIN",
                op::I64_ADD,
                &["9223372036854775807", "1"],
                i64::MIN,
            ),
            arg_case(
                "sub of i64::MIN and 1 wraps to i64::MAX",
                op::I64_SUB,
                &["-9223372036854775808", "1"],
                i64::MAX,
            ),
            arg_case(
                "mul of 65536 and 65536 is 4294967296, so the argument really is sixty-four bits wide",
                op::I64_MUL,
                &["65536", "65536"],
                4294967296,
            ),
            arg_case(
                "mul of 4294967296 and 4294967296 is 0",
                op::I64_MUL,
                &["4294967296", "4294967296"],
                0,
            ),
            arg_case(
                "and of -1 and 4294967295 is 4294967295",
                op::I64_AND,
                &["-1", "4294967295"],
                4294967295,
            ),
            Case::new(
                "a one-argument function wraps its positive i64 down to a negative i32",
                ftype(&[ValType::I64], &[ValType::I32]),
                Expr::new().local_get(0).op(op::I32_WRAP_I64),
                &["-2147483648"],
            )
            .args(&["2147483648"]),
            Case::new(
                "a one-argument function extends its i32 up to an i64 without a sign",
                ftype(&[ValType::I32], &[ValType::I64]),
                Expr::new().local_get(0).op(op::I64_EXTEND_I32_U),
                &["4294967295"],
            )
            .args(&["-1"]),
        ],
    )
});

/// `f() -> i64` returning `65536 * 65536`, the multiply that is 0 at the wrong width.
fn wide_product_module() -> Module {
    let mut b = ModuleBuilder::new("i64-65536-squared");
    let ty = b.add_type(ftype(&[], &[ValType::I64]));
    let idx = b.add_func(
        ty,
        Func::new(
            Expr::new()
                .i64_const(65536)
                .i64_const(65536)
                .op(op::I64_MUL),
        ),
    );
    b.export_func("f", idx).build()
}

/// `f() -> i64` returning `i64.extend_i32_u` of -1, the half of the pair people forget.
fn extend_u_module() -> Module {
    let mut b = ModuleBuilder::new("i64-extend-u-minus-one");
    let ty = b.add_type(ftype(&[], &[ValType::I64]));
    let idx = b.add_func(
        ty,
        Func::new(Expr::new().i32_const(-1).op(op::I64_EXTEND_I32_U)),
    );
    b.export_func("f", idx).build()
}

/// Worked examples: the multiply that catches a narrow register, and the extension pair.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module(
            "A product that needs all sixty-four bits",
            wide_product_module,
        )
        .summary("`f() -> i64` returning `i64.const 65536, i64.const 65536, i64.mul`")
        .command("run --invoke f mod.wasm")
        .output("4294967296")
        .note(
            "The same two operands at i32 give 0. If this prints 0, the multiply is \
                 being done in a thirty-two-bit register somewhere on the way.",
        ),
        ExampleSpec::module("The unsigned half of the extension pair", extend_u_module)
            .summary("`f() -> i64` returning `i32.const -1, i64.extend_i32_u`")
            .command("run --invoke f mod.wasm")
            .output("4294967295")
            .note(
                "`i64.extend_i32_s` of the same -1 is -1. The two instructions read the very \
                 same thirty-two bits and disagree about what the top bit means; implement \
                 both, and do not let one call the other.",
            ),
    ]
}
