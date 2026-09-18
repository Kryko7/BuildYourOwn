//! Stage 26 — R_X86_64_32 and R_X86_64_32S.
//!
//! A statically linked non-PIE program lives entirely below 4 GB, and the x86-64 small code
//! model leans on that: the address of anything can be an immediate, and the instruction
//! that carries it is shorter and faster than a RIP-relative form. `R_X86_64_32` stores
//! `S + A` zero-extended, `R_X86_64_32S` stores it sign-extended, and for an address under
//! 2 GB the two hold the same 32 bits — the difference only shows up at the edges, which is
//! stage 27's business.

use crate::asm::{Code, Reg, STDOUT};
use crate::assert::Check;
use crate::elf::read::Elf;
use crate::elf::write::{ObjectBuilder, RelTarget, Reloc, SectionSpec, SymbolSpec};
use crate::elf::R_X86_64_32;
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 26,
        slug: "absolute_32",
        name: "R_X86_64_32 and R_X86_64_32S",
        ext: false,
        hints: &[
            "Both kinds compute S + A and store the low 32 bits; the only difference is the \
             range they are allowed to hold, so share the arithmetic and split only the check",
            "These are the relocations that make the small code model work: the whole image \
             is below 4 GB, so a zero-extended 32-bit immediate is a complete pointer and \
             the program can dereference it as one",
            "An R_X86_64_32 can sit in a data section as easily as in an instruction — a \
             four-byte word holding an address is the same relocation as `mov eax, $sym`",
            "Do not sign-extend an R_X86_64_32 or zero-extend an R_X86_64_32S when checking \
             the range: 0xffffffff is legal for the first and out of range for the second",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "an R_X86_64_32 immediate holds the symbol's address",
                abs32_immediate,
            ),
            Test::new(
                "an R_X86_64_32S immediate holds the same value",
                abs32s_immediate,
            ),
            Test::new(
                "both kinds in one object are patched independently",
                both_kinds_together,
            ),
            Test::new(
                "an addend shifts an R_X86_64_32 immediate",
                abs32_with_addend,
            ),
            Test::new(
                "an addend shifts an R_X86_64_32S immediate",
                abs32s_with_addend,
            ),
            Test::new(
                "an R_X86_64_32 against a section symbol",
                abs32_against_a_section_symbol,
            ),
            Test::new(
                "a four-byte word in .data is a usable pointer at run time",
                abs32_in_data,
            ),
            Test::new(
                "an R_X86_64_32S resolved from another object",
                abs32s_across_objects,
            ),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Assertions
// ---------------------------------------------------------------------------------------

/// Assert the 32-bit field at `site` holds `S + A` — the value both kinds store.
///
/// The invariant is stated against the address *this* linker chose for the symbol, so a
/// layout nothing like GNU ld's passes just as well.
fn assert_abs32(c: &mut Check, exe: &Elf, path: &str, site: u64, target: u64, addend: i64) {
    let want = (target as i64).wrapping_add(addend) as u64;
    match exe.u32_at_vaddr(site) {
        Ok(got) => {
            c.that(
                path,
                &format!("S + A = 0x{target:x} + {addend} = 0x{want:x}"),
                u64::from(got) == want,
                format!("0x{got:x}"),
            );
        }
        Err(e) => {
            c.that(
                path,
                "a readable 32-bit field at the relocation site",
                false,
                e.to_string(),
            );
        }
    }
}

/// `mov esi, $<section> + offset` — the same `b8+rd` encoding, but against a section symbol.
fn mov_esi_section_addr32(code: &mut Code, section: &str, offset: i64) -> u64 {
    code.raw(&[0xb8 + Reg::Rsi.num()]);
    let site = code.len();
    code.reloc_here(RelTarget::Section(section.to_string()), R_X86_64_32, offset);
    code.raw(&0u32.to_le_bytes());
    site
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

link_test!(abs32_immediate, |ctx| {
    let message = "immediate\n";
    let mut code = Code::new();
    let site_offset = code.len() + 1;
    code.mov_r32_symbol_addr32(Reg::Rsi, "message", 0);
    code.sys_write_rsi(STDOUT, message.len() as u32);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("message", ".rodata", 0).object(message.len() as u64)),
    )?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).label("mov esi, $message"))?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, message, 0)?;
    let target = linked.address_of("message")?;
    let mut c = Check::new("the imm32 of an R_X86_64_32");
    assert_abs32(
        &mut c,
        &linked.elf,
        "output.text[mov].imm32",
        linked.address_of(DEFAULT_ENTRY)? + site_offset,
        target,
        0,
    );
    c.that(
        "output.symbol[message].address",
        "an address that fits 32 unsigned bits — a non-PIE static image lives below 4 GB",
        target <= u64::from(u32::MAX),
        format!("0x{target:x}"),
    );
    c.note(
        "the program dereferences the zero-extended immediate, so this is not only a field \
         comparison: rsi really did point at the string",
    );
    c.finish()
});

link_test!(abs32s_immediate, |ctx| {
    let message = "signed\n";
    let mut code = Code::new();
    let site_offset = code.len() + 3;
    code.mov_r64_symbol_addr32s(Reg::Rsi, "message", 0);
    code.sys_write_rsi(STDOUT, message.len() as u32);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("message", ".rodata", 0).object(message.len() as u64)),
    )?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).label("mov rsi, $message"))?;
    ctx.expect_output(&linked, message, 0)?;
    let target = linked.address_of("message")?;
    let mut c = Check::new("the imm32 of an R_X86_64_32S");
    assert_abs32(
        &mut c,
        &linked.elf,
        "output.text[mov].imm32",
        linked.address_of(DEFAULT_ENTRY)? + site_offset,
        target,
        0,
    );
    c.that(
        "output.symbol[message].address",
        "an address below 2 GB, where the sign-extended form still reaches",
        target < 0x8000_0000,
        format!("0x{target:x}"),
    );
    c.note(
        "the instruction sign-extends the immediate into rax, so the two kinds only differ \
         once an address passes 0x7fffffff — which stage 27 is about",
    );
    c.finish()
});

link_test!(both_kinds_together, |ctx| {
    let mut code = Code::new();
    let unsigned_site = code.len() + 1;
    code.mov_r32_symbol_addr32(Reg::Rsi, "first", 0);
    code.sys_write_rsi(STDOUT, 3);
    let signed_site = code.len() + 3;
    code.mov_r64_symbol_addr32s(Reg::Rsi, "second", 0);
    code.sys_write_rsi(STDOUT, 4);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"one two\n".to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("first", ".rodata", 0).object(3))
            .symbol(SymbolSpec::global("second", ".rodata", 4).object(4)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("both 32-bit kinds in one .text"),
    )?;
    ctx.expect_output(&linked, "onetwo\n", 0)?;
    let base = linked.address_of(DEFAULT_ENTRY)?;
    let mut c = Check::new("two 32-bit immediates of different kinds");
    assert_abs32(
        &mut c,
        &linked.elf,
        "output.text[R_X86_64_32].imm32",
        base + unsigned_site,
        linked.address_of("first")?,
        0,
    );
    assert_abs32(
        &mut c,
        &linked.elf,
        "output.text[R_X86_64_32S].imm32",
        base + signed_site,
        linked.address_of("second")?,
        0,
    );
    c.finish()
});

link_test!(abs32_with_addend, |ctx| {
    let mut code = Code::new();
    let site_offset = code.len() + 1;
    code.mov_r32_symbol_addr32(Reg::Rsi, "blob", 4);
    code.sys_write_rsi(STDOUT, 4);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"skipkeep".to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("blob", ".rodata", 0).object(8)),
    )?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).label("mov esi, $blob+4"))?;
    ctx.expect_output(&linked, "keep", 0)?;
    let mut c = Check::new("an R_X86_64_32 with A = 4");
    assert_abs32(
        &mut c,
        &linked.elf,
        "output.text[mov].imm32",
        linked.address_of(DEFAULT_ENTRY)? + site_offset,
        linked.address_of("blob")?,
        4,
    );
    c.finish()
});

link_test!(abs32s_with_addend, |ctx| {
    // The symbol is in the middle of the blob and the addend walks back to its start.
    let mut code = Code::new();
    let site_offset = code.len() + 3;
    code.mov_r64_symbol_addr32s(Reg::Rsi, "middle", -4);
    code.sys_write_rsi(STDOUT, 8);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"headtail".to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("middle", ".rodata", 4).object(4)),
    )?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).label("mov rsi, $middle-4"))?;
    ctx.expect_output(&linked, "headtail", 0)?;
    let mut c = Check::new("an R_X86_64_32S with A = -4");
    assert_abs32(
        &mut c,
        &linked.elf,
        "output.text[mov].imm32",
        linked.address_of(DEFAULT_ENTRY)? + site_offset,
        linked.address_of("middle")?,
        -4,
    );
    c.note(
        "a negative addend on an absolute relocation is ordinary arithmetic on the address, \
         not a signal of anything: S + A is still the value stored",
    );
    c.finish()
});

link_test!(abs32_against_a_section_symbol, |ctx| {
    let mut code = Code::new();
    let site_offset = mov_esi_section_addr32(&mut code, ".rodata", 3);
    code.sys_write_rsi(STDOUT, 3);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"no\nef\n".to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("ro_base", ".rodata", 0).object(6)),
    )?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).label("mov esi, $.rodata+3"))?;
    ctx.expect_output(&linked, "ef\n", 0)?;
    let mut c = Check::new("an R_X86_64_32 whose symbol is a section");
    assert_abs32(
        &mut c,
        &linked.elf,
        "output.text[mov].imm32",
        linked.address_of(DEFAULT_ENTRY)? + site_offset,
        linked.address_of("ro_base")?,
        3,
    );
    c.note(
        "'ro_base' is a global the fixture placed at offset 0 of the same .rodata, so the \
         expectation is stated without knowing where the linker put the section",
    );
    c.finish()
});

link_test!(abs32_in_data, |ctx| {
    // A four-byte word of .data holding an address, loaded and dereferenced.
    let message = "word\n";
    let mut code = Code::new();
    code.mov_r32_rip(Reg::Rsi, "slot", 0);
    code.sys_write_rsi(STDOUT, message.len() as u32);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .section(
                SectionSpec::data(".data", vec![0u8; 4])
                    .align(4)
                    .reloc(Reloc::sym(0, "message", R_X86_64_32, 0)),
            )
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("message", ".rodata", 0).object(message.len() as u64))
            .symbol(SymbolSpec::global("slot", ".data", 0).object(4)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("a 32-bit address stored in .data"),
    )?;
    ctx.expect_output(&linked, message, 0)?;
    let mut c = Check::new("a 32-bit address living in a data section");
    assert_abs32(
        &mut c,
        &linked.elf,
        "output.data[slot]",
        linked.address_of("slot")?,
        linked.address_of("message")?,
        0,
    );
    c.note(
        "`mov esi, [rip+slot]` zero-extends into rsi, so the four bytes are the whole \
         pointer the write(2) is given — the program crashes if the high half was not zero",
    );
    c.finish()
});

link_test!(abs32s_across_objects, |ctx| {
    let mut code = Code::new();
    let site_offset = code.len() + 3;
    code.mov_r64_symbol_addr32s(Reg::Rsi, "remote", 0);
    code.sys_write_rsi(STDOUT, 7);
    code.sys_exit(0);
    let a = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::undefined("remote")),
    )?;
    let b = build(
        ObjectBuilder::new()
            .section(SectionSpec::rodata(".rodata", b"remote\n".to_vec()))
            .symbol(SymbolSpec::global("remote", ".rodata", 0).object(7)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("a 32S immediate resolved from the second object"),
    )?;
    ctx.expect_output(&linked, "remote\n", 0)?;
    let mut c = Check::new("a 32-bit immediate whose symbol came from another input");
    assert_abs32(
        &mut c,
        &linked.elf,
        "output.text[mov].imm32",
        linked.address_of(DEFAULT_ENTRY)? + site_offset,
        linked.address_of("remote")?,
        0,
    );
    c.finish()
});

/// Worked examples: the immediate that is an address, and the same thing in `.data`.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object("An address as an immediate", "ld -o prog imm.o", || {
            let message = "immediate\n";
            let mut code = Code::new();
            code.mov_r32_symbol_addr32(Reg::Rsi, "message", 0);
            code.sys_write_rsi(STDOUT, message.len() as u32);
            code.sys_exit(0);
            build(
                ObjectBuilder::new()
                    .section(text_of(&code))
                    .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
                    .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
                    .symbol(
                        SymbolSpec::global("message", ".rodata", 0).object(message.len() as u64),
                    ),
            )
            .map_err(|f| f.messages.join("; "))
        })
        .request(
            "imm.o: `b8 00 00 00 00` — `mov esi, imm32` whose immediate is zero and whose \
             R_X86_64_32 names `message', followed by a write(2) that uses rsi as the buffer",
        )
        .response(
            "The imm32 holds the final address of `message' zero-extended into 32 bits, and \
             the program prints the string: the four bytes are a complete pointer because a \
             static non-PIE image is mapped below 4 GB",
        )
        .note(
            "This is the small code model in one instruction. It is also why a linker that \
             lays its image out above 4 GB has to reject the relocation rather than \
             truncate it.",
        )
        .runs("immediate\n", 0),
        ExampleSpec::text("32 against 32S", "ld -o prog wide.o")
            .request(
                "The same address relocated twice, once as R_X86_64_32 and once as R_X86_64_32S",
            )
            .response(
                "Both fields hold the identical 32 bits, because every address in the image is \
             below 0x80000000; the kinds differ only in the range each is allowed to hold, \
             which is a check on the value and not a change to the arithmetic",
            )
            .note(
                "Share the S + A computation between the two and split only the fits-in-range \
             test; duplicating the arithmetic is how the two drift apart.",
            ),
    ]
}
