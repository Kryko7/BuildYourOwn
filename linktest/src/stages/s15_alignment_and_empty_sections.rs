//! Stage 15 — Alignment and empty sections.
//!
//! The edges of the concatenation loop. An input section can demand any power-of-two
//! alignment from 1 to a great deal more than a page, can declare `sh_addralign` 0 (which
//! means 1), and can be completely empty — and a linker that special-cases none of these
//! ends up with an off-by-one somewhere: a zero-size section that still advances the
//! address, an `sh_addralign` of 0 used as a divisor, a 64 KiB table that breaks the page
//! congruence because the file was padded and the address was not.

use crate::asm::{Code, STDOUT};
use crate::assert::{Check, Failure};
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
        number: 15,
        slug: "alignment_and_empty_sections",
        name: "Alignment and empty sections",
        ext: true,
        hints: &[
            "Round the running address up with `(addr + align - 1) & !(align - 1)`, and \
             treat an `sh_addralign` of 0 or 1 as no constraint rather than as a divisor",
            "A zero-size section contributes nothing: do not let it advance the address, and \
             do not let it create an output section out of nothing",
            "An alignment larger than a page raises the segment's `p_align`, and the page \
             congruence still has to hold — pad the file to match, or pick the offset from \
             the address",
            "Symbols in an empty section are legal; they just all have the same address, and \
             they must not end up pointing outside every segment",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "sections aligned to 16, 64, 256, 4096 and 65536 bytes all land aligned",
                every_alignment,
            )
            .ext(),
            Test::new(
                "an sh_addralign of 0 is treated as 1",
                alignment_zero_means_one,
            )
            .ext(),
            Test::new(
                "an empty .text in one object does not break the link",
                an_empty_text,
            )
            .ext(),
            Test::new("a zero-size .rodata is harmless", a_zero_size_rodata).ext(),
            Test::new(
                "alignment is respected when several objects contribute the same section",
                alignment_across_objects,
            )
            .ext(),
            Test::new(
                "a 64 KiB alignment does not break the page congruence",
                huge_alignment_keeps_the_congruence,
            )
            .ext(),
            Test::new(
                "an object that contributes nothing but an empty section still links",
                an_object_of_nothing,
            )
            .ext(),
            Test::new(
                "the padding between two aligned contributions corrupts neither",
                padding_does_not_corrupt,
            )
            .ext(),
        ],
    }
}

link_test!(every_alignment, |ctx| {
    for align in [16u64, 64, 256, 4096, 0x10000] {
        let linked = ctx.link_ok(
            &Link::new()
                .object("a.o", aligned_rodata(align)?)
                .out(&format!("prog{align:x}"))
                .label(&format!(
                    "one object whose .rodata wants {align}-byte alignment"
                )),
        )?;
        let addr = linked.address_of("big")?;
        let mut c = Check::new(format!("a .rodata with sh_addralign {align}"));
        c.that(
            "output.symbol['big']",
            &format!("an address that is a multiple of {align} (0x{align:x})"),
            addr % align == 0,
            format!(
                "0x{addr:x}, which is 0x{:x} past the boundary",
                addr % align
            ),
        );
        if let Some(section) = linked.elf.section(".rodata") {
            c.that(
                "output.section['.rodata'].sh_addr",
                "aligned too, since the contribution starts at the section's start here",
                section.addr % align == 0,
                format!("0x{:x}", section.addr),
            );
        }
        if !c.ok() {
            c.block("output section headers", linked.elf.section_header_table());
        }
        c.finish()?;
        assert_runnable_layout(&linked)?;
        ctx.expect_output(&linked, "ok\n", 0)?;
    }
    ctx.note(
        "five separate links, one per alignment; the biggest one usually makes the output \
         file much larger, because the file is padded to keep p_offset congruent to p_vaddr",
    );
    Ok(())
});

link_test!(alignment_zero_means_one, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", aligned_rodata(0)?)
            .label("an object whose .rodata declares sh_addralign 0"),
    )?;
    assert_runnable_layout(&linked)?;
    let addr = linked.address_of("big")?;
    let mut c = Check::new("a section header that declares no alignment at all");
    c.that(
        "output.symbol['big']",
        "some address inside a PT_LOAD — sh_addralign 0 means 'no constraint', which is the \
         same as 1, and must never be used as a divisor or a mask",
        linked.elf.segment_at(addr).is_some(),
        segment_summary(&linked.elf, addr),
    );
    c.finish()?;
    ctx.expect_output(&linked, "ok\n", 0)?;
    Ok(())
});

link_test!(an_empty_text, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("e.o", empty_text()?)
            .object("b.o", print_and_exit("still\n", 0)?)
            .label("an object with a zero-size .text, then a real program"),
    )?;
    assert_runnable_layout(&linked)?;
    assert_entry_is(&linked, DEFAULT_ENTRY)?;
    let start = linked.address_of(DEFAULT_ENTRY)?;
    let nothing = linked.address_of("nothing")?;
    let mut c = Check::new("what the zero-size .text contributed");
    c.that(
        "output.symbol['nothing']",
        "an address inside a PT_LOAD: a symbol in an empty section is still a symbol",
        linked.elf.segment_at(nothing).is_some(),
        segment_summary(&linked.elf, nothing),
    );
    c.observe("output.symbol['_start']", format!("0x{start:x}"));
    c.observe("output.symbol['nothing']", format!("0x{nothing:x}"));
    c.note(
        "a zero-size contribution occupies no space, so 'nothing' and '_start' may well share \
         an address — that is correct, not a collision",
    );
    c.finish()?;
    ctx.expect_output(&linked, "still\n", 0)?;
    Ok(())
});

link_test!(a_zero_size_rodata, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("z.o", empty_rodata()?)
            .object("b.o", print_and_exit("zr\n", 6)?)
            .label("an object with a zero-size .rodata, then a real program"),
    )?;
    assert_runnable_layout(&linked)?;
    let mut c = Check::new("the output .rodata after an empty contribution was folded in");
    match linked.elf.section(".rodata") {
        Some(s) => {
            c.eq("output.section['.rodata'].sh_size", 3u64, s.size);
            c.note(
                "the empty contribution added nothing to the output section's size; only the \
                 three bytes of the real program's string are there",
            );
        }
        None => {
            c.note(
                "the output has no .rodata section header; only the run-time behaviour is \
                 checked",
            );
        }
    }
    c.finish()?;
    ctx.expect_output(&linked, "zr\n", 6)?;
    Ok(())
});

link_test!(alignment_across_objects, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", aligned_pair_head(256)?)
            .object("b.o", aligned_tail(256)?)
            .label("two objects whose .rodata contributions both want 256-byte alignment"),
    )?;
    let a = linked.address_of("sa")?;
    let b = linked.address_of("sb")?;
    let mut c = Check::new("two same-named contributions, each with its own alignment");
    c.that(
        "output.symbol['sa']",
        "a multiple of 256",
        a % 256 == 0,
        format!("0x{a:x}"),
    );
    c.that(
        "output.symbol['sb']",
        "a multiple of 256 as well — the second contribution is aligned on its own account, \
         not merely appended",
        b % 256 == 0,
        format!("0x{b:x}"),
    );
    c.at_least("output.symbol['sb'] - output.symbol['sa']", 256u64, b - a);
    if !c.ok() {
        c.block("output section headers", linked.elf.section_header_table());
    }
    c.finish()?;
    ctx.expect_output(&linked, "A\nB\n", 0)?;
    Ok(())
});

link_test!(huge_alignment_keeps_the_congruence, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", aligned_rodata(0x10000)?)
            .label("an object whose .rodata wants sixteen pages of alignment"),
    )?;
    assert_runnable_layout(&linked)?;
    let exe = &linked.elf;
    let mut c = Check::new("the mmap congruence under a 64 KiB section alignment");
    for s in exe.loads() {
        c.that(
            &format!("output.segment[{}]", s.index),
            "p_offset ≡ p_vaddr (mod 0x1000), whatever p_align was raised to",
            s.offset % PAGE_SIZE == s.vaddr % PAGE_SIZE,
            format!("p_offset 0x{:x}, p_vaddr 0x{:x}", s.offset, s.vaddr),
        );
        c.that(
            &format!("output.segment[{}].p_align", s.index),
            "still a power of two",
            s.align != 0 && s.align.is_power_of_two(),
            format!("0x{:x}", s.align),
        );
    }
    let addr = linked.address_of("big")?;
    c.that(
        "output.symbol['big']",
        "a multiple of 0x10000",
        addr % 0x10000 == 0,
        format!("0x{addr:x}"),
    );
    if !c.ok() {
        c.block("output program headers", exe.program_header_table());
    }
    c.finish()?;
    ctx.expect_output(&linked, "ok\n", 0)?;
    Ok(())
});

link_test!(an_object_of_nothing, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("nothing.o", nothing_at_all()?)
            .object("b.o", print_and_exit("anyway\n", 2)?)
            .label("an object whose only section is an empty .data"),
    )?;
    assert_runnable_layout(&linked)?;
    assert_entry_is(&linked, DEFAULT_ENTRY)?;
    ctx.note(
        "an input that contributes no bytes and no symbols still has to be read, parsed and \
         accepted; a linker that indexes its first section unconditionally falls over here",
    );
    ctx.expect_output(&linked, "anyway\n", 2)?;
    Ok(())
});

link_test!(padding_does_not_corrupt, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", aligned_pair_head(256)?)
            .object("b.o", aligned_tail(256)?)
            .label("two 256-byte-aligned contributions with padding between them"),
    )?;
    let exe = &linked.elf;
    let a = linked.address_of("sa")?;
    let b = linked.address_of("sb")?;
    let mut c = Check::new("the two strings, read back across the padding between them");
    match exe.read_at_vaddr(a, 2) {
        Ok(got) => c.bytes_eq("output[sa]", b"A\n", got),
        Err(e) => c.that("output[sa]", "readable", false, e.to_string()),
    };
    match exe.read_at_vaddr(b, 2) {
        Ok(got) => c.bytes_eq("output[sb]", b"B\n", got),
        Err(e) => c.that("output[sb]", "readable", false, e.to_string()),
    };
    c.note(
        "whatever the linker put in the 254 bytes of padding — zeros, or nothing at all \
         because the file was extended — the two strings have to be intact at the addresses \
         the symbol table gives them",
    );
    c.finish()?;
    ctx.expect_output(&linked, "A\nB\n", 0)?;
    Ok(())
});

// ---------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------

/// One object whose `.rodata` holds `ok\n` and declares `sh_addralign = align`.
fn aligned_rodata(align: u64) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "big", 3);
    code.sys_exit(0);
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"ok\n".to_vec()).align(align))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("big", ".rodata", 0).object(3)),
    )
}

/// An object whose `.text` is present but zero bytes long, with a symbol in it.
fn empty_text() -> Result<Vec<u8>, Failure> {
    build(
        ObjectBuilder::new()
            .section(SectionSpec::text(".text", Vec::new()))
            .symbol(SymbolSpec::global("nothing", ".text", 0).func()),
    )
}

/// An object whose `.rodata` is present but zero bytes long.
fn empty_rodata() -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.ret();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", Vec::new()))
            .symbol(SymbolSpec::global("empty_fn", ".text", 0).func()),
    )
}

/// An object whose only section is an empty `.data` and which defines no symbol at all.
fn nothing_at_all() -> Result<Vec<u8>, Failure> {
    build(ObjectBuilder::new().section(SectionSpec::data(".data", Vec::new()).align(4)))
}

/// `_start`: print both strings and exit 0. Owns the first 256-byte-aligned `.rodata`.
fn aligned_pair_head(align: u64) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "sa", 2);
    code.sys_write(STDOUT, "sb", 2);
    code.sys_exit(0);
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"A\n".to_vec()).align(align))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("sa", ".rodata", 0).object(2))
            .symbol(SymbolSpec::undefined("sb")),
    )
}

/// The second contribution to the same `.rodata`, wanting the same alignment.
fn aligned_tail(align: u64) -> Result<Vec<u8>, Failure> {
    build(
        ObjectBuilder::new()
            .section(SectionSpec::rodata(".rodata", b"B\n".to_vec()).align(align))
            .symbol(SymbolSpec::global("sb", ".rodata", 0).object(2)),
    )
}

/// Worked examples: the edges of the layout loop.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "Two contributions, both wanting 256-byte alignment",
            "ld -o prog a.o b.o",
            || aligned_pair_head(256).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "a.o: .rodata holds `A\\n` with sh_addralign 256 and defines sa; .text writes sa \
             then the undefined sb and exits 0. b.o is nothing but a second .rodata holding \
             `B\\n`, also with sh_addralign 256, defining sb",
        )
        .response(
            "Both sa and sb at addresses that are multiples of 256, at least 256 bytes \
             apart, with 254 bytes of padding between them; both strings readable, unchanged, \
             at the addresses the output's symbol table gives them",
        )
        .note(
            "Aligning the output section once and then appending contributions is not \
             enough: the second contribution has its own sh_addralign, and the running \
             address has to be rounded up again before it is placed.",
        )
        .runs("A\nB\n", 0),
        ExampleSpec::object(
            "A section that declares no alignment",
            "ld -o prog zero.o",
            || aligned_rodata(0).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "zero.o: .rodata holds `ok\\n` and declares sh_addralign 0 — which the ELF spec \
             defines as 'no alignment constraint', exactly like 1",
        )
        .response(
            "A normal executable that prints `ok` and exits 0; the section may be placed \
             anywhere, and the only wrong answers are a division by zero and a mask of \
             `0 - 1`",
        )
        .note(
            "`addr & !(align - 1)` with align 0 masks the address with `!0u64.wrapping_sub(1)` \
             — which is how a linker ends up placing a section at address 0 and producing a \
             file that will not start.",
        )
        .runs("ok\n", 0),
    ]
}
