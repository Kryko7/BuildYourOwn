//! Stage 22 — if / else and br_if.
//!
//! `if` is not a new kind of control flow: it is a `block` that is entered only when the
//! i32 on top of the stack is non-zero, and `br_if` is a `br` guarded the same way. Both
//! pop exactly one value, the condition, and treat every non-zero bit pattern as true —
//! `-1`, `2` and `INT_MIN` are all as true as `1`, and only `0` is false.
//!
//! The `else` branch is not a separate construct either. `if bt … else … end` is one block
//! with two bodies; both must leave the same thing on the stack, which is why an
//! `if (result i32)` without an `else` does not validate.

use crate::examples::ExampleSpec;
use crate::stages::{case_i32, run_cases, single_with_locals, Case, Stage, Test};
use crate::wasm::{ftype, op, BlockType, Expr, Module, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 22,
        slug: "if_else",
        name: "if / else and br_if",
        ext: false,
        hints: &[
            "`if` pops one i32 and runs its first body when that value is anything but zero; `br_if l` pops one i32 and branches to `l` on the same test",
            "Truth is 'not zero', not 'equal to one': -1, 2 and INT_MIN are all true, and the condition is consumed either way",
            "`if bt … else … end` is a single block with two bodies, so both arms must leave exactly what the block type promises — an `if (result i32)` with no `else` cannot validate",
            "A `br_if` that does not branch leaves everything except the condition where it was, so the value it would have carried is still on the stack",
        ],
        examples,
        tests: vec![
            Test::new("an if with no else runs its body only when the condition is true", if_without_else),
            Test::new("every non-zero condition is true and only zero is false", what_counts_as_true),
            Test::new("an if with a result type yields the value of the arm that ran", if_with_a_result),
            Test::new("nested ifs choose an arm at each level", nested_ifs),
            Test::new("an if inside a loop decides afresh on every iteration", if_inside_a_loop),
            Test::new("br_if branches when its condition is true and falls through when it is false", br_if_taken_or_not),
            Test::new("br_if consumes its condition and nothing else", br_if_and_the_stack),
            Test::new("max3 returns the largest of its three arguments", max3),
        ],
    }
}

/// One `i32` local, used as an accumulator by the cases that have no result to inspect.
const ACC: &[(u32, ValType)] = &[(1, ValType::I32)];

/// A `() -> i32` case with a single `i32` local.
fn acc_case(name: impl Into<String>, body: Expr, want: i32) -> Case {
    case_i32(name, body, want).locals(ACC)
}

wasm_test!(if_without_else, |ctx| {
    run_cases(
        ctx,
        "if-without-else",
        vec![
            acc_case(
                "a true condition runs the body",
                Expr::new()
                    .i32_const(1)
                    .if_(BlockType::Empty, Expr::new().i32_const(7).local_set(0))
                    .local_get(0),
                7,
            ),
            acc_case(
                "a false condition skips the body",
                Expr::new()
                    .i32_const(0)
                    .if_(BlockType::Empty, Expr::new().i32_const(7).local_set(0))
                    .local_get(0),
                0,
            ),
            acc_case(
                "what follows the if runs whichever way it went",
                Expr::new()
                    .i32_const(0)
                    .if_(BlockType::Empty, Expr::new().i32_const(7).local_set(0))
                    .local_get(0)
                    .i32_const(3)
                    .op(op::I32_ADD)
                    .local_set(0)
                    .local_get(0),
                3,
            ),
            // The condition is popped even when the body does not run, so the 5 pushed
            // before it is still there afterwards.
            case_i32(
                "the condition is consumed whether or not the body runs",
                Expr::new()
                    .i32_const(5)
                    .i32_const(0)
                    .if_(BlockType::Empty, Expr::new().nop())
                    .i32_const(2)
                    .op(op::I32_ADD),
                7,
            ),
        ],
    )
});

/// `if (result i32) { 1 } else { 0 }` on the given condition — a `bool` of the condition.
fn truth_of(cond: i32) -> Expr {
    Expr::new().i32_const(cond).if_else(
        BlockType::Value(ValType::I32),
        Expr::new().i32_const(1),
        Expr::new().i32_const(0),
    )
}

wasm_test!(what_counts_as_true, |ctx| {
    run_cases(
        ctx,
        "what-counts-as-true",
        vec![
            case_i32("0 is the only false value", truth_of(0), 0),
            case_i32("1 is true", truth_of(1), 1),
            case_i32("-1 is true", truth_of(-1), 1),
            case_i32("2 is true", truth_of(2), 1),
            case_i32("INT_MAX is true", truth_of(i32::MAX), 1),
            case_i32("INT_MIN is true", truth_of(i32::MIN), 1),
            // 256 is true, and its low byte is zero: an implementation that tests only the
            // bottom eight bits of the condition gets this one wrong and nothing else.
            case_i32("256 is true, low byte and all", truth_of(256), 1),
        ],
    )
});

wasm_test!(if_with_a_result, |ctx| {
    run_cases(
        ctx,
        "if-with-a-result",
        vec![
            case_i32(
                "a true condition takes the value from the then arm",
                Expr::new().i32_const(1).if_else(
                    BlockType::Value(ValType::I32),
                    Expr::new().i32_const(10),
                    Expr::new().i32_const(20),
                ),
                10,
            ),
            case_i32(
                "a false condition takes the value from the else arm",
                Expr::new().i32_const(0).if_else(
                    BlockType::Value(ValType::I32),
                    Expr::new().i32_const(10),
                    Expr::new().i32_const(20),
                ),
                20,
            ),
            case_i32(
                "an arm may compute its value rather than name it",
                Expr::new().i32_const(1).if_else(
                    BlockType::Value(ValType::I32),
                    Expr::new().i32_const(3).i32_const(4).op(op::I32_ADD),
                    Expr::new().i32_const(0),
                ),
                7,
            ),
            case_i32(
                "the value the if produced is on the stack for what follows",
                Expr::new()
                    .i32_const(0)
                    .if_else(
                        BlockType::Value(ValType::I32),
                        Expr::new().i32_const(10),
                        Expr::new().i32_const(20),
                    )
                    .i32_const(2)
                    .op(op::I32_MUL),
                40,
            ),
            // An if of type Empty is still an if: both arms must leave nothing behind.
            acc_case(
                "an if with an empty block type leaves nothing on the stack",
                Expr::new()
                    .i32_const(0)
                    .if_else(
                        BlockType::Empty,
                        Expr::new().i32_const(10).local_set(0),
                        Expr::new().i32_const(20).local_set(0),
                    )
                    .local_get(0),
                20,
            ),
        ],
    )
});

/// `if a { if b { 3 } else { 2 } } else { if b { 1 } else { 0 } }` — a two-bit decoder.
fn two_bits(a: i32, b: i32) -> Expr {
    let inner = |hi: i32, lo: i32| {
        Expr::new().i32_const(b).if_else(
            BlockType::Value(ValType::I32),
            Expr::new().i32_const(hi),
            Expr::new().i32_const(lo),
        )
    };
    Expr::new()
        .i32_const(a)
        .if_else(BlockType::Value(ValType::I32), inner(3, 2), inner(1, 0))
}

wasm_test!(nested_ifs, |ctx| {
    run_cases(
        ctx,
        "nested-ifs",
        vec![
            case_i32("both conditions true", two_bits(1, 1), 3),
            case_i32("the outer true, the inner false", two_bits(1, 0), 2),
            case_i32("the outer false, the inner true", two_bits(0, 1), 1),
            case_i32("both conditions false", two_bits(0, 0), 0),
            // Both arms branch out of the block around the if, so the store after the if
            // never happens whichever arm ran.
            acc_case(
                "both arms of an if br out of the enclosing block, the then arm",
                Expr::new()
                    .block(
                        BlockType::Empty,
                        Expr::new()
                            .i32_const(1)
                            .if_else(
                                BlockType::Empty,
                                Expr::new().i32_const(4).local_set(0).br(1),
                                Expr::new().i32_const(5).local_set(0).br(1),
                            )
                            .i32_const(99)
                            .local_set(0),
                    )
                    .local_get(0),
                4,
            ),
            acc_case(
                "both arms of an if br out of the enclosing block, the else arm",
                Expr::new()
                    .block(
                        BlockType::Empty,
                        Expr::new()
                            .i32_const(0)
                            .if_else(
                                BlockType::Empty,
                                Expr::new().i32_const(4).local_set(0).br(1),
                                Expr::new().i32_const(5).local_set(0).br(1),
                            )
                            .i32_const(99)
                            .local_set(0),
                    )
                    .local_get(0),
                5,
            ),
        ],
    )
});

wasm_test!(if_inside_a_loop, |ctx| {
    // Sum the odd numbers from 1 to 10: the if fires on five of the ten iterations.
    let sum_odd = Expr::new()
        .loop_(
            BlockType::Empty,
            Expr::new()
                .local_get(0)
                .i32_const(1)
                .op(op::I32_ADD)
                .local_tee(0)
                .i32_const(1)
                .op(op::I32_AND)
                .if_(
                    BlockType::Empty,
                    Expr::new()
                        .local_get(1)
                        .local_get(0)
                        .op(op::I32_ADD)
                        .local_set(1),
                )
                .local_get(0)
                .i32_const(10)
                .op(op::I32_LT_S)
                .br_if(0),
        )
        .local_get(1);
    run_cases(
        ctx,
        "if-inside-a-loop",
        vec![
            Case::new(
                "an if inside a loop fires on some iterations and not others",
                ftype(&[], &[ValType::I32]),
                sum_odd,
                &["25"],
            )
            .locals(&[(2, ValType::I32)]),
            // From inside the if the labels are 0 = the if, 1 = the loop, 2 = the block, so
            // br 2 is the way out of the whole thing.
            acc_case(
                "an if that branches out of the block around the loop ends it",
                Expr::new()
                    .block(
                        BlockType::Empty,
                        Expr::new().loop_(
                            BlockType::Empty,
                            Expr::new()
                                .local_get(0)
                                .i32_const(1)
                                .op(op::I32_ADD)
                                .local_tee(0)
                                .i32_const(4)
                                .op(op::I32_EQ)
                                .if_(BlockType::Empty, Expr::new().br(2))
                                .br(0),
                        ),
                    )
                    .local_get(0),
                4,
            ),
            acc_case(
                "a loop whose if never fires still terminates",
                Expr::new()
                    .loop_(
                        BlockType::Empty,
                        Expr::new()
                            .local_get(0)
                            .i32_const(1)
                            .op(op::I32_ADD)
                            .local_tee(0)
                            .i32_const(0)
                            .if_(BlockType::Empty, Expr::new().i32_const(99).local_set(0))
                            .i32_const(3)
                            .op(op::I32_LT_S)
                            .br_if(0),
                    )
                    .local_get(0),
                3,
            ),
            // The if decides whether to go round again: the loop's own br_if is gone.
            acc_case(
                "an if inside the loop drives the branch back to the top",
                Expr::new()
                    .loop_(
                        BlockType::Empty,
                        Expr::new()
                            .local_get(0)
                            .i32_const(1)
                            .op(op::I32_ADD)
                            .local_tee(0)
                            .i32_const(6)
                            .op(op::I32_LT_S)
                            .if_(BlockType::Empty, Expr::new().br(1)),
                    )
                    .local_get(0),
                6,
            ),
        ],
    )
});

/// A `br_if l` inside `depth` nested blocks, with the given condition.
///
/// When the branch is taken the stores after every enclosing `end` are skipped and the
/// local keeps its initial 0; when it is not, the innermost store runs and gives 9.
fn br_if_nest(depth: u32, cond: i32) -> Expr {
    let mut body = Expr::new()
        .i32_const(cond)
        .br_if(depth)
        .i32_const(9)
        .local_set(0);
    for _ in 0..=depth {
        body = Expr::new().block(BlockType::Empty, body);
    }
    body.local_get(0)
}

wasm_test!(br_if_taken_or_not, |ctx| {
    run_cases(
        ctx,
        "br-if-taken-or-not",
        vec![
            acc_case("br_if 0 taken leaves one block", br_if_nest(0, 1), 0),
            acc_case("br_if 0 not taken falls through", br_if_nest(0, 0), 9),
            acc_case("br_if 1 taken leaves two blocks", br_if_nest(1, 1), 0),
            acc_case("br_if 1 not taken falls through", br_if_nest(1, 0), 9),
            acc_case("br_if 2 taken leaves three blocks", br_if_nest(2, -1), 0),
            acc_case("br_if 2 not taken falls through", br_if_nest(2, 0), 9),
        ],
    )
});

wasm_test!(br_if_and_the_stack, |ctx| {
    run_cases(
        ctx,
        "br-if-and-the-stack",
        vec![
            // A typed block: when the br_if is taken it carries the 7 out; when it is not,
            // the 7 is still on the stack and the rest of the body replaces it with 9.
            case_i32(
                "a taken br_if carries the value out of a typed block",
                Expr::new().block(
                    BlockType::Value(ValType::I32),
                    Expr::new()
                        .i32_const(7)
                        .i32_const(1)
                        .br_if(0)
                        .drop()
                        .i32_const(9),
                ),
                7,
            ),
            case_i32(
                "an untaken br_if leaves the value it would have carried in place",
                Expr::new().block(
                    BlockType::Value(ValType::I32),
                    Expr::new()
                        .i32_const(7)
                        .i32_const(0)
                        .br_if(0)
                        .drop()
                        .i32_const(9),
                ),
                9,
            ),
            case_i32(
                "an untaken br_if leaves the value on the stack for the rest of the block",
                Expr::new().block(
                    BlockType::Value(ValType::I32),
                    Expr::new()
                        .i32_const(7)
                        .i32_const(0)
                        .br_if(0)
                        .i32_const(1)
                        .op(op::I32_ADD),
                ),
                8,
            ),
            // The 5 sits below the block's frame: the block cannot touch it, and neither
            // the taken nor the untaken br_if is allowed to disturb it.
            case_i32(
                "a taken br_if leaves the stack below the block alone",
                Expr::new()
                    .i32_const(5)
                    .block(
                        BlockType::Empty,
                        Expr::new().i32_const(1).br_if(0).i32_const(0).drop(),
                    )
                    .i32_const(2)
                    .op(op::I32_ADD),
                7,
            ),
            case_i32(
                "an untaken br_if leaves the stack below the block alone",
                Expr::new()
                    .i32_const(5)
                    .block(
                        BlockType::Empty,
                        Expr::new().i32_const(0).br_if(0).i32_const(0).drop(),
                    )
                    .i32_const(2)
                    .op(op::I32_ADD),
                7,
            ),
        ],
    )
});

/// `max3(a, b, c)`, written with two `if (result i32)`s and one scratch local.
fn max3_body() -> Expr {
    Expr::new()
        .local_get(0)
        .local_get(1)
        .op(op::I32_GT_S)
        .if_else(
            BlockType::Value(ValType::I32),
            Expr::new().local_get(0),
            Expr::new().local_get(1),
        )
        .local_tee(3)
        .local_get(2)
        .op(op::I32_GT_S)
        .if_else(
            BlockType::Value(ValType::I32),
            Expr::new().local_get(3),
            Expr::new().local_get(2),
        )
}

wasm_test!(max3, |ctx| {
    let inputs: &[([&str; 3], i32)] = &[
        (["1", "2", "3"], 3),
        (["3", "2", "1"], 3),
        (["2", "9", "4"], 9),
        (["7", "7", "7"], 7),
        (["-5", "-9", "-7"], -5),
        (["-2147483648", "0", "2147483647"], 2_147_483_647),
        (["-1", "-1", "-2"], -1),
    ];
    let cases = inputs
        .iter()
        .map(|(args, want)| {
            Case::new(
                format!("max3({}, {}, {}) is {want}", args[0], args[1], args[2]),
                ftype(&[ValType::I32, ValType::I32, ValType::I32], &[ValType::I32]),
                max3_body(),
                &[&want.to_string()],
            )
            .locals(&[(1, ValType::I32)])
            .args(args)
        })
        .collect();
    run_cases(ctx, "max3", cases)
});

/// The two-armed `if` that decides which of two constants comes back.
fn pick_one() -> Module {
    single_with_locals(
        "pick-one",
        ftype(&[ValType::I32], &[ValType::I32]),
        &[],
        Expr::new().local_get(0).if_else(
            BlockType::Value(ValType::I32),
            Expr::new().i32_const(10),
            Expr::new().i32_const(20),
        ),
    )
}

/// `max3` itself, as a module a learner can run.
fn max3_module() -> Module {
    single_with_locals(
        "max3",
        ftype(&[ValType::I32, ValType::I32, ValType::I32], &[ValType::I32]),
        &[(1, ValType::I32)],
        max3_body(),
    )
}

/// Worked examples: the shape of an `if`, and one doing a real job.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("An if that picks one of two numbers", pick_one)
            .summary(
                "`if (result i32)` on the function's only parameter, with 10 in the then arm \
                 and 20 in the else arm",
            )
            .command("run --invoke f mod.wasm 0")
            .output("20")
            .note(
                "Pass 1, -1 or 2 instead and the answer is 10: the condition is tested \
                 against zero, not against one. Both arms have to leave an i32 behind, \
                 which is why an `if (result i32)` with no `else` does not validate.",
            ),
        ExampleSpec::module("max3, out of two ifs", max3_module)
            .summary(
                "`max3(a, b, c)`: one `if (result i32)` for max(a, b), a `local.tee` to \
                 keep it, and a second for max of that and c",
            )
            .command("run --invoke f mod.wasm -5 -9 -7")
            .output("-5")
            .note(
                "`local.tee` stores and leaves the value on the stack, which saves the \
                 `local.get` that would otherwise follow every `local.set`.",
            ),
    ]
}
