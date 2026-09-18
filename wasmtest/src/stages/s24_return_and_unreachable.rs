//! Stage 24 — return, unreachable, nop and drop.
//!
//! `return` is a branch to the outermost label, so everything stage 21 says about `br`
//! applies to it: it takes the function's results off the top of the stack and throws the
//! rest of the frame away, however many blocks deep it was.
//!
//! `unreachable` is the opposite of a hint. Reaching it is a trap, the program stops, and
//! the exported function produces no result at all — a trapping `--invoke` prints nothing
//! on stdout, whatever the function had computed by then. Not reaching it costs nothing:
//! `i32.const 7  return  unreachable` is a well-formed function that returns 7, because
//! unreachable code still has to *validate* but never has to *run*.
//!
//! Three of the trap cases here carry an `i32.const 0` that looks pointless and is not.
//! `end` makes the code after it reachable again whatever the body did, so a `() -> i32`
//! function whose only block is `block unreachable end` still owes the validator an i32 at
//! the end of the body. The first draft of those cases left it out and `wasmtime` refused
//! the module — "type mismatch: expected i32 but nothing on stack" — which is a validation
//! error, not the trap the test was after. The polymorphic stack ends at the `end`.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::stages::{
    case_f64, case_i32, case_i64, case_trap, case_void_trap, f_i32, run_cases, trap, Case, Stage,
    Test,
};
use crate::wasm::{ftype, op, BlockType, Expr, Module, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 24,
        slug: "return_and_unreachable",
        name: "return, unreachable, nop and drop",
        ext: false,
        hints: &[
            "`return` is a branch to the function's own label: it takes the result types off the top of the stack and discards whatever is underneath, from any depth",
            "`unreachable` traps with the canonical reason 'unreachable'. The process exits non-zero, stderr says so, and stdout stays empty because the call never produced a result",
            "Unreachable code still has to type-check, so `i32.const 7  return  unreachable` validates and returns 7 — refusing to decode what follows a `return` is wrong",
            "`drop` pops exactly one value whatever its type, and `nop` does nothing at all; neither may be folded away into a type error",
        ],
        examples,
        tests: vec![
            Test::new("return hands back what is on top of the stack", return_at_the_top),
            Test::new("return leaves the function from inside a block, a loop or an if", return_from_inside),
            Test::new("the instructions after a return never run", after_a_return),
            Test::new("unreachable traps and puts nothing on stdout", unreachable_traps),
            Test::new("an unreachable control never reaches is harmless", unreachable_not_reached),
            Test::new("any number of nops change nothing", nops),
            Test::new("drop removes exactly one value of any type", drops),
        ],
    }
}

wasm_test!(return_at_the_top, |ctx| {
    run_cases(
        ctx,
        "return-at-the-top",
        vec![
            case_i32(
                "a return at the end of an i32 function",
                Expr::new().i32_const(7).return_(),
                7,
            ),
            case_i64(
                "a return at the end of an i64 function",
                Expr::new().i64_const(-9_000_000_000).return_(),
                -9_000_000_000,
            ),
            case_f64(
                "a return at the end of an f64 function",
                Expr::new().f64_const(1.5).return_(),
                1.5,
            ),
            Case::new(
                "a return from a function with no results prints nothing",
                ftype(&[], &[]),
                Expr::new().return_(),
                &[],
            ),
            // The function's arity is one, so the 1 underneath goes with the frame.
            case_i32(
                "a return takes the result off the top and drops what is below it",
                Expr::new().i32_const(1).i32_const(2).return_(),
                2,
            ),
        ],
    )
});

wasm_test!(return_from_inside, |ctx| {
    run_cases(
        ctx,
        "return-from-inside",
        vec![
            case_i32(
                "a return from inside a block",
                Expr::new()
                    .block(BlockType::Empty, Expr::new().i32_const(7).return_())
                    .i32_const(9),
                7,
            ),
            // The loop would run for ever if the return did not leave the function on the
            // very first iteration.
            case_i32(
                "a return from inside a loop, on the first iteration",
                Expr::new()
                    .loop_(BlockType::Empty, Expr::new().i32_const(7).return_())
                    .i32_const(9),
                7,
            ),
            case_i32(
                "a return from inside an if",
                Expr::new()
                    .i32_const(1)
                    .if_(BlockType::Empty, Expr::new().i32_const(7).return_())
                    .i32_const(9),
                7,
            ),
            case_i32(
                "a return from inside an else",
                Expr::new()
                    .i32_const(0)
                    .if_else(
                        BlockType::Empty,
                        Expr::new().i32_const(8).return_(),
                        Expr::new().i32_const(7).return_(),
                    )
                    .i32_const(9),
                7,
            ),
            case_i32(
                "a return from four blocks deep",
                Expr::new()
                    .block(
                        BlockType::Empty,
                        Expr::new().block(
                            BlockType::Empty,
                            Expr::new().block(
                                BlockType::Empty,
                                Expr::new()
                                    .block(BlockType::Empty, Expr::new().i32_const(7).return_()),
                            ),
                        ),
                    )
                    .i32_const(9),
                7,
            ),
            // A return from inside a typed block ignores the block's own result type: it is
            // the function's arity that decides what comes back.
            case_i32(
                "a return from inside a block with a result type",
                Expr::new()
                    .block(
                        BlockType::Value(ValType::I32),
                        Expr::new().i32_const(7).return_().i32_const(0),
                    )
                    .i32_const(1)
                    .op(op::I32_ADD),
                7,
            ),
        ],
    )
});

wasm_test!(after_a_return, |ctx| {
    run_cases(
        ctx,
        "after-a-return",
        vec![
            case_i32(
                "a constant after a return does not replace the result",
                Expr::new().i32_const(7).return_().i32_const(9),
                7,
            ),
            // The strongest form of the same claim: a trap that is never reached is not a
            // trap. Both of these would abort the program if the return did not happen.
            case_i32(
                "an unreachable after a return does not trap",
                Expr::new().i32_const(7).return_().unreachable(),
                7,
            ),
            case_i32(
                "a division by zero after a return does not trap",
                Expr::new()
                    .i32_const(7)
                    .return_()
                    .i32_const(1)
                    .i32_const(0)
                    .op(op::I32_DIV_S),
                7,
            ),
            case_i32(
                "a br after a return is never taken",
                Expr::new()
                    .block(BlockType::Empty, Expr::new().i32_const(7).return_().br(0))
                    .i32_const(9),
                7,
            ),
        ],
    )
});

wasm_test!(unreachable_traps, |ctx| {
    run_cases(
        ctx,
        "unreachable-traps",
        vec![
            case_trap(
                "unreachable at the top of a function",
                Expr::new().unreachable(),
                &[trap::UNREACHABLE],
            ),
            case_trap(
                "unreachable after some work has been done",
                Expr::new()
                    .i32_const(1)
                    .i32_const(2)
                    .op(op::I32_ADD)
                    .unreachable(),
                &[trap::UNREACHABLE],
            ),
            // The `i32.const 0` after each of these is not decoration: an `end` makes the
            // code after it reachable again whatever the body did, so a `() -> i32`
            // function still has to leave an i32 behind even when its only block can
            // never finish. Leaving it out is a validation error, not a trap.
            case_trap(
                "unreachable inside a block",
                Expr::new()
                    .block(BlockType::Empty, Expr::new().unreachable())
                    .i32_const(0),
                &[trap::UNREACHABLE],
            ),
            case_trap(
                "unreachable inside a loop",
                Expr::new()
                    .loop_(BlockType::Empty, Expr::new().unreachable())
                    .i32_const(0),
                &[trap::UNREACHABLE],
            ),
            case_trap(
                "unreachable inside the arm of an if that runs",
                Expr::new()
                    .i32_const(1)
                    .if_(BlockType::Empty, Expr::new().unreachable())
                    .i32_const(0),
                &[trap::UNREACHABLE],
            ),
            case_void_trap(
                "unreachable in a function that returns nothing",
                Expr::new().unreachable(),
                &[trap::UNREACHABLE],
            ),
        ],
    )?;

    // A trap aborts: the call never produced a result, so there is nothing for the runtime
    // to print. A runtime that reports the values it had computed so far fails here.
    let m = f_i32(
        "unreachable-prints-nothing",
        Expr::new()
            .i32_const(7)
            .i32_const(8)
            .op(op::I32_ADD)
            .unreachable(),
    );
    let run = ctx.invoke(&m, "f", &[])?;
    let mut c = Check::new("what a trapping invocation leaves on stdout", &run);
    c.module(&m);
    c.that(
        "exit",
        "a non-zero exit status, because a trap aborts the program",
        run.exit.failed(),
        run.exit.label(),
    );
    c.eq("stdout", "", run.stdout.as_str());
    c.that(
        "stderr",
        "the canonical reason 'unreachable' somewhere on stderr",
        run.stderr_has(trap::UNREACHABLE),
        crate::stages::first_meaningful_line(&run.stderr),
    );
    c.finish()
});

wasm_test!(unreachable_not_reached, |ctx| {
    run_cases(
        ctx,
        "unreachable-not-reached",
        vec![
            case_i32(
                "an unreachable in the arm of an if that does not run",
                Expr::new().i32_const(0).if_else(
                    BlockType::Value(ValType::I32),
                    Expr::new().unreachable(),
                    Expr::new().i32_const(7),
                ),
                7,
            ),
            case_i32(
                "an unreachable after a br out of the block holding it",
                Expr::new()
                    .block(BlockType::Empty, Expr::new().br(0).unreachable())
                    .i32_const(7),
                7,
            ),
            case_i32(
                "an unreachable past the end of a br_table's chosen label",
                Expr::new()
                    .block(
                        BlockType::Empty,
                        Expr::new().i32_const(0).br_table(&[0], 0).unreachable(),
                    )
                    .i32_const(7),
                7,
            ),
            // The whole tail of the function is unreachable. It has to decode and validate;
            // none of it runs.
            case_i32(
                "a tail of unreachable instructions after the return",
                Expr::new()
                    .i32_const(7)
                    .return_()
                    .unreachable()
                    .unreachable()
                    .i32_const(1)
                    .drop()
                    .unreachable(),
                7,
            ),
        ],
    )
});

wasm_test!(nops, |ctx| {
    run_cases(
        ctx,
        "nops",
        vec![
            case_i32("a nop before the answer", Expr::new().nop().i32_const(7), 7),
            case_i32(
                "nops between the operands of an add",
                Expr::new()
                    .i32_const(3)
                    .nop()
                    .i32_const(4)
                    .nop()
                    .op(op::I32_ADD)
                    .nop(),
                7,
            ),
            case_i32(
                "a thousand nops change nothing",
                Expr::new()
                    .repeat(1000, &Expr::new().nop())
                    .i32_const(7)
                    .repeat(1000, &Expr::new().nop()),
                7,
            ),
            case_i32(
                "nops inside a block and a loop",
                Expr::new()
                    .block(BlockType::Empty, Expr::new().nop().nop())
                    .loop_(BlockType::Empty, Expr::new().nop())
                    .i32_const(7),
                7,
            ),
            Case::new(
                "a function whose body is nothing but nops",
                ftype(&[], &[]),
                Expr::new().nop().nop().nop(),
                &[],
            ),
        ],
    )
});

wasm_test!(drops, |ctx| {
    run_cases(
        ctx,
        "drops",
        vec![
            case_i32(
                "drop removes one i32 and only one",
                Expr::new().i32_const(7).i32_const(9).drop(),
                7,
            ),
            case_i64(
                "drop removes one i64",
                Expr::new().i64_const(7).i64_const(9).drop(),
                7,
            ),
            case_f64(
                "drop removes one f64",
                Expr::new().f64_const(1.5).f64_const(2.5).drop(),
                1.5,
            ),
            // The type of the value being dropped is whatever happens to be on top: an i64
            // sitting above an i32 is popped by the same opcode.
            case_i32(
                "drop pops whatever type is on top",
                Expr::new().i32_const(7).i64_const(9).drop(),
                7,
            ),
            case_i32(
                "two drops remove two values",
                Expr::new()
                    .i32_const(7)
                    .f32_const(1.0)
                    .i64_const(2)
                    .drop()
                    .drop(),
                7,
            ),
            case_i32(
                "a drop inside a block",
                Expr::new()
                    .i32_const(7)
                    .block(BlockType::Empty, Expr::new().i32_const(1).drop())
                    .nop(),
                7,
            ),
            // All four instructions of this stage in one function: the drop trims the
            // stack, the nops do nothing, the return picks the top value and the
            // unreachable is never reached.
            case_i32(
                "nop, drop, return and unreachable in one function",
                Expr::new()
                    .i32_const(1)
                    .nop()
                    .i32_const(2)
                    .drop()
                    .nop()
                    .i32_const(7)
                    .return_()
                    .unreachable(),
                7,
            ),
        ],
    )
});

/// A function that returns before it can reach the `unreachable` at its end.
fn return_before_the_trap() -> Module {
    f_i32(
        "return-before-the-trap",
        Expr::new().i32_const(7).return_().unreachable(),
    )
}

/// The same function with the `return` taken out.
fn straight_to_the_trap() -> Module {
    f_i32(
        "straight-to-the-trap",
        Expr::new().i32_const(7).drop().unreachable(),
    )
}

/// Worked examples: the same `unreachable`, once skipped and once reached.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module(
            "An unreachable that is never reached",
            return_before_the_trap,
        )
        .summary("`i32.const 7  return  unreachable` — a well-formed function body")
        .command("run --invoke f mod.wasm")
        .output("7")
        .note(
            "Everything after the `return` is unreachable code. It still has to decode \
                 and type-check — a decoder that stops at the `return` will mis-read the \
                 next function's body — but it never runs, so the trap never happens.",
        ),
        ExampleSpec::module("The same unreachable, reached", straight_to_the_trap)
            .summary("the same function with the `return` replaced by a `drop`")
            .command("run --invoke f mod.wasm")
            .output(
                "nothing on stdout, `wasm trap: wasm \\`unreachable\\` instruction executed` \
                 on stderr, and a non-zero exit status",
            )
            .note(
                "A trap aborts the program. The call produced no result, so stdout stays \
                 empty; the canonical reason goes to stderr and the exit status is non-zero \
                 (the number itself is the runtime's business).",
            ),
    ]
}
