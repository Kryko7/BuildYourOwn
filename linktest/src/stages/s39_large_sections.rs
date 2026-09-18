//! Stage 39 — A five-megabyte `.rodata`.
//!
//! Size is its own kind of correctness. A five-megabyte section is where a linker that kept
//! a `u32` somewhere, that assumed a section fits one page, or that wrote the section
//! contents before it knew how long they were, finally says so. And an eight-megabyte `.bss`
//! is where the difference between `p_filesz` and `p_memsz` stops being a detail: get it
//! wrong and the executable is eight megabytes of zeros on disk.
//!
//! Every assertion here is about the image the kernel maps, never about where the linker
//! chose to put anything: the probes are read back *by the program itself*, so the bytes are
//! checked where they matter.

use crate::asm::{Code, Reg, STDOUT, SYS_WRITE};
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
        number: 39,
        slug: "large_sections",
        name: "A five-megabyte .rodata",
        ext: true,
        hints: &[
            "Keep every offset, address and size in a u64 and do the arithmetic with checked \
             or saturating operations — five megabytes is where a u32 section offset first \
             hurts",
            "p_filesz and p_memsz are different numbers: a SHT_NOBITS section adds to the \
             second and never to the first, so an eight-megabyte .bss must cost zero bytes on \
             disk",
            "The page congruence p_offset = p_vaddr (mod 4096) has to hold for a five-megabyte \
             segment exactly as it does for a forty-byte one; pad the file, do not move the \
             address",
            "Honour every input section's sh_addralign when you concatenate, including the \
             one whose contribution ends at an odd offset — the next section starts at the \
             next multiple, not the next byte",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new("a five-megabyte .rodata links", big_rodata_links)
                .ext()
                .tag("slow")
                .min_timeout_ms(60_000),
            Test::new("the five megabytes survive the link", probes_read_back)
                .ext()
                .tag("slow")
                .min_timeout_ms(60_000),
            Test::new("the output file is at least five megabytes", output_is_big)
                .ext()
                .tag("slow")
                .min_timeout_ms(60_000),
            Test::new(
                "an eight-megabyte .bss adds no bytes to the file",
                bss_costs_no_file_space,
            )
            .ext()
            .tag("slow")
            .min_timeout_ms(60_000),
            Test::new(
                "an eight-megabyte .bss is mapped and zeroed",
                big_bss_is_zeroed,
            )
            .ext()
            .tag("slow")
            .min_timeout_ms(60_000),
            Test::new(
                "a five-megabyte segment still satisfies the page congruence",
                congruence_at_scale,
            )
            .ext()
            .tag("slow")
            .min_timeout_ms(60_000),
            Test::new(
                "two hundred sixteen-kilobyte sections all land aligned",
                many_medium_sections,
            )
            .ext()
            .tag("slow")
            .min_timeout_ms(60_000),
            Test::new(
                "five megabytes of .rodata and eight of .bss in one program",
                both_at_once,
            )
            .ext()
            .tag("slow")
            .min_timeout_ms(60_000),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// The large fixtures
// ---------------------------------------------------------------------------------------

/// Five megabytes exactly.
const RODATA_SIZE: usize = 5 * 1024 * 1024;

/// Eight megabytes of `.bss`, which must cost nothing on disk.
const BSS_SIZE: u64 = 8 * 1024 * 1024;

/// How many bytes each probe writes back out.
const PROBE: usize = 16;

/// How many medium sections the alignment test builds.
const CHUNKS: usize = 200;

/// Sixteen kilobytes *and seven bytes*: a size that is not a multiple of any of the
/// alignments, so the linker has to insert real padding between the chunks.
const CHUNK_SIZE: usize = 16 * 1024 + 7;

/// Five megabytes of printable, position-dependent bytes.
///
/// Printable on purpose: when a probe comes back wrong, the failure block is readable.
fn blob() -> Vec<u8> {
    (0..RODATA_SIZE)
        .map(|i| b'a' + ((i.wrapping_mul(31).wrapping_add(7)) % 26) as u8)
        .collect()
}

/// Where the program samples the blob: the first bytes, the middle, and the last bytes.
fn probe_offsets() -> [usize; 3] {
    [0, RODATA_SIZE / 2, RODATA_SIZE - PROBE]
}

/// The bytes the program is expected to write: the three probes, concatenated.
fn expected_probes() -> Vec<u8> {
    let data = blob();
    let mut v = Vec::with_capacity(3 * PROBE);
    for off in probe_offsets() {
        v.extend_from_slice(&data[off..off + PROBE]);
    }
    v
}

/// `write(1, blob + offset, PROBE)` — a `lea` with an addend of up to five megabytes.
fn probe_write(code: &mut Code, offset: usize) {
    code.mov_r32_imm32(Reg::Rax, SYS_WRITE);
    code.mov_r32_imm32(Reg::Rdi, STDOUT);
    code.lea_rip(Reg::Rsi, "blob", offset as i64);
    code.mov_r32_imm32(Reg::Rdx, PROBE as u32);
    code.syscall();
}

/// The five-megabyte object: `.rodata` is the blob, `.text` writes three slices of it.
fn big_rodata_object(status: u32) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    for off in probe_offsets() {
        probe_write(&mut code, off);
    }
    code.sys_exit(status);
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", blob()).align(16))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("blob", ".rodata", 0).object(RODATA_SIZE as u64)),
    )
}

/// A small program that writes one string and exits, optionally carrying a huge `.bss`.
///
/// The two variants differ in nothing but the `SHT_NOBITS` section, which is exactly what
/// makes the file sizes comparable.
fn bss_variant(with_bss: bool) -> Result<Vec<u8>, Failure> {
    let message = "big\n";
    let mut code = Code::new();
    code.sys_write(STDOUT, "message", message.len() as u32);
    code.sys_exit(7);
    let mut b = ObjectBuilder::new()
        .section(text_of(&code))
        .section(SectionSpec::data(".data", message.as_bytes().to_vec()).align(8))
        .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
        .symbol(SymbolSpec::local("message", ".data", 0).object(message.len() as u64));
    if with_bss {
        b = b
            .section(SectionSpec::bss(".bss", BSS_SIZE).align(4096))
            .symbol(SymbolSpec::global("scratch", ".bss", 0).object(BSS_SIZE));
    }
    build(b)
}

/// An object that reads the *last* byte of an eight-megabyte `.bss` and exits with it plus
/// `base` — so an unmapped or unzeroed tail is a wrong exit status, not a silent pass.
fn bss_prober_object(base: u32) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.movzx_r32_byte_rip(Reg::Rax, "scratch", (BSS_SIZE - 1) as i64);
    code.add_r32_imm32(Reg::Rax, base);
    code.sys_exit_eax();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::bss(".bss", BSS_SIZE).align(4096))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("scratch", ".bss", 0).object(BSS_SIZE)),
    )
}

/// The alignment chunk `i` asks for: 8, 16, 32, 64 or 128 bytes, cycling.
fn chunk_align(i: usize) -> u64 {
    1u64 << (3 + (i % 5))
}

/// The symbol at the start of chunk `i`.
fn chunk_name(i: usize) -> String {
    format!("chunk_{i:03}")
}

/// Two hundred sixteen-kilobyte-plus-seven sections, each with its own alignment and its own
/// symbol at offset zero.
fn many_chunks_object(status: u32) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_exit(status);
    let mut b = ObjectBuilder::new()
        .section(text_of(&code))
        .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func());
    for i in 0..CHUNKS {
        let name = format!(".rodata.chunk{i:03}");
        let fill = b'0' + (i % 10) as u8;
        b = b
            .section(SectionSpec::rodata(&name, vec![fill; CHUNK_SIZE]).align(chunk_align(i)))
            .symbol(SymbolSpec::global(&chunk_name(i), &name, 0).object(CHUNK_SIZE as u64));
    }
    build(b)
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

link_test!(big_rodata_links, |ctx| {
    let obj = big_rodata_object(0)?;
    let input_len = obj.len();
    let linked = ctx.link_ok(
        &Link::new()
            .object("big.o", obj)
            .out("big")
            .label("one object with a five-megabyte .rodata"),
    )?;
    ctx.note(format!(
        "a {} KiB object linked in {} ms into a {} KiB executable",
        input_len / 1024,
        linked.run.output.duration.as_millis(),
        linked.bytes.len() / 1024
    ));
    assert_runnable_layout(&linked)?;
    let mut c = Check::new("the five-megabyte .rodata in the output");
    match linked.elf.section(".rodata") {
        Some(s) => {
            c.at_least("output..rodata.sh_size", RODATA_SIZE as u64, s.size);
            c.that(
                "output..rodata.sh_type",
                "SHT_PROGBITS — five megabytes of real content",
                s.sh_type == SHT_PROGBITS,
                section_type_name(s.sh_type),
            );
        }
        None => {
            c.that(
                "output..rodata",
                "a section named .rodata in the output",
                false,
                "no such section",
            );
        }
    }
    c.finish()
});

link_test!(probes_read_back, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("big.o", big_rodata_object(0)?)
            .out("big")
            .label("a five-megabyte .rodata"),
    )?;
    let out = ctx.run(&linked)?;
    let want = expected_probes();
    let mut c = Check::new("the three probes the program read back out of five megabytes");
    c.bytes_eq("program.stdout", &want, &out.stdout_bytes);
    c.eq("program.exit_status", Some(0), out.code);
    c.note(format!(
        "the probes are at byte 0, byte {}, and byte {} of the blob",
        RODATA_SIZE / 2,
        RODATA_SIZE - PROBE
    ));

    // Leg two: the same three slices read straight out of the output's memory image.
    let base = linked.address_of("blob")?;
    for (i, off) in probe_offsets().into_iter().enumerate() {
        let addr = base + off as u64;
        match linked.elf.read_at_vaddr(addr, PROBE as u64) {
            Ok(got) => {
                c.bytes_eq(
                    &format!("output[blob+{off}]"),
                    &want[i * PROBE..(i + 1) * PROBE],
                    got,
                );
            }
            Err(e) => {
                c.that(
                    &format!("output[blob+{off}]"),
                    "sixteen readable bytes inside a PT_LOAD",
                    false,
                    e.to_string(),
                );
            }
        }
    }
    if !c.ok() {
        c.block(
            "the segment holding blob",
            segment_summary(&linked.elf, base),
        );
    }
    c.finish()
});

link_test!(output_is_big, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("big.o", big_rodata_object(0)?)
            .out("big")
            .label("a five-megabyte .rodata"),
    )?;
    let mut c = Check::new("the size of an executable carrying five megabytes of content");
    c.at_least("output.size", RODATA_SIZE as u64, linked.bytes.len() as u64);
    let loaded: u64 = linked.elf.loads().map(|s| s.filesz).sum();
    c.at_least("output.sum(PT_LOAD.p_filesz)", RODATA_SIZE as u64, loaded);
    c.note(format!(
        "{} bytes of file for {RODATA_SIZE} bytes of .rodata — a linker that writes each \
         section twice shows up here",
        linked.bytes.len()
    ));
    c.at_most(
        "output.size",
        (RODATA_SIZE as u64) * 2,
        linked.bytes.len() as u64,
    );
    c.finish()
});

link_test!(bss_costs_no_file_space, |ctx| {
    let lean = ctx.link_ok(
        &Link::new()
            .object("lean.o", bss_variant(false)?)
            .out("lean")
            .label("the same program without the .bss"),
    )?;
    let fat = ctx.link_ok(
        &Link::new()
            .object("fat.o", bss_variant(true)?)
            .out("fat")
            .label("the same program with an eight-megabyte .bss"),
    )?;
    ctx.expect_output(&lean, "big\n", 7)?;
    ctx.expect_output(&fat, "big\n", 7)?;

    let mut c = Check::new("what an eight-megabyte .bss costs");
    let grew = fat.bytes.len() as i64 - lean.bytes.len() as i64;
    c.that(
        "output.size",
        "an eight-megabyte .bss adding at most a few pages to the file",
        grew < 64 * 1024,
        format!(
            "{} bytes without it, {} bytes with it: {grew} bytes more",
            lean.bytes.len(),
            fat.bytes.len()
        ),
    );

    let scratch = fat.address_of("scratch")?;
    match fat.elf.segment_at(scratch) {
        Some(seg) => {
            c.that(
                "output.segment[scratch].p_memsz",
                "at least eight megabytes of memory image",
                seg.memsz >= BSS_SIZE,
                seg.describe(),
            );
            c.that(
                "output.segment[scratch].p_filesz",
                "a p_filesz that does not cover the .bss",
                seg.memsz - seg.filesz >= BSS_SIZE,
                format!(
                    "filesz 0x{:x}, memsz 0x{:x}, difference 0x{:x}",
                    seg.filesz,
                    seg.memsz,
                    seg.memsz.saturating_sub(seg.filesz)
                ),
            );
            c.that(
                "output.segment[scratch].p_flags",
                "a writable segment",
                seg.writable(),
                seg.describe(),
            );
        }
        None => {
            c.that(
                "output.segment[scratch]",
                "a PT_LOAD mapping the .bss",
                false,
                format!("nothing maps 0x{scratch:x}"),
            );
        }
    }
    if !c.ok() {
        c.block("output program headers", fat.elf.program_header_table());
    }
    c.finish()
});

link_test!(big_bss_is_zeroed, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("bss.o", bss_prober_object(9)?)
            .out("bss")
            .label("an eight-megabyte .bss read at its last byte"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "", 9)?;
    ctx.note(format!(
        "the program reads scratch[{}] — the last byte of the .bss — and exits with it plus 9",
        BSS_SIZE - 1
    ));
    let mut c = Check::new("the file the eight-megabyte .bss produced");
    c.at_most("output.size", 256 * 1024u64, linked.bytes.len() as u64);
    c.finish()
});

link_test!(congruence_at_scale, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("big.o", big_rodata_object(0)?)
            .out("big")
            .label("a five-megabyte .rodata"),
    )?;
    assert_runnable_layout(&linked)?;
    let base = linked.address_of("blob")?;
    let mut c = Check::new("the segment that maps five megabytes");
    match linked.elf.segment_at(base) {
        Some(seg) => {
            c.eq(
                "output.segment[blob].p_offset mod 4096",
                seg.vaddr % PAGE_SIZE,
                seg.offset % PAGE_SIZE,
            );
            c.at_least(
                "output.segment[blob].p_filesz",
                RODATA_SIZE as u64,
                seg.filesz,
            );
            c.that(
                "output.segment[blob].p_offset + p_filesz",
                "entirely inside the file",
                seg.offset + seg.filesz <= linked.bytes.len() as u64,
                format!(
                    "0x{:x} + 0x{:x} against a file of 0x{:x} bytes",
                    seg.offset,
                    seg.filesz,
                    linked.bytes.len()
                ),
            );
            c.that(
                "output.segment[blob].p_flags",
                "readable and not writable — this is .rodata",
                seg.readable() && !seg.writable(),
                seg.describe(),
            );
        }
        None => {
            c.that(
                "output.segment[blob]",
                "a PT_LOAD mapping the blob",
                false,
                format!("nothing maps 0x{base:x}"),
            );
        }
    }
    if !c.ok() {
        c.block("output program headers", linked.elf.program_header_table());
    }
    c.finish()
});

link_test!(many_medium_sections, |ctx| {
    let obj = many_chunks_object(5)?;
    let input_len = obj.len();
    let linked = ctx.link_ok(
        &Link::new()
            .object("chunks.o", obj)
            .out("chunks")
            .label("two hundred sixteen-kilobyte sections"),
    )?;
    ctx.note(format!(
        "{CHUNKS} sections of {CHUNK_SIZE} bytes ({} KiB of input) linked in {} ms",
        input_len / 1024,
        linked.run.output.duration.as_millis()
    ));
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "", 5)?;

    let mut c = Check::new("where two hundred medium sections landed");
    let mut misaligned = Vec::new();
    let mut previous: Option<(u64, u64)> = None;
    for i in 0..CHUNKS {
        let want_align = chunk_align(i);
        let addr = linked.address_of(&chunk_name(i))?;
        if addr % want_align != 0 {
            misaligned.push(format!(
                "{} at 0x{addr:x} wants {want_align}-byte alignment",
                chunk_name(i)
            ));
        }
        if let Some((prev_addr, prev_end)) = previous {
            c.that(
                &format!("output.{}", chunk_name(i)),
                "an address at or after the end of the previous chunk",
                addr >= prev_end,
                format!("0x{addr:x} after a chunk at 0x{prev_addr:x} ending at 0x{prev_end:x}"),
            );
        }
        previous = Some((addr, addr + CHUNK_SIZE as u64));
    }
    c.that(
        "output.chunk_NNN",
        "every chunk on the boundary its sh_addralign asked for",
        misaligned.is_empty(),
        format!("{} misaligned, first few: {:?}", misaligned.len(), {
            let mut head = misaligned.clone();
            head.truncate(5);
            head
        }),
    );

    // The content of the first, middle and last chunk, read out of the memory image.
    for i in [0usize, CHUNKS / 2, CHUNKS - 1] {
        let addr = linked.address_of(&chunk_name(i))?;
        let fill = b'0' + (i % 10) as u8;
        match linked.elf.read_at_vaddr(addr, 8) {
            Ok(got) => c.bytes_eq(&format!("output[{}]", chunk_name(i)), &[fill; 8], got),
            Err(e) => c.that(
                &format!("output[{}]", chunk_name(i)),
                "eight readable bytes",
                false,
                e.to_string(),
            ),
        };
    }
    c.finish()
});

link_test!(both_at_once, |ctx| {
    let mut code = Code::new();
    for off in probe_offsets() {
        probe_write(&mut code, off);
    }
    // Read the last byte of the huge .bss, add 11, and exit with it.
    code.movzx_r32_byte_rip(Reg::Rax, "scratch", (BSS_SIZE - 1) as i64);
    code.add_r32_imm32(Reg::Rax, 11);
    code.sys_exit_eax();
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", blob()).align(16))
            .section(SectionSpec::bss(".bss", BSS_SIZE).align(4096))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("blob", ".rodata", 0).object(RODATA_SIZE as u64))
            .symbol(SymbolSpec::global("scratch", ".bss", 0).object(BSS_SIZE)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("both.o", obj)
            .out("both")
            .label("five megabytes of .rodata and eight of .bss"),
    )?;
    assert_runnable_layout(&linked)?;
    assert_entry_is(&linked, DEFAULT_ENTRY)?;

    let out = ctx.run(&linked)?;
    let mut c = Check::new("a program with both a huge .rodata and a huge .bss");
    c.bytes_eq("program.stdout", &expected_probes(), &out.stdout_bytes);
    c.eq("program.exit_status", Some(11), out.code);
    c.at_least("output.size", RODATA_SIZE as u64, linked.bytes.len() as u64);
    c.at_most(
        "output.size",
        (RODATA_SIZE as u64) + BSS_SIZE / 2,
        linked.bytes.len() as u64,
    );
    c.note(format!(
        "the file is {} bytes: five megabytes of content, and no file space at all for the \
         eight megabytes of .bss",
        linked.bytes.len()
    ));
    c.finish()
});

/// Worked examples: the blob, and the `.bss` that has to stay free.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::text(
            "A five-megabyte .rodata, sampled at three places",
            "ld -o big big.o",
        )
        .request(
            "big.o: a .rodata of 5 MiB of printable bytes with a global 'blob' at offset 0, \
             and a .text with three write(2) calls whose lea addends are 0, 2621440 and \
             5242864 — three R_X86_64_PC32 relocations against the same symbol",
        )
        .response(
            "An executable of at least five megabytes whose PT_LOAD for .rodata has \
             p_filesz >= 5 MiB, p_offset congruent to p_vaddr modulo 4096, and whose three \
             displacements differ by exactly the addends; the program writes the three \
             sixteen-byte slices and exits 0",
        )
        .note(
            "The middle probe is the one that catches a 32-bit offset somewhere in the \
             linker; the last one catches a size computed before the section was complete.",
        ),
        ExampleSpec::object(
            "Eight megabytes of .bss that must not reach the disk",
            "ld -o fat fat.o",
            || bss_variant(true).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "fat.o: a four-byte .data, a .text that writes it and exits 7, and an \
             SHT_NOBITS .bss of 8388608 bytes aligned to a page with a global 'scratch'",
        )
        .response(
            "A writable PT_LOAD whose p_memsz is at least eight megabytes larger than its \
             p_filesz, and an output file that is a few kilobytes — not eight megabytes",
        )
        .note(
            "SHT_NOBITS means 'sh_size bytes of zeros, no file content'. A linker that copies \
             sh_size bytes out of the input file for every section produces an eight-megabyte \
             executable here, and reads eight megabytes that are not in the input.",
        )
        .runs("big\n", 7),
    ]
}

/// Re-exported so the module doc can point at the parser the probes are checked with.
#[allow(dead_code)]
type ParsedOutput = Elf;
