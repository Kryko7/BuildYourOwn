//! Stage 29 — Relocations against section symbols.
//!
//! Not every relocation names a symbol a human wrote. An anonymous string literal has no
//! name, so the compiler emits `R_X86_64_PC32` against the `STT_SECTION` symbol of
//! `.rodata` with the literal's offset as the addend. The same trick covers jump tables,
//! `static` data a local symbol was folded away from, and anything else the assembler could
//! resolve within its own object.
//!
//! The rule is short: for a section symbol `S` is the address the *contributing section from
//! that object* was placed at, and the addend is the offset inside it. The trap is that
//! several objects contribute `.rodata` to one output `.rodata`, so "the address of
//! `.rodata`" is not one number — it is one number per input.

use crate::asm::{Code, Reg, STDOUT};
use crate::assert::Check;
use crate::elf::write::{ObjectBuilder, RelTarget, Reloc, SectionSpec, SymbolSpec};
use crate::elf::{R_X86_64_64, R_X86_64_PC32};
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 29,
        slug: "section_symbols",
        name: "Relocations against section symbols",
        ext: true,
        hints: &[
            "An STT_SECTION symbol's value is the address of that object's contribution to \
             the output section, so resolve it to where you placed that input section — not \
             to the start of the merged output section",
            "The offset inside the section is carried entirely in r_addend, which is why \
             these relocations almost always have a large addend and no name",
            "Two inputs both contributing .rodata have two different section symbols with \
             the same name; keying anything on the section's name instead of on the input \
             section is how the second object's literals end up pointing into the first",
            "A section symbol usually has an empty st_name, so a symbol table keyed by name \
             needs somewhere else to put them",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "an anonymous literal is reached through the .rodata section symbol",
                literal_through_a_section_symbol,
            )
            .ext(),
            Test::new(
                "several literals at different offsets of one section",
                several_literals,
            )
            .ext(),
            Test::new(
                "a zero addend on a section symbol is the start of the section",
                zero_offset_is_the_section_start,
            )
            .ext(),
            Test::new("a section symbol of .data", data_section_symbol).ext(),
            Test::new(
                "an R_X86_64_64 against a section symbol",
                pointer_to_a_section_symbol,
            )
            .ext(),
            Test::new(
                "a PC32 against the .text section symbol reaches a function",
                call_through_the_text_section_symbol,
            )
            .ext(),
            Test::new(
                "two objects each have their own .rodata section symbol",
                two_objects_two_section_symbols,
            )
            .ext(),
            Test::new(
                "a section symbol and a named symbol reach the same byte",
                both_routes_agree,
            )
            .ext(),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

link_test!(literal_through_a_section_symbol, |ctx| {
    let mut code = Code::new();
    let site_offset = code.len() + WRITE_LEA_DISP_OFFSET;
    code.sys_write_section(STDOUT, ".rodata", 3, 4);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"no\nyes\n".to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("ro_base", ".rodata", 0).object(7)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("a literal addressed as .rodata + 3"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "yes\n", 0)?;
    let mut c = Check::new("a PC32 whose symbol is a section");
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[lea].disp32",
        linked.address_of(DEFAULT_ENTRY)? + site_offset,
        linked.address_of("ro_base")?,
        -4 + 3,
    );
    c.note(
        "'ro_base' is a global the fixture pinned to offset 0 of the same .rodata, so the \
         expectation names no address of its own",
    );
    c.finish()
});

link_test!(several_literals, |ctx| {
    // Three anonymous literals packed into one .rodata, printed back to front.
    let pool = b"first\nsecond\nthird\n".to_vec();
    let mut code = Code::new();
    let mut sites = Vec::new();
    for (offset, len) in [(13usize, 6usize), (6, 7), (0, 6)] {
        sites.push((code.len() + WRITE_LEA_DISP_OFFSET, offset));
        code.sys_write_section(STDOUT, ".rodata", offset as i64, len as u32);
    }
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", pool.clone()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("ro_base", ".rodata", 0).object(pool.len() as u64)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("three literals in one section"),
    )?;
    ctx.expect_output(&linked, "third\nsecond\nfirst\n", 0)?;
    let base = linked.address_of(DEFAULT_ENTRY)?;
    let ro = linked.address_of("ro_base")?;
    let mut c = Check::new("three relocations sharing one section symbol");
    for (site, offset) in sites {
        assert_pc32(
            &mut c,
            &linked.elf,
            &format!("output.text[lea .rodata+{offset}].disp32"),
            base + site,
            ro,
            -4 + offset as i64,
        );
    }
    c.note(
        "the three relocations name the same symbol and differ only in their addends, which \
         is exactly what a compiler emits for three string literals in one translation unit",
    );
    c.finish()
});

link_test!(zero_offset_is_the_section_start, |ctx| {
    let mut code = Code::new();
    let site_offset = code.len() + WRITE_LEA_DISP_OFFSET;
    code.sys_write_section(STDOUT, ".rodata", 0, 6);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"start\n".to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("ro_base", ".rodata", 0).object(6)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("a section symbol with no offset"),
    )?;
    ctx.expect_output(&linked, "start\n", 0)?;
    let mut c = Check::new("a section symbol with a zero offset");
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[lea].disp32",
        linked.address_of(DEFAULT_ENTRY)? + site_offset,
        linked.address_of("ro_base")?,
        -4,
    );
    c.finish()
});

link_test!(data_section_symbol, |ctx| {
    let mut code = Code::new();
    let site_offset = code.len() + WRITE_LEA_DISP_OFFSET;
    code.sys_write_section(STDOUT, ".data", 4, 5);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::data(".data", b"skiphere\n".to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("data_base", ".data", 0).object(9)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("a section symbol of .data"),
    )?;
    ctx.expect_output(&linked, "here\n", 0)?;
    let mut c = Check::new("a section symbol that is not .rodata");
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[lea].disp32",
        linked.address_of(DEFAULT_ENTRY)? + site_offset,
        linked.address_of("data_base")?,
        -4 + 4,
    );
    c.note(
        "nothing about section symbols is specific to .rodata; every allocatable section of \
         an object gets one",
    );
    c.finish()
});

link_test!(pointer_to_a_section_symbol, |ctx| {
    let mut code = Code::new();
    code.mov_r64_rip(Reg::Rsi, "ptr", R_X86_64_PC32, -4);
    code.sys_write_rsi(STDOUT, 4);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"skipkeep\n".to_vec()))
            .section(
                SectionSpec::data(".data", vec![0u8; 8])
                    .align(8)
                    .reloc(Reloc::section(0, ".rodata", R_X86_64_64, 4)),
            )
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("ro_base", ".rodata", 0).object(9))
            .symbol(SymbolSpec::global("ptr", ".data", 0).object(8)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("an eight-byte pointer to .rodata + 4"),
    )?;
    ctx.expect_output(&linked, "keep", 0)?;
    let mut c = Check::new("an R_X86_64_64 whose symbol is a section");
    assert_abs64(
        &mut c,
        &linked.elf,
        "output.data[ptr]",
        linked.address_of("ptr")?,
        linked.address_of("ro_base")?,
        4,
    );
    c.finish()
});

link_test!(call_through_the_text_section_symbol, |ctx| {
    let mut helper = Code::new();
    helper.mov_r32_imm32(Reg::Rax, 29);
    helper.ret();
    let mut code = Code::new();
    code.raw(&[0xe8]); // call rel32
    let site_offset = code.len();
    code.reloc_here(RelTarget::Section(".text".to_string()), R_X86_64_PC32, 0);
    code.raw(&0u32.to_le_bytes());
    code.sys_exit_eax();
    let helper_offset = code.len();
    code.append(&helper);
    // The addend is the helper's offset inside .text, minus the four the encoding needs.
    for r in code.relocs.iter_mut() {
        if matches!(r.target, RelTarget::Section(ref s) if s == ".text") {
            r.addend = helper_offset as i64 - 4;
        }
    }
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func()),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("a call addressed as .text + offset"),
    )?;
    ctx.expect_output(&linked, "", 29)?;
    let mut c = Check::new("a call whose symbol is the .text section itself");
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[call].disp32",
        linked.address_of(DEFAULT_ENTRY)? + site_offset,
        linked.address_of(DEFAULT_ENTRY)?,
        helper_offset as i64 - 4,
    );
    c.note(
        "_start sits at offset 0 of this object's .text, so its address is that section's \
         base — which is what S means for the .text section symbol",
    );
    c.finish()
});

link_test!(two_objects_two_section_symbols, |ctx| {
    // Both objects contribute .rodata, and both address it at offsets of their own.
    let mut first = Code::new();
    let first_site = first.len() + WRITE_LEA_DISP_OFFSET;
    first.sys_write_section(STDOUT, ".rodata", 2, 4);
    first.call("second");
    first.sys_exit(0);
    let a = build(
        ObjectBuilder::new()
            .section(text_of(&first))
            .section(SectionSpec::rodata(".rodata", b"XXone\n".to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("ro_a", ".rodata", 0).object(6))
            .symbol(SymbolSpec::undefined("second")),
    )?;
    let mut tail = Code::new();
    let second_site = tail.len() + WRITE_LEA_DISP_OFFSET;
    tail.sys_write_section(STDOUT, ".rodata", 1, 4);
    tail.ret();
    let b = build(
        ObjectBuilder::new()
            .section(text_of(&tail))
            .section(SectionSpec::rodata(".rodata", b"Ytwo\n".to_vec()))
            .symbol(SymbolSpec::global("second", ".text", 0).func())
            .symbol(SymbolSpec::global("ro_b", ".rodata", 0).object(5)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("two objects, two .rodata section symbols"),
    )?;
    ctx.expect_output(&linked, "one\ntwo\n", 0)?;
    let mut c = Check::new("two section symbols with the same name and different values");
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[a.o lea].disp32",
        linked.address_of(DEFAULT_ENTRY)? + first_site,
        linked.address_of("ro_a")?,
        -4 + 2,
    );
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[b.o lea].disp32",
        linked.address_of("second")? + second_site,
        linked.address_of("ro_b")?,
        -4 + 1,
    );
    let (a_base, b_base) = (linked.address_of("ro_a")?, linked.address_of("ro_b")?);
    c.ne("output.rodata contributions", a_base, b_base);
    c.note(
        "the two .rodata contributions land at different addresses, so a linker that \
         resolves '.rodata' by name sends one object's literals into the other's bytes",
    );
    c.finish()
});

link_test!(both_routes_agree, |ctx| {
    // The same byte reached once through a named symbol and once through the section.
    let mut code = Code::new();
    let named_site = code.len() + WRITE_LEA_DISP_OFFSET;
    code.sys_write(STDOUT, "message", 4);
    let section_site = code.len() + WRITE_LEA_DISP_OFFSET;
    code.sys_write_section(STDOUT, ".rodata", 2, 4);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"pqsame".to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("ro_base", ".rodata", 0).object(6))
            .symbol(SymbolSpec::global("message", ".rodata", 2).object(4)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("the same bytes by two routes"),
    )?;
    ctx.expect_output(&linked, "samesame", 0)?;
    let base = linked.address_of(DEFAULT_ENTRY)?;
    let mut c = Check::new("a named symbol and a section symbol pointing at one byte");
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[lea message].disp32",
        base + named_site,
        linked.address_of("message")?,
        -4,
    );
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[lea .rodata+2].disp32",
        base + section_site,
        linked.address_of("ro_base")?,
        -4 + 2,
    );
    c.addr_eq(
        "output.symbol[message]",
        linked.address_of("ro_base")? + 2,
        linked.address_of("message")?,
    );
    c.finish()
});

/// Worked examples: the anonymous string literal, which is where these come from.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "An anonymous string literal",
            "ld -o prog literal.o",
            || {
                let mut code = Code::new();
                code.sys_write_section(STDOUT, ".rodata", 3, 4);
                code.sys_exit(0);
                build(
                    ObjectBuilder::new()
                        .section(text_of(&code))
                        .section(SectionSpec::rodata(".rodata", b"no\nyes\n".to_vec()))
                        .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
                        .symbol(SymbolSpec::global("ro_base", ".rodata", 0).object(7)),
                )
                .map_err(|f| f.messages.join("; "))
            },
        )
        .request(
            "literal.o: .rodata holds two literals end to end, and the lea's relocation \
             names the STT_SECTION symbol of .rodata with r_addend 3 - 4 = -1 — there is no \
             named symbol for the literal at all",
        )
        .response(
            "S is where this object's .rodata contribution was placed, A carries the offset \
             of the literal inside it, and the program prints the second string",
        )
        .note(
            "This is what a compiler emits for every `printf(\"...\")`. A linker that has \
             not implemented section symbols cannot link a single real translation unit.",
        )
        .runs("yes\n", 0),
        ExampleSpec::text("Two objects, one output section", "ld -o prog a.o b.o")
            .request(
                "a.o and b.o both have a .rodata and both relocate against their own \
             STT_SECTION symbol for it",
            )
            .response(
                "The two contributions are concatenated into one output .rodata at two different \
             addresses, and each object's relocations resolve against its own contribution's \
             base",
            )
            .note(
                "Resolve a section symbol through the input section it belongs to. Looking the \
             name up in a map of output sections gives the right answer for the first object \
             and the wrong one for every other.",
            ),
    ]
}
