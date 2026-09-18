//! Stage 27 — Multi-value blocks: results and parameters. **[ext]**
//!
//! Before multi-value, a block type was one byte: `0x40` for nothing, or a value type for
//! one result. Multi-value adds a third form — a **signed** LEB128 index into the type
//! section — and that one change gives blocks everything a function has: several results,
//! and parameters. The sign is what keeps the forms apart: type index 0 encodes as the
//! single byte `0x00`, which is not any of `0x40`, `0x7f`..`0x7c`, `0x70` or `0x6f`.
//!
//! Parameters are the half people forget. A `block (param i32 i32)` pops two values off the
//! enclosing stack when it is entered and pushes them into its own frame; everything below
//! them stays out of reach until the block ends, exactly as before.
//!
//! The loop test is where it earns its keep. A `loop`'s label arity is its **parameter**
//! count, not its result count, so `br 0` back to a loop carries the next iteration's
//! parameters — which is how an accumulator goes round a loop without ever touching a local.

use crate::examples::ExampleSpec;
use crate::stages::{expect_lines, run_cases_with, single, Case, Stage, Test};
use crate::wasm::{ftype, op, BlockType, Expr, Func, Module, ModuleBuilder, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 27,
        slug: "multi_value",
        name: "Multi-value blocks: results and parameters",
        ext: true,
        hints: &[
            "A block type that is neither 0x40 nor a value type is a **signed** LEB128 index into the type section, which gives the block both parameters and several results",
            "A block's parameters are popped off the enclosing stack when it is entered; the values below them are untouchable until the block's `end`",
            "A `loop`'s label arity is its **parameter** count, not its result count: a `br` back to a loop carries the next iteration's parameters",
            "A function with several results returns them in order, and a runtime prints one per line",
        ],
        examples,
        tests: vec![
            Test::new("a function returning two values prints one per line", two_results).ext(),
            Test::new("a function returning three values prints them in order", three_results).ext(),
            Test::new("a block with a multi-value result type leaves every value behind", block_results).ext(),
            Test::new("a block with parameters takes them off the enclosing stack", block_params).ext(),
            Test::new("a loop with parameters carries an accumulator round", loop_params).ext(),
            Test::new("a br carries every value the label's type asks for", br_with_several_values).ext(),
            Test::new("both arms of an if produce two values, and a call consumes them", if_and_a_call).ext(),
        ],
    }
}

wasm_test!(two_results, |ctx| {
    let pair = single(
        "two-results",
        ftype(&[], &[ValType::I32, ValType::I32]),
        Expr::new().i32_const(7).i32_const(-1),
    );
    expect_lines(ctx, &pair, "f", &[], &["7", "-1"])?;

    // The same two numbers pushed the other way round: the results come back in the order
    // the function's type declares, deepest first.
    let swapped = single(
        "two-results-swapped",
        ftype(&[], &[ValType::I32, ValType::I32]),
        Expr::new().i32_const(-1).i32_const(7),
    );
    expect_lines(ctx, &swapped, "f", &[], &["-1", "7"])?;

    // Two results of different types, so a runtime that prints them from one buffer has to
    // know which is which.
    let mixed = single(
        "two-results-mixed",
        ftype(&[], &[ValType::I64, ValType::F64]),
        Expr::new().i64_const(-9_000_000_000).f64_const(1.5),
    );
    expect_lines(ctx, &mixed, "f", &[], &["-9000000000", "1.5"])?;
    Ok(())
});

wasm_test!(three_results, |ctx| {
    let three = single(
        "three-results",
        ftype(&[], &[ValType::I32, ValType::I64, ValType::F64]),
        Expr::new().i32_const(1).i64_const(2).f64_const(3.5),
    );
    expect_lines(ctx, &three, "f", &[], &["1", "2", "3.5"])?;

    // Three of the same type, computed rather than named, so the order is not an accident
    // of the constants.
    let computed = single(
        "three-results-computed",
        ftype(&[ValType::I32], &[ValType::I32, ValType::I32, ValType::I32]),
        Expr::new()
            .local_get(0)
            .local_get(0)
            .i32_const(1)
            .op(op::I32_ADD)
            .local_get(0)
            .i32_const(2)
            .op(op::I32_MUL),
    );
    expect_lines(ctx, &computed, "f", &["5"], &["5", "6", "10"])?;
    Ok(())
});

wasm_test!(block_results, |ctx| {
    let mut b = ModuleBuilder::new("block-results");
    // () -> (i32 i32) and () -> (i32 i32 i32), used as block types rather than as the
    // signature of any function.
    let pair = b.add_type(ftype(&[], &[ValType::I32, ValType::I32]));
    let triple = b.add_type(ftype(&[], &[ValType::I32, ValType::I32, ValType::I32]));
    run_cases_with(
        ctx,
        b,
        vec![
            Case::new(
                "a block of type () -> (i32 i32) leaves both values",
                ftype(&[], &[ValType::I32]),
                Expr::new()
                    .block(BlockType::Type(pair), Expr::new().i32_const(3).i32_const(4))
                    .op(op::I32_ADD),
                &["7"],
            ),
            // Subtraction, so the order of the two results is visible in the answer.
            Case::new(
                "the two values come out in the order the block pushed them",
                ftype(&[], &[ValType::I32]),
                Expr::new()
                    .block(BlockType::Type(pair), Expr::new().i32_const(3).i32_const(4))
                    .op(op::I32_SUB),
                &["-1"],
            ),
            Case::new(
                "a block with three results",
                ftype(&[], &[ValType::I32]),
                Expr::new()
                    .block(
                        BlockType::Type(triple),
                        Expr::new().i32_const(1).i32_const(2).i32_const(4),
                    )
                    .op(op::I32_ADD)
                    .op(op::I32_ADD),
                &["7"],
            ),
            // The block's results become the function's results without an instruction in
            // between: one block, two lines of output.
            Case::new(
                "a block's two results become the function's two results",
                ftype(&[], &[ValType::I32, ValType::I32]),
                Expr::new().block(
                    BlockType::Type(pair),
                    Expr::new().i32_const(7).i32_const(-1),
                ),
                &["7", "-1"],
            ),
        ],
    )
});

wasm_test!(block_params, |ctx| {
    let mut b = ModuleBuilder::new("block-params");
    // (i32 i32) -> (i32): two parameters taken off the enclosing stack, one result left.
    let two_to_one = b.add_type(ftype(&[ValType::I32, ValType::I32], &[ValType::I32]));
    // (i32) -> (i32 i32): one in, two out.
    let one_to_two = b.add_type(ftype(&[ValType::I32], &[ValType::I32, ValType::I32]));
    run_cases_with(
        ctx,
        b,
        vec![
            Case::new(
                "a block with two parameters consumes them from the enclosing stack",
                ftype(&[], &[ValType::I32]),
                Expr::new()
                    .i32_const(10)
                    .i32_const(3)
                    .block(BlockType::Type(two_to_one), Expr::new().op(op::I32_SUB)),
                &["7"],
            ),
            // The 100 is below the block's parameters. The block cannot see it, and it is
            // still there when the block ends.
            Case::new(
                "only the parameters are taken, and what is under them survives",
                ftype(&[], &[ValType::I32]),
                Expr::new()
                    .i32_const(100)
                    .i32_const(10)
                    .i32_const(3)
                    .block(BlockType::Type(two_to_one), Expr::new().op(op::I32_SUB))
                    .op(op::I32_ADD),
                &["107"],
            ),
            Case::new(
                "a block that takes one value and leaves two",
                ftype(&[], &[ValType::I32]),
                Expr::new()
                    .i32_const(5)
                    .block(
                        BlockType::Type(one_to_two),
                        Expr::new().i32_const(1).op(op::I32_ADD).i32_const(2),
                    )
                    .op(op::I32_ADD),
                &["8"],
            ),
            // A block with parameters inside a block with parameters: the inner one takes
            // the outer one's second parameter and hands back a single value.
            Case::new(
                "a block with parameters nested inside another",
                ftype(&[], &[ValType::I32]),
                Expr::new().i32_const(20).i32_const(30).block(
                    BlockType::Type(two_to_one),
                    Expr::new()
                        .i32_const(5)
                        .block(BlockType::Type(two_to_one), Expr::new().op(op::I32_SUB))
                        .op(op::I32_ADD),
                ),
                &["45"],
            ),
        ],
    )
});

wasm_test!(loop_params, |ctx| {
    let mut b = ModuleBuilder::new("loop-params");
    // (i32) -> (i32): the loop carries one accumulator round.
    let carry_one = b.add_type(ftype(&[ValType::I32], &[ValType::I32]));
    // (i32 i32) -> (i32 i32): two values round, for the iterative Fibonacci below.
    let carry_two = b.add_type(ftype(
        &[ValType::I32, ValType::I32],
        &[ValType::I32, ValType::I32],
    ));
    let one_local = &[(1u32, ValType::I32)];

    // sum 1..=bound, with the running total living on the operand stack and nowhere else:
    // the `br_if 0` carries it back as the loop's parameter.
    let sum_to = |bound: i32| {
        Expr::new().i32_const(0).loop_(
            BlockType::Type(carry_one),
            Expr::new()
                .local_get(0)
                .i32_const(1)
                .op(op::I32_ADD)
                .local_tee(0)
                .op(op::I32_ADD)
                .local_get(0)
                .i32_const(bound)
                .op(op::I32_LT_S)
                .br_if(0),
        )
    };

    // (a, b) -> (b, a + b), `steps` times; a is then fib(steps).
    let fib = |steps: i32| {
        Expr::new()
            .i32_const(0)
            .i32_const(1)
            .loop_(
                BlockType::Type(carry_two),
                Expr::new()
                    // The loop's two parameters arrive on the stack as `a b`; park them in
                    // locals 2 and 1 and push `b (a + b)` for the next iteration to take.
                    .local_set(1)
                    .local_set(2)
                    .local_get(1)
                    .local_get(2)
                    .local_get(1)
                    .op(op::I32_ADD)
                    .local_get(0)
                    .i32_const(1)
                    .op(op::I32_ADD)
                    .local_tee(0)
                    .i32_const(steps)
                    .op(op::I32_LT_S)
                    .br_if(0),
            )
            .drop()
    };

    run_cases_with(
        ctx,
        b,
        vec![
            Case::new(
                "an accumulator carried round as a loop parameter sums 1 to 10",
                ftype(&[], &[ValType::I32]),
                sum_to(10),
                &["55"],
            )
            .locals(one_local),
            Case::new(
                "the same loop with a bound of 3",
                ftype(&[], &[ValType::I32]),
                sum_to(3),
                &["6"],
            )
            .locals(one_local),
            Case::new(
                "the same loop with a bound of 1 runs its body once",
                ftype(&[], &[ValType::I32]),
                sum_to(1),
                &["1"],
            )
            .locals(one_local),
            // Two values round the loop at once: the label's arity is 2 because the loop
            // has two *parameters*, even though it also has two results.
            Case::new(
                "a loop carrying two values round computes fib(10)",
                ftype(&[], &[ValType::I32]),
                fib(10),
                &["55"],
            )
            .locals(&[(3, ValType::I32)]),
            Case::new(
                "the same pair-carrying loop for fib(1)",
                ftype(&[], &[ValType::I32]),
                fib(1),
                &["1"],
            )
            .locals(&[(3, ValType::I32)]),
        ],
    )
});

wasm_test!(br_with_several_values, |ctx| {
    let mut b = ModuleBuilder::new("br-with-values");
    let pair = b.add_type(ftype(&[], &[ValType::I32, ValType::I32]));
    run_cases_with(
        ctx,
        b,
        vec![
            Case::new(
                "a br out of a block with two results carries both",
                ftype(&[], &[ValType::I32]),
                Expr::new()
                    .block(
                        BlockType::Type(pair),
                        Expr::new().i32_const(3).i32_const(4).br(0),
                    )
                    .op(op::I32_SUB),
                &["-1"],
            ),
            Case::new(
                "a br 1 carries both values out of two blocks",
                ftype(&[], &[ValType::I32]),
                Expr::new()
                    .block(
                        BlockType::Type(pair),
                        Expr::new()
                            .block(
                                BlockType::Type(pair),
                                Expr::new().i32_const(3).i32_const(4).br(1),
                            )
                            .drop()
                            .drop()
                            .i32_const(0)
                            .i32_const(0),
                    )
                    .op(op::I32_SUB),
                &["-1"],
            ),
            // The label's arity is two, so the 99 underneath goes with the rest of the
            // frame and never reaches the subtraction.
            Case::new(
                "a br takes the label's two values and drops what is below them",
                ftype(&[], &[ValType::I32]),
                Expr::new()
                    .block(
                        BlockType::Type(pair),
                        Expr::new().i32_const(99).i32_const(3).i32_const(4).br(0),
                    )
                    .op(op::I32_SUB),
                &["-1"],
            ),
            Case::new(
                "an untaken br_if leaves both values where they were",
                ftype(&[], &[ValType::I32]),
                Expr::new()
                    .block(
                        BlockType::Type(pair),
                        Expr::new()
                            .i32_const(3)
                            .i32_const(4)
                            .i32_const(0)
                            .br_if(0)
                            .drop()
                            .drop()
                            .i32_const(10)
                            .i32_const(1),
                    )
                    .op(op::I32_SUB),
                &["9"],
            ),
            // The carried pair becomes the function's own pair of results.
            Case::new(
                "a br carries two values all the way out as the function's results",
                ftype(&[], &[ValType::I32, ValType::I32]),
                Expr::new().block(
                    BlockType::Type(pair),
                    Expr::new().i32_const(7).i32_const(-1).br(0),
                ),
                &["7", "-1"],
            ),
        ],
    )
});

wasm_test!(if_and_a_call, |ctx| {
    let mut b = ModuleBuilder::new("if-and-a-call");
    let pair = b.add_type(ftype(&[], &[ValType::I32, ValType::I32]));
    let sub_ty = b.add_type(ftype(&[ValType::I32, ValType::I32], &[ValType::I32]));
    let sub = b.add_func(
        sub_ty,
        Func::new(Expr::new().local_get(0).local_get(1).op(op::I32_SUB)),
    );
    // A function whose two results feed straight into `sub`, with nothing in between.
    let make_pair = b.add_func(pair, Func::new(Expr::new().i32_const(10).i32_const(3)));
    let arms = |cond: i32| {
        Expr::new().i32_const(cond).if_else(
            BlockType::Type(pair),
            Expr::new().i32_const(10).i32_const(3),
            Expr::new().i32_const(4).i32_const(9),
        )
    };
    run_cases_with(
        ctx,
        b,
        vec![
            Case::new(
                "the then arm's two values reach the call",
                ftype(&[], &[ValType::I32]),
                arms(1).call(sub),
                &["7"],
            ),
            Case::new(
                "the else arm's two values reach the call",
                ftype(&[], &[ValType::I32]),
                arms(0).call(sub),
                &["-5"],
            ),
            Case::new(
                "the if's two values come back as the function's own results",
                ftype(&[], &[ValType::I32, ValType::I32]),
                arms(0),
                &["4", "9"],
            ),
            // One call's two results are the next call's two arguments, with no locals and
            // no reordering in between.
            Case::new(
                "a function's two results feed straight into another call",
                ftype(&[], &[ValType::I32]),
                Expr::new().call(make_pair).call(sub),
                &["7"],
            ),
            Case::new(
                "the same pair reaching the function's results untouched",
                ftype(&[], &[ValType::I32, ValType::I32]),
                Expr::new().call(make_pair),
                &["10", "3"],
            ),
        ],
    )
});

/// A function returning two values.
fn pair_module() -> Module {
    single(
        "pair",
        ftype(&[], &[ValType::I32, ValType::I32]),
        Expr::new().i32_const(7).i32_const(-1),
    )
}

/// The accumulator-carrying loop, on its own.
fn accumulating_loop() -> Module {
    let mut b = ModuleBuilder::new("loop-param");
    let carry = b.add_type(ftype(&[ValType::I32], &[ValType::I32]));
    let sig = b.add_type(ftype(&[], &[ValType::I32]));
    let f = b.add_func(
        sig,
        Func::with_locals(
            &[(1, ValType::I32)],
            Expr::new().i32_const(0).loop_(
                BlockType::Type(carry),
                Expr::new()
                    .local_get(0)
                    .i32_const(1)
                    .op(op::I32_ADD)
                    .local_tee(0)
                    .op(op::I32_ADD)
                    .local_get(0)
                    .i32_const(10)
                    .op(op::I32_LT_S)
                    .br_if(0),
            ),
        ),
    );
    b.export_func("f", f).build()
}

/// Worked examples: two results, and a loop parameter carrying an accumulator.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("A function with two results", pair_module)
            .summary("a `() -> (i32 i32)` whose body is `i32.const 7  i32.const -1`")
            .command("run --invoke f mod.wasm")
            .output("7\n-1")
            .note(
                "One line per returned value, deepest first. The only thing multi-value \
                 changes about a function type is the length of its result vector — which \
                 the binary format has always encoded as a vector.",
            ),
        ExampleSpec::module("An accumulator that lives on the stack", accumulating_loop)
            .summary(
                "`loop (param i32) (result i32)` summing 1 to 10, with the running total \
                 carried back by the `br_if` and never stored in a local",
            )
            .command("run --invoke f mod.wasm")
            .output("55")
            .note(
                "The loop's label arity is its **parameter** count, so the `br_if 0` takes \
                 the total off the top of the stack and hands it to the next iteration. \
                 Fall out of the bottom instead and the same value is the loop's result.",
            ),
    ]
}
