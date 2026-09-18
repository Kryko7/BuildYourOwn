//! The ELF64 reader.
//!
//! This is leg two of the three-legged verification: whatever the linker under test wrote,
//! the suite parses it back with its own parser and asserts on the structure — header
//! fields, program headers, segment permissions, the entry point, where a symbol ended up,
//! and the bytes a relocation was supposed to patch.
//!
//! It is deliberately paranoid. The input is a file a half-finished linker produced, so
//! every offset is bounds-checked and nothing here can panic: parsing returns
//! [`Result<Elf, ElfError>`] and so does every lookup that could run off the end.

use super::*;
use std::fmt;

/// Why a file could not be read as ELF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElfError {
    /// What went wrong, in one line.
    pub message: String,
}

impl ElfError {
    fn new(message: impl Into<String>) -> ElfError {
        ElfError {
            message: message.into(),
        }
    }
}

impl fmt::Display for ElfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ElfError {}

/// A parsed section header, with its name resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// Index in the section header table.
    pub index: usize,
    /// Resolved `sh_name`; empty when `.shstrtab` could not be read.
    pub name: String,
    /// `sh_type`.
    pub sh_type: u32,
    /// `sh_flags`.
    pub flags: u64,
    /// `sh_addr`.
    pub addr: u64,
    /// `sh_offset`.
    pub offset: u64,
    /// `sh_size`.
    pub size: u64,
    /// `sh_link`.
    pub link: u32,
    /// `sh_info`.
    pub info: u32,
    /// `sh_addralign`.
    pub align: u64,
    /// `sh_entsize`.
    pub entsize: u64,
}

impl Section {
    /// True when the section is mapped at run time.
    pub fn is_alloc(&self) -> bool {
        self.flags & SHF_ALLOC != 0
    }

    /// True when the section is writable at run time.
    pub fn is_write(&self) -> bool {
        self.flags & SHF_WRITE != 0
    }

    /// True when the section holds instructions.
    pub fn is_exec(&self) -> bool {
        self.flags & SHF_EXECINSTR != 0
    }

    /// True when the section occupies no file space.
    pub fn is_nobits(&self) -> bool {
        self.sh_type == SHT_NOBITS
    }
}

/// A parsed program header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    /// Index in the program header table.
    pub index: usize,
    /// `p_type`.
    pub p_type: u32,
    /// `p_flags`.
    pub flags: u32,
    /// `p_offset`.
    pub offset: u64,
    /// `p_vaddr`.
    pub vaddr: u64,
    /// `p_paddr`.
    pub paddr: u64,
    /// `p_filesz`.
    pub filesz: u64,
    /// `p_memsz`.
    pub memsz: u64,
    /// `p_align`.
    pub align: u64,
}

impl Segment {
    /// True when the segment is readable.
    pub fn readable(&self) -> bool {
        self.flags & PF_R != 0
    }

    /// True when the segment is writable.
    pub fn writable(&self) -> bool {
        self.flags & PF_W != 0
    }

    /// True when the segment is executable.
    pub fn executable(&self) -> bool {
        self.flags & PF_X != 0
    }

    /// True when `addr` falls inside the segment's memory image.
    pub fn contains(&self, addr: u64) -> bool {
        addr >= self.vaddr && addr < self.vaddr.saturating_add(self.memsz)
    }

    /// How the report names this segment.
    pub fn describe(&self) -> String {
        format!(
            "{} vaddr 0x{:x} offset 0x{:x} filesz 0x{:x} memsz 0x{:x} flags {} align 0x{:x}",
            segment_type_name(self.p_type),
            self.vaddr,
            self.offset,
            self.filesz,
            self.memsz,
            flags_string(self.flags),
            self.align
        )
    }
}

/// A parsed symbol table entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    /// Index in `.symtab`.
    pub index: usize,
    /// Resolved `st_name`.
    pub name: String,
    /// `st_info`'s binding half.
    pub bind: u8,
    /// `st_info`'s type half.
    pub stype: u8,
    /// `st_other`.
    pub visibility: u8,
    /// `st_shndx`.
    pub shndx: u16,
    /// `st_value`.
    pub value: u64,
    /// `st_size`.
    pub size: u64,
}

impl Symbol {
    /// True when the symbol is a reference, not a definition.
    pub fn is_undefined(&self) -> bool {
        self.shndx == SHN_UNDEF
    }

    /// True when the symbol is absolute.
    pub fn is_absolute(&self) -> bool {
        self.shndx == SHN_ABS
    }

    /// True when the symbol is a tentative definition.
    pub fn is_common(&self) -> bool {
        self.shndx == SHN_COMMON
    }
}

/// A parsed relocation entry, with the section it applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relocation {
    /// Index of the section the relocation patches (`sh_info` of the `.rela` section).
    pub target_section: usize,
    /// `r_offset`.
    pub offset: u64,
    /// Symbol table index out of `r_info`.
    pub sym: u32,
    /// Relocation type out of `r_info`.
    pub kind: u32,
    /// `r_addend`; 0 for `SHT_REL`.
    pub addend: i64,
}

/// A parsed ELF64 file — a relocatable object or an executable.
#[derive(Debug, Clone)]
pub struct Elf {
    /// The bytes that were parsed.
    pub bytes: Vec<u8>,
    /// `e_ident`.
    pub ident: [u8; EI_NIDENT],
    /// `e_type`.
    pub e_type: u16,
    /// `e_machine`.
    pub e_machine: u16,
    /// `e_version`.
    pub e_version: u32,
    /// `e_entry`.
    pub entry: u64,
    /// `e_phoff`.
    pub phoff: u64,
    /// `e_shoff`.
    pub shoff: u64,
    /// `e_flags`.
    pub e_flags: u32,
    /// `e_ehsize`.
    pub ehsize: u16,
    /// `e_phentsize`.
    pub phentsize: u16,
    /// `e_phnum`.
    pub phnum: u16,
    /// `e_shentsize`.
    pub shentsize: u16,
    /// `e_shnum`.
    pub shnum: u16,
    /// `e_shstrndx`.
    pub shstrndx: u16,
    /// Every section header, in table order.
    pub sections: Vec<Section>,
    /// Every program header, in table order.
    pub segments: Vec<Segment>,
    /// `.symtab`, when the file has one.
    pub symbols: Vec<Symbol>,
    /// Every `SHT_RELA`/`SHT_REL` entry in the file.
    pub relocations: Vec<Relocation>,
}

fn u16le(b: &[u8], at: usize) -> Result<u16, ElfError> {
    let end = at + 2;
    if end > b.len() {
        return Err(ElfError::new(format!(
            "wanted 2 bytes at offset {at}, the file is only {} bytes",
            b.len()
        )));
    }
    Ok(u16::from_le_bytes([b[at], b[at + 1]]))
}

fn u32le(b: &[u8], at: usize) -> Result<u32, ElfError> {
    let end = at + 4;
    if end > b.len() {
        return Err(ElfError::new(format!(
            "wanted 4 bytes at offset {at}, the file is only {} bytes",
            b.len()
        )));
    }
    let mut v = [0u8; 4];
    v.copy_from_slice(&b[at..end]);
    Ok(u32::from_le_bytes(v))
}

fn u64le(b: &[u8], at: usize) -> Result<u64, ElfError> {
    let end = at + 8;
    if end > b.len() {
        return Err(ElfError::new(format!(
            "wanted 8 bytes at offset {at}, the file is only {} bytes",
            b.len()
        )));
    }
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[at..end]);
    Ok(u64::from_le_bytes(v))
}

/// A NUL-terminated string starting at `at` inside `table`.
fn cstr(table: &[u8], at: usize) -> String {
    if at >= table.len() {
        return String::new();
    }
    let end = table[at..]
        .iter()
        .position(|b| *b == 0)
        .map(|n| at + n)
        .unwrap_or(table.len());
    String::from_utf8_lossy(&table[at..end]).to_string()
}

impl Elf {
    /// Parse a file. Anything malformed is an error, never a panic.
    pub fn parse(bytes: &[u8]) -> Result<Elf, ElfError> {
        if bytes.len() < EHDR_SIZE as usize {
            return Err(ElfError::new(format!(
                "the file is {} bytes; an ELF64 header alone is {EHDR_SIZE}",
                bytes.len()
            )));
        }
        if bytes[..4] != ELF_MAGIC {
            return Err(ElfError::new(format!(
                "bad magic {:02x?}, expected 7f 45 4c 46",
                &bytes[..4]
            )));
        }
        if bytes[4] != ELFCLASS64 {
            return Err(ElfError::new(format!(
                "e_ident[EI_CLASS] is {}, this parser only reads ELFCLASS64 (2)",
                bytes[4]
            )));
        }
        if bytes[5] != ELFDATA2LSB {
            return Err(ElfError::new(format!(
                "e_ident[EI_DATA] is {}, this parser only reads little-endian (1)",
                bytes[5]
            )));
        }
        let mut ident = [0u8; EI_NIDENT];
        ident.copy_from_slice(&bytes[..EI_NIDENT]);

        let mut elf = Elf {
            bytes: bytes.to_vec(),
            ident,
            e_type: u16le(bytes, 16)?,
            e_machine: u16le(bytes, 18)?,
            e_version: u32le(bytes, 20)?,
            entry: u64le(bytes, 24)?,
            phoff: u64le(bytes, 32)?,
            shoff: u64le(bytes, 40)?,
            e_flags: u32le(bytes, 48)?,
            ehsize: u16le(bytes, 52)?,
            phentsize: u16le(bytes, 54)?,
            phnum: u16le(bytes, 56)?,
            shentsize: u16le(bytes, 58)?,
            shnum: u16le(bytes, 60)?,
            shstrndx: u16le(bytes, 62)?,
            sections: Vec::new(),
            segments: Vec::new(),
            symbols: Vec::new(),
            relocations: Vec::new(),
        };
        elf.parse_segments()?;
        elf.parse_sections()?;
        elf.parse_symbols()?;
        elf.parse_relocations()?;
        Ok(elf)
    }

    fn parse_segments(&mut self) -> Result<(), ElfError> {
        if self.phnum == 0 {
            return Ok(());
        }
        let entry_size = self.phentsize as usize;
        if entry_size < PHDR_SIZE as usize {
            return Err(ElfError::new(format!(
                "e_phentsize is {entry_size}, an ELF64 program header is {PHDR_SIZE} bytes"
            )));
        }
        let base = self.phoff as usize;
        for i in 0..self.phnum as usize {
            let at = base
                .checked_add(i * entry_size)
                .ok_or_else(|| ElfError::new("e_phoff overflows"))?;
            let b = &self.bytes;
            self.segments.push(Segment {
                index: i,
                p_type: u32le(b, at)?,
                flags: u32le(b, at + 4)?,
                offset: u64le(b, at + 8)?,
                vaddr: u64le(b, at + 16)?,
                paddr: u64le(b, at + 24)?,
                filesz: u64le(b, at + 32)?,
                memsz: u64le(b, at + 40)?,
                align: u64le(b, at + 48)?,
            });
        }
        Ok(())
    }

    fn parse_sections(&mut self) -> Result<(), ElfError> {
        if self.shnum == 0 || self.shoff == 0 {
            return Ok(());
        }
        let entry_size = self.shentsize as usize;
        if entry_size < SHDR_SIZE as usize {
            return Err(ElfError::new(format!(
                "e_shentsize is {entry_size}, an ELF64 section header is {SHDR_SIZE} bytes"
            )));
        }
        let base = self.shoff as usize;
        let mut raw = Vec::new();
        for i in 0..self.shnum as usize {
            let at = base
                .checked_add(i * entry_size)
                .ok_or_else(|| ElfError::new("e_shoff overflows"))?;
            let b = &self.bytes;
            raw.push((
                u32le(b, at)?, // sh_name
                Section {
                    index: i,
                    name: String::new(),
                    sh_type: u32le(b, at + 4)?,
                    flags: u64le(b, at + 8)?,
                    addr: u64le(b, at + 16)?,
                    offset: u64le(b, at + 24)?,
                    size: u64le(b, at + 32)?,
                    link: u32le(b, at + 40)?,
                    info: u32le(b, at + 44)?,
                    align: u64le(b, at + 48)?,
                    entsize: u64le(b, at + 56)?,
                },
            ));
        }
        // `.shstrtab` names the rest; a file whose e_shstrndx is nonsense still parses, it
        // just has nameless sections — which is what lets a test say so precisely.
        let names: Vec<u8> = raw
            .get(self.shstrndx as usize)
            .map(|(_, s)| self.slice(s.offset, s.size).unwrap_or_default().to_vec())
            .unwrap_or_default();
        self.sections = raw
            .into_iter()
            .map(|(name_off, mut s)| {
                s.name = cstr(&names, name_off as usize);
                s
            })
            .collect();
        Ok(())
    }

    fn parse_symbols(&mut self) -> Result<(), ElfError> {
        let Some(symtab) = self
            .sections
            .iter()
            .find(|s| s.sh_type == SHT_SYMTAB)
            .cloned()
        else {
            return Ok(());
        };
        let strings: Vec<u8> = self
            .sections
            .get(symtab.link as usize)
            .and_then(|s| self.slice(s.offset, s.size).ok())
            .map(<[u8]>::to_vec)
            .unwrap_or_default();
        let data = self.slice(symtab.offset, symtab.size)?.to_vec();
        let step = if symtab.entsize == 0 {
            SYM_SIZE
        } else {
            symtab.entsize
        } as usize;
        if step < SYM_SIZE as usize {
            return Err(ElfError::new(format!(
                ".symtab has sh_entsize {step}, an ELF64 symbol is {SYM_SIZE} bytes"
            )));
        }
        for (i, chunk) in data.chunks(step).enumerate() {
            if chunk.len() < SYM_SIZE as usize {
                break;
            }
            let name_off = u32le(chunk, 0)?;
            let info = chunk[4];
            self.symbols.push(Symbol {
                index: i,
                name: cstr(&strings, name_off as usize),
                bind: st_bind(info),
                stype: st_type(info),
                visibility: chunk[5] & 0x3,
                shndx: u16le(chunk, 6)?,
                value: u64le(chunk, 8)?,
                size: u64le(chunk, 16)?,
            });
        }
        Ok(())
    }

    fn parse_relocations(&mut self) -> Result<(), ElfError> {
        let rela_sections: Vec<Section> = self
            .sections
            .iter()
            .filter(|s| s.sh_type == SHT_RELA || s.sh_type == SHT_REL)
            .cloned()
            .collect();
        for s in rela_sections {
            let explicit_addend = s.sh_type == SHT_RELA;
            let step = if explicit_addend { RELA_SIZE } else { REL_SIZE } as usize;
            let data = self.slice(s.offset, s.size)?.to_vec();
            for chunk in data.chunks(step) {
                if chunk.len() < step {
                    break;
                }
                let info = u64le(chunk, 8)?;
                self.relocations.push(Relocation {
                    target_section: s.info as usize,
                    offset: u64le(chunk, 0)?,
                    sym: r_sym(info),
                    kind: r_type(info),
                    addend: if explicit_addend {
                        u64le(chunk, 16)? as i64
                    } else {
                        0
                    },
                });
            }
        }
        Ok(())
    }

    /// A bounds-checked slice of the file.
    pub fn slice(&self, offset: u64, size: u64) -> Result<&[u8], ElfError> {
        let start = offset as usize;
        let end = start.checked_add(size as usize).ok_or_else(|| {
            ElfError::new(format!("offset 0x{offset:x} + size 0x{size:x} overflows"))
        })?;
        self.bytes.get(start..end).ok_or_else(|| {
            ElfError::new(format!(
                "bytes 0x{start:x}..0x{end:x} are past the end of a {}-byte file",
                self.bytes.len()
            ))
        })
    }

    /// The first section with this name.
    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name == name)
    }

    /// Every section with this name (a linker may keep several).
    pub fn sections_named<'s>(&'s self, name: &'s str) -> impl Iterator<Item = &'s Section> + 's {
        self.sections.iter().filter(move |s| s.name == name)
    }

    /// A section's bytes.
    pub fn section_data(&self, name: &str) -> Result<&[u8], ElfError> {
        let s = self
            .section(name)
            .ok_or_else(|| ElfError::new(format!("the file has no section named '{name}'")))?;
        if s.is_nobits() {
            return Ok(&[]);
        }
        self.slice(s.offset, s.size)
    }

    /// The first symbol with this name.
    pub fn symbol(&self, name: &str) -> Option<&Symbol> {
        self.symbols.iter().find(|s| s.name == name)
    }

    /// The address of a defined symbol.
    pub fn symbol_address(&self, name: &str) -> Result<u64, ElfError> {
        let s = self
            .symbol(name)
            .ok_or_else(|| ElfError::new(format!("the output has no symbol named '{name}'")))?;
        if s.is_undefined() {
            return Err(ElfError::new(format!(
                "symbol '{name}' is still undefined in the output"
            )));
        }
        Ok(s.value)
    }

    /// Every `PT_LOAD` segment.
    pub fn loads(&self) -> impl Iterator<Item = &Segment> {
        self.segments.iter().filter(|s| s.p_type == PT_LOAD)
    }

    /// The `PT_LOAD` whose memory image contains `addr`.
    pub fn segment_at(&self, addr: u64) -> Option<&Segment> {
        self.loads().find(|s| s.contains(addr))
    }

    /// `len` bytes of the file image at virtual address `addr`.
    ///
    /// This is how a test reads back the value a relocation was supposed to write: find the
    /// `PT_LOAD` that maps the address, convert to a file offset, slice.
    pub fn read_at_vaddr(&self, addr: u64, len: u64) -> Result<&[u8], ElfError> {
        let seg = self
            .segment_at(addr)
            .ok_or_else(|| ElfError::new(format!("no PT_LOAD segment maps address 0x{addr:x}")))?;
        let delta = addr - seg.vaddr;
        if delta + len > seg.filesz {
            return Err(ElfError::new(format!(
                "address 0x{addr:x} is inside the segment at 0x{:x} but past its {} file bytes \
                 (p_filesz 0x{:x}) — .bss has no bytes in the file",
                seg.vaddr, seg.filesz, seg.filesz
            )));
        }
        self.slice(seg.offset + delta, len)
    }

    /// The 32-bit little-endian value at a virtual address.
    pub fn u32_at_vaddr(&self, addr: u64) -> Result<u32, ElfError> {
        let b = self.read_at_vaddr(addr, 4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// The signed 32-bit little-endian value at a virtual address.
    pub fn i32_at_vaddr(&self, addr: u64) -> Result<i32, ElfError> {
        Ok(self.u32_at_vaddr(addr)? as i32)
    }

    /// The 64-bit little-endian value at a virtual address.
    pub fn u64_at_vaddr(&self, addr: u64) -> Result<u64, ElfError> {
        let b = self.read_at_vaddr(addr, 8)?;
        let mut v = [0u8; 8];
        v.copy_from_slice(b);
        Ok(u64::from_le_bytes(v))
    }

    /// The 16-bit little-endian value at a virtual address.
    pub fn u16_at_vaddr(&self, addr: u64) -> Result<u16, ElfError> {
        let b = self.read_at_vaddr(addr, 2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    /// The single byte at a virtual address.
    pub fn u8_at_vaddr(&self, addr: u64) -> Result<u8, ElfError> {
        Ok(self.read_at_vaddr(addr, 1)?[0])
    }

    /// A one-line summary for a failure's `context` block.
    pub fn describe(&self) -> String {
        format!(
            "e_type {} entry 0x{:x}, {} program headers, {} sections, {} symbols",
            match self.e_type {
                ET_REL => "ET_REL".to_string(),
                ET_EXEC => "ET_EXEC".to_string(),
                ET_DYN => "ET_DYN".to_string(),
                other => format!("{other}"),
            },
            self.entry,
            self.segments.len(),
            self.sections.len(),
            self.symbols.len()
        )
    }

    /// The `readelf -lW`-style table of program headers, for a failure block.
    pub fn program_header_table(&self) -> String {
        let mut out =
            String::from("  Type       Offset     VirtAddr   FileSiz    MemSiz     Flg Align\n");
        for s in &self.segments {
            out.push_str(&format!(
                "  {:<10} 0x{:08x} 0x{:08x} 0x{:08x} 0x{:08x} {} 0x{:x}\n",
                segment_type_name(s.p_type),
                s.offset,
                s.vaddr,
                s.filesz,
                s.memsz,
                flags_string(s.flags),
                s.align
            ));
        }
        out.trim_end().to_string()
    }

    /// The `readelf -SW`-style table of section headers, for a failure block.
    pub fn section_header_table(&self) -> String {
        let mut out =
            String::from("  Nr Name              Type       Addr       Off        Size       Al\n");
        for s in &self.sections {
            out.push_str(&format!(
                "  {:>2} {:<17} {:<10} 0x{:08x} 0x{:08x} 0x{:08x} {}\n",
                s.index,
                s.name,
                section_type_name(s.sh_type),
                s.addr,
                s.offset,
                s.size,
                s.align
            ));
        }
        out.trim_end().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elf::write::{ObjectBuilder, SectionSpec, SymbolSpec};

    #[test]
    fn a_written_object_reads_back() {
        let obj = ObjectBuilder::new()
            .section(SectionSpec::text(".text", vec![0xc3]))
            .section(SectionSpec::rodata(".rodata", b"hi".to_vec()))
            .symbol(SymbolSpec::global("_start", ".text", 0).func())
            .build()
            .expect("the writer must produce an object");
        let elf = Elf::parse(&obj).expect("and the reader must read it");
        assert_eq!(elf.e_type, ET_REL);
        assert_eq!(elf.e_machine, EM_X86_64);
        assert_eq!(elf.ehsize, EHDR_SIZE);
        assert_eq!(elf.shentsize, SHDR_SIZE);
        assert_eq!(elf.section(".text").map(|s| s.size), Some(1));
        assert_eq!(elf.section_data(".rodata").ok(), Some(&b"hi"[..]));
        let start = elf.symbol("_start").expect("_start must be in .symtab");
        assert_eq!(start.bind, STB_GLOBAL);
        assert_eq!(start.stype, STT_FUNC);
    }

    #[test]
    fn short_and_wrong_files_are_errors_not_panics() {
        assert!(Elf::parse(&[]).is_err());
        assert!(Elf::parse(&[0u8; 32]).is_err());
        let mut bad = vec![0u8; 128];
        bad[..4].copy_from_slice(b"\x7fELG");
        assert!(Elf::parse(&bad)
            .expect_err("bad magic")
            .message
            .contains("magic"));
    }

    #[test]
    fn a_lying_section_header_is_an_error_not_a_panic() {
        let obj = ObjectBuilder::new()
            .section(SectionSpec::text(".text", vec![0xc3]).offset_override(0xffff_0000))
            .build()
            .expect("build");
        let elf = Elf::parse(&obj).expect("the headers still parse");
        assert!(
            elf.section_data(".text").is_err(),
            "reading past the end must be an error"
        );
    }
}
