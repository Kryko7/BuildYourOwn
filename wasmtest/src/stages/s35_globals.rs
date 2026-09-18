//! Stage 35 — Globals: mutable, immutable and imported.
//!
//! A global is the one piece of module state that is neither memory nor a table: a single
//! typed cell, filled by a constant expression at instantiation and thereafter read with
//! `global.get` and — if the module said `mut` — written with `global.set`.
//!
//! Two things in here are worth reading before the tests.
//!
//! **The imported global has no positive test.** `wasmtime run` gives a module WASI and
//! nothing else, so an `ImportKind::Global` can never be satisfied: the module validates and
//! then fails to instantiate with `unknown import`. That is still worth a test — the failure
//! has to happen before `f` runs, with nothing on stdout — but it means the interesting half
//! of importing, an initialiser that reads an imported global, cannot be exercised against
//! the reference at all.
//!
//! **So the initialiser test uses the other form.** A defined global whose initialiser is
//! `global.get` of an earlier one. That form is newer than it looks: in the 1.0 binary
//! format a `global.get` initialiser could only name an *imported* global, and naming a
//! preceding defined global arrived with the later reference-types and GC work. `wasmtime`
//! 48 accepts it, so the test asserts that; a 1.0-era validator that refuses it is refusing
//! something it was once right to refuse, and the note in the test says so.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::stages::{
    case_f32, case_f64, case_i32, case_i64, expect, expect_rejected, first_meaningful_line,
    run_cases_with, Stage, Test,
};
use crate::wasm::{
    const_f32, const_f64, const_global_get, const_i32, const_i64, ftype, global_i32, global_i64,
    op, Expr, Func, Global, ImportKind, Module, ModuleBuilder, ValType,
};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 35,
        slug: "globals",
        name: "Globals: mutable, immutable and imported",
        ext: false,
        hints: &[
            "A global section entry is three things: a value type byte, a one-byte mutability flag (00 const, 01 mut), and a constant expression ending in 0x0b",
            "Evaluate the initialisers at instantiation, in order, into a per-instance vector of values; `global.get n` and `global.set n` then index that vector",
            "`global.set` naming a global whose flag is 00 is a validation error, not a runtime trap — refuse the module before a single instruction runs",
            "Imported globals come first in the global index space, exactly as imported functions come first in the function index space",
        ],
        examples,
        tests: vec![
            Test::new(
                "an immutable global reads back its initialiser",
                immutable_reads_back,
            ),
            Test::new("a mutable global keeps what global.set wrote", mutable_set),
            Test::new(
                "each of several globals is reached by its own index, whatever its type",
                index_and_types,
            ),
            Test::new(
                "a global initialiser may read an earlier immutable global",
                initialiser_reads_earlier,
            ),
            Test::new(
                "an imported global nobody provides stops the module instantiating",
                imported_global_unsatisfied,
            ),
            Test::new(
                "global.set on an immutable global is refused",
                set_immutable_refused,
            ),
            Test::new(
                "a global whose initialiser has the wrong type is refused",
                wrong_type_initialiser_refused,
            ),
            Test::new(
                "a global initialised from a later global is refused",
                forward_reference_refused,
            ),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Globals the encoder has no shorthand for
// ---------------------------------------------------------------------------------------

/// An `f32` global with an `f32.const` initialiser.
fn global_f32(v: f32, mutable: bool) -> Global {
    Global {
        ty: ValType::F32,
        mutable,
        init: const_f32(v),
        init_text: format!("f32.const {v}"),
    }
}

/// An `f64` global with an `f64.const` initialiser.
fn global_f64(v: f64, mutable: bool) -> Global {
    Global {
        ty: ValType::F64,
        mutable,
        init: const_f64(v),
        init_text: format!("f64.const {v}"),
    }
}

/// A global whose initialiser is `global.get n` — the only non-literal form there is.
fn global_from(ty: ValType, mutable: bool, n: u32) -> Global {
    Global {
        ty,
        mutable,
        init: const_global_get(n),
        init_text: format!("global.get {n}"),
    }
}

// ---------------------------------------------------------------------------------------
// Modules
// ---------------------------------------------------------------------------------------

/// One immutable `i32` global holding `v`, and an `f` that returns it.
fn one_immutable_global(v: i32) -> Module {
    let mut b = ModuleBuilder::new("global-const").global(global_i32(v, false));
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(ty, Func::new(Expr::new().global_get(0)));
    b.export_func("f", idx).build()
}

/// One mutable `i32` global, incremented `n` times and then returned.
fn counter(n: usize) -> Module {
    let mut b = ModuleBuilder::new("global-counter").global(global_i32(0, true));
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let step = Expr::new()
        .global_get(0)
        .i32_const(1)
        .op(op::I32_ADD)
        .global_set(0);
    let idx = b.add_func(ty, Func::new(Expr::new().repeat(n, &step).global_get(0)));
    b.export_func("f", idx).build()
}

/// The innermost line of a runtime's error report.
///
/// `wasmtime` writes a chain — "failed to run main module", then "failed to instantiate",
/// then the reason — and it is the last line that says `unknown import`. The first
/// meaningful line, which most checks quote, only names the file.
fn root_cause(stderr: &str) -> String {
    let deepest = stderr
        .lines()
        .rfind(|l| !l.trim().is_empty() && !l.trim_start().to_lowercase().starts_with("warning"))
        .unwrap_or("")
        .trim()
        .to_string();
    if deepest.is_empty() {
        first_meaningful_line(stderr)
    } else {
        deepest
    }
}

/// A module importing `env::answer` as a const `i32` and returning it.
fn imported_global() -> Module {
    let mut b = ModuleBuilder::new("global-imported").import(
        "env",
        "answer",
        ImportKind::Global {
            ty: ValType::I32,
            mutable: false,
        },
    );
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(ty, Func::new(Expr::new().global_get(0)));
    b.export_func("f", idx).build()
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

wasm_test!(immutable_reads_back, |ctx| {
    // Three separate modules, so a runtime cannot pass by remembering one value: the
    // initialiser really is read out of the global section every time.
    for v in [7i32, 0, -1] {
        expect(ctx, &one_immutable_global(v), &v.to_string())?;
    }
    Ok(())
});

wasm_test!(mutable_set, |ctx| {
    // Written once, written twice, and read in between: the cell is per-instance state, not
    // a constant the validator could fold away.
    let base = ModuleBuilder::new("global-mutable").global(global_i32(10, true));
    run_cases_with(
        ctx,
        base,
        vec![
            case_i32(
                "global.set then global.get",
                Expr::new().i32_const(42).global_set(0).global_get(0),
                42,
            ),
            case_i32(
                "an untouched global still holds its initialiser",
                Expr::new().global_get(0),
                10,
            ),
            case_i32(
                "the second write wins",
                Expr::new()
                    .i32_const(1)
                    .global_set(0)
                    .i32_const(2)
                    .global_set(0)
                    .global_get(0),
                2,
            ),
            case_i32(
                "global.set takes its value off the stack, so an expression works too",
                Expr::new()
                    .global_get(0)
                    .global_get(0)
                    .op(op::I32_MUL)
                    .global_set(0)
                    .global_get(0),
                100,
            ),
        ],
    )?;
    // Every --invoke is a fresh instance, so the counter answers 3, never 6.
    expect(ctx, &counter(3), "3")?;
    Ok(())
});

wasm_test!(index_and_types, |ctx| {
    // Five globals of four types. Each case reads one index, so a runtime that keeps a
    // single cell, or that indexes the vector the wrong way round, fails a different case.
    let base = ModuleBuilder::new("global-indices")
        .global(global_i32(11, false))
        .global(global_i64(-9_000_000_000, false))
        .global(global_f32(1.5, false))
        .global(global_f64(-0.25, false))
        .global(global_i32(22, true));
    run_cases_with(
        ctx,
        base,
        vec![
            case_i32("global 0 is the i32 11", Expr::new().global_get(0), 11),
            case_i64(
                "global 1 is the i64 -9000000000",
                Expr::new().global_get(1),
                -9_000_000_000,
            ),
            case_f32("global 2 is the f32 1.5", Expr::new().global_get(2), 1.5),
            case_f64(
                "global 3 is the f64 -0.25",
                Expr::new().global_get(3),
                -0.25,
            ),
            case_i32(
                "global 4 is the mutable i32 22",
                Expr::new().global_get(4),
                22,
            ),
            case_i32(
                "writing global 4 leaves global 0 alone",
                Expr::new()
                    .i32_const(99)
                    .global_set(4)
                    .global_get(0)
                    .global_get(4)
                    .op(op::I32_ADD),
                110,
            ),
        ],
    )
});

wasm_test!(initialiser_reads_earlier, |ctx| {
    // global 1 is initialised from global 0, and global 2 from global 1: the initialisers
    // run in order, each one able to see the ones before it.
    let mut b = ModuleBuilder::new("global-init-chain")
        .global(global_i32(41, false))
        .global(global_from(ValType::I32, false, 0))
        .global(global_from(ValType::I32, true, 1));
    let ty = b.add_type(ftype(&[], &[ValType::I32, ValType::I32, ValType::I32]));
    let idx = b.add_func(
        ty,
        Func::new(Expr::new().global_get(0).global_get(1).global_get(2)),
    );
    let m = b.export_func("f", idx).build();

    let run = ctx.invoke(&m, "f", &[])?;
    let mut c = Check::new("a global initialised by global.get of an earlier one", &run);
    c.module(&m);
    c.eq(
        "stdout.lines",
        vec!["41".to_string(), "41".to_string(), "41".to_string()],
        run.lines(),
    );
    c.note(
        "naming a *defined* global in an initialiser is newer than the 1.0 format, where \
         only an imported global could be named; it is the only non-literal constant \
         expression a module the reference will instantiate can use, because the reference \
         provides no host globals to import",
    );
    c.finish()
});

wasm_test!(imported_global_unsatisfied, |ctx| {
    // Well-formed, valid, and impossible to instantiate: there is no host global to bind to.
    // The failure belongs in the same place a missing function import fails.
    let m = imported_global();
    let run = ctx.invoke(&m, "f", &[])?;
    let mut c = Check::new("a global import nothing satisfies", &run);
    c.module(&m);
    c.that(
        "exit",
        "a non-zero exit status: an unsatisfied import is an instantiation failure",
        run.exit.failed(),
        run.exit.label(),
    );
    c.that(
        "stdout",
        "nothing at all — there is no instance for `f` to run in",
        run.stdout.is_empty(),
        run.stdout.clone(),
    );
    c.that(
        "stderr",
        "a message naming the import that could not be found",
        !run.stderr.trim().is_empty(),
        "(empty)",
    );
    c.finish()?;
    ctx.note(format!("the runtime answered: {}", root_cause(&run.stderr)));
    Ok(())
});

wasm_test!(set_immutable_refused, |ctx| {
    // The mutability flag is the validator's business, so this module never reaches the
    // point of writing anything and stdout stays empty.
    let mut b = ModuleBuilder::new("global-set-const").global(global_i32(1, false));
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(
        ty,
        Func::new(Expr::new().i32_const(2).global_set(0).global_get(0)),
    );
    let m = b.export_func("f", idx).build();
    expect_rejected(
        ctx,
        &m,
        "global 0 is declared const, and `global.set` may only name a mut global",
    )?;
    Ok(())
});

wasm_test!(wrong_type_initialiser_refused, |ctx| {
    // An i32 global whose initialiser leaves an i64 on the stack. Every byte decodes; the
    // constant expression simply has the wrong result type.
    let mut b = ModuleBuilder::new("global-init-type").global(Global {
        ty: ValType::I32,
        mutable: false,
        init: const_i64(1),
        init_text: "i64.const 1 — an i64 initialising an i32 global".to_string(),
    });
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(ty, Func::new(Expr::new().global_get(0)));
    let m = b.export_func("f", idx).build();
    expect_rejected(
        ctx,
        &m,
        "the initialiser of an i32 global must have result type i32, and i64.const does not",
    )?;
    Ok(())
});

wasm_test!(forward_reference_refused, |ctx| {
    // global 0 reads global 1, which is declared after it. Initialisers run in order, so
    // there is nothing there to read; a validator says so rather than inventing a zero.
    let mut b = ModuleBuilder::new("global-forward")
        .global(global_from(ValType::I32, false, 1))
        .global(Global {
            ty: ValType::I32,
            mutable: false,
            init: const_i32(5),
            init_text: "i32.const 5".to_string(),
        });
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(ty, Func::new(Expr::new().global_get(0)));
    let m = b.export_func("f", idx).build();
    expect_rejected(
        ctx,
        &m,
        "a constant expression may only name globals declared before it, and global 1 is not",
    )?;
    Ok(())
});

/// Worked examples: a counter, and the import that cannot be satisfied.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("A counter in a mutable global", || counter(3))
            .summary(
                "one `mut i32` global initialised to 0, and an exported `f` that adds one to \
                 it three times with `global.get` / `global.set` before returning it",
            )
            .command("run --invoke f mod.wasm")
            .output("3")
            .note(
                "The value belongs to the *instance*, not the module: invoking `f` again \
                 builds a new instance and runs the initialiser again, so it answers 3 every \
                 time, never 6.",
            ),
        ExampleSpec::module("A global the host does not provide", imported_global)
            .summary(
                "an import section asking for `env::answer` as a const i32, and an `f` that \
                 returns `global.get 0` — the imported global, which comes first in the \
                 index space",
            )
            .command("run --invoke f mod.wasm")
            .output(
                "nothing on stdout, `unknown import` on stderr, and a non-zero exit — the \
                 module validates and then fails to instantiate",
            )
            .note(
                "Imported globals take indices 0..n before any defined global, so adding one \
                 import renumbers every `global.get` in the module. This is the one global \
                 test that cannot pass: the reference gives a module no host globals at all.",
            ),
    ]
}
