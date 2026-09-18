//! Stage 12 — Validation runs before anything executes.
//!
//! The stages before this one asked whether a broken module is refused. This one asks
//! *when*. A module is validated as a whole, and only a module that validates in full is
//! instantiated and run — so a defect in a function nobody ever calls stops the module dead,
//! and the observable consequence is that **stdout stays empty**.
//!
//! Every module here is a WASI command with a start section pointing at a function that
//! writes `ran` to stdout through `fd_write`. If the module ran at all, the four bytes come
//! out; if validation refused it, they do not. There is nothing else to look at, which is
//! the point: "did it validate before it executed" is not a question about a message, it is
//! a question about whether a side effect happened. The modules use the start *section*
//! rather than an exported `_start` because `wasmtime` runs both when both are present, and
//! the output would then arrive twice.
//!
//! The data-segment test is deliberately not a validation failure and says so. A data
//! segment whose offset is past the end of the memory is perfectly well typed: the offset is
//! a constant expression of the right type, and whether it fits is only knowable once the
//! memory exists. So the module validates, and then **instantiation** fails —
//! `wasmtime` answers `wasm trap: out of bounds memory access` and exits 134, not 1. The
//! ordering promise is the same either way (the start function still has not run, and stdout
//! is still empty), but the phase is different, and a runtime that folds the two together
//! will report the wrong thing for one of them. That test therefore asserts the trap and the
//! empty stdout rather than going through `expect_start_rejected`, which would have passed
//! for the wrong reason.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::stages::wasi::{self, W};
use crate::stages::{expect_start_rejected, expect_stdout, trap, Ctx, Stage, Test};
use crate::wasm::{ftype, BlockType, Expr, Func, Module, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 12,
        slug: "validation_before_execution",
        name: "Validation runs before anything executes",
        ext: false,
        hints: &[
            "Validate the whole module — every function body, not only the ones that are reachable — before you instantiate it or run its start function",
            "A refused module must produce no side effects at all: nothing on stdout, no memory initialised, no start function called",
            "Instantiation is a separate phase after validation: an active data or element segment that does not fit is a trap at instantiation, and the start function still never runs",
            "Every label of a `br_table` has to carry the same result types, because one instruction has to be able to jump to any of them",
        ],
        examples,
        tests: vec![
            Test::new("a bad index in a function nobody calls stops the start function", bad_index_elsewhere),
            Test::new("a type error in a later function stops the start function", type_error_elsewhere),
            Test::new("an undefined opcode in a function nobody calls stops the start function", bad_opcode_elsewhere),
            Test::new("every label of a br_table must carry the same result types", br_table_arity),
            Test::new("a start function whose type is not () -> () is refused", bad_start_signature),
            Test::new("a data segment past the end of memory fails at instantiation", data_segment_out_of_range),
            Test::new("the same module with nothing wrong prints from its start function", control),
        ],
    }
}

wasm_test!(bad_index_elsewhere, |ctx| {
    let m = command("start-and-bad-index", Defect::UnknownFunction);
    silent(ctx, &m, "an unreachable function calls function 777")
});

wasm_test!(type_error_elsewhere, |ctx| {
    let m = command("start-and-type-error", Defect::TypeError);
    silent(
        ctx,
        &m,
        "a later function declares () -> i32 and leaves an i64",
    )
});

wasm_test!(bad_opcode_elsewhere, |ctx| {
    let m = command("start-and-bad-opcode", Defect::UndefinedOpcode);
    silent(
        ctx,
        &m,
        "a function nobody calls holds the byte 0x27, which is not an opcode",
    )
});

wasm_test!(br_table_arity, |ctx| {
    let m = command("start-and-br-table", Defect::BrTableArity);
    silent(
        ctx,
        &m,
        "a br_table whose two labels have different arities: label 0 carries nothing and \
         label 1 carries an i32, and one instruction cannot satisfy both",
    )
});

wasm_test!(bad_start_signature, |ctx| {
    let m = command("bad-start-signature", Defect::StartSignature);
    silent(
        ctx,
        &m,
        "the start section names a function of type () -> i32; a start function takes no \
         arguments and returns nothing, because there is nobody to give the result to",
    )
});

wasm_test!(data_segment_out_of_range, |ctx| {
    // Not a validation failure: see the module doc-comment. The module is well typed, so it
    // is instantiation that fails, and the promise being tested is only that the start
    // function had not run by then.
    let m = command("start-and-data-out-of-range", Defect::DataOutOfRange);
    let run = ctx.start(&m, &[])?;
    let mut c = Check::new(
        "a data segment whose offset is past the end of the memory",
        &run,
    );
    c.module(&m);
    c.that(
        "stdout",
        "nothing at all: the start function runs after the segments are copied in, so it \
         never ran",
        run.stdout.is_empty(),
        run.stdout.clone(),
    );
    c.that(
        "exit",
        "a non-zero exit status",
        run.exit.failed(),
        run.exit.label(),
    );
    c.that(
        "stderr",
        "the canonical reason 'out of bounds memory access' — this one fails at \
         instantiation, after validation, so it is a trap and not a type error",
        run.stderr_has(trap::MEMORY_OUT_OF_BOUNDS),
        crate::stages::first_meaningful_line(&run.stderr),
    );
    c.finish()?;
    ctx.note(
        "the data segment case is an instantiation trap, not a validation failure; what it \
         shares with the others is that stdout is empty",
    );
    Ok(())
});

wasm_test!(control, |ctx| {
    // Without this every test above could be passed by refusing everything.
    let m = command("start-prints", Defect::None);
    expect_stdout(ctx, &m, &[], "ran\n")?;
    Ok(())
});

/// Assert the module is refused and, above all, that it printed nothing.
fn silent(ctx: &mut Ctx, m: &Module, why: &str) -> Result<(), crate::assert::Failure> {
    let run = expect_start_rejected(ctx, m, why)?;
    let mut c = Check::new("that the start function never ran", &run);
    c.module(m);
    c.eq("stdout", "", run.stdout.as_str());
    c.note(
        "this module's start function writes 'ran' to stdout through fd_write; a single byte \
         there means the module was executed before it was fully validated",
    );
    c.finish()
}

// ---------------------------------------------------------------------------------------
// Modules
// ---------------------------------------------------------------------------------------

/// The one thing wrong with a module, everything else being in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Defect {
    /// Nothing: the control module, which really does print.
    None,
    /// A function nobody calls calls a function index that does not exist.
    UnknownFunction,
    /// A later function's body does not match its declared result type.
    TypeError,
    /// A function nobody calls holds a byte that is not an opcode.
    UndefinedOpcode,
    /// A `br_table` whose labels have different arities.
    BrTableArity,
    /// The start section names a function of the wrong type.
    StartSignature,
    /// An active data segment whose offset is past the end of the memory.
    DataOutOfRange,
}

/// The message the start function writes, and the whole of the module's data.
const MSG: &[u8] = b"ran\n";

/// A WASI command whose start function prints `ran`, with `defect` somewhere else in it.
///
/// The printing function is function index 1 (`fd_write` is the import, index 0) and every
/// defect is added as a function index 2 or later, or as a second data segment — never in
/// the start function itself, except for [`Defect::StartSignature`], where the point is the
/// start section's own entry.
fn command(label: &str, defect: Defect) -> Module {
    let (mut b, _) = wasi::wasi_command(label, 1, &[W::FdWrite]);
    let void = b.add_type(ftype(&[], &[]));
    let printer = b.add_func(
        void,
        Func::new(wasi::puts(wasi::STDOUT, 0, MSG.len() as i32, 0)),
    );

    match defect {
        Defect::None | Defect::DataOutOfRange | Defect::StartSignature => {}
        Defect::UnknownFunction => {
            b.add_func(void, Func::new(Expr::new().call(777).drop()));
        }
        Defect::TypeError => {
            let ty = b.add_type(ftype(&[], &[ValType::I32]));
            b.add_func(ty, Func::new(Expr::new().i64_const(1)));
        }
        Defect::UndefinedOpcode => {
            b.add_func(void, Func::raw(&[0x27, 0x0b], "0x27 (no such opcode), end"));
        }
        Defect::BrTableArity => {
            let ty = b.add_type(ftype(&[], &[ValType::I32]));
            b.add_func(
                ty,
                Func::new(
                    Expr::new().block(
                        BlockType::Value(ValType::I32),
                        Expr::new()
                            .block(
                                BlockType::Empty,
                                Expr::new().i32_const(0).br_table(&[0, 1], 0),
                            )
                            .i32_const(5),
                    ),
                ),
            );
        }
    }

    let mut b = b.data_active(0, MSG);
    if defect == Defect::DataOutOfRange {
        // The memory is one page, so 0x11170 is a little past its end.
        b = b.data_active(70_000, b"x");
    }
    if defect == Defect::StartSignature {
        let ty = b.add_type(ftype(&[], &[ValType::I32]));
        let wrong = b.add_func(
            ty,
            Func::new(wasi::puts(wasi::STDOUT, 0, MSG.len() as i32, 0).i32_const(1)),
        );
        b = b.start(wrong);
    } else {
        b = b.start(printer);
    }
    b.build()
}

/// The control module, as a named builder for the catalog.
fn printing_command() -> Module {
    command("start-prints", Defect::None)
}

/// The same module with an unreachable function that calls function 777.
fn broken_command() -> Module {
    command("start-and-bad-index", Defect::UnknownFunction)
}

/// Worked examples: the module that prints, and the same module with one unreachable defect.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("A start function that prints", printing_command)
            .summary(
                "a WASI command importing `fd_write`, exporting its memory, with `ran\\n` at \
                 address 0 and a start section naming the function that writes it",
            )
            .command("run mod.wasm")
            .output("ran")
            .note(
                "The start section runs its function at instantiation, before any export is \
                 callable and without anyone asking. It has to be a `() -> ()`: there are no \
                 arguments to give it and nobody to take a result.",
            ),
        ExampleSpec::module(
            "The same module, plus one function nobody calls",
            broken_command,
        )
        .summary(
            "exactly the module above with one more function added — a `() -> ()` whose \
                 body is `call 777`, an index the module does not have",
        )
        .command("run mod.wasm")
        .output(
            "nothing on stdout, a message on stderr, and a non-zero exit status — the \
                 start function never runs",
        )
        .note(
            "Nothing ever calls function 2, and it makes no difference: validation covers \
                 every body in the code section, and a module that fails it is not \
                 instantiated at all. The empty stdout is the whole test.",
        ),
    ]
}
