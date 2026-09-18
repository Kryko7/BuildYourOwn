//! Stage 23 — The entry symbol and `-e`.
//!
//! `e_entry` is the one address in the file the kernel actually uses: after `execve` maps the
//! segments it jumps straight there, with no C runtime to arrange anything first. By default
//! the address comes from the symbol `_start`; `-e name` (or `--entry=name`) picks another.
//! The stage is small, but it is the point where symbol resolution stops being bookkeeping
//! and starts deciding what the program does.

use crate::asm::{Code, Reg, STDOUT};
use crate::assert::{Check, Failure};
use crate::elf::write::{ObjectBuilder, SectionSpec, SymbolSpec};
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// The length of one `exit(n)` sequence: `mov eax, 60`, `mov edi, n`, `syscall`.
const EXIT_SEQUENCE_LEN: u64 = 12;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 23,
        slug: "entry_symbol",
        name: "The entry symbol and -e",
        ext: false,
        hints: &[
            "Resolve the entry name through the same global symbol table as everything else, \
             then put its final address — section address plus st_value — in e_entry",
            "`-e name` and `--entry=name` are the same option; with neither, the name is \
             `_start`, and it may be defined in any input, not just the first",
            "The entry symbol need not sit at offset 0 of its section: st_value is part of the \
             answer and dropping it points e_entry at whatever happens to be first",
            "Decide what to do when the entry name is undefined and say so — GNU ld warns and \
             carries on with a defaulted address, which is friendly but easy to miss; failing \
             outright is also defensible",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new("with no -e the entry point is _start", default_entry),
            Test::new("-e names another entry point", dash_e_moves_the_entry),
            Test::new(
                "--entry= is the same option in its long spelling",
                long_entry_spelling,
            ),
            Test::new(
                "the program really starts at the chosen entry",
                entry_decides_what_runs,
            ),
            Test::new(
                "an entry symbol away from offset 0 keeps its st_value",
                entry_not_at_offset_zero,
            ),
            Test::new(
                "the entry symbol may be defined in the second object",
                entry_in_the_second_object,
            ),
            Test::new("a missing _start is diagnosed", missing_start_is_diagnosed),
            Test::new(
                "an -e naming nothing is diagnosed",
                missing_named_entry_is_diagnosed,
            ),
        ],
    }
}

link_test!(default_entry, |ctx| {
    let obj = two_entries(DEFAULT_ENTRY, 31, "other", 62)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("an object with two candidate entry points and no -e"),
    )?;
    assert_runnable_layout(&linked)?;
    assert_entry_is(&linked, DEFAULT_ENTRY)?;
    // 31 is _start's exit; 62 belongs to the symbol twelve bytes later.
    ctx.expect_output(&linked, "", 31)?;
    Ok(())
});

link_test!(dash_e_moves_the_entry, |ctx| {
    let obj = two_entries(DEFAULT_ENTRY, 31, "other", 62)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .entry("other")
            .label("the same object with -e other"),
    )?;
    assert_runnable_layout(&linked)?;
    assert_entry_is(&linked, "other")?;
    ctx.expect_output(&linked, "", 62)?;
    let mut c = Check::new("that -e really moved e_entry");
    let start = linked.address_of(DEFAULT_ENTRY)?;
    c.ne("output.e_entry", start, linked.elf.entry);
    if !c.ok() {
        c.note(format!(
            "_start is at 0x{start:x} and is not what -e asked for"
        ));
        c.block("linker command", linked.run.output.command_line());
    }
    c.finish()
});

link_test!(long_entry_spelling, |ctx| {
    let obj = two_entries(DEFAULT_ENTRY, 31, "other", 62)?;
    let short = ctx.link_ok(
        &Link::new()
            .object("a.o", obj.clone())
            .entry("other")
            .label("-e other"),
    )?;
    let long = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .entry_long("other")
            .out("long")
            .label("--entry=other"),
    )?;
    assert_runnable_layout(&long)?;
    assert_entry_is(&long, "other")?;
    ctx.expect_output(&long, "", 62)?;
    let mut c = Check::new("that the two spellings mean the same thing");
    c.addr_eq(
        "output.e_entry",
        short.elf.entry - short.address_of(DEFAULT_ENTRY)?,
        long.elf.entry - long.address_of(DEFAULT_ENTRY)?,
    );
    c.note(
        "the two links may choose different addresses; what has to match is which symbol \
         e_entry lands on, measured from _start",
    );
    c.finish()
});

link_test!(entry_decides_what_runs, |ctx| {
    // One object, two entry points, two links: the only difference is -e, and the programs
    // print different things and exit differently.
    let obj = printing_entries("alpha\n", 11, "beta\n", 22)?;
    let alpha = ctx.link_ok(
        &Link::new()
            .object("a.o", obj.clone())
            .entry("alpha")
            .out("alpha")
            .label("-e alpha"),
    )?;
    ctx.expect_output(&alpha, "alpha\n", 11)?;
    let beta = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .entry("beta")
            .out("beta")
            .label("-e beta"),
    )?;
    assert_runnable_layout(&beta)?;
    ctx.expect_output(&beta, "beta\n", 22)?;
    let mut c = Check::new("that the entry symbol chose which code ran");
    c.that(
        "output.e_entry",
        "two links of one object whose entry points differ",
        alpha.elf.entry != beta.elf.entry
            || alpha.address_of("alpha")? != alpha.address_of("beta")?,
        format!(
            "alpha entry 0x{:x}, beta entry 0x{:x}",
            alpha.elf.entry, beta.elf.entry
        ),
    );
    c.finish()
});

link_test!(entry_not_at_offset_zero, |ctx| {
    let obj = two_entries(DEFAULT_ENTRY, 31, "late", 62)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .entry("late")
            .label("an entry symbol twelve bytes into .text"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "", 62)?;
    let start = linked.address_of(DEFAULT_ENTRY)?;
    let late = linked.address_of("late")?;
    let mut c = Check::new("that st_value was added to the section address");
    c.addr_eq(
        "output.symtab['late'].st_value",
        start + EXIT_SEQUENCE_LEN,
        late,
    );
    c.addr_eq("output.e_entry", late, linked.elf.entry);
    if !c.ok() {
        c.note(
            "`late` is at st_value 12 of the same .text as _start; an e_entry equal to \
             _start's address means st_value was dropped",
        );
        c.block("output section headers", linked.elf.section_header_table());
    }
    c.finish()
});

link_test!(entry_in_the_second_object, |ctx| {
    // The first object defines no entry at all; _start arrives with the second.
    let a = callee_returning("helper", 3)?;
    let b = print_and_exit("second object\n", 46)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("_start defined by the second input"),
    )?;
    assert_runnable_layout(&linked)?;
    assert_entry_is(&linked, DEFAULT_ENTRY)?;
    ctx.expect_output(&linked, "second object\n", 46)?;
    let mut c = Check::new("that e_entry is not simply the first address in the image");
    let helper = linked.address_of("helper")?;
    c.ne("output.e_entry", helper, linked.elf.entry);
    if !c.ok() {
        c.note(
            "`helper` is the first thing in the first object; an e_entry pointing at it means \
             the entry was taken from the layout rather than from the symbol table",
        );
        c.block("output section headers", linked.elf.section_header_table());
    }
    c.finish()
});

link_test!(missing_start_is_diagnosed, |ctx| {
    // Nothing in this link defines `_start`, and no -e was given.
    let obj = exit_only_named("main", 0)?;
    let run = ctx.link(
        &Link::new()
            .object("a.o", obj)
            .label("an object with no _start and no -e"),
    )?;
    let said_so = run.diagnostics().contains(DEFAULT_ENTRY);
    let mut c = Check::new("what the linker did about the missing entry symbol");
    c.that(
        "linker",
        "either a non-zero exit or a diagnostic naming the entry symbol — the user has to \
         learn that the entry was not found",
        !run.output.success() || said_so,
        format!(
            "{}; stderr: {}",
            run.output.status_line(),
            run.diagnostics()
        ),
    );
    if !c.ok() {
        c.block("linker command", run.output.command_line());
    }
    c.finish()?;
    if run.output.success() {
        ctx.note(
            "this linker only warns about a missing _start and exits 0, defaulting e_entry to \
             the start of the text — GNU ld does exactly that, so the test requires a \
             diagnostic naming the symbol rather than a failed link",
        );
    }
    Ok(())
});

link_test!(missing_named_entry_is_diagnosed, |ctx| {
    // -e names a symbol no input defines.
    let obj = print_and_exit("entry\n", 0)?;
    let run = ctx.link(
        &Link::new()
            .object("a.o", obj)
            .entry("nosuch_entry")
            .label("-e naming a symbol nothing defines"),
    )?;
    let said_so = run.diagnostics().contains("nosuch_entry");
    let mut c = Check::new("what the linker did about the unresolvable -e");
    c.that(
        "linker",
        "either a non-zero exit or a diagnostic naming `nosuch_entry`",
        !run.output.success() || said_so,
        format!(
            "{}; stderr: {}",
            run.output.status_line(),
            run.diagnostics()
        ),
    );
    if !c.ok() {
        c.block("linker command", run.output.command_line());
    }
    c.finish()?;
    if run.output.success() {
        ctx.note(
            "an -e naming nothing is a warning here too, not an error; the invariant the test \
             holds both linkers to is that the name appears in the diagnostic",
        );
    }
    Ok(())
});

// ---------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------

/// One `.text` with two independent `exit(n)` sequences and a symbol on each — the second
/// twelve bytes in, so its `st_value` is not zero.
fn two_entries(
    first: &str,
    first_status: u32,
    second: &str,
    second_status: u32,
) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_exit(first_status);
    let second_at = code.len();
    code.sys_exit(second_status);
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(first, ".text", 0).func())
            .symbol(SymbolSpec::global(second, ".text", second_at).func()),
    )
}

/// The same idea, but each entry prints its own string first, so the two roads are visible
/// on stdout as well as in the exit status.
fn printing_entries(
    first_message: &str,
    first_status: u32,
    second_message: &str,
    second_status: u32,
) -> Result<Vec<u8>, Failure> {
    let mut rodata = first_message.as_bytes().to_vec();
    rodata.extend_from_slice(second_message.as_bytes());
    let mut code = Code::new();
    code.sys_write(STDOUT, "first_message", first_message.len() as u32);
    code.sys_exit(first_status);
    let second_at = code.len();
    code.sys_write(STDOUT, "second_message", second_message.len() as u32);
    code.sys_exit(second_status);
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", rodata))
            .symbol(SymbolSpec::global("alpha", ".text", 0).func())
            .symbol(SymbolSpec::global("beta", ".text", second_at).func())
            .symbol(
                SymbolSpec::local("first_message", ".rodata", 0).object(first_message.len() as u64),
            )
            .symbol(
                SymbolSpec::local("second_message", ".rodata", first_message.len() as u64)
                    .object(second_message.len() as u64),
            ),
    )
}

/// An object whose `_start` returns a value in eax instead of exiting — used only to give an
/// example a first input that defines no entry point.
fn callee_returning_named(name: &str, value: u32) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.mov_r32_imm32(Reg::Rax, value);
    code.ret();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(name, ".text", 0).func()),
    )
}

/// Worked examples: choosing between two entry points, and the input with none.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "One object, two entry points",
            "ld -o prog a.o        # and: ld -e other -o prog a.o",
            || two_entries(DEFAULT_ENTRY, 31, "other", 62).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "a.o's .text is 24 bytes: two complete `exit(n)` sequences back to back. `_start` \
             is STB_GLOBAL STT_FUNC at st_value 0 and `other` is the same at st_value 12 — one \
             section, two symbols, two different values",
        )
        .response(
            "With no -e, e_entry is the address of `_start` and the program exits 31. With \
             `-e other`, e_entry is that same address plus 12 and the program exits 62. \
             Nothing else in the file changes",
        )
        .note(
            "e_entry is section address + st_value. A linker that sets e_entry to the address \
             of the section holding the entry symbol passes the default case and fails this \
             one.",
        )
        .runs("", 31),
        ExampleSpec::object(
            "An input that defines no entry at all",
            "ld -o prog a.o b.o",
            || callee_returning_named("helper", 3).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "a.o defines only `helper`. It is the *first* input, so its .text is very likely \
             the first thing in the image, but it contains nothing that could be an entry \
             point",
        )
        .response(
            "e_entry has to come from the symbol table, not from the layout: `_start` is found \
             in b.o and e_entry is its address, several bytes past where a.o's code sits",
        )
        .note(
            "`e_entry = address of the first executable section` looks right on every \
             single-object test and is wrong the moment a program has two files.",
        ),
        ExampleSpec::text(
            "An entry symbol nothing defines",
            "ld -e nosuch_entry -o prog a.o",
        )
        .request(
            "the same well-formed object, but -e names a symbol no input defines. There is \
             nothing for e_entry to be",
        )
        .response(
            "The user must be told, and the message must contain `nosuch_entry`. GNU ld prints \
             `warning: cannot find entry symbol nosuch_entry; defaulting to 0000000000401000` \
             and still exits 0; exiting non-zero instead is equally acceptable, and the suite \
             requires one or the other",
        )
        .note(
            "This is the one place in the whole track where GNU ld's behaviour is a warning \
             rather than an error, so the test asserts the invariant both answers share: the \
             name is named.",
        ),
    ]
}
