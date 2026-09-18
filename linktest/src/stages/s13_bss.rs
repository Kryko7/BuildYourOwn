//! Stage 13 — `.bss`: memory without file bytes.
//!
//! `.bss` is the one section that exists in memory and not in the file. `SHT_NOBITS` says
//! "give me this much space, zeroed, and store nothing"; the linker honours it by advancing
//! the address but not the file offset, and by making the covering segment's `p_memsz`
//! larger than its `p_filesz`. The kernel does the rest: everything past `p_filesz` in the
//! last page is zeroed and everything past that page is an anonymous zero mapping.

use crate::asm::{Code, Reg};
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
        number: 13,
        slug: "bss",
        name: ".bss: memory without file bytes",
        ext: false,
        hints: &[
            "An `SHT_NOBITS` input section has a size but no bytes: advance the output \
             address by its size and copy nothing, because `sh_offset` points at other \
             sections' data",
            "The segment that covers `.bss` gets `p_memsz` larger than `p_filesz`; the \
             difference is what the kernel zero-fills, and it is the whole trick",
            "Put every `SHT_NOBITS` section last inside its segment — file bytes have to be \
             contiguous from `p_offset`, so nothing with real bytes may follow the hole",
            "`.bss` must land in a writable, non-executable segment, and a megabyte of it \
             must not add a megabyte to the output file",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(".bss comes out of the linker as SHT_NOBITS", bss_is_nobits),
            Test::new(
                ".bss occupies no bytes in the output file",
                bss_has_no_file_bytes,
            ),
            Test::new(
                "the PT_LOAD covering .bss has p_memsz larger than p_filesz",
                memsz_exceeds_filesz,
            ),
            Test::new("the .bss bytes are zero at run time", bss_is_zeroed),
            Test::new(
                "a program writes to .bss and reads it back",
                bss_is_writable,
            ),
            Test::new(
                ".bss from two objects is allocated for both",
                bss_from_two_objects,
            ),
            Test::new(
                "a one-megabyte .bss does not make the output a megabyte bigger",
                a_megabyte_costs_nothing,
            ),
            Test::new(
                ".bss is not inside a read-only segment",
                bss_is_not_read_only,
            ),
        ],
    }
}

link_test!(bss_is_nobits, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", bss_prober("nb\n", 4096, 5)?))?;
    let exe = &linked.elf;
    let mut c = Check::new("the .bss section header in the output");
    match exe.section(".bss") {
        Some(bss) => {
            c.eq("output.section['.bss'].sh_type", SHT_NOBITS, bss.sh_type);
            c.that(
                "output.section['.bss'].sh_flags",
                "SHF_ALLOC | SHF_WRITE",
                bss.is_alloc() && bss.is_write(),
                format!("0x{:x}", bss.flags),
            );
            c.at_least("output.section['.bss'].sh_size", 4096u64, bss.size);
            c.that(
                "output.section['.bss'].sh_addr",
                "an address inside a PT_LOAD",
                exe.segment_at(bss.addr).is_some(),
                segment_summary(exe, bss.addr),
            );
        }
        None => {
            // A linker is free to rename or merge the output section, so fall back to the
            // symbol: what matters is that `scratch` is mapped and outside the file image.
            let addr = linked.address_of("scratch")?;
            c.note(
                "the output has no section named .bss; the check falls back to the address \
                 of 'scratch', which is what the program actually uses",
            );
            c.that(
                "output.symbol['scratch']",
                "an address inside a PT_LOAD",
                exe.segment_at(addr).is_some(),
                segment_summary(exe, addr),
            );
        }
    }
    if !c.ok() {
        c.block("output section headers", exe.section_header_table());
    }
    c.finish()?;
    ctx.expect_output(&linked, "nb\n", 5)?;
    Ok(())
});

link_test!(bss_has_no_file_bytes, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", bss_prober("nf\n", 8192, 3)?))?;
    let exe = &linked.elf;
    let addr = linked.address_of("scratch")?;
    let mut c = Check::new("whether .bss takes up room in the file");
    match exe.segment_at(addr) {
        Some(seg) => {
            c.that(
                &format!("output.segment[{}].p_filesz", seg.index),
                "ending at or before .bss starts — an SHT_NOBITS section has no bytes in the \
                 file, so nothing past p_filesz may be read from it",
                seg.filesz <= addr - seg.vaddr,
                format!(
                    "p_filesz 0x{:x}, .bss starts 0x{:x} into the segment",
                    seg.filesz,
                    addr - seg.vaddr
                ),
            );
        }
        None => {
            c.that(
                "output.segment(.bss)",
                "some PT_LOAD covering .bss",
                false,
                format!("0x{addr:x} is in no PT_LOAD"),
            );
        }
    }
    c.that(
        "output.read_at_vaddr(scratch)",
        "unreadable through the file image — the bytes exist only once the kernel has \
         mapped them",
        exe.read_at_vaddr(addr, 1).is_err(),
        match exe.read_at_vaddr(addr, 1) {
            Ok(b) => format!("the file has {b:02x?} there"),
            Err(e) => e.to_string(),
        },
    );
    if !c.ok() {
        c.block("output program headers", exe.program_header_table());
    }
    c.finish()?;
    ctx.expect_output(&linked, "nf\n", 3)?;
    Ok(())
});

link_test!(memsz_exceeds_filesz, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", bss_prober("ms\n", 4096, 4)?))?;
    let exe = &linked.elf;
    let addr = linked.address_of("scratch")?;
    let mut c = Check::new("the memory image against the file image of the segment with .bss");
    match exe.segment_at(addr) {
        Some(seg) => {
            c.that(
                &format!("output.segment[{}]", seg.index),
                "p_memsz strictly larger than p_filesz — the difference is the .bss the \
                 kernel zero-fills",
                seg.memsz > seg.filesz,
                seg.describe(),
            );
            c.at_least(
                &format!("output.segment[{}].p_memsz - p_filesz", seg.index),
                4096u64,
                seg.memsz - seg.filesz,
            );
        }
        None => {
            c.that(
                "output.segment(.bss)",
                "some PT_LOAD covering .bss",
                false,
                format!("0x{addr:x} is in no PT_LOAD"),
            );
        }
    }
    if !c.ok() {
        c.block("output program headers", exe.program_header_table());
    }
    c.finish()?;
    ctx.expect_output(&linked, "ms\n", 4)?;
    Ok(())
});

link_test!(bss_is_zeroed, |ctx| {
    // bss_prober exits with `base` plus the byte it finds at the start of .bss, so an exit
    // status of exactly `base` is the statement "the .bss byte was zero".
    let linked = ctx.link_ok(&Link::new().object("a.o", bss_prober("zero\n", 4096, 64)?))?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "zero\n", 64)?;
    ctx.note(
        "the program exits with 64 + the first byte of .bss; anything above 64 means the \
         linker left file bytes showing through where the zero-fill should be",
    );
    Ok(())
});

link_test!(bss_is_writable, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", bss_writer(51)?))?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "", 51)?;
    Ok(())
});

link_test!(bss_from_two_objects, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", two_buffer_reader()?)
            .object("b.o", second_buffer(256)?)
            .label("two objects that each contribute a .bss"),
    )?;
    let a = linked.address_of("buf_a")?;
    let b = linked.address_of("buf_b")?;
    let mut c = Check::new("two .bss contributions, both allocated");
    c.ne("output.symbol['buf_b']", a, b);
    c.that(
        "output.symbol['buf_a'] .. output.symbol['buf_b']",
        "far enough apart that the 128-byte buf_a and the 256-byte buf_b do not overlap",
        b.abs_diff(a) >= 128,
        format!("buf_a 0x{a:x}, buf_b 0x{b:x}, {} apart", b.abs_diff(a)),
    );
    for (name, addr) in [("buf_a", a), ("buf_b", b)] {
        c.that(
            &format!("output.segment('{name}')"),
            "inside a writable PT_LOAD",
            linked
                .elf
                .segment_at(addr)
                .map(|s| s.writable())
                .unwrap_or(false),
            segment_summary(&linked.elf, addr),
        );
    }
    if !c.ok() {
        c.block("output section headers", linked.elf.section_header_table());
    }
    c.finish()?;
    // Both buffers start zeroed, so the program exits with its base of 9.
    ctx.expect_output(&linked, "", 9)?;
    Ok(())
});

link_test!(a_megabyte_costs_nothing, |ctx| {
    let small = ctx.link_ok(
        &Link::new()
            .object("a.o", bss_prober("mb\n", 64, 5)?)
            .out("small")
            .label("the same program with a 64-byte .bss"),
    )?;
    ctx.expect_output(&small, "mb\n", 5)?;
    let big = ctx.link_ok(
        &Link::new()
            .object("a.o", bss_prober("mb\n", 1 << 20, 5)?)
            .out("big")
            .label("the same program with a one-megabyte .bss"),
    )?;
    ctx.expect_output(&big, "mb\n", 5)?;

    let grew = big.bytes.len().saturating_sub(small.bytes.len());
    let mut c = Check::new("what a megabyte of .bss costs in file bytes");
    c.that(
        "output.size",
        "at most a page or two larger than the same program with a 64-byte .bss — .bss is \
         an amount of memory, not an amount of data",
        grew < 0x10000,
        format!(
            "{} bytes vs {} bytes, a growth of {grew}",
            big.bytes.len(),
            small.bytes.len()
        ),
    );
    let addr = big.address_of("scratch")?;
    if let Some(seg) = big.elf.segment_at(addr) {
        c.at_least(
            "output.segment(.bss).p_memsz - p_filesz",
            1u64 << 20,
            seg.memsz - seg.filesz,
        );
    }
    c.finish()
});

link_test!(bss_is_not_read_only, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", bss_prober("rw\n", 1024, 8)?))?;
    let exe = &linked.elf;
    let addr = linked.address_of("scratch")?;
    let mut c = Check::new("the permissions of the segment .bss landed in");
    match exe.segment_at(addr) {
        Some(seg) => {
            c.that(
                &format!("output.segment[{}].p_flags", seg.index),
                "PF_W — a .bss the program cannot write to is a .bss for nothing",
                seg.writable(),
                flags_string(seg.flags),
            );
            c.that(
                &format!("output.segment[{}].p_flags", seg.index),
                "not PF_X: zero-filled writable memory must never be executable",
                !seg.executable(),
                flags_string(seg.flags),
            );
        }
        None => {
            c.that(
                "output.segment(.bss)",
                "some PT_LOAD covering .bss",
                false,
                format!("0x{addr:x} is in no PT_LOAD"),
            );
        }
    }
    if !c.ok() {
        c.block("output program headers", exe.program_header_table());
    }
    c.finish()?;
    ctx.expect_output(&linked, "rw\n", 8)?;
    Ok(())
});

/// An object that stores `value` into `.bss`, loads it back and exits with it.
fn bss_writer(value: u32) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.mov_r32_imm32(Reg::Rax, value);
    code.mov_rip_r32("scratch", Reg::Rax, 0);
    code.mov_r32_imm32(Reg::Rax, 0);
    code.mov_r32_rip(Reg::Rax, "scratch", 0);
    code.sys_exit_eax();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::bss(".bss", 64).align(16))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("scratch", ".bss", 0).object(64)),
    )
}

/// The first half of the two-object `.bss` fixture: a 128-byte buffer of its own, a
/// reference to the other object's buffer, and an exit status of 9 plus both first bytes.
fn two_buffer_reader() -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.movzx_r32_byte_rip(Reg::Rax, "buf_a", 0);
    code.movzx_r32_byte_rip(Reg::Rdx, "buf_b", 0);
    code.add_r32_r32(Reg::Rax, Reg::Rdx);
    code.add_r32_imm32(Reg::Rax, 9);
    code.sys_exit_eax();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::bss(".bss", 128).align(16))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("buf_a", ".bss", 0).object(128))
            .symbol(SymbolSpec::undefined("buf_b")),
    )
}

/// The second half: nothing but a `.bss` buffer and the global that names it.
fn second_buffer(size: u64) -> Result<Vec<u8>, Failure> {
    build(
        ObjectBuilder::new()
            .section(SectionSpec::bss(".bss", size).align(16))
            .symbol(SymbolSpec::global("buf_b", ".bss", 0).object(size)),
    )
}

/// Worked examples: space that costs nothing to store.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "A megabyte that is not in the file",
            "ld -o prog probe.o",
            || bss_prober("zero\n", 1 << 20, 64).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "probe.o: a .data holding the message, a .bss declared SHT_NOBITS with sh_size \
             0x100000 and sh_addralign 16, and a .text that writes the message, loads the \
             first byte of .bss and exits with 64 plus it",
        )
        .response(
            "A .bss still SHT_NOBITS in the output, a PT_LOAD whose p_memsz is a megabyte \
             larger than its p_filesz, an output file of a few kilobytes — and an exit \
             status of exactly 64, because the byte the program read was zero",
        )
        .note(
            "The trap is copying sh_size bytes from sh_offset: for SHT_NOBITS, sh_offset \
             points at whatever happens to follow, so the program starts with a megabyte of \
             someone else's section headers instead of zeros.",
        )
        .runs("zero\n", 64),
        ExampleSpec::object(
            "Writing to .bss and reading it back",
            "ld -o prog writer.o",
            || bss_writer(51).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "writer.o: a 64-byte .bss, a .text that stores 51 into it, clears the register, \
             loads the value back and exits with it",
        )
        .response(
            "An executable whose .bss lands in a PF_R|PF_W (never PF_X) segment, so the \
             store succeeds and the program exits 51 rather than dying with SIGSEGV",
        )
        .note(
            "Two different bugs both show up as signal 11 here: a .bss placed in a read-only \
             segment, and a p_memsz that was never grown to cover it.",
        )
        .runs("", 51),
    ]
}
