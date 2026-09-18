//! Stage 21 — block, loop and br to every depth.
//!
//! The one thing every implementation gets wrong first is where a branch lands. `br` does
//! not mean "jump forward" and it does not mean "jump backward": it means "leave this
//! label". For a `block` the label sits at the `end`, so branching to it continues after
//! the block. For a `loop` the label sits at the `loop` instruction itself, so branching to
//! it starts the body again. Two tests here are the same program twice, once with each, and
//! the only difference is the opcode of the enclosing construct.
//!
//! A `loop` that counts needs a conditional branch, so `br_if` appears here a stage before
//! it is tested properly. Stage 22 is where the condition itself is put under a microscope;
//! here it is only ever `i32.lt_s` against a bound, and the thing on trial is the label.

use crate::examples::ExampleSpec;
use crate::stages::{case_i32, run_cases, single_with_locals, Case, Stage, Test};
use crate::wasm::{ftype, op, BlockType, Expr, Module, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 21,
        slug: "blocks_and_loops",
        name: "block, loop and br to every depth",
        ext: false,
        hints: &[
            "`br l` counts labels outward from the branch: 0 is the innermost enclosing block, loop or if, and the function body itself is the outermost label",
            "Branching to a `block` continues after its `end`; branching to a `loop` jumps back to the instruction after the `loop`. That one difference is the whole of iteration",
            "A `br` takes the label's arity off the top of the stack, throws the rest of the frame away, and makes everything after it in the same block unreachable",
            "Falling off the end of a block or a loop is not a branch: control simply carries on, which is why a `loop` nobody branches back to runs exactly once",
        ],
        examples,
        tests: vec![
            Test::new("a br to a block continues after its end", br_to_a_block),
            Test::new("a br to a loop starts the body again", br_to_a_loop),
            Test::new("br 0 through br 5 each land at their own depth", every_depth),
            Test::new("a br out of a typed block carries a value with it", br_with_a_value),
            Test::new("a loop counts to a bound and stops", a_loop_that_counts),
            Test::new("a br out of the block enclosing a loop ends the loop", out_of_the_enclosing_block),
            Test::new("a br as the last instruction of a function returns from it", br_at_the_end),
            Test::new("a block entered and left with no branch changes nothing", no_branch_at_all),
        ],
    }
}

/// One `i32` local, which most of the cases here use as an accumulator.
const ACC: &[(u32, ValType)] = &[(1, ValType::I32)];

/// A `() -> i32` case with a single `i32` local.
fn acc_case(name: impl Into<String>, body: Expr, want: i32) -> Case {
    case_i32(name, body, want).locals(ACC)
}

wasm_test!(br_to_a_block, |ctx| {
    run_cases(
        ctx,
        "br-to-a-block",
        vec![
            acc_case(
                "br 0 skips the rest of the block",
                Expr::new()
                    .block(
                        BlockType::Empty,
                        Expr::new()
                            .i32_const(1)
                            .local_set(0)
                            .br(0)
                            .i32_const(2)
                            .local_set(0),
                    )
                    .local_get(0),
                1,
            ),
            acc_case(
                "a br 0 at the top of a block skips all of it",
                Expr::new()
                    .block(
                        BlockType::Empty,
                        Expr::new().br(0).i32_const(5).local_set(0),
                    )
                    .local_get(0),
                0,
            ),
            acc_case(
                "what follows the block still runs",
                Expr::new()
                    .block(BlockType::Empty, Expr::new().br(0))
                    .i32_const(7)
                    .local_set(0)
                    .local_get(0),
                7,
            ),
            // The load-bearing case: the body increments a counter and ends in `br 0`. In a
            // block that is a jump to the end and the counter reaches 1; in a loop the same
            // bytes never terminate.
            acc_case(
                "a br at the end of a block does not re-enter it",
                Expr::new()
                    .block(
                        BlockType::Empty,
                        Expr::new()
                            .local_get(0)
                            .i32_const(1)
                            .op(op::I32_ADD)
                            .local_set(0)
                            .br(0),
                    )
                    .local_get(0),
                1,
            ),
        ],
    )
});

wasm_test!(br_to_a_loop, |ctx| {
    run_cases(
        ctx,
        "br-to-a-loop",
        vec![
            // The same shape as the last case of the previous test, with `loop` in place of
            // `block`: now the br goes back to the top and the bound is what stops it.
            acc_case(
                "a br at the end of a loop body starts the body again",
                Expr::new()
                    .loop_(
                        BlockType::Empty,
                        Expr::new()
                            .local_get(0)
                            .i32_const(1)
                            .op(op::I32_ADD)
                            .local_tee(0)
                            .i32_const(3)
                            .op(op::I32_LT_S)
                            .br_if(0),
                    )
                    .local_get(0),
                3,
            ),
            acc_case(
                "a loop whose body never branches runs once",
                Expr::new()
                    .loop_(
                        BlockType::Empty,
                        Expr::new()
                            .local_get(0)
                            .i32_const(1)
                            .op(op::I32_ADD)
                            .local_set(0),
                    )
                    .local_get(0),
                1,
            ),
            acc_case(
                "what follows the loop runs once the loop falls out of its end",
                Expr::new()
                    .loop_(BlockType::Empty, Expr::new().nop())
                    .i32_const(7)
                    .local_set(0)
                    .local_get(0),
                7,
            ),
            // A br 1 from inside a block nested in the loop still names the loop, so it goes
            // back to the top rather than out of anything.
            acc_case(
                "a br 1 from a block inside the loop still goes back to the loop's start",
                Expr::new()
                    .loop_(
                        BlockType::Empty,
                        Expr::new()
                            .local_get(0)
                            .i32_const(1)
                            .op(op::I32_ADD)
                            .local_set(0)
                            .block(
                                BlockType::Empty,
                                Expr::new()
                                    .local_get(0)
                                    .i32_const(4)
                                    .op(op::I32_LT_S)
                                    .br_if(1),
                            ),
                    )
                    .local_get(0),
                4,
            ),
        ],
    )
});

/// Six nested blocks around a single `br target`.
///
/// The landing site after the `end` of the `d`-th block stores `10 + d` and branches clear
/// of everything, so each depth produces its own number: `br 0` gives 10, `br 5` gives the
/// 15 the local was seeded with because the outermost label is the one at the very end.
fn six_deep(target: u32) -> Expr {
    let mut nest = Expr::new().br(target);
    for depth in 0..6u32 {
        nest = Expr::new().block(BlockType::Empty, nest);
        if depth < 5 {
            nest = nest.i32_const(10 + depth as i32).local_set(0).br(4 - depth);
        }
    }
    Expr::new()
        .i32_const(15)
        .local_set(0)
        .then(nest)
        .local_get(0)
}

wasm_test!(every_depth, |ctx| {
    let cases = (0..6u32)
        .map(|target| {
            acc_case(
                format!("br {target} lands at the end of the block {target} levels out"),
                six_deep(target),
                10 + target as i32,
            )
        })
        .collect();
    run_cases(ctx, "every-depth", cases)
});

wasm_test!(br_with_a_value, |ctx| {
    run_cases(
        ctx,
        "br-with-a-value",
        vec![
            case_i32(
                "a br out of a block (result i32) yields the value it carried",
                Expr::new().block(
                    BlockType::Value(ValType::I32),
                    Expr::new().i32_const(7).br(0),
                ),
                7,
            ),
            case_i32(
                "the carried value wins over the one that would have fallen out",
                Expr::new().block(
                    BlockType::Value(ValType::I32),
                    Expr::new()
                        .block(
                            BlockType::Value(ValType::I32),
                            Expr::new().i32_const(7).br(1),
                        )
                        .drop()
                        .i32_const(9),
                ),
                7,
            ),
            // A br takes the label's arity off the top and discards the rest of the frame,
            // so the 1 underneath never reaches the block's end.
            case_i32(
                "a br carries the top of the stack and drops what is below it",
                Expr::new().block(
                    BlockType::Value(ValType::I32),
                    Expr::new().i32_const(1).i32_const(2).br(0),
                ),
                2,
            ),
            case_i32(
                "a br out of an untyped block carries nothing",
                Expr::new()
                    .block(BlockType::Empty, Expr::new().br(0))
                    .i32_const(3),
                3,
            ),
        ],
    )
});

/// `local 0` counted up one at a time until it reaches `bound`.
fn count_to(bound: i32) -> Expr {
    Expr::new()
        .loop_(
            BlockType::Empty,
            Expr::new()
                .local_get(0)
                .i32_const(1)
                .op(op::I32_ADD)
                .local_tee(0)
                .i32_const(bound)
                .op(op::I32_LT_S)
                .br_if(0),
        )
        .local_get(0)
}

wasm_test!(a_loop_that_counts, |ctx| {
    run_cases(
        ctx,
        "a-loop-that-counts",
        vec![
            acc_case("a loop counting to 10 stops at 10", count_to(10), 10),
            acc_case("a loop counting to 1 stops at 1", count_to(1), 1),
            // The test is at the bottom, so the body always runs at least once: a bound of
            // zero is a do-while, not a no-op, and a runtime that checks first gets 0.
            acc_case(
                "a loop whose test is at the bottom runs its body once even at a bound of 0",
                count_to(0),
                1,
            ),
            acc_case(
                "a loop of a hundred thousand iterations still terminates",
                count_to(100_000),
                100_000,
            ),
            // Two locals: the counter and the running sum.
            Case::new(
                "a loop summing 1 to 10 ends at 55",
                ftype(&[], &[ValType::I32]),
                Expr::new()
                    .loop_(
                        BlockType::Empty,
                        Expr::new()
                            .local_get(0)
                            .i32_const(1)
                            .op(op::I32_ADD)
                            .local_tee(0)
                            .local_get(1)
                            .op(op::I32_ADD)
                            .local_set(1)
                            .local_get(0)
                            .i32_const(10)
                            .op(op::I32_LT_S)
                            .br_if(0),
                    )
                    .local_get(1),
                &["55"],
            )
            .locals(&[(2, ValType::I32)]),
        ],
    )
});

wasm_test!(out_of_the_enclosing_block, |ctx| {
    // block { loop { count; if we are done, br 1 out of the block; br 0 round again } }
    let escaping_loop = Expr::new().block(
        BlockType::Empty,
        Expr::new().loop_(
            BlockType::Empty,
            Expr::new()
                .local_get(0)
                .i32_const(1)
                .op(op::I32_ADD)
                .local_tee(0)
                .i32_const(5)
                .op(op::I32_EQ)
                .br_if(1)
                .br(0),
        ),
    );
    run_cases(
        ctx,
        "out-of-the-enclosing-block",
        vec![
            acc_case(
                "a br 1 from a loop body leaves the block around it",
                escaping_loop.clone().local_get(0),
                5,
            ),
            // The same loop with a store between the loop's `end` and the block's `end`.
            // The br 1 lands past both, so the store never happens.
            acc_case(
                "the br 1 lands after the block, not after the loop",
                Expr::new()
                    .block(
                        BlockType::Empty,
                        Expr::new()
                            .loop_(
                                BlockType::Empty,
                                Expr::new()
                                    .local_get(0)
                                    .i32_const(1)
                                    .op(op::I32_ADD)
                                    .local_tee(0)
                                    .i32_const(5)
                                    .op(op::I32_EQ)
                                    .br_if(1)
                                    .br(0),
                            )
                            .i32_const(99)
                            .local_set(0),
                    )
                    .local_get(0),
                5,
            ),
            acc_case(
                "what follows the enclosing block runs after the escape",
                escaping_loop
                    .local_get(0)
                    .i32_const(1)
                    .op(op::I32_ADD)
                    .local_set(0)
                    .local_get(0),
                6,
            ),
        ],
    )
});

wasm_test!(br_at_the_end, |ctx| {
    run_cases(
        ctx,
        "br-at-the-end",
        vec![
            // The function body is itself a labelled block, and it is the outermost one, so
            // `br 0` at the top level of a function is a return.
            case_i32(
                "a br 0 at the top level of a function returns its result",
                Expr::new().i32_const(7).br(0),
                7,
            ),
            case_i32(
                "the instructions after that br never run",
                Expr::new().i32_const(7).br(0).i32_const(9),
                7,
            ),
            case_i32(
                "a br 1 from inside a block is a return too",
                Expr::new()
                    .block(
                        BlockType::Value(ValType::I32),
                        Expr::new().i32_const(7).br(1),
                    )
                    .i32_const(1)
                    .op(op::I32_ADD),
                7,
            ),
            Case::new(
                "a br 0 at the end of a function that returns nothing prints nothing",
                ftype(&[], &[]),
                Expr::new().br(0),
                &[],
            ),
        ],
    )
});

wasm_test!(no_branch_at_all, |ctx| {
    run_cases(
        ctx,
        "no-branch-at-all",
        vec![
            case_i32(
                "an empty block changes nothing",
                Expr::new()
                    .block(BlockType::Empty, Expr::new())
                    .i32_const(7),
                7,
            ),
            case_i32(
                "a block's body runs in order and falls out of its end",
                Expr::new().block(
                    BlockType::Value(ValType::I32),
                    Expr::new().i32_const(3).i32_const(4).op(op::I32_ADD),
                ),
                7,
            ),
            case_i32(
                "three blocks one inside another run straight through",
                Expr::new()
                    .block(
                        BlockType::Empty,
                        Expr::new().block(
                            BlockType::Empty,
                            Expr::new().block(BlockType::Empty, Expr::new().nop()),
                        ),
                    )
                    .i32_const(7),
                7,
            ),
            acc_case(
                "a loop nobody branches back to runs its body exactly once",
                Expr::new()
                    .loop_(
                        BlockType::Empty,
                        Expr::new()
                            .local_get(0)
                            .i32_const(1)
                            .op(op::I32_ADD)
                            .local_set(0),
                    )
                    .loop_(
                        BlockType::Empty,
                        Expr::new()
                            .local_get(0)
                            .i32_const(1)
                            .op(op::I32_ADD)
                            .local_set(0),
                    )
                    .local_get(0),
                2,
            ),
        ],
    )
});

/// A block a `br` leaves before its second store can run.
fn leaving_a_block() -> Module {
    single_with_locals(
        "br-out-of-block",
        ftype(&[], &[ValType::I32]),
        ACC,
        Expr::new()
            .block(
                BlockType::Empty,
                Expr::new()
                    .i32_const(1)
                    .local_set(0)
                    .br(0)
                    .i32_const(2)
                    .local_set(0),
            )
            .local_get(0),
    )
}

/// The same `br`, in a loop, counting to ten.
fn counting_loop() -> Module {
    single_with_locals(
        "count-to-ten",
        ftype(&[], &[ValType::I32]),
        ACC,
        count_to(10),
    )
}

/// Worked examples: the same branch against each of the two constructs.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("A br that leaves a block", leaving_a_block)
            .summary(
                "a block that stores 1, branches to its own label, and would have stored 2 \
                 had it carried on",
            )
            .command("run --invoke f mod.wasm")
            .output("1")
            .note(
                "A block's label is at its `end`, so `br 0` continues after the block. The \
                 two instructions between the br and the end are unreachable — they still \
                 have to validate, and they never run.",
            ),
        ExampleSpec::module("The same br, in a loop", counting_loop)
            .summary(
                "a loop that adds one to a local and branches back to its own label while \
                 the local is still below ten",
            )
            .command("run --invoke f mod.wasm")
            .output("10")
            .note(
                "A loop's label is at the `loop` instruction, so the identical `br` goes \
                 backwards here. The test is at the bottom of the body, which makes this a \
                 do-while: the body always runs at least once.",
            ),
    ]
}
