//! Stage 06 — the smallest modules there are, and the section that gives them a door.
//!
//! The eight-byte header is a complete module. So is the header plus a type section nobody
//! uses, and the header plus a custom section. None of them exports anything, none of them
//! has a function, and all three are valid: a decoder that requires a type section, or a code
//! section, or any section at all, refuses programs the spec accepts.
//!
//! That makes them awkward to test, because "accepted" is not something the command line says
//! out loud. **How the two outcomes are told apart** is the whole trick of the first three
//! tests, and it is worth writing down:
//!
//! * `run --invoke <name>` on a module that decoded but has no such export fails **naming the
//!   export**: `wasmtime` writes ``no func export named `nothing_is_exported` found``. Every
//!   runtime that reaches the point of looking one up can name the one it looked for, so the
//!   test asserts the name appears on stderr — and, on a module with a broken header, that it
//!   does not. Those two assertions together say "the bytes were fine, the door was missing".
//! * `run` with no `--invoke` is the WASI command path. The expectation here was wrong when
//!   this stage was first written: a module with no `_start` was assumed to fail. `wasmtime`
//!   **exits 0** and does nothing — it instantiates the module, finds no command to run, and
//!   stops. So the test asserts the weaker true thing: nothing on stdout, and either success
//!   or a complaint that names `_start`, never a complaint about the module's bytes.
//!
//! The rest of the stage is the export section itself. Two names may point at one function; a
//! memory, a global and a function can be exported side by side; a name may be empty and it
//! may be any UTF-8 at all. The one rule is that **names are unique within a module**, which
//! is a validation error rather than a decoding one — the bytes are perfectly well formed.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::stages::{
    expect, expect_lines, expect_rejected, first_meaningful_line, Ctx, Stage, Test,
};
use crate::wasm::{
    ftype, global_i32, op, section, uleb, vector, wasm_name, Enc, ExportKind, Expr, Func, Limits,
    Module, ModuleBuilder, ValType,
};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 6,
        slug: "minimal_modules",
        name: "Minimal modules and the export section",
        ext: false,
        hints: &[
            "Every section is optional: the eight-byte header alone is a valid module, so build your decoder around a loop that may run zero times rather than around a list of sections you require",
            "An export is a name, a one-byte kind (0 func, 1 table, 2 memory, 3 global) and an index into that kind's index space — and the same index may be exported under as many names as you like",
            "Export names are arbitrary UTF-8, including the empty string, so store them as bytes you validate rather than as an identifier you parse",
            "Two exports with the same name is a validation error, not a decoding one: the bytes decode, the module is still refused, and nothing runs",
        ],
        examples,
        tests: vec![
            Test::new("the eight-byte header on its own is a complete module", bare_header),
            Test::new(
                "a module holding only a type section or only a custom section is complete",
                one_section_only,
            ),
            Test::new(
                "a missing export is a different failure from a module that will not decode",
                missing_export_is_not_a_decode_error,
            ),
            Test::new("one function exported under two names answers on both", two_names_one_func),
            Test::new(
                "a memory, a global and a function can be exported side by side",
                three_kinds,
            ),
            Test::new("an export name may be empty or non-ASCII", odd_names),
            Test::new("two exports with the same name are refused", duplicate_name),
        ],
    }
}

/// The export name the "is it there?" tests look for.
///
/// Long and unmistakable on purpose: the assertion is that the runtime echoes the name it
/// could not find, and a one-letter name would match by accident inside words like "failed".
const ABSENT: &str = "nothing_is_exported";

/// The module that is nothing but the header.
fn bare() -> Module {
    Enc::with_header().finish("module-header-only")
}

/// The header plus one type section that no function uses.
fn type_only() -> Module {
    let mut e = Enc::with_header();
    e.section(section::TYPE, &vector(&[vec![0x60, 0x00, 0x01, 0x7f]]));
    e.finish("module-type-section-only")
}

/// The header plus one custom section.
fn custom_only() -> Module {
    let mut e = Enc::with_header();
    let mut body = wasm_name("note");
    body.extend_from_slice(b"written by hand");
    e.section(section::CUSTOM, &body);
    e.finish("module-custom-section-only")
}

/// Assert `m` decodes, instantiates, and simply has nothing called `ABSENT` in it.
///
/// See the module comment: the two halves are the `--invoke` path naming the export it could
/// not find, and the command path leaving stdout alone without blaming the bytes.
fn assert_complete_but_empty(ctx: &mut Ctx, m: &Module) -> Result<(), crate::assert::Failure> {
    let run = ctx.invoke(m, ABSENT, &[])?;
    let mut c = Check::new(format!("'{}' decodes but exports nothing", m.label), &run);
    c.module(m);
    c.that(
        "exit",
        "a non-zero exit status: there is nothing to invoke",
        run.exit.failed(),
        run.exit.label(),
    );
    c.eq("stdout", "", run.stdout.as_str());
    c.that(
        "stderr",
        &format!("the name '{ABSENT}' that was looked up and not found, which is what says the module itself was fine"),
        run.stderr.contains(ABSENT),
        first_meaningful_line(&run.stderr),
    );
    c.finish()?;

    let run = ctx.start(m, &[])?;
    let mut c = Check::new(format!("'{}' as a WASI command", m.label), &run);
    c.module(m);
    c.eq("stdout", "", run.stdout.as_str());
    c.that(
        "outcome",
        "either success — there is no _start, so there is nothing to do — or a failure that names _start",
        run.exit.success() || run.stderr.to_lowercase().contains("_start"),
        format!("{}: {}", run.exit.label(), first_meaningful_line(&run.stderr)),
    );
    c.finish()
}

wasm_test!(bare_header, |ctx| {
    assert_complete_but_empty(ctx, &bare())?;
    ctx.note("wasmtime runs the eight-byte module as a WASI command and exits 0, doing nothing");
    Ok(())
});

wasm_test!(one_section_only, |ctx| {
    assert_complete_but_empty(ctx, &type_only())?;
    assert_complete_but_empty(ctx, &custom_only())?;
    Ok(())
});

wasm_test!(missing_export_is_not_a_decode_error, |ctx| {
    // The discriminating pair. A module whose header is wrong must not produce the same
    // complaint as a module that decoded and has no such export; if a runtime answers "no
    // such export" for both, it never read the bytes at all.
    let run = ctx.invoke(&bare(), ABSENT, &[])?;
    let named = run.stderr.contains(ABSENT);

    let mut bytes = bare().bytes.clone();
    bytes[3] = b'X';
    let broken = Module {
        bytes,
        anns: bare().anns.clone(),
        label: "module-bad-magic".to_string(),
    };
    let broken_run = ctx.invoke(&broken, ABSENT, &[])?;

    let mut c = Check::new(
        "telling a missing export apart from a broken module",
        &broken_run,
    );
    c.module(&broken);
    c.that(
        "stderr (valid module, absent export)",
        &format!("names '{ABSENT}'"),
        named,
        first_meaningful_line(&run.stderr),
    );
    c.that(
        "stderr (broken header)",
        "a complaint about the bytes, which cannot name an export the runtime never looked for",
        !broken_run.stderr.contains(ABSENT),
        first_meaningful_line(&broken_run.stderr),
    );
    c.that(
        "exit (broken header)",
        "a non-zero exit status",
        broken_run.exit.failed(),
        broken_run.exit.label(),
    );
    c.finish()?;
    ctx.note(format!(
        "absent export: {} — broken header: {}",
        first_meaningful_line(&run.stderr),
        first_meaningful_line(&broken_run.stderr)
    ));
    Ok(())
});

wasm_test!(two_names_one_func, |ctx| {
    // One function, three export entries. The export section is a list of names pointing into
    // the index spaces, not a list of things, so nothing stops three of them pointing at the
    // same function — and all three must answer.
    let mut b = ModuleBuilder::new("one-func-three-names");
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(ty, Func::new(Expr::new().i32_const(7)));
    let m = b
        .export_func("f", idx)
        .export_func("also_f", idx)
        .export_func("f_again", idx)
        .build();
    for name in ["f", "also_f", "f_again"] {
        expect_lines(ctx, &m, name, &[], &["7"])?;
    }
    Ok(())
});

wasm_test!(three_kinds, |ctx| {
    // A function, a memory and a global out of one module, each with its own kind byte in the
    // export section. The function proves the module really instantiated by reading the
    // global and a byte of the memory back out.
    let mut b = ModuleBuilder::new("three-export-kinds");
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(
        ty,
        Func::new(
            Expr::new()
                .global_get(0)
                .i32_const(0)
                .mem(op::I32_LOAD8_U, 0, 0)
                .op(op::I32_ADD),
        ),
    );
    let m = b
        .memory(Limits::min(1))
        .global(global_i32(40, false))
        .export_func("f", idx)
        .export_memory()
        .export("g", ExportKind::Global, 0)
        .data_active(0, &[2])
        .build();
    expect(ctx, &m, "42")?;
    ctx.note("kind bytes 0, 2 and 3 in one export section: func, memory and global");
    Ok(())
});

wasm_test!(odd_names, |ctx| {
    // An empty name is a name of length zero, and it is legal. So is any other valid UTF-8,
    // which is why an export name is bytes to be validated rather than an identifier.
    let mut b = ModuleBuilder::new("odd-export-names");
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(ty, Func::new(Expr::new().i32_const(7)));
    let m = b
        .export_func("", idx)
        .export_func("héllo→wörld", idx)
        .export_func("f", idx)
        .build();
    expect(ctx, &m, "7")?;
    expect_lines(ctx, &m, "", &[], &["7"])?;
    expect_lines(ctx, &m, "héllo→wörld", &[], &["7"])?;
    ctx.note("wasmtime looks up an empty export name and a multi-byte UTF-8 one without complaint");

    // Bytes that are not UTF-8 at all are a different matter: `name` is UTF-8 in the spec.
    let mut entry = uleb(2);
    entry.extend_from_slice(&[0xff, 0xfe]);
    entry.push(0x00);
    entry.extend_from_slice(&uleb(0));
    let mut good = wasm_name("f");
    good.push(0x00);
    good.extend_from_slice(&uleb(0));
    let mut e = Enc::with_header();
    e.section(section::TYPE, &vector(&[vec![0x60, 0x00, 0x01, 0x7f]]));
    e.section(section::FUNCTION, &vector(&[uleb(0)]));
    e.section(section::EXPORT, &vector(&[entry, good]));
    let mut inner = uleb(0);
    inner.extend_from_slice(&[0x41, 0x07, 0x0b]);
    let mut body = uleb(inner.len() as u64);
    body.extend_from_slice(&inner);
    e.section(section::CODE, &vector(&[body]));
    let m = e.finish("export-name-not-utf8");
    expect_rejected(ctx, &m, "an export name is UTF-8, and ff fe is not")?;
    Ok(())
});

wasm_test!(duplicate_name, |ctx| {
    // Every byte here decodes. What is wrong is a rule about the module as a whole: an export
    // name identifies one thing, so a runtime that keeps its exports in a list and answers
    // with the first match accepts a module the spec refuses.
    let mut b = ModuleBuilder::new("duplicate-export-name");
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let a = b.add_func(ty, Func::new(Expr::new().i32_const(7)));
    let c = b.add_func(ty, Func::new(Expr::new().i32_const(9)));
    let m = b.export_func("f", a).export_func("f", c).build();
    expect_rejected(
        ctx,
        &m,
        "two exports called 'f' — an export name is unique in a module",
    )?;

    // And the same name pointing at the same function twice, in case a runtime only compares
    // the targets.
    let mut b = ModuleBuilder::new("duplicate-export-name-same-target");
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let a = b.add_func(ty, Func::new(Expr::new().i32_const(7)));
    let m = b.export_func("f", a).export_func("f", a).build();
    expect_rejected(
        ctx,
        &m,
        "the duplicate is about the name, not about what it points at",
    )?;
    Ok(())
});

/// Worked examples: the smallest module there is, and one function behind three doors.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("The empty module", bare)
            .summary("the eight-byte header and nothing else: no sections at all")
            .command("run mod.wasm")
            .output("nothing at all, and an exit status of 0")
            .note(
                "Every section is optional, so this is a valid module — it simply has no \
                 functions, no exports and no `_start`. `run --invoke f` on it fails naming \
                 `f`, not the bytes, and that difference is how you tell a module that would \
                 not decode from one that decoded and had nothing to offer.",
            ),
        ExampleSpec::module("One function, three names", || {
            let mut b = ModuleBuilder::new("one-func-three-names");
            let ty = b.add_type(ftype(&[], &[ValType::I32]));
            let idx = b.add_func(ty, Func::new(Expr::new().i32_const(7)));
            b.export_func("f", idx)
                .export_func("also_f", idx)
                .export_func("f_again", idx)
                .build()
        })
        .summary(
            "one `() -> i32` function returning 7, and an export section with three entries \
             all pointing at function index 0",
        )
        .command("run --invoke also_f mod.wasm")
        .output("7")
        .note(
            "An export is a name plus a kind byte plus an index, so the mapping is many names \
             to one thing. The one constraint is the other way round: no two entries may share \
             a name, and a module that breaks it is refused at validation with nothing run.",
        ),
    ]
}
