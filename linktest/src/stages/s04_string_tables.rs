//! Stage 04 — String tables: section and symbol names.
//!
//! An ELF file holds no names, only offsets into string tables. Section names live in the
//! table `e_shstrndx` names; symbol names live in the table `.symtab`'s `sh_link` names; the
//! two are different tables and there is no rule that either is the last section. A name is
//! the bytes from its offset up to the next NUL — of any length, containing anything but
//! NUL, and never to be compared by prefix or by suffix.

use crate::asm::{Code, Reg, STDOUT};
use crate::assert::{Check, Failure};
use crate::elf::read::Elf;
use crate::elf::write::{ObjectBuilder, SectionSpec, SymbolSpec};
use crate::elf::*;
use crate::examples::ExampleSpec;
use crate::link::{Link, Linked};
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 4,
        slug: "string_tables",
        name: "String tables: section and symbol names",
        ext: false,
        hints: &[
            "Section names come out of the string table e_shstrndx points at; symbol names \
             come out of the one .symtab's sh_link points at — two tables, two indices, and \
             neither is guaranteed to be the last section",
            "A name is the NUL-terminated string at its offset: read to the NUL, never to a \
             fixed maximum, and keep the whole thing",
            "Compare whole names and nothing less: `strings` is a suffix of `.rodata.strings` \
             and `counter` is a suffix of `mycounter`, and they are four different names",
            "Assembler and compiler output is full of dots, dollars and digits in symbol \
             names; a name is bytes, not an identifier",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "a 66-character section name survives the link and its code runs",
                long_section_name,
            ),
            Test::new(
                "a 131-character symbol name resolves across two objects",
                long_symbol_name,
            ),
            Test::new(
                "an undefined symbol with a 131-character name is named in full",
                long_symbol_in_diagnostic,
            ),
            Test::new(
                "symbol names containing dots, dollars, underscores and digits resolve",
                punctuation_in_symbol_names,
            ),
            Test::new(
                "two sections whose names share a suffix stay distinct",
                suffix_sharing_sections,
            ),
            Test::new(
                "a symbol whose name is a suffix of another's is not confused with it",
                suffix_sharing_symbols,
            ),
            Test::new(
                "section names are found through e_shstrndx, not at a fixed index",
                shstrndx_is_authoritative,
            ),
            Test::new(
                "long, punctuated and suffix-sharing names in one object still print",
                all_the_odd_names_at_once,
            ),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------

/// A section name of 66 characters.
fn long_section() -> String {
    format!(".text.{}", "x".repeat(60))
}

/// A symbol name of 131 characters.
fn long_symbol() -> String {
    format!("f{}", "o".repeat(130))
}

/// An object that prints `message` from `.rodata` and exits `status`, with `.text` under a
/// name the caller chooses.
fn printer(section: &str, message: &str, status: u32) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "message", message.len() as u32);
    code.sys_exit(status);
    build(
        ObjectBuilder::new()
            .section(text_named(section, &code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, section, 0).func())
            .symbol(SymbolSpec::local("message", ".rodata", 0).object(message.len() as u64)),
    )
}

/// An object that prints `message`, calls `callee` (undefined here) and exits with whatever
/// the callee returned in `eax`.
fn caller_of(callee: &str, message: &str) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "message", message.len() as u32);
    code.call(callee);
    code.sys_exit_eax();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::local("message", ".rodata", 0).object(message.len() as u64))
            .symbol(SymbolSpec::undefined(callee)),
    )
}

/// One function per name: each returns its own value in `eax`, all in one object, each in
/// its own section so that the linker cannot tell them apart by position either.
fn functions(names: &[(&str, u32)]) -> Result<Vec<u8>, Failure> {
    let mut b = ObjectBuilder::new();
    for (i, (name, value)) in names.iter().enumerate() {
        let mut code = Code::new();
        code.mov_r32_imm32(Reg::Rax, *value);
        code.ret();
        let section = format!(".text.f{i}");
        b = b
            .section(text_named(&section, &code))
            .symbol(SymbolSpec::global(name, &section, 0).func());
    }
    build(b)
}

/// Append one more, empty, unnamed section header and bump `e_shnum`, so that `e_shstrndx`
/// no longer names the last entry of the table.
///
/// The extra header is `SHT_NULL` with `sh_name` 0 (the empty name), `sh_offset` 0 and
/// `sh_size` 0: legal, inert, and fatal to a linker that believes `.shstrtab` is whatever
/// the last section header happens to be.
fn extra_trailing_section_header(bytes: Vec<u8>) -> Result<Vec<u8>, Failure> {
    let elf = Elf::parse(&bytes)
        .map_err(|e| Failure::harness(format!("the suite built an object it cannot read: {e}")))?;
    let table_end = elf.shoff as usize + elf.shnum as usize * SHDR_SIZE as usize;
    if table_end > bytes.len() {
        return Err(Failure::harness(
            "the fixture's section header table does not fit in the fixture",
        ));
    }
    let mut header = vec![0u8; SHDR_SIZE as usize];
    header[32..40].copy_from_slice(&1u64.to_le_bytes()); // sh_addralign = 1
    let mut out = bytes[..table_end].to_vec();
    out.extend_from_slice(&header);
    out.extend_from_slice(&bytes[table_end..]);
    let shnum = elf.shnum + 1;
    out[0x3c..0x3e].copy_from_slice(&shnum.to_le_bytes());
    Ok(out)
}

/// The addresses of two symbols in the output, when the linker kept both in its `.symtab`.
fn two_addresses(linked: &Linked, a: &str, b: &str) -> Option<(u64, u64)> {
    let first = linked.elf.symbol_address(a).ok()?;
    let second = linked.elf.symbol_address(b).ok()?;
    Some((first, second))
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

link_test!(long_section_name, |ctx| {
    let name = long_section();
    let obj = printer(&name, "long section\n", 0)?;
    let mut c = Check::new("the fixture's section name");
    c.eq("input.section.name.len", 66usize, name.len());
    c.that(
        "input.sections",
        "the long name really is in the object's .shstrtab",
        Elf::parse(&obj)
            .map(|e| e.section(&name).is_some())
            .unwrap_or(false),
        name.clone(),
    );
    c.finish()?;

    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("a 66-character section name"),
    )?;
    assert_runnable_layout(&linked)?;
    assert_entry_is(&linked, DEFAULT_ENTRY)?;
    ctx.expect_output(&linked, "long section\n", 0)?;
    ctx.note(
        "GNU ld folds this section into the output's .text; where the bytes end up is the \
         linker's business, and the suite only asks that the code runs",
    );
    Ok(())
});

link_test!(long_symbol_name, |ctx| {
    let name = long_symbol();
    let a = caller_of(&name, "long symbol\n")?;
    let b = functions(&[(name.as_str(), 5)])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("a 131-character symbol name"),
    )?;
    assert_runnable_layout(&linked)?;
    let mut c = Check::new("the long name in the output's own symbol table");
    c.eq("input.symbol.name.len", 131usize, name.len());
    if let Some(sym) = linked.elf.symbol(&name) {
        c.eq("output.symbol.name", name.clone(), sym.name.clone());
    } else {
        c.note("the linker kept no symbol of that name, so only the exit status proves it");
    }
    c.finish()?;
    ctx.expect_output(&linked, "long symbol\n", 5)?;
    Ok(())
});

link_test!(long_symbol_in_diagnostic, |ctx| {
    let name = long_symbol();
    let a = caller_of(&name, "long symbol\n")?;
    let run = ctx.link_fails(
        &Link::new()
            .object("a.o", a)
            .label("an undefined 131-character symbol"),
        "linking a reference to an undefined 131-character symbol",
    )?;
    let mut c = Check::new("the diagnostic for the undefined symbol");
    c.mentions("linker.stderr", &name, &run.output.stderr);
    if !c.ok() {
        c.note(
            "the whole name has to reach the message; truncating it at 64 or 128 bytes does not",
        );
        c.block("linker output", run.diagnostics());
    }
    c.finish()
});

link_test!(punctuation_in_symbol_names, |ctx| {
    let name = "ns.fn$impl_42";
    let a = caller_of(name, "punctuation\n")?;
    let b = functions(&[(name, 9)])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("a symbol named ns.fn$impl_42"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "punctuation\n", 9)?;
    ctx.note(
        "a dot, a dollar and digits are ordinary bytes in a symbol name — C++ mangling, Rust \
         symbol hashes and assembler-local labels all produce them",
    );
    Ok(())
});

link_test!(suffix_sharing_sections, |ctx| {
    // `.rodata.strings` and `strings` are different sections. The program prints the six
    // bytes of the first one; if the linker confuses the two, the output is `XXXXXX`.
    let mut code = Code::new();
    code.sys_write(STDOUT, "msg_long", 6);
    code.sys_exit(3);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata.strings", b"hello\n".to_vec()))
            .section(SectionSpec::rodata("strings", b"XXXXXX".to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::local("msg_long", ".rodata.strings", 0).object(6))
            .symbol(SymbolSpec::local("msg_short", "strings", 0).object(6)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("two suffix-sharing sections"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "hello\n", 3)?;
    let mut c = Check::new("that the two sections were kept apart");
    if let Some((long, short)) = two_addresses(&linked, "msg_long", "msg_short") {
        c.that(
            "output.symbol_addresses",
            "msg_long and msg_short at different addresses — they are in different sections",
            long != short,
            format!("both at 0x{long:x}"),
        );
    } else {
        c.note("the linker kept no local symbols, so only the program's own output proves it");
    }
    c.finish()
});

link_test!(suffix_sharing_symbols, |ctx| {
    // `counter` is a suffix of `mycounter`. The call is to `counter`, which returns 11;
    // `mycounter` returns 99, so the exit status says which one the linker chose.
    let a = caller_of("counter", "suffixes\n")?;
    let b = functions(&[("counter", 11), ("mycounter", 99)])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("counter next to mycounter"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "suffixes\n", 11)?;
    let mut c = Check::new("that the two symbols are two symbols");
    if let Some((short, long)) = two_addresses(&linked, "counter", "mycounter") {
        c.that(
            "output.symbol_addresses",
            "counter and mycounter at different addresses",
            short != long,
            format!("both at 0x{short:x}"),
        );
    } else {
        c.note("the linker kept neither symbol in its output, so only the exit status proves it");
    }
    c.finish()
});

link_test!(shstrndx_is_authoritative, |ctx| {
    let obj = extra_trailing_section_header(printer(".text", "shstrndx\n", 0)?)?;
    let parsed = Elf::parse(&obj)
        .map_err(|e| Failure::harness(format!("the suite built an object it cannot read: {e}")))?;
    let mut c = Check::new("the fixture: one more section header after .shstrtab");
    c.that(
        "input.e_shstrndx",
        "an index that is no longer the last entry of the section header table",
        (parsed.shstrndx as usize) + 1 < parsed.shnum as usize,
        format!("shstrndx {} of {} sections", parsed.shstrndx, parsed.shnum),
    );
    c.that(
        "input.sections[e_shstrndx].sh_type",
        "SHT_STRTAB — the section names really are read through this index",
        parsed
            .sections
            .get(parsed.shstrndx as usize)
            .map(|s| s.sh_type == SHT_STRTAB)
            .unwrap_or(false),
        parsed
            .sections
            .get(parsed.shstrndx as usize)
            .map(|s| section_type_name(s.sh_type).to_string())
            .unwrap_or_else(|| "no such section".to_string()),
    );
    c.finish()?;

    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("an object whose .shstrtab is not the last section"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "shstrndx\n", 0)?;
    ctx.note(
        "the trailing header is an inert SHT_NULL entry with the empty name; its only job is \
         to move .shstrtab away from the last index, where a linker might have guessed it",
    );
    Ok(())
});

link_test!(all_the_odd_names_at_once, |ctx| {
    let section = long_section();
    let callee = long_symbol();
    let mut code = Code::new();
    code.sys_write(STDOUT, "msg.v1$long", 8);
    code.call(&callee);
    code.sys_exit_eax();
    let a = build(
        ObjectBuilder::new()
            .section(text_named(&section, &code))
            .section(SectionSpec::rodata(
                ".rodata.strings",
                b"all odd\n".to_vec(),
            ))
            .section(SectionSpec::rodata("strings", b"XXXXXXXX".to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, &section, 0).func())
            .symbol(SymbolSpec::local("msg.v1$long", ".rodata.strings", 0).object(8))
            .symbol(SymbolSpec::local("msg.v1", "strings", 0).object(8))
            .symbol(SymbolSpec::undefined(&callee)),
    )?;
    let b = functions(&[(callee.as_str(), 21), ("o", 99)])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("every odd name at once"),
    )?;
    assert_runnable_layout(&linked)?;
    assert_entry_is(&linked, DEFAULT_ENTRY)?;
    ctx.expect_output(&linked, "all odd\n", 21)?;
    Ok(())
});

// ---------------------------------------------------------------------------------------
// Examples
// ---------------------------------------------------------------------------------------

/// The long-names fixture, for the worked example.
fn example_long_names() -> Result<Vec<u8>, String> {
    let section = long_section();
    let callee = long_symbol();
    let mut code = Code::new();
    code.sys_write(STDOUT, "msg.v1$long", 8);
    code.call(&callee);
    code.sys_exit_eax();
    build(
        ObjectBuilder::new()
            .section(text_named(&section, &code))
            .section(SectionSpec::rodata(
                ".rodata.strings",
                b"all odd\n".to_vec(),
            ))
            .section(SectionSpec::rodata("strings", b"XXXXXXXX".to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, &section, 0).func())
            .symbol(SymbolSpec::local("msg.v1$long", ".rodata.strings", 0).object(8))
            .symbol(SymbolSpec::local("msg.v1", "strings", 0).object(8))
            .symbol(SymbolSpec::undefined(&callee)),
    )
    .map_err(|f| f.messages.join("; "))
}

/// The fixture whose `.shstrtab` is not the last section.
fn example_moved_shstrtab() -> Result<Vec<u8>, String> {
    let obj = printer(".text", "shstrndx\n", 0).map_err(|f| f.messages.join("; "))?;
    extra_trailing_section_header(obj).map_err(|f| f.messages.join("; "))
}

/// Worked examples: names that break the shortcuts.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "Names that break the shortcuts",
            "ld -o prog odd-names.o long.o",
            example_long_names,
        )
        .request(
            "odd-names.o: a .text called `.text.xxx…` (66 characters), a `.rodata.strings` \
             next to a plain `strings`, locals called `msg.v1$long` and `msg.v1`, and an \
             undefined 131-character callee",
        )
        .response(
            "The program prints the eight bytes of `.rodata.strings` — not the eight `X` in \
             `strings` — and exits with what the long-named function returned",
        )
        .note(
            "Every name here is a whole string compared against a whole string. Prefix \
             matching resolves `msg.v1` to `msg.v1$long`; suffix matching confuses `strings` \
             with `.rodata.strings`; a fixed-size name buffer truncates the callee.",
        ),
        ExampleSpec::object(
            "A .shstrtab that is not the last section",
            "ld -o prog shstrndx.o",
            example_moved_shstrtab,
        )
        .request(
            "shstrndx.o: an ordinary object with one extra, empty SHT_NULL section header \
             appended to the table, so e_shstrndx names a section in the middle",
        )
        .response(
            "The same executable as for the untouched object: section names are read through \
             e_shstrndx, which is a number in the header, not a position in the table",
        )
        .note(
            "The extra header is deliberately inert — type SHT_NULL, size 0, name 0. A \
             linker that walks the table and treats the last SHT_STRTAB it sees as the \
             section name table reads the symbol string table instead and produces garbage \
             names.",
        )
        .runs("shstrndx\n", 0),
    ]
}
