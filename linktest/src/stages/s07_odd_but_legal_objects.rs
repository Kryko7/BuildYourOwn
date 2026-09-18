//! Stage 07 — Odd but legal objects.
//!
//! The mirror image of stage 05 and 06: nothing here is malformed, it is only unlike what
//! `gcc -c hello.c` happens to emit. A section whose type you have never heard of, `.rela`
//! before the section it patches, an input that contributes no bytes at all, `sh_addralign`
//! of 0, an empty `.text`. Every one of these is a file a real toolchain can produce, and
//! every one of them has to link and run.

use crate::asm::{Code, STDOUT};
use crate::assert::{Check, Failure};
use crate::elf::read::Elf;
use crate::elf::write::{ObjectBuilder, SectionSpec, SymbolSpec};
use crate::elf::*;
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 7,
        slug: "odd_but_legal_objects",
        name: "Odd but legal objects",
        ext: true,
        hints: &[
            "A section type you do not recognise is not an error: if it is not SHF_ALLOC it \
             contributes nothing to the image, so skip it and say nothing",
            "Nothing says .rela.text comes after .text, or that .symtab is last: index the \
             section header table by number, never walk it expecting an order",
            "sh_addralign 0 and 1 both mean 'no constraint', and an empty SHF_ALLOC section \
             is legal — give it an address and move on",
            "An input that contributes no bytes at all still contributes its symbols, and an \
             input with no symbols at all is still a legal object",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "an object with nothing but a symbol table links beside a real one",
                nothing_but_a_symtab,
            )
            .ext(),
            Test::new(
                "an unknown OS-specific section type that is not SHF_ALLOC is ignored",
                unknown_section_type,
            )
            .ext(),
            Test::new(
                ".rela.text emitted before .text still relocates correctly",
                rela_before_its_target,
            )
            .ext(),
            Test::new(
                "non-allocatable .comment and .note sections are ignored",
                non_allocatable_sections,
            )
            .ext(),
            Test::new(
                "a section with sh_addralign 0 is treated as 1",
                addralign_zero,
            )
            .ext(),
            Test::new(
                "an empty .text in one object with the real code in another links and runs",
                empty_text_beside_real_code,
            )
            .ext(),
            Test::new(
                "an object with no section symbols at all links and runs",
                without_section_symbols_at_all,
            )
            .ext(),
            Test::new(
                "a relocatable object whose .text carries a non-zero sh_addr is relocated",
                nonzero_sh_addr,
            )
            .ext(),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------

/// What every program in this stage prints.
const MESSAGE: &str = "odd but legal\n";

/// The stage's program as a code block, so each fixture can wrap it differently.
fn program_code() -> Code {
    let mut code = Code::new();
    code.sys_write(STDOUT, "message", MESSAGE.len() as u32);
    code.sys_exit(0);
    code
}

/// The `.rodata` every fixture carries.
fn message_section() -> SectionSpec {
    SectionSpec::rodata(".rodata", MESSAGE.as_bytes().to_vec())
}

/// The two symbols every fixture carries.
fn message_symbols(b: ObjectBuilder) -> ObjectBuilder {
    b.symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
        .symbol(SymbolSpec::local("message", ".rodata", 0).object(MESSAGE.len() as u64))
}

/// The plain program, for the tests that only need one good input.
fn plain() -> Result<Vec<u8>, Failure> {
    let code = program_code();
    build(message_symbols(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(message_section()),
    ))
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

link_test!(nothing_but_a_symtab, |ctx| {
    // No content sections at all: just .symtab, .strtab and .shstrtab, with one absolute
    // symbol nobody references. An input may contribute nothing but names.
    let empty = build(ObjectBuilder::new().symbol(SymbolSpec::absolute("nothing_here", 42)))?;
    let parsed = Elf::parse(&empty)
        .map_err(|e| Failure::harness(format!("the suite built an object it cannot read: {e}")))?;
    let mut c = Check::new("the fixture: an object with no content sections");
    c.that(
        "input.sections",
        "no SHF_ALLOC section at all",
        parsed.sections.iter().all(|s| !s.is_alloc()),
        parsed
            .sections
            .iter()
            .map(|s| s.name.clone())
            .collect::<Vec<_>>()
            .join(" "),
    );
    c.finish()?;

    let linked = ctx.link_ok(
        &Link::new()
            .object("empty.o", empty)
            .object("a.o", plain()?)
            .label("an empty object before a real one"),
    )?;
    assert_runnable_layout(&linked)?;
    assert_entry_is(&linked, DEFAULT_ENTRY)?;
    ctx.expect_output(&linked, MESSAGE, 0)?;
    ctx.note(
        "GNU ld does refuse an object whose e_shnum is 0 while e_shoff is not — the suite \
         therefore asks only for the case a toolchain really produces: a section header \
         table with nothing allocatable in it",
    );
    Ok(())
});

link_test!(unknown_section_type, |ctx| {
    let code = program_code();
    let obj = build(message_symbols(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(message_section())
            .section(SectionSpec::new(
                ".weird",
                SHT_UNKNOWN_OS,
                0,
                vec![1, 2, 3, 4],
            )),
    ))?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("an object carrying a section of an unknown type"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, MESSAGE, 0)?;
    let mut c = Check::new("what the unknown section did to the image");
    for section in allocated_sections(&linked.elf) {
        c.that(
            &format!("output.section[{}].name", section.index),
            "not the unknown section: it is not SHF_ALLOC, so it has no place in memory",
            section.name != ".weird",
            format!("{} at 0x{:x}", section.name, section.addr),
        );
    }
    if !c.ok() {
        c.block("output section headers", linked.elf.section_header_table());
    }
    c.finish()?;
    ctx.note(
        "GNU ld copies the section through to its output without allocating it; dropping it \
         entirely is just as correct — what matters is that it is not an error and not in a \
         PT_LOAD",
    );
    Ok(())
});

link_test!(rela_before_its_target, |ctx| {
    let code = program_code();
    let obj = build(
        message_symbols(
            ObjectBuilder::new()
                .section(text_of(&code))
                .section(message_section()),
        )
        .rela_before_target(),
    )?;
    let parsed = Elf::parse(&obj)
        .map_err(|e| Failure::harness(format!("the suite built an object it cannot read: {e}")))?;
    let mut c = Check::new("the fixture: .rela.text before .text");
    let rela = parsed.section(".rela.text").map(|s| s.index);
    let text = parsed.section(".text").map(|s| s.index);
    c.that(
        "input.section_order",
        ".rela.text at a lower section index than .text",
        matches!((rela, text), (Some(r), Some(t)) if r < t),
        format!("{rela:?} vs {text:?}"),
    );
    c.finish()?;

    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("an object whose .rela.text comes first"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, MESSAGE, 0)?;

    // Leg three: the displacement the linker wrote is the one its own addresses imply.
    let start = linked.address_of(DEFAULT_ENTRY)?;
    let message = linked.address_of("message")?;
    let site = start + WRITE_LEA_DISP_OFFSET;
    let mut c = Check::new("the lea's displacement, relocated out of order");
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[lea].disp32",
        site,
        message,
        -4,
    );
    if !c.ok() {
        c.block("output section headers", linked.elf.section_header_table());
    }
    c.finish()
});

link_test!(non_allocatable_sections, |ctx| {
    let code = program_code();
    let obj = build(message_symbols(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(message_section())
            .section(SectionSpec::new(
                ".comment",
                SHT_PROGBITS,
                0,
                b"linktest 0.1\0".to_vec(),
            ))
            .section(SectionSpec::new(".note.x", SHT_NOTE, 0, Vec::new()).align(4)),
    ))?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("an object with .comment and an empty .note"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, MESSAGE, 0)?;
    let mut c = Check::new("that nothing non-allocatable reached the image");
    for section in allocated_sections(&linked.elf) {
        c.that(
            &format!("output.section[{}].flags", section.index),
            "an allocated section that the input actually asked to allocate",
            section.name != ".comment" && section.name != ".note.x",
            format!("{} at 0x{:x}", section.name, section.addr),
        );
    }
    c.finish()
});

link_test!(addralign_zero, |ctx| {
    let code = program_code();
    let obj = build(message_symbols(
        ObjectBuilder::new()
            .section(text_of(&code).align(0))
            .section(message_section().align(0)),
    ))?;
    let parsed = Elf::parse(&obj)
        .map_err(|e| Failure::harness(format!("the suite built an object it cannot read: {e}")))?;
    let mut c = Check::new("the fixture's alignments");
    c.eq(
        "input.section[.text].sh_addralign",
        0u64,
        parsed.section(".text").map(|s| s.align).unwrap_or(u64::MAX),
    );
    c.finish()?;

    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("sections with sh_addralign 0"),
    )?;
    assert_runnable_layout(&linked)?;
    assert_entry_is(&linked, DEFAULT_ENTRY)?;
    ctx.expect_output(&linked, MESSAGE, 0)?;
    ctx.note(
        "sh_addralign 0 means the same as 1; a linker that uses it as a modulus divides by \
         zero, and one that uses it as a mask aligns everything to nothing",
    );
    Ok(())
});

link_test!(empty_text_beside_real_code, |ctx| {
    let empty = build(
        ObjectBuilder::new()
            .section(SectionSpec::text(".text", Vec::new()))
            .section(SectionSpec::rodata(".rodata", Vec::new()))
            .symbol(SymbolSpec::global("filler", ".text", 0).func()),
    )?;
    for (label, first, second) in [
        ("empty first", empty.clone(), plain()?),
        ("empty second", plain()?, empty.clone()),
    ] {
        let linked = ctx.link_ok(
            &Link::new()
                .object("one.o", first)
                .object("two.o", second)
                .out(&format!("prog-{}", label.replace(' ', "-")))
                .label(label),
        )?;
        assert_runnable_layout(&linked)?;
        assert_entry_is(&linked, DEFAULT_ENTRY)?;
        ctx.expect_output(&linked, MESSAGE, 0)?;
    }
    ctx.note(
        "an empty SHF_ALLOC section still gets an address; what it must not do is displace \
         the real code or leave a zero-length hole the entry point lands in",
    );
    Ok(())
});

link_test!(without_section_symbols_at_all, |ctx| {
    let code = program_code();
    let obj = build(
        message_symbols(
            ObjectBuilder::new()
                .section(text_of(&code))
                .section(message_section()),
        )
        .without_section_symbols(),
    )?;
    let parsed = Elf::parse(&obj)
        .map_err(|e| Failure::harness(format!("the suite built an object it cannot read: {e}")))?;
    let mut c = Check::new("the fixture's symbol table");
    c.that(
        "input.symbols",
        "no STT_SECTION symbol at all",
        parsed.symbols.iter().all(|s| s.stype != STT_SECTION),
        parsed.symbols.len(),
    );
    c.finish()?;

    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("an object with no section symbols"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, MESSAGE, 0)?;
    ctx.note(
        "section symbols are a convention of assemblers, not a requirement: hand-written \
         objects often have none, and nothing here refers to one",
    );
    Ok(())
});

link_test!(nonzero_sh_addr, |ctx| {
    // sh_addr is meaningless in a relocatable object — the linker chooses addresses — but
    // nothing forbids a non-zero value, and a linker that treats it as "this section is
    // already placed" produces a binary that jumps into nowhere.
    let code = program_code();
    let mut text = text_of(&code);
    text.addr = 0x1000;
    let obj = build(message_symbols(
        ObjectBuilder::new()
            .section(text)
            .section(message_section()),
    ))?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("an input .text with sh_addr 0x1000"),
    )?;
    assert_runnable_layout(&linked)?;
    assert_entry_is(&linked, DEFAULT_ENTRY)?;
    ctx.expect_output(&linked, MESSAGE, 0)?;
    let mut c = Check::new("the address the linker chose for the code");
    c.that(
        "output.e_entry",
        "an address the linker allocated, not the input's sh_addr",
        linked.elf.entry != 0x1000,
        format!("0x{:x}", linked.elf.entry),
    );
    c.finish()
});

// ---------------------------------------------------------------------------------------
// Examples
// ---------------------------------------------------------------------------------------

/// The unknown-section fixture, for the worked example.
fn example_unknown_section() -> Result<Vec<u8>, String> {
    let code = program_code();
    build(message_symbols(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(message_section())
            .section(SectionSpec::new(
                ".weird",
                SHT_UNKNOWN_OS,
                0,
                vec![1, 2, 3, 4],
            )),
    ))
    .map_err(|f| f.messages.join("; "))
}

/// The `.rela`-first fixture, for the worked example.
fn example_rela_first() -> Result<Vec<u8>, String> {
    let code = program_code();
    build(
        message_symbols(
            ObjectBuilder::new()
                .section(text_of(&code))
                .section(message_section()),
        )
        .rela_before_target(),
    )
    .map_err(|f| f.messages.join("; "))
}

/// Worked examples: two objects that look wrong and are not.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "A section type nobody has heard of",
            "ld -o prog weird.o",
            example_unknown_section,
        )
        .request(
            "weird.o: the usual .text and .rodata, plus a four-byte section called `.weird` \
             whose sh_type is 0x60000000 (the start of the OS-specific range) and whose \
             sh_flags are 0",
        )
        .response(
            "The ordinary executable, printing `odd but legal` and exiting 0. `.weird` is not \
             SHF_ALLOC, so it contributes nothing to the memory image — copying it into the \
             output or dropping it are both fine, refusing the object is not",
        )
        .note(
            "The rule is sh_flags, not sh_type: allocate what asks to be allocated, and \
             ignore what does not. A linker that switches on sh_type and panics in the \
             default arm fails on the first real object it meets.",
        )
        .runs("odd but legal\n", 0),
        ExampleSpec::object(
            "Relocations before the section they patch",
            "ld -o prog rela-first.o",
            example_rela_first,
        )
        .request(
            "rela-first.o: identical to the ordinary object except that `.rela.text` is \
             section 1 and `.text` is section 2 — the relocations come first in the table",
        )
        .response(
            "The same executable. `.rela.text` names its target in sh_info and its symbol \
             table in sh_link; both are indices, and neither says anything about order",
        )
        .note(
            "Collect the section headers into an array first, then resolve sh_info and \
             sh_link by index. A single pass that applies relocations as it meets them works \
             on gcc output and on nothing else.",
        )
        .runs("odd but legal\n", 0),
    ]
}
