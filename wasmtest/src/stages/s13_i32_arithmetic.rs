//! Stage 13 — i32 arithmetic and its edge values.
//!
//! Every expectation here is written the way Rust's `wrapping_add`, `wrapping_sub` and
//! `wrapping_mul` compute it, because that is exactly what the spec asks for: the three
//! arithmetic operators are addition, subtraction and multiplication modulo 2^32, with the
//! result read back as a signed 32-bit integer. There is no overflow trap anywhere in this
//! stage — `i32.mul` of `i32::MIN` by `-1` quietly gives `i32::MIN` back, where
//! `i32.div_s` of the same pair traps (stage 15).
//!
//! The one thing worth saying about the *output*: an `i32` result is printed as a **signed**
//! decimal, so a function returning the bit pattern `0xffffffff` prints `-1` and one
//! returning `0x80000000` prints `-2147483648`. A runtime that prints `4294967295` has got
//! the arithmetic right and the formatting wrong, and the suite will still call it red,
//! because the contract is what a caller sees.

use crate::examples::ExampleSpec;
use crate::stages::{case_i32, run_cases, Case, Stage, Test};
use crate::wasm::{ftype, op, Expr, Module, ModuleBuilder, Op, ValType};
use crate::wasm::{Func, FuncType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 13,
        slug: "i32_arithmetic",
        name: "i32 arithmetic and its edge values",
        ext: false,
        hints: &[
            "i32.add, i32.sub and i32.mul are arithmetic modulo 2^32: compute in a wider type and mask, or use your language's wrapping operators — never the ones that panic on overflow",
            "None of the three ever traps, so i32.mul of -2147483648 by -1 is -2147483648 again; only div_s and rem_s have an overflow trap, and that is the next stage but one",
            "The operands come off the stack right to left: the value pushed first is the left operand, so `i32.const 10, i32.const 3, i32.sub` is 7 and not -7",
            "and, or and xor operate on all thirty-two bits including the sign bit; keep the value in an unsigned 32-bit register internally and only reinterpret it as signed when you print it",
        ],
        examples,
        tests: vec![
            Test::new("add wraps at both ends of the range", add_wraps),
            Test::new("sub is add of the negation, and it wraps too", sub_wraps),
            Test::new("mul keeps the low thirty-two bits and throws the rest away", mul_wraps),
            Test::new("and, or and xor work on all thirty-two bits", bitwise),
            Test::new("the sign bit is just another bit to the bitwise operators", sign_bit),
            Test::new("zero and one are the identities, and zero annihilates", identities),
            Test::new("add and sub undo one another even across the wrap", inverses),
            Test::new("operands are taken off the stack in the order they were pushed", operand_order),
            Test::new("arguments given on the command line reach the function", from_the_command_line),
        ],
    }
}

/// `a op b` as a whole `() -> i32` body.
fn bin(o: Op, a: i32, b: i32) -> Expr {
    Expr::new().i32_const(a).i32_const(b).op(o)
}

/// The two operands that share no bit: `0xaaaaaaaa` and `0x55555555`.
const ALT_HIGH: i32 = 0xaaaa_aaaau32 as i32;
/// The other half of that pair.
const ALT_LOW: i32 = 0x5555_5555;
/// A pattern with a byte in every nibble position, used where a value needs to be arbitrary.
const PATTERN: i32 = 0x1234_5678;

wasm_test!(add_wraps, |ctx| {
    run_cases(
        ctx,
        "i32-add",
        vec![
            case_i32("0 + 0 is 0", bin(op::I32_ADD, 0, 0), 0),
            case_i32("0 + 7 leaves 7 alone", bin(op::I32_ADD, 0, 7), 7),
            case_i32("-1 + 1 is 0", bin(op::I32_ADD, -1, 1), 0),
            case_i32("-1 + -1 is -2", bin(op::I32_ADD, -1, -1), -2),
            case_i32(
                "i32::MAX + 1 wraps round to i32::MIN",
                bin(op::I32_ADD, i32::MAX, 1),
                i32::MIN,
            ),
            case_i32(
                "i32::MAX + i32::MAX is -2, not a saturated i32::MAX",
                bin(op::I32_ADD, i32::MAX, i32::MAX),
                i32::MAX.wrapping_add(i32::MAX),
            ),
            case_i32(
                "i32::MIN + -1 wraps the other way, to i32::MAX",
                bin(op::I32_ADD, i32::MIN, -1),
                i32::MAX,
            ),
            case_i32(
                "i32::MIN + i32::MIN is 0: 2^32 modulo 2^32",
                bin(op::I32_ADD, i32::MIN, i32::MIN),
                0,
            ),
            case_i32(
                "i32::MIN + i32::MAX is -1, the two ends one apart",
                bin(op::I32_ADD, i32::MIN, i32::MAX),
                -1,
            ),
            case_i32(
                "1000000 + -1000000 is 0",
                bin(op::I32_ADD, 1_000_000, -1_000_000),
                0,
            ),
        ],
    )
});

wasm_test!(sub_wraps, |ctx| {
    run_cases(
        ctx,
        "i32-sub",
        vec![
            case_i32("0 - 0 is 0", bin(op::I32_SUB, 0, 0), 0),
            case_i32("0 - 1 is -1", bin(op::I32_SUB, 0, 1), -1),
            case_i32("7 - 7 is 0", bin(op::I32_SUB, 7, 7), 0),
            case_i32("-1 - -1 is 0", bin(op::I32_SUB, -1, -1), 0),
            case_i32("5 - 9 is -4", bin(op::I32_SUB, 5, 9), -4),
            case_i32(
                "9 - 5 is 4: sub does not commute",
                bin(op::I32_SUB, 9, 5),
                4,
            ),
            case_i32(
                "0 - i32::MIN is i32::MIN again — the one value that is its own negation",
                bin(op::I32_SUB, 0, i32::MIN),
                i32::MIN,
            ),
            case_i32(
                "i32::MIN - 1 wraps round to i32::MAX",
                bin(op::I32_SUB, i32::MIN, 1),
                i32::MAX,
            ),
            case_i32(
                "i32::MAX - -1 wraps the other way, to i32::MIN",
                bin(op::I32_SUB, i32::MAX, -1),
                i32::MIN,
            ),
            case_i32(
                "i32::MAX - i32::MIN is -1, because the true answer is 2^32 - 1",
                bin(op::I32_SUB, i32::MAX, i32::MIN),
                -1,
            ),
            case_i32(
                "i32::MIN - i32::MAX is 1, the same difference the other way round",
                bin(op::I32_SUB, i32::MIN, i32::MAX),
                1,
            ),
        ],
    )
});

wasm_test!(mul_wraps, |ctx| {
    run_cases(
        ctx,
        "i32-mul",
        vec![
            case_i32("0 * i32::MIN is 0", bin(op::I32_MUL, 0, i32::MIN), 0),
            case_i32(
                "1 * i32::MIN leaves it alone",
                bin(op::I32_MUL, 1, i32::MIN),
                i32::MIN,
            ),
            case_i32("-1 * -1 is 1", bin(op::I32_MUL, -1, -1), 1),
            case_i32("3 * -7 is -21", bin(op::I32_MUL, 3, -7), -21),
            case_i32(
                "i32::MIN * -1 is i32::MIN again: mul has no overflow trap",
                bin(op::I32_MUL, i32::MIN, -1),
                i32::MIN,
            ),
            case_i32(
                "65536 * 65536 is 0: the only bit set would be bit 32, and there is no bit 32",
                bin(op::I32_MUL, 65536, 65536),
                0,
            ),
            case_i32(
                "65535 * 65537 is -1: 2^32 - 1 read back as a signed integer",
                bin(op::I32_MUL, 65535, 65537),
                -1,
            ),
            case_i32(
                "46341 squared overshoots i32::MAX and comes back negative",
                bin(op::I32_MUL, 46341, 46341),
                46341i32.wrapping_mul(46341),
            ),
            case_i32(
                "i32::MAX * 2 is -2",
                bin(op::I32_MUL, i32::MAX, 2),
                i32::MAX.wrapping_mul(2),
            ),
            case_i32(
                "i32::MAX * i32::MAX is 1",
                bin(op::I32_MUL, i32::MAX, i32::MAX),
                i32::MAX.wrapping_mul(i32::MAX),
            ),
            case_i32(
                "i32::MIN * i32::MIN is 0: every bit of the product is above bit 31",
                bin(op::I32_MUL, i32::MIN, i32::MIN),
                0,
            ),
        ],
    )
});

wasm_test!(bitwise, |ctx| {
    run_cases(
        ctx,
        "i32-bitwise",
        vec![
            case_i32(
                "0xaaaaaaaa and 0x55555555 is 0: the two patterns share no bit",
                bin(op::I32_AND, ALT_HIGH, ALT_LOW),
                0,
            ),
            case_i32(
                "0xaaaaaaaa or 0x55555555 is -1: between them they are every bit",
                bin(op::I32_OR, ALT_HIGH, ALT_LOW),
                -1,
            ),
            case_i32(
                "0xaaaaaaaa xor 0x55555555 is -1 as well",
                bin(op::I32_XOR, ALT_HIGH, ALT_LOW),
                -1,
            ),
            case_i32(
                "0xaaaaaaaa xor itself is 0",
                bin(op::I32_XOR, ALT_HIGH, ALT_HIGH),
                0,
            ),
            case_i32(
                "0xaaaaaaaa and itself is 0xaaaaaaaa, which prints as a negative number",
                bin(op::I32_AND, ALT_HIGH, ALT_HIGH),
                ALT_HIGH,
            ),
            case_i32(
                "-1 and 0x0f0f0f0f selects the low nibble of every byte",
                bin(op::I32_AND, -1, 0x0f0f_0f0f),
                0x0f0f_0f0f,
            ),
            case_i32(
                "0x0f0f0f0f xor -1 is the one's complement, 0xf0f0f0f0",
                bin(op::I32_XOR, 0x0f0f_0f0f, -1),
                0xf0f0_f0f0u32 as i32,
            ),
            case_i32(
                "0x12345678 and 0xff keeps only the low byte",
                bin(op::I32_AND, PATTERN, 0xff),
                0x78,
            ),
            case_i32(
                "0x12345678 or 0xff000000 sets the whole top byte",
                bin(op::I32_OR, PATTERN, 0xff00_0000u32 as i32),
                0xff34_5678u32 as i32,
            ),
            case_i32(
                "0x12345678 xor 0x12345678 is 0",
                bin(op::I32_XOR, PATTERN, PATTERN),
                0,
            ),
        ],
    )
});

wasm_test!(sign_bit, |ctx| {
    run_cases(
        ctx,
        "i32-sign-bit",
        vec![
            case_i32(
                "the sign bit on its own is i32::MIN",
                bin(op::I32_OR, i32::MIN, 0),
                i32::MIN,
            ),
            case_i32(
                "i32::MIN and i32::MAX is 0: they have no bit in common",
                bin(op::I32_AND, i32::MIN, i32::MAX),
                0,
            ),
            case_i32(
                "i32::MIN or i32::MAX is -1: together they are the whole word",
                bin(op::I32_OR, i32::MIN, i32::MAX),
                -1,
            ),
            case_i32(
                "i32::MIN xor i32::MIN is 0",
                bin(op::I32_XOR, i32::MIN, i32::MIN),
                0,
            ),
            case_i32(
                "-1 and i32::MIN keeps the sign bit and nothing else",
                bin(op::I32_AND, -1, i32::MIN),
                i32::MIN,
            ),
            case_i32(
                "clearing the sign bit of -1 with and 0x7fffffff gives i32::MAX",
                bin(op::I32_AND, -1, i32::MAX),
                i32::MAX,
            ),
            case_i32(
                "setting the sign bit of 1 with or i32::MIN gives -2147483647",
                bin(op::I32_OR, 1, i32::MIN),
                -2147483647,
            ),
            case_i32(
                "flipping the sign bit of 0 with xor i32::MIN gives i32::MIN",
                bin(op::I32_XOR, 0, i32::MIN),
                i32::MIN,
            ),
            case_i32(
                "flipping the sign bit of -1 with xor i32::MIN gives i32::MAX",
                bin(op::I32_XOR, -1, i32::MIN),
                i32::MAX,
            ),
        ],
    )
});

wasm_test!(identities, |ctx| {
    run_cases(
        ctx,
        "i32-identities",
        vec![
            case_i32(
                "adding 0 to i32::MIN changes nothing",
                bin(op::I32_ADD, i32::MIN, 0),
                i32::MIN,
            ),
            case_i32(
                "adding 0 to i32::MAX changes nothing",
                bin(op::I32_ADD, i32::MAX, 0),
                i32::MAX,
            ),
            case_i32(
                "subtracting 0 from i32::MIN changes nothing",
                bin(op::I32_SUB, i32::MIN, 0),
                i32::MIN,
            ),
            case_i32(
                "multiplying i32::MIN by 1 changes nothing",
                bin(op::I32_MUL, i32::MIN, 1),
                i32::MIN,
            ),
            case_i32(
                "multiplying i32::MAX by 0 gives 0",
                bin(op::I32_MUL, i32::MAX, 0),
                0,
            ),
            case_i32(
                "and with 0 clears every bit of 0x12345678",
                bin(op::I32_AND, PATTERN, 0),
                0,
            ),
            case_i32(
                "and with -1 leaves 0x12345678 alone",
                bin(op::I32_AND, PATTERN, -1),
                PATTERN,
            ),
            case_i32(
                "or with 0 leaves 0x12345678 alone",
                bin(op::I32_OR, PATTERN, 0),
                PATTERN,
            ),
            case_i32(
                "or with -1 sets every bit",
                bin(op::I32_OR, PATTERN, -1),
                -1,
            ),
            case_i32(
                "xor with 0 leaves 0x12345678 alone",
                bin(op::I32_XOR, PATTERN, 0),
                PATTERN,
            ),
        ],
    )
});

wasm_test!(inverses, |ctx| {
    run_cases(
        ctx,
        "i32-inverses",
        vec![
            case_i32(
                "(i32::MAX + 1) - 1 is back at i32::MAX",
                bin(op::I32_ADD, i32::MAX, 1).i32_const(1).op(op::I32_SUB),
                i32::MAX,
            ),
            case_i32(
                "(i32::MIN - 1) + 1 is back at i32::MIN",
                bin(op::I32_SUB, i32::MIN, 1).i32_const(1).op(op::I32_ADD),
                i32::MIN,
            ),
            case_i32(
                "(i32::MIN + i32::MIN) - i32::MIN is i32::MIN",
                bin(op::I32_ADD, i32::MIN, i32::MIN)
                    .i32_const(i32::MIN)
                    .op(op::I32_SUB),
                i32::MIN,
            ),
            case_i32(
                "negating i32::MIN twice gives i32::MIN, and never trapped on the way",
                Expr::new()
                    .i32_const(0)
                    .then(bin(op::I32_SUB, 0, i32::MIN))
                    .op(op::I32_SUB),
                i32::MIN,
            ),
            case_i32(
                "(i32::MAX + i32::MAX) - i32::MAX is i32::MAX",
                bin(op::I32_ADD, i32::MAX, i32::MAX)
                    .i32_const(i32::MAX)
                    .op(op::I32_SUB),
                i32::MAX,
            ),
            case_i32(
                "(i32::MAX + 1) + 1 is -2147483647",
                bin(op::I32_ADD, i32::MAX, 1).i32_const(1).op(op::I32_ADD),
                i32::MAX.wrapping_add(2),
            ),
            case_i32(
                "i32::MAX + (1 + 1) is the same -2147483647: wrapping addition is associative",
                Expr::new()
                    .i32_const(i32::MAX)
                    .then(bin(op::I32_ADD, 1, 1))
                    .op(op::I32_ADD),
                i32::MAX.wrapping_add(2),
            ),
            case_i32(
                "(7 * 1000) - 7000 is 0",
                bin(op::I32_MUL, 7, 1000).i32_const(7000).op(op::I32_SUB),
                0,
            ),
            case_i32(
                "(0x12345678 xor -1) xor -1 is 0x12345678 again",
                bin(op::I32_XOR, PATTERN, -1).i32_const(-1).op(op::I32_XOR),
                PATTERN,
            ),
        ],
    )
});

wasm_test!(operand_order, |ctx| {
    run_cases(
        ctx,
        "i32-operand-order",
        vec![
            case_i32(
                "10 then 3 then sub is 7: the value pushed first is the left operand",
                bin(op::I32_SUB, 10, 3),
                7,
            ),
            case_i32(
                "3 then 10 then sub is -7, which is how you can tell",
                bin(op::I32_SUB, 3, 10),
                -7,
            ),
            case_i32(
                "(100 - 10) - 1 is 89",
                bin(op::I32_SUB, 100, 10).i32_const(1).op(op::I32_SUB),
                89,
            ),
            case_i32(
                "100 - (10 - 1) is 91",
                Expr::new()
                    .i32_const(100)
                    .then(bin(op::I32_SUB, 10, 1))
                    .op(op::I32_SUB),
                91,
            ),
            case_i32(
                "(1 + 2) * 3 is 9",
                bin(op::I32_ADD, 1, 2).i32_const(3).op(op::I32_MUL),
                9,
            ),
            case_i32(
                "1 + (2 * 3) is 7, so the nesting really is the stack shape",
                Expr::new()
                    .i32_const(1)
                    .then(bin(op::I32_MUL, 2, 3))
                    .op(op::I32_ADD),
                7,
            ),
            case_i32(
                "(i32::MAX - 0) - 1 stays inside the range",
                bin(op::I32_SUB, i32::MAX, 0).i32_const(1).op(op::I32_SUB),
                2147483646,
            ),
            case_i32(
                "i32::MAX - (0 - 1) wraps, because the right operand is -1",
                Expr::new()
                    .i32_const(i32::MAX)
                    .then(bin(op::I32_SUB, 0, 1))
                    .op(op::I32_SUB),
                i32::MIN,
            ),
            case_i32(
                "0 - (i32::MIN - i32::MIN) is 0",
                Expr::new()
                    .i32_const(0)
                    .then(bin(op::I32_SUB, i32::MIN, i32::MIN))
                    .op(op::I32_SUB),
                0,
            ),
        ],
    )
});

/// A `(i32, i32) -> i32` case applying one operator to the two arguments.
fn arg_case(name: &str, o: Op, args: &[&str], want: i32) -> Case {
    Case::new(
        name,
        ftype(&[ValType::I32, ValType::I32], &[ValType::I32]),
        Expr::new().local_get(0).local_get(1).op(o),
        &[&want.to_string()],
    )
    .args(args)
}

wasm_test!(from_the_command_line, |ctx| {
    run_cases(
        ctx,
        "i32-args",
        vec![
            arg_case("add of 3 and 4 is 7", op::I32_ADD, &["3", "4"], 7),
            arg_case("add of 0 and 0 is 0", op::I32_ADD, &["0", "0"], 0),
            arg_case(
                "add of -5 and 2 is -3: a negative argument is an argument, not a flag",
                op::I32_ADD,
                &["-5", "2"],
                -3,
            ),
            arg_case(
                "add of 2147483647 and 1 wraps to i32::MIN",
                op::I32_ADD,
                &["2147483647", "1"],
                i32::MIN,
            ),
            arg_case(
                "sub of -2147483648 and 1 wraps to i32::MAX",
                op::I32_SUB,
                &["-2147483648", "1"],
                i32::MAX,
            ),
            arg_case(
                "mul of 65536 and 65536 is 0",
                op::I32_MUL,
                &["65536", "65536"],
                0,
            ),
            arg_case("and of -1 and 255 is 255", op::I32_AND, &["-1", "255"], 255),
            arg_case("xor of -1 and -1 is 0", op::I32_XOR, &["-1", "-1"], 0),
            Case::new(
                "a one-argument function negates -2147483648 back to itself",
                ftype(&[ValType::I32], &[ValType::I32]),
                Expr::new().i32_const(0).local_get(0).op(op::I32_SUB),
                &["-2147483648"],
            )
            .args(&["-2147483648"]),
        ],
    )
});

/// The wrapping add of the two ends of the range, as its own module.
fn wrap_module() -> Module {
    let mut b = ModuleBuilder::new("i32-max-plus-one");
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(
        ty,
        Func::new(Expr::new().i32_const(i32::MAX).i32_const(1).op(op::I32_ADD)),
    );
    b.export_func("f", idx).build()
}

/// A two-argument adder, the module behind the command-line test.
fn adder_module() -> Module {
    let sig: FuncType = ftype(&[ValType::I32, ValType::I32], &[ValType::I32]);
    let mut b = ModuleBuilder::new("i32-add-args");
    let ty = b.add_type(sig);
    let idx = b.add_func(
        ty,
        Func::new(Expr::new().local_get(0).local_get(1).op(op::I32_ADD)),
    );
    b.export_func("f", idx).build()
}

/// Worked examples: the wrap nobody expects, and arguments arriving from the shell.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("Adding one to the largest i32", wrap_module)
            .summary("`f() -> i32` returning `i32.const 2147483647, i32.const 1, i32.add`")
            .command("run --invoke f mod.wasm")
            .output("-2147483648")
            .note(
                "i32.add is addition modulo 2^32 and never traps. Compute in a wider type \
                 and mask back to thirty-two bits, or use a wrapping operator; a language \
                 whose `+` panics on overflow will fail here and nowhere else.",
            ),
        ExampleSpec::module("Two arguments off the command line", adder_module)
            .summary("`f(i32, i32) -> i32` returning `local.get 0, local.get 1, i32.add`")
            .command("run --invoke f mod.wasm -5 2")
            .output("-3")
            .note(
                "The arguments follow the module path directly, are parsed as signed \
                 decimals, and are bound to the locals in order. A value outside the signed \
                 range is a parse error, not a wrap.",
            ),
    ]
}
