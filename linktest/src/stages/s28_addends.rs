//! Stage 28 — Addends, positive and negative.
//!
//! `SHT_RELA` relocations carry their addend in the relocation entry, not in the field being
//! patched, and the value stored is always `S + A` (or `S + A - P`). The addend is how a
//! compiler says `&array[3]`, `&s.field`, `"hello" + 2` or "the byte after this object" — it
//! is ordinary arithmetic on an address, and it is signed.
//!
//! Two mistakes live here: reading the addend out of the field instead of out of the
//! relocation, which works exactly as long as the field happens to be zero; and treating
//! `r_addend` as unsigned, which turns `-4` into four billion.

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
        number: 28,
        slug: "addends",
        name: "Addends, positive and negative",
        ext: false,
        hints: &[
            "r_addend is a signed 64-bit field of the relocation entry: read it as an i64, \
             add it to S, and never look at what the field being patched already holds",
            "In a SHT_RELA object the contents of the field are irrelevant — the assembler \
             usually leaves zeros there, but a linker that adds to them instead of storing \
             over them is only right by accident",
            "The -4 on every RIP-relative relocation is an addend like any other; there is \
             no special case for it, and adding a second -4 of your own doubles it",
            "An addend may point outside the symbol it names — one past the end is normal \
             and must not be rejected",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "a positive addend prints from the middle of a string",
                positive_addend,
            ),
            Test::new(
                "a negative addend walks back to the start of a string",
                negative_addend,
            ),
            Test::new(
                "an addend moves an R_X86_64_64 pointer",
                addend_on_a_pointer,
            ),
            Test::new("a large addend reaches across a big section", large_addend),
            Test::new(
                "an addend against a section symbol is the offset into the section",
                addend_on_a_section_symbol,
            ),
            Test::new(
                "an addend may land exactly on the end of a symbol",
                addend_at_the_end_of_a_symbol,
            ),
            Test::new("a zero addend is the plain address", zero_addend_control),
            Test::new(
                "the addend comes from the relocation, not from the field",
                field_contents_are_ignored,
            ),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

link_test!(positive_addend, |ctx| {
    // "hello\n" relocated to message + 2 prints "llo\n".
    let mut code = Code::new();
    let site_offset = code.len() + 3;
    code.lea_rip(Reg::Rsi, "message", 2);
    code.sys_write_rsi(STDOUT, 4);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"hello\n".to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("message", ".rodata", 0).object(6)),
    )?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).label("lea rsi, [message+2]"))?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "llo\n", 0)?;
    let mut c = Check::new("a displacement carrying a positive addend");
    // The encoder folded the -4 of the RIP-relative form together with the +2.
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[lea].disp32",
        linked.address_of(DEFAULT_ENTRY)? + site_offset,
        linked.address_of("message")?,
        -4 + 2,
    );
    c.note(
        "the program printing 'llo\\n' rather than 'hello\\n' is the run-time half of the \
         same claim: rsi was two bytes past the symbol",
    );
    c.finish()
});

link_test!(negative_addend, |ctx| {
    // 'tail' sits three bytes into the blob; A = -3 walks back to the front of it.
    let mut code = Code::new();
    let site_offset = code.len() + 3;
    code.lea_rip(Reg::Rsi, "tail", -3);
    code.sys_write_rsi(STDOUT, 6);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"abcdef".to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("tail", ".rodata", 3).object(3)),
    )?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).label("lea rsi, [tail-3]"))?;
    ctx.expect_output(&linked, "abcdef", 0)?;
    let mut c = Check::new("a displacement carrying a negative addend");
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[lea].disp32",
        linked.address_of(DEFAULT_ENTRY)? + site_offset,
        linked.address_of("tail")?,
        -4 - 3,
    );
    c.note(
        "r_addend is signed; read as an unsigned 64-bit value this addend becomes \
         0xfffffffffffffffd and the displacement lands nowhere",
    );
    c.finish()
});

link_test!(addend_on_a_pointer, |ctx| {
    let mut code = Code::new();
    code.mov_r64_rip(Reg::Rsi, "ptr", R_X86_64_PC32, -4);
    code.sys_write_rsi(STDOUT, 3);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"abcXYZ".to_vec()))
            .section(
                SectionSpec::data(".data", vec![0u8; 8])
                    .align(8)
                    .reloc(Reloc::sym(0, "blob", R_X86_64_64, 3)),
            )
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("blob", ".rodata", 0).object(6))
            .symbol(SymbolSpec::global("ptr", ".data", 0).object(8)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("an eight-byte pointer with A = 3"),
    )?;
    ctx.expect_output(&linked, "XYZ", 0)?;
    let mut c = Check::new("an addend on a 64-bit pointer");
    assert_abs64(
        &mut c,
        &linked.elf,
        "output.data[ptr]",
        linked.address_of("ptr")?,
        linked.address_of("blob")?,
        3,
    );
    c.finish()
});

link_test!(large_addend, |ctx| {
    // A 64 KiB pool with the message at the far end of it.
    let size = 0x1_0000usize;
    let offset = size - 8;
    let mut pool = vec![b'.'; size];
    pool[offset..offset + 4].copy_from_slice(b"far\n");
    let mut code = Code::new();
    let site_offset = code.len() + 3;
    code.lea_rip(Reg::Rsi, "pool", offset as i64);
    code.sys_write_rsi(STDOUT, 4);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", pool))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("pool", ".rodata", 0).object(size as u64)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("an addend of 0xfff8 into a 64 KiB pool"),
    )?;
    ctx.expect_output(&linked, "far\n", 0)?;
    let mut c = Check::new("a large positive addend");
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[lea].disp32",
        linked.address_of(DEFAULT_ENTRY)? + site_offset,
        linked.address_of("pool")?,
        -4 + offset as i64,
    );
    c.finish()
});

link_test!(addend_on_a_section_symbol, |ctx| {
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
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).label("lea rsi, [.rodata+3]"))?;
    ctx.expect_output(&linked, "yes\n", 0)?;
    let mut c = Check::new("an addend measured from the start of a section");
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[lea].disp32",
        linked.address_of(DEFAULT_ENTRY)? + site_offset,
        linked.address_of("ro_base")?,
        -4 + 3,
    );
    c.note(
        "for a section symbol S is where the section landed, so the addend is the whole of \
         the offset — 'ro_base' is a global the fixture pinned to offset 0 so the \
         expectation can be written without naming an address",
    );
    c.finish()
});

link_test!(addend_at_the_end_of_a_symbol, |ctx| {
    // A = 3 with a three-byte symbol points one past its end, which is legal and common.
    let mut code = Code::new();
    let site_offset = code.len() + 3;
    code.lea_rip(Reg::Rsi, "head", 3);
    code.sys_write_rsi(STDOUT, 4);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"abcdef\n".to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("head", ".rodata", 0).object(3))
            .symbol(SymbolSpec::global("rest", ".rodata", 3).object(4)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("an addend exactly one past the end"),
    )?;
    ctx.expect_output(&linked, "def\n", 0)?;
    let mut c = Check::new("an addend that lands on the end of its symbol");
    let base = linked.address_of(DEFAULT_ENTRY)? + site_offset;
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[lea].disp32",
        base,
        linked.address_of("head")?,
        -4 + 3,
    );
    // The same address, reached through the symbol that starts there.
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[lea].disp32 (as rest+0)",
        base,
        linked.address_of("rest")?,
        -4,
    );
    c.note(
        "st_size is documentation, not a bound: 'head + 3' is one past a three-byte object \
         and a linker must not treat that as an error",
    );
    c.finish()
});

link_test!(zero_addend_control, |ctx| {
    let message = "plain\n";
    let mut code = Code::new();
    let site_offset = code.len() + 3;
    code.lea_rip(Reg::Rsi, "message", 0);
    code.sys_write_rsi(STDOUT, message.len() as u32);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("message", ".rodata", 0).object(message.len() as u64)),
    )?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).label("the control case"))?;
    ctx.expect_output(&linked, message, 0)?;
    let mut c = Check::new("the same reference with no addend of its own");
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[lea].disp32",
        linked.address_of(DEFAULT_ENTRY)? + site_offset,
        linked.address_of("message")?,
        -4,
    );
    c.note(
        "the -4 is still an addend; it is the one the RIP-relative encoding always needs, \
         and the tests above only add to it",
    );
    c.finish()
});

link_test!(field_contents_are_ignored, |ctx| {
    // The disp32 is pre-filled with 0x7f7f7f7f. A SHT_RELA relocation must overwrite it.
    let message = "over\n";
    let mut code = Code::new();
    code.raw(&[0x48, 0x8d, 0x35]); // lea rsi, [rip + disp32]
    let site_offset = code.len();
    code.reloc_here(RelTarget::Symbol("message".to_string()), R_X86_64_PC32, -4);
    code.raw(&0x7f7f_7f7fu32.to_le_bytes());
    code.sys_write_rsi(STDOUT, message.len() as u32);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("message", ".rodata", 0).object(message.len() as u64)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("a field pre-filled with rubbish"),
    )?;
    ctx.expect_output(&linked, message, 0)?;
    let mut c = Check::new("a RELA field whose previous contents must be discarded");
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[lea].disp32",
        linked.address_of(DEFAULT_ENTRY)? + site_offset,
        linked.address_of("message")?,
        -4,
    );
    c.note(
        "in SHT_REL objects the field *is* the addend; in SHT_RELA — which is all x86-64 \
         uses — it is dead space, and a linker that adds to it lands 0x7f7f7f7f bytes away",
    );
    c.finish()
});

/// Worked examples: the addend as a pointer into the middle of a string.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "Printing from the middle of a string",
            "ld -o prog middle.o",
            || {
                let mut code = Code::new();
                code.lea_rip(Reg::Rsi, "message", 2);
                code.sys_write_rsi(STDOUT, 4);
                code.sys_exit(0);
                build(
                    ObjectBuilder::new()
                        .section(text_of(&code))
                        .section(SectionSpec::rodata(".rodata", b"hello\n".to_vec()))
                        .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
                        .symbol(SymbolSpec::global("message", ".rodata", 0).object(6)),
                )
                .map_err(|f| f.messages.join("; "))
            },
        )
        .request(
            ".rodata holds \"hello\\n\" and the lea's relocation names `message' with an \
             r_addend of -2: the -4 the RIP-relative form always needs, plus the +2 that \
             skips the first two characters",
        )
        .response(
            "The displacement is S + A - P with that addend, so rsi points at the third byte \
             of the string and the program prints \"llo\\n\"",
        )
        .note(
            "The output tells you which half is wrong on its own: \"hello\\n\" means the \
             addend was dropped, a crash means it was applied twice or read unsigned.",
        )
        .runs("llo\n", 0),
        ExampleSpec::text("Where the addend lives", "ld -o prog rela.o")
            .request(
                "An SHT_RELA section: each entry is r_offset, r_info and r_addend, and the four \
             bytes at r_offset hold 0x7f7f7f7f rather than zero",
            )
            .response(
                "The linker stores S + A - P over those four bytes. What they held before is \
             never read: RELA carries its addend in the entry, which is precisely why \
             x86-64 chose it over REL",
            )
            .note(
                "Assemblers do leave zeros there, so reading the addend from the field appears \
             to work on every object you are likely to build by hand.",
            ),
    ]
}
