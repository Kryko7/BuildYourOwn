//! Stage 07 — Function bodies: arity and result types.
//!
//! The first stage of the validator. Everything here happens before a single instruction
//! runs: a body is checked against the signature its function declares, and the two vectors
//! that describe the functions — the function section's type indices and the code section's
//! bodies — are checked against each other.
//!
//! Two of these are decode-time failures rather than type errors, and `wasmtime` says so:
//! a type index the type section does not have is `unknown type 7: type index out of
//! bounds`, and mismatched section lengths are `function and code section have inconsistent
//! lengths`. The suite does not care which phase a runtime calls them — [`expect_rejected`]
//! only asks for a non-zero exit, an empty stdout and a message — but a from-scratch runtime
//! usually catches both while reading the sections, long before it looks at an opcode.

use crate::examples::ExampleSpec;
use crate::stages::{
    expect_lines, expect_rejected, run_cases, single, single_with_locals, Case, Stage, Test,
};
use crate::wasm::{
    ftype, op, section, uleb, vector, wasm_name, Enc, Expr, Func, Module, ModuleBuilder, ValType,
};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 7,
        slug: "function_bodies",
        name: "Function bodies: arity and result types",
        ext: false,
        hints: &[
            "Type-check every body against its declared signature before you execute anything: at the body's final `end` the stack must hold exactly the result types, in order",
            "Falling off the end of a `() -> i32` body with an empty stack is a validation error, not an implicit zero — and a leftover value in a `() -> ()` body is one too",
            "The function section and the code section are two parallel vectors: entry i of one describes entry i of the other, so different lengths make the module malformed",
            "A function's local index space is its parameters first and then its declared locals, which come in runs of a count and a type — `3 i32` is three locals, not three bytes",
        ],
        examples,
        tests: vec![
            Test::new("a body that leaves exactly its declared results is accepted", right_results),
            Test::new("a body that falls off the end with an empty stack is refused", falls_off_the_end),
            Test::new("a body that leaves an i64 where an i32 belongs is refused", wrong_result_type),
            Test::new("a body that leaves two values where one is declared is refused", too_many_results),
            Test::new("a body with no results that leaves a value behind is refused", void_leftover),
            Test::new("a function whose declared type index does not exist is refused", unknown_type_index),
            Test::new("the code section must hold exactly one body per function", body_count),
            Test::new("locals declared in runs of a count and a type are accepted", several_local_runs),
        ],
    }
}

wasm_test!(right_results, |ctx| {
    // The balancing case for the whole stage: five signatures whose bodies leave exactly
    // what they promise. A runtime that refuses every module fails here.
    run_cases(
        ctx,
        "arity-ok",
        vec![
            Case::new(
                "() -> i32 leaves one i32",
                ftype(&[], &[ValType::I32]),
                Expr::new().i32_const(7),
                &["7"],
            ),
            Case::new(
                "() -> i64 leaves one i64",
                ftype(&[], &[ValType::I64]),
                Expr::new().i64_const(-9),
                &["-9"],
            ),
            Case::new(
                "() -> f64 leaves one f64",
                ftype(&[], &[ValType::F64]),
                Expr::new().f64_const(1.5),
                &["1.5"],
            ),
            Case::new(
                "() -> () leaves nothing",
                ftype(&[], &[]),
                Expr::new().nop(),
                &[],
            ),
            Case::new(
                "(i32 i32) -> i32 consumes both parameters",
                ftype(&[ValType::I32, ValType::I32], &[ValType::I32]),
                Expr::new().local_get(0).local_get(1).op(op::I32_ADD),
                &["7"],
            )
            .args(&["3", "4"]),
        ],
    )
});

wasm_test!(falls_off_the_end, |ctx| {
    let m = single(
        "falls-off-the-end",
        ftype(&[], &[ValType::I32]),
        Expr::new(),
    );
    expect_rejected(
        ctx,
        &m,
        "a () -> i32 body whose only instruction is the terminating end: there is no i32 on \
         the stack to return, and a missing result is not a zero",
    )?;
    Ok(())
});

wasm_test!(wrong_result_type, |ctx| {
    let m = single(
        "result-i64-for-i32",
        ftype(&[], &[ValType::I32]),
        Expr::new().i64_const(1),
    );
    expect_rejected(
        ctx,
        &m,
        "the body leaves an i64 where the signature promises an i32; the two are different \
         types and nothing narrows one into the other implicitly",
    )?;
    Ok(())
});

wasm_test!(too_many_results, |ctx| {
    let m = single(
        "two-results-for-one",
        ftype(&[], &[ValType::I32]),
        Expr::new().i32_const(1).i32_const(2),
    );
    expect_rejected(
        ctx,
        &m,
        "the body leaves two i32s where one is declared: the stack must be exactly the \
         result types at the end, not merely start with them",
    )?;
    Ok(())
});

wasm_test!(void_leftover, |ctx| {
    let m = single("void-leftover", ftype(&[], &[]), Expr::new().i32_const(1));
    expect_rejected(
        ctx,
        &m,
        "a () -> () body that leaves an i32 behind; a value nobody asked for is as wrong as \
         a value nobody supplied",
    )?;
    Ok(())
});

wasm_test!(unknown_type_index, |ctx| {
    let m = missing_type_index();
    expect_rejected(
        ctx,
        &m,
        "the function section names type 7 and the type section holds one type, so there is \
         no signature to check the body against",
    )?;
    Ok(())
});

wasm_test!(body_count, |ctx| {
    // Both directions: the function section may not run ahead of the code section, nor
    // behind it. A runtime that zips the two vectors without comparing their lengths
    // silently loses a function or reads past the end of one.
    let more_funcs = counts("two-funcs-one-body", 2, 1);
    expect_rejected(
        ctx,
        &more_funcs,
        "the function section declares two functions and the code section holds one body",
    )?;
    let more_bodies = counts("one-func-two-bodies", 1, 2);
    expect_rejected(
        ctx,
        &more_bodies,
        "the code section holds two bodies and the function section declares one function",
    )?;
    Ok(())
});

wasm_test!(several_local_runs, |ctx| {
    let m = local_runs();
    let run = expect_lines(ctx, &m, "f", &[], &["11"])?;
    ctx.note(format!(
        "six locals in three runs (2 i32, 1 i64, 3 f64) answered {}",
        run.first_line()
    ));
    Ok(())
});

// ---------------------------------------------------------------------------------------
// Modules
// ---------------------------------------------------------------------------------------

/// `() -> i32` with six locals declared as three runs of different types, all of them used.
///
/// The runs are `2 × i32`, `1 × i64`, `3 × f64`, so the local indices are 0..=5 and every
/// one of them must have been given the right type by the decoder: the body adds an i32, a
/// wrapped i64 and a truncated f64 together and must answer 11.
fn local_runs() -> Module {
    single_with_locals(
        "several-local-runs",
        ftype(&[], &[ValType::I32]),
        &[(2, ValType::I32), (1, ValType::I64), (3, ValType::F64)],
        Expr::new()
            .i32_const(1)
            .local_set(0)
            .i32_const(2)
            .local_set(1)
            .i64_const(3)
            .local_set(2)
            .f64_const(4.5)
            .local_set(3)
            .f64_const(0.5)
            .local_set(4)
            .local_get(3)
            .local_get(4)
            .op(op::F64_ADD)
            .local_set(5)
            .local_get(0)
            .local_get(1)
            .op(op::I32_ADD)
            .local_get(2)
            .op(op::I32_WRAP_I64)
            .op(op::I32_ADD)
            .local_get(5)
            .op(op::I32_TRUNC_F64_S)
            .op(op::I32_ADD),
    )
}

/// A module whose single function declares type 7 while the type section holds one type.
fn missing_type_index() -> Module {
    let mut b = ModuleBuilder::new("unknown-type-index");
    b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(7, Func::new(Expr::new().i32_const(1)));
    b.export_func("f", idx).build()
}

/// A module whose function and code sections disagree about how many functions there are.
///
/// [`ModuleBuilder`] keeps the two in step by construction, so this one is written with
/// [`Enc`]: one type, `funcs` entries in the function section and `bodies` identical
/// `i32.const 7` bodies in the code section.
fn counts(label: &str, funcs: usize, bodies: usize) -> Module {
    let mut e = Enc::with_header();

    // type[0] = () -> i32, written by hand: 0x60, no params, one result of 0x7f.
    e.section(section::TYPE, &vector(&[vec![0x60, 0x00, 0x01, 0x7f]]));

    let func_entries: Vec<Vec<u8>> = (0..funcs).map(|_| uleb(0)).collect();
    let func_body = vector(&func_entries);
    let at = e.at();
    e.section(section::FUNCTION, &func_body);
    e.annotate(
        at + 1 + uleb(func_body.len() as u64).len(),
        uleb(funcs as u64).len(),
        "function.count",
        format!("{funcs} function(s) declared"),
    );

    let mut export = wasm_name("f");
    export.push(0x00);
    export.extend_from_slice(&uleb(0));
    e.section(section::EXPORT, &vector(&[export]));

    // One body: no local runs, `i32.const 7`, `end`.
    let inner = vec![0x00, 0x41, 0x07, 0x0b];
    let mut one = uleb(inner.len() as u64);
    one.extend_from_slice(&inner);
    let code_entries: Vec<Vec<u8>> = (0..bodies).map(|_| one.clone()).collect();
    let code_body = vector(&code_entries);
    let at = e.at();
    e.section(section::CODE, &code_body);
    e.annotate(
        at + 1 + uleb(code_body.len() as u64).len(),
        uleb(bodies as u64).len(),
        "code.count",
        format!(
            "{bodies} function bod{}",
            if bodies == 1 { "y" } else { "ies" }
        ),
    );

    e.finish(label)
}

/// The module `body_count` builds first, as a named builder for the catalog.
fn two_funcs_one_body() -> Module {
    counts("two-funcs-one-body", 2, 1)
}

/// Worked examples: a body that is right, and two vectors that do not line up.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("Six locals in three runs", local_runs)
            .summary(
                "a `() -> i32` whose code entry declares `2 × i32, 1 × i64, 3 × f64` — six \
                 locals, indices 0 to 5 — and adds an i32, a wrapped i64 and a truncated f64 \
                 together",
            )
            .command("run --invoke f mod.wasm")
            .output("11")
            .note(
                "Locals are declared as runs of a count and a type, and every local starts \
                 as the zero of its type. The count is a number of locals, not a number of \
                 bytes, and the local index space begins with the function's parameters.",
            ),
        ExampleSpec::module("Two functions, one body", two_funcs_one_body)
            .summary(
                "a function section declaring two functions of type 0 and a code section \
                 holding a single body",
            )
            .command("run --invoke f mod.wasm")
            .output(
                "nothing on stdout, a message on stderr, and a non-zero exit status — the \
                 module never runs",
            )
            .note(
                "The two sections are parallel vectors: entry i of the function section gives \
                 the type of the function whose body is entry i of the code section. Compare \
                 their lengths as you read them; zipping them without checking silently drops \
                 a function.",
            ),
    ]
}
