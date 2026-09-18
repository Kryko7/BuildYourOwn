//! Stage 21 — Common symbols (`SHN_COMMON`).
//!
//! A common symbol is a *tentative* definition: `int buf[64];` at file scope in C, compiled
//! without `-fno-common`. The object does not allocate the bytes; it only says "somebody
//! needs this many bytes, this well aligned, under this name". Several objects may say it
//! at once, and that is not a duplicate definition — the linker merges them into one
//! allocation with the largest size and the largest alignment anyone asked for, puts it in
//! `.bss`, and zeroes it. A real definition anywhere in the link wins outright.
//!
//! The trap is the encoding: for `SHN_COMMON` symbols `st_value` is the **alignment**, not
//! an offset, and `st_size` is the size. Reading `st_value` as an offset gives nonsense.

use crate::asm::{Code, Reg};
use crate::assert::{Check, Failure};
use crate::elf::read::Section;
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
        number: 21,
        slug: "common_symbols",
        name: "Common symbols (SHN_COMMON)",
        ext: true,
        hints: &[
            "For a symbol whose st_shndx is SHN_COMMON, st_value is the required alignment \
             and st_size the required size — neither means what it means for a normal symbol",
            "Merge every common of one name into a single .bss allocation whose size is the \
             largest st_size and whose alignment is the largest st_value, then give every \
             reference that one address",
            "Commons never collide with each other, but any real definition — .data, .bss, \
             .text, strong or weak — takes precedence and the commons are dropped",
            "The allocation is zero-filled and writable: it belongs in a SHT_NOBITS section \
             inside a read-write PT_LOAD, and costs no bytes in the file",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "a single common symbol is allocated once, zeroed, in .bss",
                one_common_allocated,
            )
            .ext(),
            Test::new(
                "two commons of different sizes get the largest size",
                largest_size_wins,
            )
            .ext(),
            Test::new(
                "two commons of different alignments get the largest alignment",
                largest_alignment_wins,
            )
            .ext(),
            Test::new(
                "a real definition beats a common and is not an error",
                definition_beats_common,
            )
            .ext(),
            Test::new(
                "the same common in three objects is not a duplicate definition",
                three_objects_one_common,
            )
            .ext(),
            Test::new(
                "a common symbol is writable at run time",
                common_is_writable,
            )
            .ext(),
            Test::new(
                "two objects referring to one common share the one allocation",
                one_allocation_two_objects,
            )
            .ext(),
        ],
    }
}

link_test!(one_common_allocated, |ctx| {
    // movzx eax, byte [rip+buf]; add eax, 5; exit(eax) — 5 iff the allocation is zeroed.
    let mut code = Code::new();
    code.movzx_r32_byte_rip(Reg::Rax, "buf", 0);
    code.add_r32_imm32(Reg::Rax, 5);
    code.sys_exit_eax();
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::common("buf", 64, 16)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("one object with one common symbol"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "", 5)?;

    let addr = linked.address_of("buf")?;
    let mut c = Check::new("where the common symbol was allocated");
    c.that(
        "output.symtab['buf'].st_shndx",
        "not SHN_COMMON any more — the tentative definition became a real one",
        linked
            .elf
            .symbol("buf")
            .map(|s| !s.is_common())
            .unwrap_or(false),
        linked.elf.symbol("buf").map(|s| s.shndx).unwrap_or(0),
    );
    c.eq(
        "output.symtab['buf'].count",
        1usize,
        linked
            .elf
            .symbols
            .iter()
            .filter(|s| s.name == "buf")
            .count(),
    );
    let seg = linked.elf.segment_at(addr);
    c.that(
        "output.symtab['buf'].st_value",
        "an address inside a writable PT_LOAD",
        seg.map(|s| s.writable()).unwrap_or(false),
        segment_summary(&linked.elf, addr),
    );
    match section_covering(&linked, addr) {
        Some(s) => {
            c.observe("output.section[buf].name", s.name.clone());
            c.that(
                "output.section[buf].sh_type",
                "SHT_NOBITS — a zero-filled allocation costs no bytes in the file",
                s.is_nobits(),
                section_type_name(s.sh_type),
            );
            c.that(
                "output.section[buf].sh_flags",
                "SHF_ALLOC | SHF_WRITE",
                s.is_alloc() && s.is_write(),
                format!("0x{:x}", s.flags),
            );
        }
        None => {
            c.observe(
                "output.section[buf]",
                "no section header covers the address; only the segment does",
            );
            c.note("a section header table is not required in the output, so this is allowed");
        }
    }
    if !c.ok() {
        c.block("output section headers", linked.elf.section_header_table());
        c.block("output program headers", linked.elf.program_header_table());
    }
    c.finish()
});

link_test!(largest_size_wins, |ctx| {
    // a.o asks for 16 bytes, b.o for 256. The program writes four bytes at buf+252 — inside
    // the larger request and far outside the smaller one — and reads them back.
    let mut code = Code::new();
    code.mov_r32_imm32(Reg::Rax, 99);
    code.mov_rip_r32("buf", Reg::Rax, 252);
    code.xor_r32_r32(Reg::Rax, Reg::Rax);
    code.mov_r32_rip(Reg::Rax, "buf", 252);
    code.sys_exit_eax();
    let a = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::common("buf", 16, 8)),
    )?;
    let b = common_only("buf", 256, 8)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("commons of 16 and 256 bytes"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "", 99)?;

    let addr = linked.address_of("buf")?;
    let mut c = Check::new("the size the merged common was given");
    if let Some(s) = linked.elf.symbol("buf") {
        c.at_least("output.symtab['buf'].st_size", 256u64, s.size);
    }
    let end = addr + 256;
    c.that(
        "output.symtab['buf'].st_value + 256",
        "the last byte of the larger request still inside a writable PT_LOAD",
        linked
            .elf
            .segment_at(end - 1)
            .map(|s| s.writable())
            .unwrap_or(false),
        segment_summary(&linked.elf, end - 1),
    );
    if !c.ok() {
        c.block("output section headers", linked.elf.section_header_table());
    }
    c.finish()
});

link_test!(largest_alignment_wins, |ctx| {
    let a = common_main("buf", 8, 8)?;
    let b = common_only("buf", 8, 64)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("commons aligned 8 and 64"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "", 0)?;
    let addr = linked.address_of("buf")?;
    let mut c = Check::new("the alignment the merged common was given");
    c.that(
        "output.symtab['buf'].st_value % 64",
        "0 — the largest alignment any input asked for wins, and st_value of a common is the \
         alignment, not an offset",
        addr % 64 == 0,
        format!("0x{addr:x} % 64 = {}", addr % 64),
    );
    if !c.ok() {
        c.block("output section headers", linked.elf.section_header_table());
    }
    c.finish()
});

link_test!(definition_beats_common, |ctx| {
    // a.o has a tentative `val`; b.o really defines it, worth 123.
    let a = read_word_main("val", Some(SymbolSpec::common("val", 4, 4)))?;
    let b = build(
        ObjectBuilder::new()
            .section(SectionSpec::data(".data", 123u32.to_le_bytes().to_vec()).align(4))
            .symbol(SymbolSpec::global("val", ".data", 0).object(4)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("a common and a real definition of 'val'"),
    )?;
    assert_runnable_layout(&linked)?;
    // 123 is b.o's initialiser; 0 would mean the common allocation had won.
    ctx.expect_output(&linked, "", 123)?;
    let addr = linked.address_of("val")?;
    let mut c = Check::new("which definition of 'val' survived");
    c.that(
        "output.symtab['val'].st_value",
        "an address whose bytes are in the file — the real definition, not a zeroed common",
        linked
            .elf
            .read_at_vaddr(addr, 4)
            .map(|b| b == 123u32.to_le_bytes())
            .unwrap_or(false),
        linked
            .elf
            .read_at_vaddr(addr, 4)
            .map(|b| format!("{b:02x?}"))
            .unwrap_or_else(|e| e.to_string()),
    );
    c.note("this is not a duplicate definition: a common yields to a real one silently");
    c.finish()
});

link_test!(three_objects_one_common, |ctx| {
    let a = common_main("shared_buf", 32, 8)?;
    let b = common_only("shared_buf", 32, 8)?;
    let c = common_only("shared_buf", 32, 8)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .object("c.o", c)
            .label("three objects with the same common"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "", 0)?;
    let mut check = Check::new("that three tentative definitions made one allocation");
    check.eq(
        "output.symtab['shared_buf'].count",
        1usize,
        linked
            .elf
            .symbols
            .iter()
            .filter(|s| s.name == "shared_buf")
            .count(),
    );
    check.note(
        "three real definitions of this name would be two duplicate-definition errors; three \
         commons are none",
    );
    check.finish()
});

link_test!(common_is_writable, |ctx| {
    // Store 0x5a into the allocation, read it back, exit with it.
    let mut code = Code::new();
    code.mov_r32_imm32(Reg::Rax, 0x5a);
    code.mov_rip_r32("buf", Reg::Rax, 0);
    code.xor_r32_r32(Reg::Rax, Reg::Rax);
    code.mov_r32_rip(Reg::Rax, "buf", 0);
    code.sys_exit_eax();
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::common("buf", 128, 16)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("a program that writes to its common symbol"),
    )?;
    assert_runnable_layout(&linked)?;
    // A read-only mapping would have died with SIGSEGV before printing anything.
    ctx.expect_output(&linked, "", 0x5a)?;
    Ok(())
});

link_test!(one_allocation_two_objects, |ctx| {
    // b.o writes 37 into the common; a.o reads it back. One allocation or nothing works.
    let mut code = Code::new();
    code.call("poke");
    code.movzx_r32_byte_rip(Reg::Rax, "buf", 0);
    code.sys_exit_eax();
    let a = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::common("buf", 32, 8))
            .symbol(SymbolSpec::undefined("poke")),
    )?;
    let mut poke = Code::new();
    poke.mov_r32_imm32(Reg::Rax, 37);
    poke.mov_rip_r32("buf", Reg::Rax, 0);
    poke.ret();
    let b = build(
        ObjectBuilder::new()
            .section(text_of(&poke))
            .symbol(SymbolSpec::global("poke", ".text", 0).func())
            .symbol(SymbolSpec::common("buf", 32, 8)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("two objects sharing one common allocation"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "", 37)?;
    Ok(())
});

// ---------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------

/// The allocated section whose address range covers `addr`, if the output has section
/// headers at all.
fn section_covering(linked: &Linked, addr: u64) -> Option<&Section> {
    linked
        .elf
        .sections
        .iter()
        .find(|s| s.is_alloc() && s.size > 0 && s.addr <= addr && addr < s.addr + s.size)
}

/// An object that declares a common symbol and nothing else.
fn common_only(name: &str, size: u64, align: u64) -> Result<Vec<u8>, Failure> {
    build(ObjectBuilder::new().symbol(SymbolSpec::common(name, size, align)))
}

/// `_start` reads the first byte of the common `name` and exits with it — 0 when the
/// allocation is properly zeroed.
fn common_main(name: &str, size: u64, align: u64) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.movzx_r32_byte_rip(Reg::Rax, name, 0);
    code.sys_exit_eax();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::common(name, size, align)),
    )
}

/// `_start` loads the four bytes at `name` and exits with them; `extra` is the declaration
/// this object makes of that name (a common, or nothing at all).
fn read_word_main(name: &str, extra: Option<SymbolSpec>) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.mov_r32_rip(Reg::Rax, name, 0);
    code.sys_exit_eax();
    let mut b = ObjectBuilder::new()
        .section(text_of(&code))
        .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func());
    b = match extra {
        Some(s) => b.symbol(s),
        None => b.symbol(SymbolSpec::undefined(name)),
    };
    build(b)
}

/// Worked examples: what a tentative definition looks like on the wire.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object("One tentative definition", "ld -o prog a.o", || {
            let mut code = Code::new();
            code.movzx_r32_byte_rip(Reg::Rax, "buf", 0);
            code.add_r32_imm32(Reg::Rax, 5);
            code.sys_exit_eax();
            build(
                ObjectBuilder::new()
                    .section(text_of(&code))
                    .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
                    .symbol(SymbolSpec::common("buf", 64, 16)),
            )
            .map_err(|f| f.messages.join("; "))
        })
        .request(
            "a.o has no .bss at all. `buf` is STT_OBJECT with st_shndx = SHN_COMMON (0xfff2), \
             st_size = 64 and st_value = 16 — and that 16 is the *alignment*, not an offset \
             into anything",
        )
        .response(
            "The linker allocates 64 zero bytes, 16-aligned, in a SHT_NOBITS section inside a \
             read-write PT_LOAD, and resolves the `movzx` against that address. The program \
             reads a zero and exits 5",
        )
        .note(
            "Treating st_value as an offset gives an address 16 bytes into something that \
             does not exist. The SHN_COMMON case has to be branched on before you touch \
             either field.",
        )
        .runs("", 5),
        ExampleSpec::object(
            "The same name, a bigger request",
            "ld -o prog a.o b.o",
            || common_only("buf", 256, 8).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "b.o is nothing but a symbol table: one common `buf`, st_size 256, st_value 8. It \
             has no sections of its own to contribute",
        )
        .response(
            "One allocation, 256 bytes long (the largest st_size) and 16-aligned (the largest \
             st_value of the two). Both objects' references resolve to that single address",
        )
        .note(
            "Largest size *and* largest alignment, computed independently. Taking the size and \
             alignment of the same, first-seen symbol is the usual bug, and it only shows up \
             when the program writes past the smaller request.",
        ),
    ]
}
