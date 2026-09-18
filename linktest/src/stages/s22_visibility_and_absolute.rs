//! Stage 22 — Visibility, absolute and section symbols.
//!
//! The leftovers of the symbol table, and every one of them is a place a hand-written linker
//! quietly does the wrong thing. `st_other` carries a visibility that means nothing inside a
//! static link but must not make the link fail. `SHN_ABS` symbols are values, not addresses:
//! they are the one kind of symbol a linker must *not* relocate. `STT_SECTION` symbols are
//! how a compiler refers to anonymous data. `STT_FILE` is debugging furniture that has to be
//! stepped over rather than choked on.

use crate::asm::{Code, Reg, STDOUT};
use crate::assert::{Check, Failure};
use crate::elf::write::{ObjectBuilder, SectionSpec, SymbolSpec};
use crate::elf::*;
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// The absolute value the `SHN_ABS` fixtures carry — distinctive enough to spot in a hex
/// dump and small enough to fit an unsigned 32-bit relocation.
const ABS_VALUE: u64 = 0x0042_4243;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 22,
        slug: "visibility_and_absolute",
        name: "Visibility, absolute and section symbols",
        ext: true,
        hints: &[
            "st_other's low two bits are the visibility; in a static link with no dynamic \
             table they change nothing about resolution, so read them, keep them, and never \
             refuse a link over them",
            "A symbol whose st_shndx is SHN_ABS already has its final value: S is st_value \
             itself, with no section address added — this is the one case where relocating \
             is the bug",
            "A relocation may name a STT_SECTION symbol instead of a real one; S is then the \
             output address the section was given, and the addend picks the byte inside it",
            "STT_FILE entries are local, absolute and carry a source file name; skip them \
             rather than treating them as definitions of anything",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "an STV_HIDDEN global still resolves inside the link",
                hidden_resolves,
            )
            .ext(),
            Test::new(
                "an STV_INTERNAL global still resolves inside the link",
                internal_resolves,
            )
            .ext(),
            Test::new(
                "an SHN_ABS symbol keeps its exact value through the link",
                absolute_value_survives,
            )
            .ext(),
            Test::new(
                "a relocation against an SHN_ABS symbol produces that exact value",
                absolute_relocation,
            )
            .ext(),
            Test::new(
                "a section symbol is usable as a relocation target",
                section_symbol_target,
            )
            .ext(),
            Test::new(
                "an STT_FILE symbol is ignored rather than rejected",
                file_symbol_ignored,
            )
            .ext(),
            Test::new(
                "st_size and st_info of a defined global survive into the output",
                size_and_info_survive,
            )
            .ext(),
            Test::new(
                "visibility does not change the address anything resolves to",
                visibility_does_not_move_anything,
            )
            .ext(),
        ],
    }
}

link_test!(hidden_resolves, |ctx| {
    let a = caller("hidden\n", "h")?;
    let b = callee_with_visibility("h", 19, STV_HIDDEN)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("a call to an STV_HIDDEN global"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "hidden\n", 19)?;
    let mut c = Check::new("what the linker did with the hidden symbol");
    match linked.elf.symbol("h") {
        Some(s) => {
            c.observe("output.symtab['h'].st_other", s.visibility);
            c.observe("output.symtab['h'].st_info.bind", s.bind);
            if s.bind == STB_LOCAL {
                c.note(
                    "this linker demoted the hidden global to STB_LOCAL in the output, which \
                     is what GNU ld does; keeping it STB_GLOBAL is equally correct in a \
                     static link",
                );
            }
        }
        None => {
            c.observe("output.symtab['h']", "absent from the output symbol table");
        }
    }
    c.finish()
});

link_test!(internal_resolves, |ctx| {
    let a = caller("internal\n", "i")?;
    let b = callee_with_visibility("i", 27, STV_INTERNAL)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("a call to an STV_INTERNAL global"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "internal\n", 27)?;
    let mut c = Check::new("the call to the internal symbol");
    let site = linked.address_of(DEFAULT_ENTRY)? + CALLER_CALL_DISP_OFFSET;
    let target = linked.address_of("i")?;
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[call].disp32",
        site,
        target,
        -4,
    );
    c.note(
        "STV_INTERNAL promises the symbol is never referenced from outside its component; \
         inside one static link that is simply always true",
    );
    c.finish()
});

link_test!(absolute_value_survives, |ctx| {
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&{
                let mut code = Code::new();
                code.sys_exit(0);
                code
            }))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::absolute("answer", ABS_VALUE)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("an object carrying an SHN_ABS symbol"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "", 0)?;
    let mut c = Check::new("the absolute symbol in the output");
    match linked.elf.symbol("answer") {
        Some(s) => {
            c.addr_eq("output.symtab['answer'].st_value", ABS_VALUE, s.value);
            c.that(
                "output.symtab['answer'].st_shndx",
                "still SHN_ABS — an absolute symbol is a value, not a place",
                s.is_absolute(),
                format!("st_shndx 0x{:x}", s.shndx),
            );
        }
        None => {
            c.that(
                "output.symtab['answer']",
                "an entry for the absolute symbol",
                false,
                "no symbol of that name in the output",
            );
        }
    }
    if !c.ok() {
        c.note(
            "adding a section address to an SHN_ABS symbol is the classic bug here: the value \
             comes out shifted by wherever .text landed",
        );
        c.block("linker command", linked.run.output.command_line());
    }
    c.finish()
});

link_test!(absolute_relocation, |ctx| {
    // mov eax, <magic>  ; R_X86_64_32 against an SHN_ABS symbol worth 55
    let mut code = Code::new();
    let imm_site = code.len() + 1;
    code.mov_r32_symbol_addr32(Reg::Rax, "magic", 0);
    code.sys_exit_eax();
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::absolute("magic", 55)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("a relocation against an SHN_ABS symbol"),
    )?;
    assert_runnable_layout(&linked)?;
    // 55 exactly; anything section-relative would be a five-figure address truncated to 8 bits.
    ctx.expect_output(&linked, "", 55)?;
    let mut c = Check::new("the value written at the relocation site");
    let site = linked.address_of(DEFAULT_ENTRY)? + imm_site;
    match linked.elf.u32_at_vaddr(site) {
        Ok(got) => {
            c.eq("output.text[mov eax, magic].imm32", 55u32, got);
        }
        Err(e) => {
            c.that(
                "output.text[mov eax, magic].imm32",
                "a readable 32-bit field at the relocation site",
                false,
                e.to_string(),
            );
        }
    }
    if !c.ok() {
        c.block("output program headers", linked.elf.program_header_table());
    }
    c.finish()
});

link_test!(section_symbol_target, |ctx| {
    // Both the string and its length come through the section symbol, with an addend
    // choosing the byte: write(1, .rodata + 6, 6) prints the second half only.
    let mut code = Code::new();
    code.sys_write_section(STDOUT, ".rodata", 6, 6);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"first second\n".to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func()),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("a relocation against a STT_SECTION symbol"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "second", 0)?;
    let mut c = Check::new("the displacement written for the section-symbol reference");
    let site = linked.address_of(DEFAULT_ENTRY)? + WRITE_LEA_DISP_OFFSET;
    match linked.elf.section(".rodata") {
        Some(s) => assert_pc32(
            &mut c,
            &linked.elf,
            "output.text[lea rsi, .rodata+6].disp32",
            site,
            s.addr,
            2, // A = -4 + 6
        ),
        None => {
            c.observe(
                "output.sections['.rodata']",
                "the output has no section by that name; only the run-time behaviour is checked",
            );
        }
    }
    c.finish()
});

link_test!(file_symbol_ignored, |ctx| {
    // STT_FILE: local, SHN_ABS, st_value 0, and named after a source file. It defines
    // nothing and must not be mistaken for a definition of anything.
    let mut file_sym = SymbolSpec::absolute("fixture.c", 0);
    file_sym.bind = STB_LOCAL;
    file_sym.stype = STT_FILE;
    let mut code = Code::new();
    code.sys_write(STDOUT, "message", 6);
    code.sys_exit(4);
    let a = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"file\n\n".to_vec()))
            .symbol(file_sym)
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::local("message", ".rodata", 0).object(6)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .label("an object carrying an STT_FILE symbol"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "file\n\n", 4)?;
    let mut c = Check::new("what became of the STT_FILE entry");
    match linked.elf.symbol("fixture.c") {
        Some(s) => {
            c.observe("output.symtab['fixture.c'].st_info.type", s.stype);
            c.observe(
                "output.symtab['fixture.c'].st_shndx",
                format!("0x{:x}", s.shndx),
            );
        }
        None => {
            c.observe("output.symtab['fixture.c']", "dropped from the output");
        }
    }
    c.note("keeping the entry and dropping it are both fine; failing the link is not");
    c.finish()
});

link_test!(size_and_info_survive, |ctx| {
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&{
                let mut code = Code::new();
                code.sys_exit(0);
                code
            }))
            .section(SectionSpec::data(".data", vec![7u8; 16]).align(8))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("blob", ".data", 0).object(16)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("an object with a sized global object symbol"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "", 0)?;
    let mut c = Check::new("the fields of the global as the output records them");
    match linked.elf.symbol("blob") {
        Some(s) => {
            c.eq("output.symtab['blob'].st_size", 16u64, s.size);
            c.eq("output.symtab['blob'].st_info.type", STT_OBJECT, s.stype);
            c.eq("output.symtab['blob'].st_info.bind", STB_GLOBAL, s.bind);
            c.that(
                "output.symtab['blob'].st_value",
                "an address whose 16 bytes are the ones the input carried",
                linked
                    .elf
                    .read_at_vaddr(s.value, 16)
                    .map(|b| b == [7u8; 16])
                    .unwrap_or(false),
                linked
                    .elf
                    .read_at_vaddr(s.value, 16)
                    .map(|b| format!("{b:02x?}"))
                    .unwrap_or_else(|e| e.to_string()),
            );
        }
        None => {
            c.that(
                "output.symtab['blob']",
                "an entry for the defined global",
                false,
                "no symbol of that name in the output",
            );
        }
    }
    if !c.ok() {
        c.note(
            "st_size and st_info are copied through unchanged; only st_value and st_shndx are \
             the linker's to rewrite",
        );
    }
    c.finish()
});

link_test!(visibility_does_not_move_anything, |ctx| {
    // The same program three times, differing only in st_other. The exit status is the
    // callee's return value, so if visibility changed what the call resolved to, it shows.
    let mut c = Check::new("that st_other does not change resolution");
    for (vis, label) in [
        (STV_DEFAULT, "STV_DEFAULT"),
        (STV_HIDDEN, "STV_HIDDEN"),
        (STV_INTERNAL, "STV_INTERNAL"),
        (STV_PROTECTED, "STV_PROTECTED"),
    ] {
        let a = caller("vis\n", "v")?;
        let b = callee_with_visibility("v", 44, vis)?;
        let linked = ctx.link_ok(
            &Link::new()
                .object("a.o", a)
                .object("b.o", b)
                .out(&format!("prog_{label}"))
                .label(label),
        )?;
        let out = ctx.run(&linked)?;
        c.eq(
            &format!("program.stdout[{label}]"),
            "vis\n".to_string(),
            out.stdout.clone(),
        );
        c.eq(&format!("program.exit_status[{label}]"), Some(44), out.code);
        let site = linked.address_of(DEFAULT_ENTRY)? + CALLER_CALL_DISP_OFFSET;
        let target = linked.address_of("v")?;
        assert_pc32(
            &mut c,
            &linked.elf,
            &format!("output.text[call].disp32[{label}]"),
            site,
            target,
            -4,
        );
    }
    c.note(
        "visibility only starts to matter once there is a dynamic symbol table to decide what \
         to export; a static link has none",
    );
    c.finish()
});

/// A callee returning `value`, with an explicit `st_other` visibility.
fn callee_with_visibility(name: &str, value: u32, visibility: u8) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.mov_r32_imm32(Reg::Rax, value);
    code.ret();
    build(
        ObjectBuilder::new().section(text_of(&code)).symbol(
            SymbolSpec::global(name, ".text", 0)
                .func()
                .visibility(visibility),
        ),
    )
}

/// Worked examples: the absolute symbol and the hidden one.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "A symbol that is a number, not a place",
            "ld -o prog a.o",
            || {
                let mut code = Code::new();
                code.mov_r32_symbol_addr32(Reg::Rax, "magic", 0);
                code.sys_exit_eax();
                build(
                    ObjectBuilder::new()
                        .section(text_of(&code))
                        .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
                        .symbol(SymbolSpec::absolute("magic", 55)),
                )
                .map_err(|f| f.messages.join("; "))
            },
        )
        .request(
            "a.o's `magic` has st_shndx = SHN_ABS (0xfff1) and st_value = 55. The .rela.text \
             entry is an R_X86_64_32 against it, patching the imm32 of `mov eax, imm32`",
        )
        .response(
            "S is 55 — st_value taken literally, with nothing added. The imm32 comes out as \
             0x37 and the program exits 55. In the output symbol table `magic` is still \
             SHN_ABS with st_value 55",
        )
        .note(
            "Every other symbol's S is `section address + st_value`. Running that formula over \
             an SHN_ABS symbol adds whatever address .text happened to get, and the program \
             exits with a truncated address instead of the constant.",
        )
        .runs("", 55),
        ExampleSpec::object("A hidden global", "ld -o prog a.o b.o", || {
            callee_with_visibility("h", 19, STV_HIDDEN).map_err(|f| f.messages.join("; "))
        })
        .request(
            "b.o defines `h` as STB_GLOBAL STT_FUNC, but st_other is 2 (STV_HIDDEN): the \
             compiler is saying this name must not leave the shared object it ends up in",
        )
        .response(
            "Inside a static link that promise is free. `h` resolves exactly as a default \
             global would, the call is patched to its address, and the program exits 19. \
             GNU ld records it as STB_LOCAL in the output symbol table; keeping it global is \
             equally correct here",
        )
        .note(
            "Visibility is not a resolution rule and never a reason to fail. Read st_other, \
             keep it, and carry on.",
        )
        .runs("hidden\n", 19),
    ]
}
