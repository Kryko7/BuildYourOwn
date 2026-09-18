//! Stage 10 — The entry point.
//!
//! `e_entry` is the address of the first instruction the kernel jumps to, and a linker gets
//! it right by looking `_start` up in the symbol table it has just built — not by assuming
//! it is at the start of `.text`, not by assuming it came from the first input file. The
//! sharpest test here does not look at addresses at all: whatever address the linker chose,
//! the *bytes* at `e_entry` have to be the bytes the input's `.text` had at `_start`.

use crate::asm::{Code, Reg, STDOUT};
use crate::assert::{Check, Failure};
use crate::elf::read::{Elf, Segment};
use crate::elf::write::{ObjectBuilder, SectionSpec, SymbolSpec};
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 10,
        slug: "entry_point",
        name: "The entry point",
        ext: false,
        hints: &[
            "`e_entry` is `symbol_address(\"_start\")` after resolution — look it up, never \
             assume it is the base of `.text` or the first byte of the first input",
            "`_start` can live in any input, at any offset in any section, with other \
             symbols in front of it; the address is section base plus `st_value`",
            "`-e name` replaces the symbol that is looked up, and nothing else: the layout, \
             the sections and every other address stay exactly as they were",
            "The kernel jumps straight to `e_entry` with no return address on the stack, so \
             `_start` must never `ret` — it exits with a syscall",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "e_entry is the address _start has in the output's own symbol table",
                entry_is_start_address,
            ),
            Test::new(
                "the bytes at e_entry are the input's first .text bytes",
                bytes_at_entry_are_the_input_text,
            ),
            Test::new(
                "the entry point sits in an executable segment",
                entry_segment_is_exec,
            ),
            Test::new(
                "_start in the second object is still found",
                start_in_the_second_object,
            ),
            Test::new(
                "_start at a non-zero offset in its section is found",
                start_at_an_offset,
            ),
            Test::new(
                "a symbol defined before _start does not become the entry point",
                a_symbol_in_front_of_start,
            ),
            Test::new("-e names a different entry symbol", dash_e_moves_the_entry),
            Test::new(
                "the program really begins at e_entry",
                the_program_starts_there,
            ),
        ],
    }
}

link_test!(entry_is_start_address, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", print_and_exit("entry\n", 0)?))?;
    assert_entry_is(&linked, DEFAULT_ENTRY)?;
    let mut c = Check::new("e_entry against the output's symbol table");
    c.ne("output.e_entry", 0u64, linked.elf.entry);
    c.finish()?;
    ctx.expect_output(&linked, "entry\n", 0)?;
    Ok(())
});

link_test!(bytes_at_entry_are_the_input_text, |ctx| {
    let obj = exit_only(29)?;
    let want = input_text(&obj)?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj))?;
    compare_entry_bytes(&linked.elf, &want, "the whole of the input's .text")?;
    ctx.expect_output(&linked, "", 29)?;
    Ok(())
});

link_test!(entry_segment_is_exec, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", print_and_exit("x\n", 0)?))?;
    let exe = &linked.elf;
    let seg = exe.segment_at(exe.entry);
    let mut c = Check::new("the segment e_entry falls in");
    c.that(
        "output.e_entry",
        "inside a PT_LOAD carrying PF_X",
        seg.map(Segment::executable).unwrap_or(false),
        match seg {
            Some(s) => s.describe(),
            None => format!("no PT_LOAD maps 0x{:x}", exe.entry),
        },
    );
    if !c.ok() {
        c.block("output program headers", exe.program_header_table());
    }
    c.finish()?;
    ctx.expect_output(&linked, "x\n", 0)?;
    Ok(())
});

link_test!(start_in_the_second_object, |ctx| {
    let first = callee_returning("helper", 3)?;
    let second = exit_only(19)?;
    let want = input_text(&second)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", first)
            .object("b.o", second)
            .label("a helper object followed by the one that defines _start"),
    )?;
    assert_entry_is(&linked, DEFAULT_ENTRY)?;
    compare_entry_bytes(
        &linked.elf,
        &want,
        "the second input's .text, not the first's",
    )?;
    ctx.expect_output(&linked, "", 19)?;
    Ok(())
});

link_test!(start_at_an_offset, |ctx| {
    let (obj, offset) = start_after_padding(23)?;
    let text = input_text(&obj)?;
    let want = text[offset as usize..].to_vec();
    let linked = ctx.link_ok(&Link::new().object("a.o", obj))?;
    assert_entry_is(&linked, DEFAULT_ENTRY)?;
    compare_entry_bytes(
        &linked.elf,
        &want,
        &format!("the input's .text from offset {offset} on"),
    )?;
    let mut c = Check::new("what st_value contributed to e_entry");
    c.that(
        "output.e_entry",
        "not the base of the text segment: _start is not the first byte of .text",
        linked
            .elf
            .section(".text")
            .map(|s| s.addr != linked.elf.entry)
            .unwrap_or(true),
        format!("entry 0x{:x}", linked.elf.entry),
    );
    c.finish()?;
    ctx.expect_output(&linked, "", 23)?;
    Ok(())
});

link_test!(a_symbol_in_front_of_start, |ctx| {
    let (obj, offset) = start_after_padding(31)?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj))?;
    let exe = &linked.elf;
    let before = exe.symbol_address("before").map_err(|e| {
        linked.attach(Failure::new(
            crate::assert::FailureKind::MalformedOutput,
            format!("the output should still name 'before': {e}"),
        ))
    })?;
    let start = linked.address_of(DEFAULT_ENTRY)?;
    let mut c = Check::new("which of two symbols in one .text the entry point is");
    c.addr_eq("output.e_entry", start, exe.entry);
    c.ne("output.e_entry", before, exe.entry);
    c.addr_eq(
        "output.symbol['_start'] - output.symbol['before']",
        offset,
        start - before,
    );
    c.note(
        "'before' is the lower address and would be what a linker that used 'the first \
         symbol in .text' or 'the base of .text' picked",
    );
    c.finish()?;
    ctx.expect_output(&linked, "", 31)?;
    Ok(())
});

link_test!(dash_e_moves_the_entry, |ctx| {
    let obj = two_entries(1, 37)?;
    let default = ctx.link_ok(&Link::new().object("a.o", obj.clone()).out("default"))?;
    ctx.expect_output(&default, "", 1)?;

    let moved = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .entry("other")
            .out("moved")
            .label("the same object with -e other"),
    )?;
    let want = moved.address_of("other")?;
    let mut c = Check::new("what -e did to e_entry");
    c.addr_eq("output.e_entry", want, moved.elf.entry);
    c.ne("output.e_entry", default.elf.entry, moved.elf.entry);
    c.finish()?;
    ctx.expect_output(&moved, "", 37)?;
    Ok(())
});

link_test!(the_program_starts_there, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", print_and_exit("first\n", 41)?))?;
    assert_runnable_layout(&linked)?;
    assert_entry_is(&linked, DEFAULT_ENTRY)?;
    ctx.expect_output(&linked, "first\n", 41)?;
    ctx.note(
        "leg one and leg two agree: the kernel jumped to the address the symbol table says \
         _start has, and the first instruction it ran was the first instruction of .text",
    );
    Ok(())
});

/// The `.text` bytes of an object the suite just built.
fn input_text(object: &[u8]) -> Result<Vec<u8>, Failure> {
    let elf = Elf::parse(object)
        .map_err(|e| Failure::harness(format!("the suite's own object does not parse: {e}")))?;
    let data = elf
        .section_data(".text")
        .map_err(|e| Failure::harness(format!("the suite's own object has no .text: {e}")))?;
    Ok(data.to_vec())
}

/// Assert that the bytes the output maps at `e_entry` are `want`.
///
/// This is the invariant that does not care about layout: wherever the linker put the code,
/// the instruction stream starting at the entry point is the one the input carried.
fn compare_entry_bytes(exe: &Elf, want: &[u8], what: &str) -> Result<(), Failure> {
    let mut c = Check::new(format!("the bytes the output maps at e_entry — {what}"));
    match exe.read_at_vaddr(exe.entry, want.len() as u64) {
        Ok(got) => {
            c.bytes_eq("output[e_entry..]", want, got);
        }
        Err(e) => {
            c.that(
                "output[e_entry..]",
                "readable through some PT_LOAD",
                false,
                e.to_string(),
            );
        }
    }
    if !c.ok() {
        c.note(format!("e_entry is 0x{:x}", exe.entry));
        c.block("output program headers", exe.program_header_table());
        c.block("output section headers", exe.section_header_table());
    }
    c.finish()
}

/// One object whose `.text` holds a six-byte function called `before`, then `_start`.
///
/// Returns the object and the offset `_start` sits at, so a test can slice the input's own
/// `.text` and compare it with what the output maps at `e_entry`.
fn start_after_padding(status: u32) -> Result<(Vec<u8>, u64), Failure> {
    let mut code = Code::new();
    code.mov_r32_imm32(Reg::Rax, 3);
    code.ret();
    let at = code.len();
    code.sys_exit(status);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global("before", ".text", 0).func())
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", at).func()),
    )?;
    Ok((obj, at))
}

/// One object defining two possible entry points: `_start` exits `a`, `other` exits `b`.
fn two_entries(a: u32, b: u32) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_exit(a);
    let at = code.len();
    code.sys_exit(b);
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"e\n".to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("other", ".text", at).func()),
    )
}

/// An object that prints and exits, kept here so the second example can name it.
fn printing_entry() -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "message", 6);
    code.sys_exit(41);
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"first\n".to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::local("message", ".rodata", 0).object(6)),
    )
}

/// Worked examples: where the first instruction comes from.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "_start behind another function",
            "ld -o prog padded.o",
            || {
                start_after_padding(23)
                    .map(|(bytes, _)| bytes)
                    .map_err(|f| f.messages.join("; "))
            },
        )
        .request(
            "padded.o: one .text holding `before` (mov eax, 3; ret) at offset 0 and `_start` \
             (exit(23)) at offset 6, both global STT_FUNC",
        )
        .response(
            "e_entry equal to the address of _start — six bytes past the start of the text \
             the linker laid out, not the base of .text — and the twelve bytes mapped there \
             equal to the input's .text[6..18]",
        )
        .note(
            "A linker that sets e_entry to the address of .text runs `before` first, falls \
             into `_start` after the `ret`, and appears to work — until the day something \
             other than a `ret` is in front of the entry point.",
        )
        .runs("", 23),
        ExampleSpec::object("The entry point that prints", "ld -o prog first.o", || {
            printing_entry().map_err(|f| f.messages.join("; "))
        })
        .request(
            "first.o: .text writes six bytes of .rodata to stdout and exits 41; one \
             R_X86_64_PC32 for the lea that finds the string",
        )
        .response(
            "e_entry is the address of _start, the entry's PT_LOAD carries PF_X, and running \
             the file prints `first` and exits 41 — leg one and leg two agreeing",
        )
        .note(
            "The kernel jumps to e_entry with no return address on the stack. `_start` that \
             ends in `ret` jumps to whatever the stack happened to hold, which on Linux is \
             argc.",
        )
        .runs("first\n", 41),
    ]
}
