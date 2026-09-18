//! Shared fixtures and shared assertions.
//!
//! Two halves:
//!
//! - **builders** — the freestanding objects nearly every stage needs (an object that prints
//!   a string and exits, a caller/callee pair, an object with data and `.bss`), each one
//!   assembled from [`crate::asm`] and written by [`crate::elf::write`];
//! - **assertions** — the structural checks that are true of *every* correct output, so a
//!   stage only has to spell out what is new about it. [`assert_runnable_layout`] is leg two
//!   of the verification model and is called by most stages.

use crate::asm::{Code, Reg, STDOUT};
use crate::assert::{Check, Failure};
use crate::elf::read::{Elf, Section};
use crate::elf::write::{ObjectBuilder, Reloc, SectionSpec, SymbolSpec};
use crate::elf::*;
use crate::link::Linked;

/// The entry symbol a linker defaults to when `-e` is not given.
pub const DEFAULT_ENTRY: &str = "_start";

/// Turn a writer error into a harness failure — the suite's own bug, not the linker's.
pub fn build(b: ObjectBuilder) -> Result<Vec<u8>, Failure> {
    b.build()
        .map_err(|e| Failure::harness(format!("the suite could not build an input object: {e}")))
}

/// A `.text` section carrying a [`Code`] block and its relocations.
pub fn text_of(code: &Code) -> SectionSpec {
    SectionSpec::text(".text", code.bytes.clone()).relocs(code.relocs.clone())
}

/// A `.text` section under another name (`.text.hot`, `.init`, …).
pub fn text_named(name: &str, code: &Code) -> SectionSpec {
    SectionSpec::text(name, code.bytes.clone()).relocs(code.relocs.clone())
}

// ---------------------------------------------------------------------------------------
// The standard freestanding objects
// ---------------------------------------------------------------------------------------

/// One object: `_start` writes `message` to stdout and exits with `status`.
///
/// ```text
/// .rodata: "<message>"
/// .text:   write(1, msg, len); exit(status)
/// ```
pub fn print_and_exit(message: &str, status: u32) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "message", message.len() as u32);
    code.sys_exit(status);
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::local("message", ".rodata", 0).object(message.len() as u64)),
    )
}

/// One object: `_start` exits with `status` and prints nothing.
pub fn exit_only(status: u32) -> Result<Vec<u8>, Failure> {
    exit_only_named(DEFAULT_ENTRY, status)
}

/// The same, with the entry symbol under another name (for `-e`).
pub fn exit_only_named(entry: &str, status: u32) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_exit(status);
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(entry, ".text", 0).func()),
    )
}

/// The caller half of a two-object program: print `message`, `call <callee>`, exit with
/// whatever the callee left in `eax`.
///
/// The `call` is an `R_X86_64_PLT32` — what every compiler emits for a call to a global —
/// and the string reference is an `R_X86_64_PC32` against a local symbol.
pub fn caller(message: &str, callee: &str) -> Result<Vec<u8>, Failure> {
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

/// The offset of the `call`'s displacement inside [`caller`]'s `.text`.
///
/// `sys_write` is 5 + 5 + 7 + 5 + 2 = 24 bytes, then the `call` opcode is one byte.
pub const CALLER_CALL_DISP_OFFSET: u64 = 25;

/// The offset of the `lea`'s displacement inside [`caller`]'s and [`print_and_exit`]'s
/// `.text`: `mov eax` (5) + `mov edi` (5) + the three-byte `lea` opcode.
pub const WRITE_LEA_DISP_OFFSET: u64 = 13;

/// The callee half: a function that returns `value` in `eax`.
pub fn callee_returning(name: &str, value: u32) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.mov_r32_imm32(Reg::Rax, value);
    code.ret();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(name, ".text", 0).func()),
    )
}

/// The same, but the function is weak.
pub fn weak_callee_returning(name: &str, value: u32) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.mov_r32_imm32(Reg::Rax, value);
    code.ret();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::weak(name, ".text", 0).func()),
    )
}

/// An object that prints a string held in `.data` and exits with the byte it finds in
/// `.bss` plus `base` — which proves `.bss` is mapped, zeroed and writable.
pub fn bss_prober(message: &str, bss_size: u64, base: u32) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "message", message.len() as u32);
    // eax = zero-extended byte at scratch[0]; a correct linker zeroes it, so this is 0.
    code.movzx_r32_byte_rip(Reg::Rax, "scratch", 0);
    code.add_r32_imm32(Reg::Rax, base);
    code.sys_exit_eax();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::data(".data", message.as_bytes().to_vec()))
            .section(SectionSpec::bss(".bss", bss_size).align(16))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::local("message", ".data", 0).object(message.len() as u64))
            .symbol(SymbolSpec::global("scratch", ".bss", 0).object(bss_size)),
    )
}

/// An object whose `.data` holds an eight-byte pointer to `.rodata`, patched by an
/// `R_X86_64_64`; `_start` loads the pointer and writes the string through it.
pub fn pointer_program(message: &str, status: u32) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    // rsi = *(u64*)ptr — the pointer the R_X86_64_64 filled in.
    code.mov_r64_rip(Reg::Rsi, "ptr", R_X86_64_PC32, -4);
    code.sys_write_rsi(STDOUT, message.len() as u32);
    code.sys_exit(status);
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .section(
                SectionSpec::data(".data", vec![0u8; 8])
                    .align(8)
                    .reloc(Reloc::sym(0, "message", R_X86_64_64, 0)),
            )
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("message", ".rodata", 0).object(message.len() as u64))
            .symbol(SymbolSpec::global("ptr", ".data", 0).object(8)),
    )
}

// ---------------------------------------------------------------------------------------
// Shared assertions — leg two of the verification model
// ---------------------------------------------------------------------------------------

/// Every structural invariant a correct static executable satisfies, whatever its layout.
///
/// Deliberately about invariants, never about GNU ld's choices: how many `PT_LOAD`s there
/// are, what address `.text` gets and in what order the sections come out are all the
/// linker's business. What is *not* its business:
///
/// - `ET_EXEC`, `EM_X86_64`, ELFCLASS64, little-endian, `e_version` 1;
/// - `e_ehsize` 64, `e_phentsize` 56, `e_shentsize` 64 (or 0 sections at all);
/// - at least one `PT_LOAD`, and the entry point inside an executable one;
/// - `p_filesz <= p_memsz` for every segment;
/// - no two `PT_LOAD`s overlapping in memory;
/// - `p_offset ≡ p_vaddr (mod page size)`, which is what makes `mmap` possible;
/// - no `PT_INTERP` and no `PT_DYNAMIC`: this is a static link.
pub fn assert_runnable_layout(linked: &Linked) -> Result<(), Failure> {
    let exe = &linked.elf;
    let mut c = Check::new("the structure of the linked executable");
    c.eq("output.e_ident[EI_CLASS]", ELFCLASS64, exe.ident[4]);
    c.eq("output.e_ident[EI_DATA]", ELFDATA2LSB, exe.ident[5]);
    c.eq("output.e_ident[EI_VERSION]", EV_CURRENT, exe.ident[6]);
    c.eq("output.e_type", ET_EXEC, exe.e_type);
    c.eq("output.e_machine", EM_X86_64, exe.e_machine);
    c.eq("output.e_version", EV_CURRENT as u32, exe.e_version);
    c.eq("output.e_ehsize", EHDR_SIZE, exe.ehsize);
    c.eq("output.e_phentsize", PHDR_SIZE, exe.phentsize);
    if exe.shnum > 0 {
        c.eq("output.e_shentsize", SHDR_SIZE, exe.shentsize);
    }

    let loads: Vec<_> = exe.loads().copied().collect();
    c.that(
        "output.program_headers",
        "at least one PT_LOAD segment",
        !loads.is_empty(),
        exe.segments.len(),
    );
    for s in &exe.segments {
        c.that(
            &format!("output.segment[{}].p_filesz", s.index),
            "at most p_memsz — file bytes cannot exceed the memory image",
            s.filesz <= s.memsz,
            format!("filesz 0x{:x} > memsz 0x{:x}", s.filesz, s.memsz),
        );
        c.that(
            &format!("output.segment[{}].p_type", s.index),
            "not PT_INTERP or PT_DYNAMIC — this is a static link with no loader",
            s.p_type != PT_INTERP && s.p_type != PT_DYNAMIC,
            segment_type_name(s.p_type),
        );
    }
    for s in &loads {
        let page = if s.align > 1 { s.align } else { PAGE_SIZE };
        c.that(
            &format!("output.segment[{}].p_offset", s.index),
            "congruent to p_vaddr modulo the page size, so the kernel can mmap it",
            s.offset % page.min(PAGE_SIZE) == s.vaddr % page.min(PAGE_SIZE),
            format!(
                "offset 0x{:x} vaddr 0x{:x} (0x{:x} vs 0x{:x} mod 0x{:x})",
                s.offset,
                s.vaddr,
                s.offset % PAGE_SIZE,
                s.vaddr % PAGE_SIZE,
                PAGE_SIZE
            ),
        );
    }
    for (i, a) in loads.iter().enumerate() {
        for b in loads.iter().skip(i + 1) {
            let overlap = a.vaddr < b.vaddr.saturating_add(b.memsz)
                && b.vaddr < a.vaddr.saturating_add(a.memsz);
            c.that(
                "output.PT_LOAD",
                "no two loadable segments covering the same address",
                !overlap || a.memsz == 0 || b.memsz == 0,
                format!("{} overlaps {}", a.describe(), b.describe()),
            );
        }
    }
    let entry_seg = exe.segment_at(exe.entry);
    c.that(
        "output.e_entry",
        "an address inside an executable PT_LOAD",
        entry_seg.map(Segment::executable).unwrap_or(false),
        match entry_seg {
            Some(s) => format!("entry 0x{:x} is in {}", exe.entry, s.describe()),
            None => format!("entry 0x{:x} is in no PT_LOAD at all", exe.entry),
        },
    );
    if !c.ok() {
        c.block("linker command", linked.run.output.command_line());
        c.block("output program headers", exe.program_header_table());
        c.block("output section headers", exe.section_header_table());
    }
    c.finish()
}

use crate::elf::read::Segment;

/// The entry point is the address of `symbol`.
pub fn assert_entry_is(linked: &Linked, symbol: &str) -> Result<(), Failure> {
    let want = linked.address_of(symbol)?;
    let mut c = Check::new(format!("that the entry point is the address of '{symbol}'"));
    c.addr_eq("output.e_entry", want, linked.elf.entry);
    if !c.ok() {
        c.note(format!(
            "'{symbol}' is at 0x{want:x} in the output's own symbol table"
        ));
        c.block("output program headers", linked.elf.program_header_table());
    }
    c.finish()
}

/// Assert the 32-bit field at `site` holds `S + A - P`, the PC-relative value.
///
/// This is leg three done honestly: the suite does not care what addresses the linker chose,
/// only that the displacement it wrote is the one those addresses imply.
pub fn assert_pc32(c: &mut Check, exe: &Elf, path: &str, site: u64, target: u64, addend: i64) {
    let want = (target as i64) + addend - (site as i64);
    match exe.i32_at_vaddr(site) {
        Ok(got) => {
            c.that(
                path,
                &format!(
                    "S + A - P = 0x{target:x} + {addend} - 0x{site:x} = {want} (0x{:x})",
                    want as i32
                ),
                i64::from(got) == want,
                format!("{got} (0x{got:x})"),
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

/// Assert the 64-bit field at `site` holds `S + A`.
pub fn assert_abs64(c: &mut Check, exe: &Elf, path: &str, site: u64, target: u64, addend: i64) {
    let want = (target as i64).wrapping_add(addend) as u64;
    match exe.u64_at_vaddr(site) {
        Ok(got) => {
            c.that(
                path,
                &format!("S + A = 0x{target:x} + {addend} = 0x{want:x}"),
                got == want,
                format!("0x{got:x}"),
            );
        }
        Err(e) => {
            c.that(
                path,
                "a readable 64-bit field at the relocation site",
                false,
                e.to_string(),
            );
        }
    }
}

/// Every allocatable section of the output that has a non-zero size, in address order.
pub fn allocated_sections(exe: &Elf) -> Vec<&Section> {
    let mut v: Vec<&Section> = exe
        .sections
        .iter()
        .filter(|s| s.is_alloc() && s.size > 0)
        .collect();
    v.sort_by_key(|s| s.addr);
    v
}

/// The segment that maps `addr`, described for a report row.
pub fn segment_summary(exe: &Elf, addr: u64) -> String {
    match exe.segment_at(addr) {
        Some(s) => s.describe(),
        None => format!("no PT_LOAD maps 0x{addr:x}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_standard_objects_build_and_parse() {
        for bytes in [
            print_and_exit("hello\n", 0).expect("print_and_exit"),
            exit_only(7).expect("exit_only"),
            caller("hi\n", "other").expect("caller"),
            callee_returning("other", 3).expect("callee"),
            bss_prober("d\n", 64, 5).expect("bss_prober"),
            pointer_program("p\n", 1).expect("pointer_program"),
        ] {
            let elf = Elf::parse(&bytes).expect("every fixture must parse");
            assert_eq!(elf.e_type, ET_REL);
            assert!(elf.section(".text").is_some());
        }
    }

    #[test]
    fn the_caller_offsets_point_at_the_displacements() {
        let bytes = caller("hi\n", "other").expect("caller");
        let elf = Elf::parse(&bytes).expect("parse");
        let mut offsets: Vec<u64> = elf.relocations.iter().map(|r| r.offset).collect();
        offsets.sort_unstable();
        assert_eq!(
            offsets,
            vec![WRITE_LEA_DISP_OFFSET, CALLER_CALL_DISP_OFFSET],
            "the documented offsets must match what the assembler emitted"
        );
    }
}
