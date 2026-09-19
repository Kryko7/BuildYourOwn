//! ELF64 for x86-64: the constants, the writer and the reader the suite uses.
//!
//! Everything the harness puts on disk it writes itself ([`write`]), and everything the
//! linker under test produces it parses itself ([`read`]). Nothing here shells out to
//! binutils, so the suite runs on a machine that has no `as` and no `ld` — only the
//! learner's own `your_program.sh`.
//!
//! The names and values are the ones in the ELF-64 gABI and the x86-64 psABI; where a
//! constant has a shorter conventional spelling (`SHT_PROGBITS`, `R_X86_64_PC32`) that is
//! the spelling used, so a learner can grep the spec for it.

pub mod archive;
pub mod read;
pub mod write;

// ---------------------------------------------------------------------------------------
// e_ident
// ---------------------------------------------------------------------------------------

/// `\x7fELF`, the first four bytes of every ELF file.
pub const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];
/// Size of `e_ident`.
pub const EI_NIDENT: usize = 16;
/// `e_ident[EI_CLASS]` for a 32-bit file.
pub const ELFCLASS32: u8 = 1;
/// `e_ident[EI_CLASS]` for a 64-bit file.
pub const ELFCLASS64: u8 = 2;
/// `e_ident[EI_DATA]` for little-endian.
pub const ELFDATA2LSB: u8 = 1;
/// `e_ident[EI_DATA]` for big-endian.
pub const ELFDATA2MSB: u8 = 2;
/// `e_ident[EI_VERSION]` and `e_version`: the only defined ELF version.
pub const EV_CURRENT: u8 = 1;
/// `e_ident[EI_OSABI]` for System V.
pub const ELFOSABI_SYSV: u8 = 0;

// ---------------------------------------------------------------------------------------
// e_type / e_machine
// ---------------------------------------------------------------------------------------

/// No file type.
pub const ET_NONE: u16 = 0;
/// A relocatable object (`.o`), the linker's input.
pub const ET_REL: u16 = 1;
/// A non-PIE executable, the linker's output for this track.
pub const ET_EXEC: u16 = 2;
/// A shared object (or a PIE executable).
pub const ET_DYN: u16 = 3;
/// A core dump.
pub const ET_CORE: u16 = 4;
/// `e_machine` for x86-64.
pub const EM_X86_64: u16 = 62;
/// `e_machine` for 32-bit x86, used by the "wrong machine" fixtures.
pub const EM_386: u16 = 3;
/// `e_machine` for AArch64, used by the "wrong machine" fixtures.
pub const EM_AARCH64: u16 = 183;

/// Size of the ELF64 file header.
pub const EHDR_SIZE: u16 = 64;
/// Size of one ELF64 program header.
pub const PHDR_SIZE: u16 = 56;
/// Size of one ELF64 section header.
pub const SHDR_SIZE: u16 = 64;
/// Size of one ELF64 symbol table entry.
pub const SYM_SIZE: u64 = 24;
/// Size of one ELF64 `Elf64_Rela`.
pub const RELA_SIZE: u64 = 24;
/// Size of one ELF64 `Elf64_Rel` (no explicit addend).
pub const REL_SIZE: u64 = 16;

/// The page size the kernel maps with on x86-64, and the modulus of the
/// `p_offset ≡ p_vaddr (mod page)` congruence every `PT_LOAD` must satisfy.
pub const PAGE_SIZE: u64 = 0x1000;

// ---------------------------------------------------------------------------------------
// Section headers
// ---------------------------------------------------------------------------------------

/// Inactive section header (index 0).
pub const SHT_NULL: u32 = 0;
/// Bytes that live in the file: `.text`, `.data`, `.rodata`.
pub const SHT_PROGBITS: u32 = 1;
/// A symbol table.
pub const SHT_SYMTAB: u32 = 2;
/// A string table.
pub const SHT_STRTAB: u32 = 3;
/// Relocations with explicit addends.
pub const SHT_RELA: u32 = 4;
/// A symbol hash table.
pub const SHT_HASH: u32 = 5;
/// Dynamic linking information.
pub const SHT_DYNAMIC: u32 = 6;
/// A note.
pub const SHT_NOTE: u32 = 7;
/// Occupies no file space: `.bss`.
pub const SHT_NOBITS: u32 = 8;
/// Relocations without addends.
pub const SHT_REL: u32 = 9;
/// An array of function pointers run before `main`.
pub const SHT_INIT_ARRAY: u32 = 14;
/// An array of function pointers run after it.
pub const SHT_FINI_ARRAY: u32 = 15;
/// An array run before even the init array.
pub const SHT_PREINIT_ARRAY: u32 = 16;
/// A section the suite uses to check "unknown types are handled sensibly".
pub const SHT_UNKNOWN_OS: u32 = 0x6000_0042;

/// The section occupies memory at run time.
pub const SHF_ALLOC: u64 = 0x2;
/// The section is writable at run time.
pub const SHF_WRITE: u64 = 0x1;
/// The section holds instructions.
pub const SHF_EXECINSTR: u64 = 0x4;
/// The section holds fixed-size mergeable elements.
pub const SHF_MERGE: u64 = 0x10;
/// The merged elements are NUL-terminated strings.
pub const SHF_STRINGS: u64 = 0x20;
/// `sh_info` holds a section header index.
pub const SHF_INFO_LINK: u64 = 0x40;
/// The section is a member of a group.
pub const SHF_GROUP: u64 = 0x200;
/// Thread-local storage: one copy per thread, not one per process.
pub const SHF_TLS: u64 = 0x400;

/// Undefined section index: the symbol is a reference, not a definition.
pub const SHN_UNDEF: u16 = 0;
/// The symbol's value is absolute and must not be relocated.
pub const SHN_ABS: u16 = 0xfff1;
/// A tentative definition: the linker allocates it once, largest size wins.
pub const SHN_COMMON: u16 = 0xfff2;
/// The real index lives in `SHT_SYMTAB_SHNDX` (the suite never emits this).
pub const SHN_XINDEX: u16 = 0xffff;
/// First reserved section index.
pub const SHN_LORESERVE: u16 = 0xff00;

// ---------------------------------------------------------------------------------------
// Program headers
// ---------------------------------------------------------------------------------------

/// Unused program header.
pub const PT_NULL: u32 = 0;
/// A segment the kernel maps.
pub const PT_LOAD: u32 = 1;
/// Dynamic linking information.
pub const PT_DYNAMIC: u32 = 2;
/// The path of the dynamic loader — a static link must not have one.
pub const PT_INTERP: u32 = 3;
/// A note segment.
pub const PT_NOTE: u32 = 4;
/// The program header table itself.
pub const PT_PHDR: u32 = 6;
/// TLS template.
pub const PT_TLS: u32 = 7;
/// The unwinder's binary search table, written by `--eh-frame-hdr`.
pub const PT_GNU_EH_FRAME: u32 = 0x6474_e550;
/// Stack permissions (`p_flags` without `PF_X` means a non-executable stack).
pub const PT_GNU_STACK: u32 = 0x6474_e551;
/// The x86 feature-bit note GNU ld copies out of its inputs.
pub const PT_GNU_PROPERTY: u32 = 0x6474_e553;

/// Segment is executable.
pub const PF_X: u32 = 0x1;
/// Segment is writable.
pub const PF_W: u32 = 0x2;
/// Segment is readable.
pub const PF_R: u32 = 0x4;

/// Render `p_flags` the way `readelf -l` does (`R E`, `RW`, …).
pub fn flags_string(flags: u32) -> String {
    let mut s = String::new();
    s.push(if flags & PF_R != 0 { 'R' } else { ' ' });
    s.push(if flags & PF_W != 0 { 'W' } else { ' ' });
    s.push(if flags & PF_X != 0 { 'X' } else { ' ' });
    s
}

// ---------------------------------------------------------------------------------------
// Symbols
// ---------------------------------------------------------------------------------------

/// A local symbol: invisible outside its own object.
pub const STB_LOCAL: u8 = 0;
/// A global symbol.
pub const STB_GLOBAL: u8 = 1;
/// A weak symbol: a strong definition wins over it, and an undefined one resolves to 0.
pub const STB_WEAK: u8 = 2;

/// No type.
pub const STT_NOTYPE: u8 = 0;
/// A data object.
pub const STT_OBJECT: u8 = 1;
/// A function.
pub const STT_FUNC: u8 = 2;
/// A section symbol; its value is the section's address.
pub const STT_SECTION: u8 = 3;
/// A file name symbol.
pub const STT_FILE: u8 = 4;
/// A common block (rare; the suite uses `SHN_COMMON` instead).
pub const STT_COMMON: u8 = 5;

/// Visibility as declared (the binding decides).
pub const STV_DEFAULT: u8 = 0;
/// Not visible outside the component being linked.
pub const STV_INTERNAL: u8 = 1;
/// Not visible to other components; still resolvable inside this link.
pub const STV_HIDDEN: u8 = 2;
/// Visible outside but not preemptible.
pub const STV_PROTECTED: u8 = 3;

/// Pack `st_info` from a binding and a type.
pub fn st_info(bind: u8, stype: u8) -> u8 {
    (bind << 4) | (stype & 0xf)
}

/// The binding half of `st_info`.
pub fn st_bind(info: u8) -> u8 {
    info >> 4
}

/// The type half of `st_info`.
pub fn st_type(info: u8) -> u8 {
    info & 0xf
}

/// Pack `r_info` from a symbol index and a relocation type.
pub fn r_info(sym: u32, kind: u32) -> u64 {
    ((sym as u64) << 32) | (kind as u64)
}

/// The symbol index half of `r_info`.
pub fn r_sym(info: u64) -> u32 {
    (info >> 32) as u32
}

/// The relocation type half of `r_info`.
pub fn r_type(info: u64) -> u32 {
    (info & 0xffff_ffff) as u32
}

// ---------------------------------------------------------------------------------------
// x86-64 relocation types
// ---------------------------------------------------------------------------------------

/// No relocation.
pub const R_X86_64_NONE: u32 = 0;
/// `S + A`, 64 bits.
pub const R_X86_64_64: u32 = 1;
/// `S + A - P`, 32 bits, signed range check.
pub const R_X86_64_PC32: u32 = 2;
/// `G + A`: the offset of the symbol's GOT entry.
pub const R_X86_64_GOT32: u32 = 3;
/// `L + A - P`: a call through the PLT; identical to `PC32` in a static link with no PLT.
pub const R_X86_64_PLT32: u32 = 4;
/// `GOT + G + A - P`: the address of the symbol's GOT entry, PC-relative.
pub const R_X86_64_GOTPCREL: u32 = 9;
/// `S + A`, 32 bits, zero-extended; must fit in an unsigned 32-bit value.
pub const R_X86_64_32: u32 = 10;
/// `S + A`, 32 bits, sign-extended; must fit in a *signed* 32-bit value.
pub const R_X86_64_32S: u32 = 11;
/// `S + A`, 16 bits.
pub const R_X86_64_16: u32 = 12;
/// `S + A - P`, 16 bits.
pub const R_X86_64_PC16: u32 = 13;
/// `S + A`, 8 bits.
pub const R_X86_64_8: u32 = 14;
/// `S + A - P`, 8 bits.
pub const R_X86_64_PC8: u32 = 15;
/// `S + A - P`, 64 bits.
pub const R_X86_64_PC64: u32 = 24;
/// An offset from the thread pointer, for the local-exec TLS model.
pub const R_X86_64_TPOFF32: u32 = 23;
/// Like [`R_X86_64_GOTPCREL`], but the linker may relax the instruction.
pub const R_X86_64_GOTPCRELX: u32 = 41;
/// Like [`R_X86_64_GOTPCRELX`] for a REX-prefixed instruction.
pub const R_X86_64_REX_GOTPCRELX: u32 = 42;

/// The conventional name of a relocation type, for reports.
pub fn reloc_name(kind: u32) -> String {
    match kind {
        R_X86_64_NONE => "R_X86_64_NONE".into(),
        R_X86_64_64 => "R_X86_64_64".into(),
        R_X86_64_PC32 => "R_X86_64_PC32".into(),
        R_X86_64_GOT32 => "R_X86_64_GOT32".into(),
        R_X86_64_PLT32 => "R_X86_64_PLT32".into(),
        R_X86_64_GOTPCREL => "R_X86_64_GOTPCREL".into(),
        R_X86_64_32 => "R_X86_64_32".into(),
        R_X86_64_32S => "R_X86_64_32S".into(),
        R_X86_64_16 => "R_X86_64_16".into(),
        R_X86_64_PC16 => "R_X86_64_PC16".into(),
        R_X86_64_8 => "R_X86_64_8".into(),
        R_X86_64_PC8 => "R_X86_64_PC8".into(),
        R_X86_64_PC64 => "R_X86_64_PC64".into(),
        R_X86_64_GOTPCRELX => "R_X86_64_GOTPCRELX".into(),
        R_X86_64_REX_GOTPCRELX => "R_X86_64_REX_GOTPCRELX".into(),
        other => format!("R_X86_64_<{other}>"),
    }
}

/// The conventional name of a section type, for reports.
pub fn section_type_name(sh_type: u32) -> String {
    match sh_type {
        SHT_NULL => "NULL".into(),
        SHT_PROGBITS => "PROGBITS".into(),
        SHT_SYMTAB => "SYMTAB".into(),
        SHT_STRTAB => "STRTAB".into(),
        SHT_RELA => "RELA".into(),
        SHT_HASH => "HASH".into(),
        SHT_DYNAMIC => "DYNAMIC".into(),
        SHT_NOTE => "NOTE".into(),
        SHT_NOBITS => "NOBITS".into(),
        SHT_INIT_ARRAY => "INIT_ARRAY".into(),
        SHT_FINI_ARRAY => "FINI_ARRAY".into(),
        SHT_PREINIT_ARRAY => "PREINIT_ARRAY".into(),
        SHT_REL => "REL".into(),
        other => format!("0x{other:x}"),
    }
}

/// The conventional name of a segment type, for reports.
pub fn segment_type_name(p_type: u32) -> String {
    match p_type {
        PT_NULL => "NULL".into(),
        PT_LOAD => "LOAD".into(),
        PT_DYNAMIC => "DYNAMIC".into(),
        PT_INTERP => "INTERP".into(),
        PT_NOTE => "NOTE".into(),
        PT_PHDR => "PHDR".into(),
        PT_TLS => "TLS".into(),
        PT_GNU_EH_FRAME => "GNU_EH_FRAME".into(),
        PT_GNU_STACK => "GNU_STACK".into(),
        PT_GNU_PROPERTY => "GNU_PROPERTY".into(),
        other => format!("0x{other:x}"),
    }
}

/// `cat -n`-free hex dump, the same shape the terminal report uses elsewhere in the repo.
///
/// `marks` are byte ranges to underline with `^^`; at most `limit` bytes are shown.
pub fn hexdump(bytes: &[u8], marks: &[std::ops::Range<usize>], limit: usize) -> String {
    let shown = bytes.len().min(limit);
    let mut out = String::new();
    for (row, chunk) in bytes[..shown].chunks(16).enumerate() {
        let base = row * 16;
        let mut hex = String::new();
        let mut ascii = String::new();
        for (i, b) in chunk.iter().enumerate() {
            hex.push_str(&format!("{b:02x} "));
            if i == 7 {
                hex.push(' ');
            }
            ascii.push(if (0x20..0x7f).contains(b) {
                *b as char
            } else {
                '.'
            });
        }
        out.push_str(&format!("{base:04x}  {hex:<50}|{ascii}|\n"));
        // The `^^` row, when any mark falls inside this line.
        let mut caret = String::new();
        let mut any = false;
        for i in 0..chunk.len() {
            let at = base + i;
            let hit = marks.iter().any(|m| m.contains(&at));
            any |= hit;
            caret.push_str(if hit { "^^ " } else { "   " });
            if i == 7 {
                caret.push(' ');
            }
        }
        if any {
            out.push_str(&format!("      {caret}\n"));
        }
    }
    if bytes.len() > shown {
        out.push_str(&format!("      ... {} more bytes\n", bytes.len() - shown));
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_fields_pack_and_unpack() {
        let info = st_info(STB_WEAK, STT_FUNC);
        assert_eq!(info, 0x22);
        assert_eq!(st_bind(info), STB_WEAK);
        assert_eq!(st_type(info), STT_FUNC);

        let r = r_info(7, R_X86_64_PC32);
        assert_eq!(r, 0x0000_0007_0000_0002);
        assert_eq!(r_sym(r), 7);
        assert_eq!(r_type(r), R_X86_64_PC32);
    }

    #[test]
    fn hexdump_marks_the_interesting_bytes() {
        let mark = 1..3;
        let dump = hexdump(&[0u8, 1, 2, 3], std::slice::from_ref(&mark), 64);
        assert!(dump.contains("0000  00 01 02 03"), "{dump}");
        assert!(dump.contains("^^"), "{dump}");
    }

    #[test]
    fn flag_strings_read_like_readelf() {
        assert_eq!(flags_string(PF_R | PF_X), "R X");
        assert_eq!(flags_string(PF_R | PF_W), "RW ");
    }
}
