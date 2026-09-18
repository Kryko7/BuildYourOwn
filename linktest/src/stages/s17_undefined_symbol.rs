//! Stage 17 — An undefined symbol is an error.
//!
//! The other half of stage 16. A reference the linker cannot satisfy is not something to
//! paper over with a zero: the program would jump into nothing. The link has to stop, say
//! *which* name it could not find, exit non-zero, and — this is the part people forget —
//! not leave a half-written executable on disk for a Makefile to treat as up to date.

use crate::asm::{Code, Reg, STDOUT};
use crate::assert::{Check, Failure};
use crate::elf::write::{ObjectBuilder, Reloc, SectionSpec, SymbolSpec};
use crate::elf::*;
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 17,
        slug: "undefined_symbol",
        name: "An undefined symbol is an error",
        ext: false,
        hints: &[
            "After collecting every definition, walk every relocation: if the symbol it names \
             is still SHN_UNDEF and not weak, that is an error, not a zero",
            "The message has to contain the symbol's name — a learner debugging a 200-object \
             link needs the name far more than they need your wording",
            "Report every unresolved name you find, not just the first, and exit non-zero",
            "Delete (or never create) the output file on failure: a stale a.out that a build \
             system thinks is fresh is worse than no output at all",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "an undefined function symbol names the symbol in the diagnostic",
                undefined_function,
            ),
            Test::new(
                "an undefined data symbol names the symbol in the diagnostic",
                undefined_data,
            ),
            Test::new("the failing link exits non-zero", exit_status_is_non_zero),
            Test::new(
                "a failed link leaves no runnable output behind",
                no_runnable_output,
            ),
            Test::new("two undefined symbols are both reported", two_undefined),
            Test::new(
                "an undefined symbol referenced from .data is an error too",
                undefined_from_data,
            ),
            Test::new(
                "a reference another object satisfies is not an error",
                satisfied_reference_is_fine,
            ),
            Test::new(
                "an undefined weak symbol is not an error at all",
                undefined_weak_is_fine,
            ),
        ],
    }
}

link_test!(undefined_function, |ctx| {
    let a = caller("never printed\n", "nosuch_function")?;
    let run = ctx.link_fails(
        &Link::new()
            .object("a.o", a)
            .label("an object calling a function nothing defines"),
        "linking an object whose call has no definition anywhere",
    )?;
    let mut c = Check::new("the diagnostic for an unresolved call");
    c.mentions("linker.stderr", "nosuch_function", &run.diagnostics());
    if !c.ok() {
        c.note(
            "the wording is yours; the name is not optional — say which symbol could not be \
             found",
        );
        c.block("linker command", run.output.command_line());
    }
    c.finish()
});

link_test!(undefined_data, |ctx| {
    let a = read_global_and_exit("nosuch_datum")?;
    let run = ctx.link_fails(
        &Link::new()
            .object("a.o", a)
            .label("an object loading from a name nothing defines"),
        "linking an object whose data reference has no definition",
    )?;
    let mut c = Check::new("the diagnostic for an unresolved data reference");
    c.mentions("linker.stderr", "nosuch_datum", &run.diagnostics());
    if !c.ok() {
        c.block("linker command", run.output.command_line());
    }
    c.finish()
});

link_test!(exit_status_is_non_zero, |ctx| {
    let a = caller("x\n", "missing_one")?;
    let run = ctx.link(&Link::new().object("a.o", a).label("an unresolvable link"))?;
    let mut c = Check::new("how the linker ended a link it could not complete");
    c.that(
        "linker.exit_status",
        "a non-zero exit status — `make` only notices failure through the status",
        !run.output.success(),
        run.output.status_line(),
    );
    c.ne("linker.exit_status", Some(0), run.output.code);
    c.that(
        "linker.stderr",
        "a diagnostic on stderr",
        !run.output.stderr.trim().is_empty(),
        "(empty)",
    );
    if !c.ok() {
        c.block("linker command", run.output.command_line());
        c.block("linker output", run.diagnostics());
    }
    c.finish()
});

link_test!(no_runnable_output, |ctx| {
    let a = caller("x\n", "missing_two")?;
    let run = ctx.link_fails(
        &Link::new().object("a.o", a).out("prog"),
        "linking an object with an unresolved reference",
    )?;
    let exists = run.out_path.exists();
    let runnable = exists && crate::stages::is_executable(&run.out_path);
    let mut c = Check::new("what the failed link left on disk");
    c.that(
        "output.file",
        "either no output file at all, or one that is not marked executable — a failed link \
         must never produce something the shell will run",
        !runnable,
        format!(
            "{} exists: {exists}, executable: {runnable}",
            run.out_path.display()
        ),
    );
    if exists {
        c.note(
            "GNU ld unlinks its output when the link fails; leaving a non-executable partial \
             file is the other acceptable answer",
        );
    }
    if !c.ok() {
        c.block("linker command", run.output.command_line());
        c.block("linker output", run.diagnostics());
    }
    c.finish()
});

link_test!(two_undefined, |ctx| {
    let a = two_references("alpha_missing", "beta_missing")?;
    let run = ctx.link_fails(
        &Link::new()
            .object("a.o", a)
            .label("an object with two unresolved references"),
        "linking an object with two unresolved references",
    )?;
    let text = run.diagnostics();
    let mut c = Check::new("the diagnostic for two unresolved references");
    c.mentions("linker.stderr", "alpha_missing", &text);
    c.mentions("linker.stderr", "beta_missing", &text);
    if !c.ok() {
        c.note(
            "stopping at the first unresolved name turns one build into N builds; collect \
             them all and report them together",
        );
        c.block("linker command", run.output.command_line());
    }
    c.finish()
});

link_test!(undefined_from_data, |ctx| {
    // The reference is an R_X86_64_64 in .data, not a call: still a reference.
    let mut code = Code::new();
    code.sys_exit(0);
    let a = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(
                SectionSpec::data(".data", vec![0u8; 8])
                    .align(8)
                    .reloc(Reloc::sym(0, "missing_pointee", R_X86_64_64, 0)),
            )
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("ptr", ".data", 0).object(8))
            .symbol(SymbolSpec::undefined("missing_pointee")),
    )?;
    let run = ctx.link_fails(
        &Link::new()
            .object("a.o", a)
            .label("a .data pointer to a name nothing defines"),
        "linking a .data pointer whose target is undefined",
    )?;
    let mut c = Check::new("the diagnostic for an unresolved pointer in .data");
    c.mentions("linker.stderr", "missing_pointee", &run.diagnostics());
    if !c.ok() {
        c.note("a relocation in .data is exactly as unresolvable as one in .text");
        c.block("linker command", run.output.command_line());
    }
    c.finish()
});

link_test!(satisfied_reference_is_fine, |ctx| {
    // The control: the very same reference, with the definition supplied.
    let a = caller("resolved\n", "supplied")?;
    let b = callee_returning("supplied", 23)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("the same reference, satisfied"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "resolved\n", 23)?;
    Ok(())
});

link_test!(undefined_weak_is_fine, |ctx| {
    // STB_WEAK + SHN_UNDEF means "if nobody defines it, it is zero" — never an error.
    let mut code = Code::new();
    code.mov_r64_symbol_addr32s(Reg::Rax, "maybe_absent", 0);
    code.sys_exit(0);
    let a = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::weak_undefined("maybe_absent")),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .label("an object whose only unresolved name is weak"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "", 0)?;
    let mut c = Check::new("what the linker did with the unresolved weak symbol");
    if let Some(s) = linked.elf.symbol("maybe_absent") {
        c.observe(
            "output.symtab['maybe_absent'].st_value",
            format!("0x{:x}", s.value),
        );
        c.observe("output.symtab['maybe_absent'].st_shndx", s.shndx);
    }
    c.note("stage 20 pins down the value: an unresolved weak symbol is 0");
    c.finish()
});

/// An object whose `_start` loads the four bytes at `name` — which nothing defines — and
/// exits with them.
fn read_global_and_exit(name: &str) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.mov_r32_rip(Reg::Rax, name, 0);
    code.sys_exit_eax();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::undefined(name)),
    )
}

/// An object that calls two undefined functions in a row, so one link has two unresolved
/// names to report.
fn two_references(first: &str, second: &str) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "message", 2);
    code.call(first);
    code.call(second);
    code.sys_exit_eax();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"hi".to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::local("message", ".rodata", 0).object(2))
            .symbol(SymbolSpec::undefined(first))
            .symbol(SymbolSpec::undefined(second)),
    )
}

/// Worked examples: the input that must be refused, and the one letter of difference that
/// makes it acceptable.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::error("A call with nothing to call", "ld -o prog a.o", || {
            caller("never printed\n", "nosuch_function").map_err(|f| f.messages.join("; "))
        })
        .request(
            "a.o alone: its .symtab declares `nosuch_function` with st_shndx = SHN_UNDEF, and \
             .rela.text has an R_X86_64_PLT32 against it. No other input is given, so no \
             definition can ever arrive",
        )
        .response(
            "The linker exits non-zero with a message naming `nosuch_function` on stderr, and \
             leaves no executable behind — GNU ld prints `undefined reference to \
             `nosuch_function'` and unlinks its output",
        )
        .note(
            "Patching an unresolved call with zero produces a binary that jumps to address 0 \
             and dies with SIGSEGV thousands of instructions later. Failing here is the whole \
             point of the stage.",
        ),
        ExampleSpec::object(
            "A weak reference nothing satisfies",
            "ld -o prog a.o",
            || {
                let mut code = Code::new();
                code.mov_r64_symbol_addr32s(Reg::Rax, "maybe_absent", 0);
                code.sys_exit(0);
                build(
                    ObjectBuilder::new()
                        .section(text_of(&code))
                        .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
                        .symbol(SymbolSpec::weak_undefined("maybe_absent")),
                )
                .map_err(|f| f.messages.join("; "))
            },
        )
        .request(
            "the same shape of input, but `maybe_absent` is STB_WEAK as well as SHN_UNDEF: \
             the object is saying `use it if it is there`",
        )
        .response(
            "No error. The relocation is computed with S = 0, the link succeeds, and the \
             program runs",
        )
        .note(
            "The binding half of st_info is the whole difference between a failed build and a \
             successful one. Read it before you decide a reference is an error.",
        )
        .runs("", 0),
    ]
}
