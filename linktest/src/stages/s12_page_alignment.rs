//! Stage 12 — Page alignment and the `mmap` congruence.
//!
//! The kernel does not copy a segment into memory: it maps it, and `mmap` can only start a
//! mapping at a page-aligned file offset. So for every `PT_LOAD` the file offset and the
//! virtual address have to agree modulo the page size — `p_offset ≡ p_vaddr (mod 0x1000)`.
//! Get it wrong and `execve` returns `ENOEXEC`, or worse, the program starts with its code
//! shifted by a few bytes.

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
        number: 12,
        slug: "page_alignment",
        name: "Page alignment and the mmap congruence",
        ext: false,
        hints: &[
            "For every `PT_LOAD`, `p_offset % 0x1000` must equal `p_vaddr % 0x1000`; the \
             easy way to guarantee it is to start each segment on a page boundary in both",
            "`p_align` is the alignment the segment was laid out for: a power of two, and at \
             least the page size for anything the kernel maps",
            "Rounding the address up to a page and the offset up to a page independently is \
             the classic bug — round one of them and then derive the other",
            "A section whose `sh_addralign` is larger than a page (a 64 KiB table, say) \
             raises the segment's `p_align` too, and the congruence still has to hold",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "every PT_LOAD has p_offset congruent to p_vaddr modulo the page size",
                the_congruence,
            ),
            Test::new(
                "every PT_LOAD's p_align is a power of two and at least a page",
                p_align_is_sane,
            ),
            Test::new(
                "the kernel runs the result, which is the real judge",
                the_kernel_agrees,
            ),
            Test::new(
                "a .rodata that spills onto a second page still runs with its bytes intact",
                two_pages_of_rodata,
            ),
            Test::new(
                "a section aligned to 64 KiB keeps the congruence",
                a_huge_section_alignment,
            ),
            Test::new(
                "the loadable segments come out in increasing address order",
                segments_in_address_order,
            ),
            Test::new(
                "a program with .text, .rodata, .data and .bss is congruent everywhere",
                every_segment_of_a_full_program,
            ),
            Test::new(
                "every segment's file range lies inside the file",
                file_ranges_are_inside_the_file,
            ),
        ],
    }
}

link_test!(the_congruence, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", print_and_exit("page\n", 0)?))?;
    assert_congruence(&linked.elf)?;
    ctx.expect_output(&linked, "page\n", 0)?;
    Ok(())
});

link_test!(p_align_is_sane, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", bss_prober("al\n", 4096, 5)?))?;
    let exe = &linked.elf;
    let mut c = Check::new("p_align on every loadable segment");
    for s in exe.loads() {
        c.that(
            &format!("output.segment[{}].p_align", s.index),
            "a power of two",
            s.align != 0 && s.align.is_power_of_two(),
            format!("0x{:x}", s.align),
        );
        c.that(
            &format!("output.segment[{}].p_align", s.index),
            "at least the 4 KiB page size — the kernel maps in pages whatever the header says",
            s.align >= PAGE_SIZE,
            format!("0x{:x}", s.align),
        );
    }
    if !c.ok() {
        c.block("output program headers", exe.program_header_table());
    }
    c.finish()?;
    ctx.expect_output(&linked, "al\n", 5)?;
    Ok(())
});

link_test!(the_kernel_agrees, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", print_and_exit("judge\n", 7)?))?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "judge\n", 7)?;
    ctx.note(
        "execve is the only authority on whether the congruence holds: a file that violates \
         it either fails to start or starts with its code offset by a few bytes",
    );
    Ok(())
});

link_test!(two_pages_of_rodata, |ctx| {
    let (obj, tail_offset) = spilling_rodata()?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj))?;
    assert_congruence(&linked.elf)?;
    let head = linked.address_of("blob")?;
    let tail = linked.address_of("tail")?;
    let mut c = Check::new("a .rodata larger than one page, read back out of the output");
    c.addr_eq("output.symbol['tail']", head + tail_offset, tail);
    match linked.elf.read_at_vaddr(head, 3) {
        Ok(got) => c.bytes_eq("output[blob..blob+3]", b"hi\n", got),
        Err(e) => c.that("output[blob..blob+3]", "readable", false, e.to_string()),
    };
    match linked.elf.read_at_vaddr(tail, 3) {
        Ok(got) => c.bytes_eq("output[tail..tail+3]", b"ZZ\n", got),
        Err(e) => c.that("output[tail..tail+3]", "readable", false, e.to_string()),
    };
    c.that(
        "output.symbol['blob'] and output.symbol['tail']",
        "addresses on different pages — the point of the fixture",
        head / PAGE_SIZE != tail / PAGE_SIZE,
        format!("0x{head:x} and 0x{tail:x}"),
    );
    c.finish()?;
    ctx.expect_output(&linked, "hi\nZZ\n", 0)?;
    Ok(())
});

link_test!(a_huge_section_alignment, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", aligned_rodata(0x10000)?))?;
    assert_congruence(&linked.elf)?;
    assert_runnable_layout(&linked)?;
    let addr = linked.address_of("big")?;
    let mut c = Check::new("a section whose sh_addralign is sixteen pages");
    c.that(
        "output.symbol['big']",
        "an address that is a multiple of 0x10000",
        addr % 0x10000 == 0,
        format!("0x{addr:x}"),
    );
    if let Some(seg) = linked.elf.segment_at(addr) {
        c.observe("output.segment(big).p_align", format!("0x{:x}", seg.align));
    }
    c.note(
        "a p_align larger than a page is fine and common; what matters is that p_offset and \
         p_vaddr stay congruent modulo 0x1000, so the file usually grows to match",
    );
    c.finish()?;
    ctx.expect_output(&linked, "ok\n", 0)?;
    Ok(())
});

link_test!(segments_in_address_order, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", bss_prober("ord\n", 2048, 6)?))?;
    let exe = &linked.elf;
    let addrs: Vec<u64> = exe.loads().map(|s| s.vaddr).collect();
    let sorted = addrs.windows(2).all(|w| w[0] <= w[1]);
    if sorted {
        ctx.note(format!(
            "the {} PT_LOAD segments are in increasing address order, which is what every \
             linker does and what readelf assumes",
            addrs.len()
        ));
    } else {
        ctx.note(
            "this linker emitted its PT_LOAD segments out of address order; the ABI does not \
             require the order, only that the segments do not overlap, so the suite records \
             it rather than failing",
        );
    }
    let mut c = Check::new("the loadable segments, as ranges of memory");
    for s in exe.loads() {
        c.observe(&format!("output.segment[{}]", s.index), s.describe());
    }
    // The invariant that *is* required, whatever the order: no two of them overlap.
    let loads: Vec<_> = exe.loads().copied().collect();
    for (i, a) in loads.iter().enumerate() {
        for b in loads.iter().skip(i + 1) {
            let overlap = a.memsz > 0
                && b.memsz > 0
                && a.vaddr < b.vaddr.saturating_add(b.memsz)
                && b.vaddr < a.vaddr.saturating_add(a.memsz);
            c.that(
                "output.PT_LOAD",
                "segments that do not overlap, in whatever order they are listed",
                !overlap,
                format!("{} overlaps {}", a.describe(), b.describe()),
            );
        }
    }
    c.finish()?;
    ctx.expect_output(&linked, "ord\n", 6)?;
    Ok(())
});

link_test!(every_segment_of_a_full_program, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", print_and_exit("f1\n", 0)?)
            .object("b.o", bss_and_data_only()?)
            .label("a printing object and one that only contributes .data and .bss"),
    )?;
    assert_congruence(&linked.elf)?;
    assert_runnable_layout(&linked)?;
    let mut c = Check::new("how many distinct kinds of section this output carries");
    for name in [".text", ".rodata", ".data", ".bss"] {
        c.observe(
            &format!("output.section['{name}']"),
            match linked.elf.section(name) {
                Some(s) => format!("addr 0x{:x} size 0x{:x}", s.addr, s.size),
                None => "absent".to_string(),
            },
        );
    }
    c.finish()?;
    ctx.expect_output(&linked, "f1\n", 0)?;
    Ok(())
});

link_test!(file_ranges_are_inside_the_file, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", bss_prober("rng\n", 65536, 9)?))?;
    let exe = &linked.elf;
    let len = linked.bytes.len() as u64;
    let mut c = Check::new("the file ranges the program headers claim");
    for s in &exe.segments {
        c.that(
            &format!("output.segment[{}].p_offset + p_filesz", s.index),
            "inside the file the linker wrote",
            s.offset.saturating_add(s.filesz) <= len,
            format!(
                "0x{:x}..0x{:x} of a 0x{len:x}-byte file",
                s.offset,
                s.offset + s.filesz
            ),
        );
    }
    c.that(
        "output.size",
        "a file far smaller than the 64 KiB of .bss it maps — .bss costs memory, not bytes",
        len < 64 * 1024,
        len,
    );
    if !c.ok() {
        c.block("output program headers", exe.program_header_table());
    }
    c.finish()?;
    ctx.expect_output(&linked, "rng\n", 9)?;
    Ok(())
});

/// `p_offset ≡ p_vaddr (mod PAGE_SIZE)` for every loadable segment.
fn assert_congruence(exe: &crate::elf::read::Elf) -> Result<(), Failure> {
    let mut c = Check::new("the mmap congruence, for every loadable segment");
    for s in exe.loads() {
        c.that(
            &format!("output.segment[{}]", s.index),
            "p_offset ≡ p_vaddr (mod 0x1000) — mmap can only start a mapping on a page \
             boundary, so the two have to share their offset within the page",
            s.offset % PAGE_SIZE == s.vaddr % PAGE_SIZE,
            format!(
                "p_offset 0x{:x} (mod 0x1000 = 0x{:x}) vs p_vaddr 0x{:x} (mod 0x1000 = 0x{:x})",
                s.offset,
                s.offset % PAGE_SIZE,
                s.vaddr,
                s.vaddr % PAGE_SIZE
            ),
        );
    }
    if !c.ok() {
        c.block("output program headers", exe.program_header_table());
        c.note(
            "the fix is to pick the address first and then place the bytes at a file offset \
             with the same remainder, padding the file if necessary",
        );
    }
    c.finish()
}

/// An object whose `.rodata` is nine kilobytes: a marker at offset 0, another at 8000, and
/// `_start` prints both. Returns the object and the offset of the second marker.
fn spilling_rodata() -> Result<(Vec<u8>, u64), Failure> {
    const TAIL: usize = 8000;
    let mut blob = vec![b'.'; 9000];
    blob[..3].copy_from_slice(b"hi\n");
    blob[TAIL..TAIL + 3].copy_from_slice(b"ZZ\n");
    let mut code = Code::new();
    code.sys_write(STDOUT, "blob", 3);
    code.sys_write(STDOUT, "tail", 3);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", blob))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("blob", ".rodata", 0).object(9000))
            .symbol(SymbolSpec::global("tail", ".rodata", TAIL as u64).object(3)),
    )?;
    Ok((obj, TAIL as u64))
}

/// An object whose `.rodata` demands `align` bytes of alignment and holds `ok\n`.
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

/// An object with no code at all: it only contributes a `.data` word and a `.bss` buffer.
fn bss_and_data_only() -> Result<Vec<u8>, Failure> {
    build(
        ObjectBuilder::new()
            .section(SectionSpec::data(".data", vec![9, 0, 0, 0]).align(4))
            .section(SectionSpec::bss(".bss", 512).align(32))
            .symbol(SymbolSpec::global("shared_word", ".data", 0).object(4))
            .symbol(SymbolSpec::global("shared_buf", ".bss", 0).object(512)),
    )
}

/// Worked examples: why the remainder matters.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "Nine kilobytes of read-only data",
            "ld -o prog spill.o",
            || {
                spilling_rodata()
                    .map(|(bytes, _)| bytes)
                    .map_err(|f| f.messages.join("; "))
            },
        )
        .request(
            "spill.o: a 9000-byte .rodata with `hi\\n` at offset 0 and `ZZ\\n` at offset 8000, \
             and a .text that writes both — so the data cannot fit on one page",
        )
        .response(
            "A PT_LOAD covering the whole 9000 bytes, with p_offset ≡ p_vaddr (mod 0x1000); \
             both markers readable through the output at the addresses the symbol table \
             gives them; the program prints `hi` then `ZZ`",
        )
        .note(
            "Rounding p_vaddr up to a page and p_offset up to a page separately looks right \
             in readelf and fails here: the two remainders drift apart as soon as a segment \
             does not start at a page boundary in the file.",
        )
        .runs("hi\nZZ\n", 0),
        ExampleSpec::object(
            "A section that wants sixteen pages of alignment",
            "ld -o prog aligned.o",
            || aligned_rodata(0x10000).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "aligned.o: .rodata holds `ok\\n` and declares sh_addralign 0x10000; .text writes \
             it and exits 0",
        )
        .response(
            "`big` at an address that is a multiple of 0x10000, the covering PT_LOAD's \
             p_align raised to match, and p_offset still congruent to p_vaddr modulo 0x1000",
        )
        .note(
            "GNU ld pads the file out to the same 64 KiB boundary so that the congruence \
             holds trivially. A linker may instead keep the file compact as long as the two \
             remainders still agree.",
        )
        .runs("ok\n", 0),
    ]
}
