//! Stage 18 — A duplicate definition is an error.
//!
//! Two inputs both claiming to define the same strong global is not a choice for the linker
//! to make: one of the two programs is wrong, and picking silently would give a build whose
//! behaviour depends on the order of the object files. The link stops and names the symbol.
//! The interesting part is everything this rule does *not* cover — a definition plus a
//! reference, a strong definition plus a weak one — which is where most hand-written
//! linkers get it wrong in the strict direction.

use crate::asm::Code;
use crate::assert::{Check, Failure};
use crate::elf::write::{ObjectBuilder, SectionSpec, SymbolSpec};
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 18,
        slug: "duplicate_definition",
        name: "A duplicate definition is an error",
        ext: false,
        hints: &[
            "Keep one table keyed by name; inserting a strong definition where a strong \
             definition already sits is the error, and the message must carry the name",
            "Only *definitions* collide — an SHN_UNDEF entry for the same name is a \
             reference and must never trip the check",
            "A strong definition and a weak one are legal: the strong one wins silently, and \
             a second weak one is not an error either",
            "The same file named twice on the command line is two inputs, not one; do not \
             deduplicate by path to make the error go away",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "two strong definitions of one function name the symbol",
                duplicate_function,
            ),
            Test::new(
                "two strong definitions of one datum name the symbol",
                duplicate_data,
            ),
            Test::new(
                "the same object passed twice is an error",
                same_object_twice,
            ),
            Test::new(
                "a definition in .text and another in .data still collide",
                duplicate_across_sections,
            ),
            Test::new(
                "a duplicate nothing references is still an error",
                unused_duplicate,
            ),
            Test::new(
                "the failing link exits non-zero and leaves no runnable output",
                no_runnable_output,
            ),
            Test::new(
                "a definition plus a reference is not a duplicate",
                definition_plus_reference,
            ),
            Test::new(
                "a strong definition beside a weak one is legal and the strong one wins",
                strong_beats_weak,
            ),
        ],
    }
}

link_test!(duplicate_function, |ctx| {
    let a = caller("never printed\n", "twice_defined")?;
    let b = callee_returning("twice_defined", 1)?;
    let c = callee_returning("twice_defined", 2)?;
    let run = ctx.link_fails(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .object("c.o", c)
            .label("two objects both defining 'twice_defined'"),
        "linking two objects that both define the same function",
    )?;
    let mut check = Check::new("the diagnostic for a duplicate function definition");
    check.mentions("linker.stderr", "twice_defined", &run.diagnostics());
    if !check.ok() {
        check.note("the name is the only part of the message a learner can act on");
        check.block("linker command", run.output.command_line());
    }
    check.finish()
});

link_test!(duplicate_data, |ctx| {
    let a = data_word("shared_word", 1)?;
    let b = data_word("shared_word", 2)?;
    let main = exit_only(0)?;
    let run = ctx.link_fails(
        &Link::new()
            .object("main.o", main)
            .object("a.o", a)
            .object("b.o", b)
            .label("two objects both defining 'shared_word'"),
        "linking two objects that both define the same datum",
    )?;
    let mut c = Check::new("the diagnostic for a duplicate data definition");
    c.mentions("linker.stderr", "shared_word", &run.diagnostics());
    if !c.ok() {
        c.note("a duplicate in .data is exactly as fatal as one in .text");
        c.block("linker command", run.output.command_line());
    }
    c.finish()
});

link_test!(same_object_twice, |ctx| {
    // Nothing distinguishes the two copies, so every global in the object collides with
    // itself — starting with _start.
    let obj = print_and_exit("never printed\n", 0)?;
    let run = ctx.link_fails(
        &Link::new()
            .object("a.o", obj.clone())
            .object("a.o", obj)
            .label("the same object named twice"),
        "linking the same object file twice",
    )?;
    let mut c = Check::new("the diagnostic for the same object given twice");
    c.mentions("linker.stderr", DEFAULT_ENTRY, &run.diagnostics());
    if !c.ok() {
        c.note(
            "a linker that silently deduplicates identical inputs hides a real bug in the \
             build system that produced the command line",
        );
        c.block("linker command", run.output.command_line());
    }
    c.finish()
});

link_test!(duplicate_across_sections, |ctx| {
    // One definition is code, the other is data: still one name, still two definitions.
    let a = callee_returning("collide", 4)?;
    let b = data_word("collide", 9)?;
    let main = exit_only(0)?;
    let run = ctx.link_fails(
        &Link::new()
            .object("main.o", main)
            .object("a.o", a)
            .object("b.o", b)
            .label("a function and a datum with one name"),
        "linking a function and a datum that share a name",
    )?;
    let mut c = Check::new("the diagnostic for definitions in different sections");
    c.mentions("linker.stderr", "collide", &run.diagnostics());
    if !c.ok() {
        c.note("the section a symbol lives in has nothing to do with whether its name is taken");
        c.block("linker command", run.output.command_line());
    }
    c.finish()
});

link_test!(unused_duplicate, |ctx| {
    // Neither definition is referenced by anything. Both objects are named on the command
    // line, so both are loaded in full, so the collision is real.
    let main = print_and_exit("never printed\n", 0)?;
    let a = callee_returning("unused_dup", 1)?;
    let b = callee_returning("unused_dup", 2)?;
    let run = ctx.link_fails(
        &Link::new()
            .object("main.o", main)
            .object("a.o", a)
            .object("b.o", b)
            .label("two unreferenced definitions of one name"),
        "linking two objects that duplicate a symbol nothing calls",
    )?;
    let mut c = Check::new("the diagnostic for an unreferenced duplicate");
    c.mentions("linker.stderr", "unused_dup", &run.diagnostics());
    if !c.ok() {
        c.note(
            "an object named on the command line is loaded whether or not it is needed, so \
             its globals are defined whether or not they are used — garbage collection of \
             unreferenced sections is a separate, opt-in feature",
        );
        c.block("linker command", run.output.command_line());
    }
    c.finish()
});

link_test!(no_runnable_output, |ctx| {
    let main = exit_only(0)?;
    let a = data_word("dup_word", 1)?;
    let b = data_word("dup_word", 2)?;
    let run = ctx.link(
        &Link::new()
            .object("main.o", main)
            .object("a.o", a)
            .object("b.o", b)
            .out("prog")
            .label("a duplicated definition"),
    )?;
    let exists = run.out_path.exists();
    let runnable = exists && crate::stages::is_executable(&run.out_path);
    let mut c = Check::new("how the linker ended a link with a duplicate definition");
    c.that(
        "linker.exit_status",
        "a non-zero exit status",
        !run.output.success(),
        run.output.status_line(),
    );
    c.that(
        "output.file",
        "either no output file at all, or one that is not marked executable",
        !runnable,
        format!(
            "{} exists: {exists}, executable: {runnable}",
            run.out_path.display()
        ),
    );
    if !c.ok() {
        c.block("linker command", run.output.command_line());
        c.block("linker output", run.diagnostics());
    }
    c.finish()
});

link_test!(definition_plus_reference, |ctx| {
    // Three objects mention `shared`; only one defines it. That is the normal case and must
    // never be mistaken for a duplicate.
    let a = caller("one definition\n", "shared")?;
    let b = tail_call("middle", "shared")?;
    let c = callee_returning("shared", 31)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .object("c.o", c)
            .label("one definition and two references"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "one definition\n", 31)?;
    Ok(())
});

link_test!(strong_beats_weak, |ctx| {
    let a = caller("strong wins\n", "dual")?;
    let weak = weak_callee_returning("dual", 10)?;
    let strong = callee_returning("dual", 55)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("weak.o", weak)
            .object("strong.o", strong)
            .label("a weak and a strong definition of 'dual'"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "strong wins\n", 55)?;
    let mut c = Check::new("that the strong definition is the one the call reached");
    let target = linked.address_of("dual")?;
    let site = linked.address_of(DEFAULT_ENTRY)? + CALLER_CALL_DISP_OFFSET;
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[call].disp32",
        site,
        target,
        -4,
    );
    if let Some(s) = linked.elf.symbol("dual") {
        c.observe("output.symtab['dual'].st_info.bind", s.bind);
    }
    c.note(
        "a weak definition is a default; the exit status, not the symbol table, is what \
         proves which body ran",
    );
    c.finish()
});

/// An object defining `name` as a function that calls `next` and returns its result.
fn tail_call(name: &str, next: &str) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.call(next);
    code.ret();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(name, ".text", 0).func())
            .symbol(SymbolSpec::undefined(next)),
    )
}

/// An object whose `.data` holds one 32-bit `value` under the global name `name`.
fn data_word(name: &str, value: u32) -> Result<Vec<u8>, Failure> {
    build(
        ObjectBuilder::new()
            .section(SectionSpec::data(".data", value.to_le_bytes().to_vec()).align(4))
            .symbol(SymbolSpec::global(name, ".data", 0).object(4)),
    )
}

/// Worked examples: the collision, and the near-miss that must be allowed.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::error(
            "Two objects, one name, two bodies",
            "ld -o prog a.o b.o c.o",
            || callee_returning("twice_defined", 1).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "b.o defines `twice_defined` as STB_GLOBAL at st_value 0 of its .text; c.o (not \
             shown) is byte-for-byte the same shape with a different constant. Both are named \
             on the command line, so both are loaded",
        )
        .response(
            "The linker refuses the link, exits non-zero and names `twice_defined` — GNU ld \
             says `multiple definition of `twice_defined'` and points at both objects",
        )
        .note(
            "Silently taking the first would make the program's behaviour depend on the order \
             of the .o files in the Makefile, which is the kind of bug that survives for years.",
        ),
        ExampleSpec::object(
            "A weak definition beside a strong one",
            "ld -o prog a.o weak.o strong.o",
            || weak_callee_returning("dual", 10).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "weak.o's `dual` has STB_WEAK in the binding half of st_info; strong.o defines the \
             same name STB_GLOBAL. Both are definitions, and both are loaded",
        )
        .response(
            "No error at all: the strong definition wins, the weak body is simply never \
             reached, and the call site is patched with the strong symbol's address — the \
             program exits 55, not 10",
        )
        .note(
            "This is the rule that makes `__attribute__((weak))` useful: a library ships a \
             default and the application overrides it without touching the library.",
        )
        .runs("strong wins\n", 55),
    ]
}
