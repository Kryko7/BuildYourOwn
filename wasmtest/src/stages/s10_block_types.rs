//! Stage 10 — Block, loop and if result types.
//!
//! A `block`, a `loop` and an `if` each carry a block type, and a block type is a whole
//! function type in miniature: what the construct takes off the stack when it starts and
//! what it must leave when it ends. The single-byte forms cover the two common cases —
//! `0x40` for no result, a value type byte for exactly one — and anything richer is a
//! *signed* LEB128 naming a type in the type section.
//!
//! Two of these are easy to get wrong in opposite directions. An `if` with a result type and
//! no `else` is a validation error, not an if-with-an-empty-else: the path where the
//! condition is false produces nothing and the block promised an i32, so there is no way to
//! satisfy it. And a `loop`'s label is its **parameters**, not its results — `br` to a loop
//! jumps back to the top, so the values it carries are the ones the next iteration starts
//! with. That is why the back-edge test here needs a block type with parameters, which only
//! the type-index form can express.

use crate::examples::ExampleSpec;
use crate::stages::{expect, expect_rejected, single, single_with_locals, Stage, Test};
use crate::wasm::{ftype, op, BlockType, Expr, Func, Module, ModuleBuilder, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 10,
        slug: "block_types",
        name: "Block, loop and if result types",
        ext: false,
        hints: &[
            "Read a block type as a function type: 0x40 means `() -> ()`, a value type byte means `() -> (that type)`, and anything else is a signed LEB128 naming a type in the type section",
            "Push a frame at `block`, `loop` and `if` recording the stack height and the block's results, and at `end` check that exactly those results sit above that height",
            "An `if` with a result type and no `else` cannot validate: the false path produces nothing, so a missing `else` is an error rather than an empty one",
            "A `loop`'s label carries its parameter types, not its results — `br` to a loop is a jump back to the top, and the values it carries start the next iteration",
        ],
        examples,
        tests: vec![
            Test::new("a block with a result that leaves nothing is refused", block_leaves_nothing),
            Test::new("a block with a result that leaves the wrong type is refused", block_wrong_type),
            Test::new("a block with one result that leaves two values is refused", block_leaves_two),
            Test::new("the two arms of an if must produce the same type", if_arms_disagree),
            Test::new("an if with a result and no else at all is refused", if_without_else),
            Test::new("a loop's back edge must carry the loop's label types", loop_back_edge),
            Test::new("nested blocks with results give the right answer", nested_blocks),
        ],
    }
}

wasm_test!(block_leaves_nothing, |ctx| {
    let m = single(
        "block-leaves-nothing",
        ftype(&[], &[ValType::I32]),
        Expr::new().block(BlockType::Value(ValType::I32), Expr::new().nop()),
    );
    expect_rejected(
        ctx,
        &m,
        "the block promises an i32 at its end and its body pushes nothing",
    )?;
    Ok(())
});

wasm_test!(block_wrong_type, |ctx| {
    let m = single(
        "block-wrong-type",
        ftype(&[], &[ValType::I32]),
        Expr::new().block(BlockType::Value(ValType::I32), Expr::new().i64_const(1)),
    );
    expect_rejected(
        ctx,
        &m,
        "the block promises an i32 and leaves an i64; the result type is checked at the \
         block's end, not only at the function's",
    )?;
    Ok(())
});

wasm_test!(block_leaves_two, |ctx| {
    let m = single(
        "block-leaves-two",
        ftype(&[], &[ValType::I32]),
        Expr::new().block(
            BlockType::Value(ValType::I32),
            Expr::new().i32_const(1).i32_const(2),
        ),
    );
    expect_rejected(
        ctx,
        &m,
        "two values above the height the block started at, where one was promised: the frame \
         remembers the height, so a value left over inside a block is caught at its end",
    )?;
    Ok(())
});

wasm_test!(if_arms_disagree, |ctx| {
    // The then arm gives an i32, the else arm an i64. Both are checked against the *block
    // type*, so the two arms can never disagree with each other without one of them
    // disagreeing with it.
    let m = if_arms();
    expect_rejected(
        ctx,
        &m,
        "the then arm produces an i32 and the else arm an i64, and an `if (result i32)` has \
         to be satisfied by both paths",
    )?;

    // The same mistake with no declared result at all: one arm leaves a value behind.
    let empty = single(
        "if-arms-disagree-empty",
        ftype(&[], &[]),
        Expr::new()
            .i32_const(1)
            .if_else(BlockType::Empty, Expr::new().i32_const(1), Expr::new()),
    );
    expect_rejected(
        ctx,
        &empty,
        "an `if` with no result whose then arm leaves an i32 on the stack",
    )?;
    Ok(())
});

wasm_test!(if_without_else, |ctx| {
    let m = if_no_else();
    expect_rejected(
        ctx,
        &m,
        "an `if (result i32)` with no else: the false path runs no instructions at all and \
         so produces nothing, and nothing is not an i32",
    )?;
    Ok(())
});

wasm_test!(loop_back_edge, |ctx| {
    // A loop whose block type takes an i32 and gives an i32. `br 0` jumps back to the top of
    // the loop, so it has to carry an i32 for the next iteration; this one carries an i64.
    let m = loop_bad_back_edge();
    expect_rejected(
        ctx,
        &m,
        "br 0 targets the loop's own label, whose types are the loop's parameters — an i32 — \
         and the body offers an i64",
    )?;

    // The loop that falls off its own end without producing its result.
    let no_result = single(
        "loop-leaves-nothing",
        ftype(&[], &[ValType::I32]),
        Expr::new().loop_(BlockType::Value(ValType::I32), Expr::new().nop()),
    );
    expect_rejected(
        ctx,
        &no_result,
        "a `loop (result i32)` whose body pushes nothing: reaching a loop's `end` is falling \
         out of it, and the result type applies there as it does to a block",
    )?;

    // A loop that counts down and leaves the stack exactly as its block type says.
    let ok = countdown();
    expect(ctx, &ok, "99")?;
    Ok(())
});

wasm_test!(nested_blocks, |ctx| {
    let m = nested();
    expect(ctx, &m, "16")?;
    ctx.note("four nested blocks with results: (2 × 3) + wrap(10) = 16");
    Ok(())
});

// ---------------------------------------------------------------------------------------
// Modules
// ---------------------------------------------------------------------------------------

/// `if (result i32)` whose then arm gives an i32 and whose else arm gives an i64.
fn if_arms() -> Module {
    single(
        "if-arms-disagree",
        ftype(&[], &[ValType::I32]),
        Expr::new().i32_const(1).if_else(
            BlockType::Value(ValType::I32),
            Expr::new().i32_const(1),
            Expr::new().i64_const(2),
        ),
    )
}

/// `if (result i32)` with no `else` at all.
fn if_no_else() -> Module {
    single(
        "if-without-else",
        ftype(&[], &[ValType::I32]),
        Expr::new()
            .i32_const(1)
            .if_(BlockType::Value(ValType::I32), Expr::new().i32_const(5)),
    )
}

/// A `loop (i32) -> (i32)` whose `br 0` carries an i64 back to the top.
///
/// The block type has to be the type-index form: the single-byte forms cannot give a block
/// parameters, and without a parameter a loop's label carries nothing and no `br` to it can
/// be wrong.
fn loop_bad_back_edge() -> Module {
    let mut b = ModuleBuilder::new("loop-bad-back-edge");
    let f_ty = b.add_type(ftype(&[], &[ValType::I32]));
    let loop_ty = b.add_type(ftype(&[ValType::I32], &[ValType::I32]));
    let idx = b.add_func(
        f_ty,
        Func::new(Expr::new().i32_const(1).loop_(
            BlockType::Type(loop_ty),
            Expr::new().drop().i64_const(1).br(0),
        )),
    );
    b.export_func("f", idx).build()
}

/// A loop that counts a local down to zero and then leaves 99 on the stack.
fn countdown() -> Module {
    single_with_locals(
        "loop-countdown",
        ftype(&[], &[ValType::I32]),
        &[(1, ValType::I32)],
        Expr::new()
            .i32_const(5)
            .local_set(0)
            .loop_(
                BlockType::Empty,
                Expr::new()
                    .local_get(0)
                    .i32_const(1)
                    .op(op::I32_SUB)
                    .local_tee(0)
                    .br_if(0),
            )
            .i32_const(99),
    )
}

/// Four nested blocks with results: `(2 × 3) + i32.wrap(10)`.
fn nested() -> Module {
    single(
        "nested-blocks",
        ftype(&[], &[ValType::I32]),
        Expr::new().block(
            BlockType::Value(ValType::I32),
            Expr::new()
                .block(BlockType::Value(ValType::I32), Expr::new().i32_const(2))
                .block(BlockType::Value(ValType::I32), Expr::new().i32_const(3))
                .op(op::I32_MUL)
                .block(BlockType::Value(ValType::I64), Expr::new().i64_const(10))
                .op(op::I32_WRAP_I64)
                .op(op::I32_ADD),
        ),
    )
}

/// Worked examples: blocks that nest and carry values, and the `if` that cannot be saved.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("Blocks that hand values outwards", nested)
            .summary(
                "a `() -> i32` built from four nested blocks: two `block (result i32)`s \
                 multiplied together, plus a `block (result i64)` narrowed with \
                 `i32.wrap_i64`, all inside one more `block (result i32)`",
            )
            .command("run --invoke f mod.wasm")
            .output("16")
            .note(
                "A block's result is left on the operand stack of the block that encloses it, \
                 so nesting composes: (2 × 3) + 10. Record the stack height when you push the \
                 frame and compare against it at `end`.",
            ),
        ExampleSpec::module("An if that promises a value and has one path", if_no_else)
            .summary(
                "a `() -> i32` whose body is `i32.const 1`, `if (result i32) { i32.const 5 }`, \
                 `end` — a result type and no `else`",
            )
            .command("run --invoke f mod.wasm")
            .output(
                "nothing on stdout, a message on stderr, and a non-zero exit status — the \
                 module never runs",
            )
            .note(
                "It looks like it could default to nothing on the false path, and that is the \
                 mistake: a block type is a promise both paths have to keep. An `if` with no \
                 `else` is only valid when its block type produces no results.",
            ),
    ]
}
