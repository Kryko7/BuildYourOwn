//! Turning an input object into annotated bytes.
//!
//! Every worked example shows the learner an actual relocatable object — the same bytes the
//! tests feed the linker — with each field labelled: the ELF header, every section header,
//! every symbol table entry and every relocation. The walk is generic, so a stage's example
//! only has to *build* an object and everything else follows.
//!
//! Offsets are byte offsets into the file, which is also the offset into the hex string's
//! bytes, exactly as the other testers in this repo define them.

use crate::elf::read::Elf;
use crate::elf::*;
use serde::{Deserialize, Serialize};

/// One annotated field inside an example's bytes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FieldAnn {
    /// Byte offset into the file.
    pub offset: usize,
    /// Length in bytes.
    pub length: usize,
    /// Field path, e.g. `header.e_type` or `sections[1].sh_offset`.
    pub field: String,
    /// The decoded value, written the way the report writes it.
    pub value: String,
}

fn ann(offset: u64, length: u64, field: impl Into<String>, value: impl Into<String>) -> FieldAnn {
    FieldAnn {
        offset: offset as usize,
        length: length as usize,
        field: field.into(),
        value: value.into(),
    }
}

/// At most this many entries of any table are annotated individually; the rest are covered
/// by one summary entry, so an example with 200 symbols does not produce 200 rows.
const TABLE_LIMIT: usize = 6;

/// Annotate a relocatable object (or any ELF64 file this parser can read).
pub fn object(bytes: &[u8]) -> Result<Vec<FieldAnn>, String> {
    let elf = Elf::parse(bytes).map_err(|e| format!("cannot annotate: {e}"))?;
    let mut out = Vec::new();

    // --- the file header ---------------------------------------------------------------
    out.push(ann(0, 4, "header.e_ident.magic", "7f 45 4c 46 — \\x7fELF"));
    out.push(ann(
        4,
        1,
        "header.e_ident[EI_CLASS]",
        match elf.ident[4] {
            ELFCLASS64 => "2 (ELFCLASS64)".to_string(),
            other => format!("{other}"),
        },
    ));
    out.push(ann(
        5,
        1,
        "header.e_ident[EI_DATA]",
        match elf.ident[5] {
            ELFDATA2LSB => "1 (little-endian)".to_string(),
            other => format!("{other}"),
        },
    ));
    out.push(ann(
        6,
        1,
        "header.e_ident[EI_VERSION]",
        format!("{}", elf.ident[6]),
    ));
    out.push(ann(
        7,
        1,
        "header.e_ident[EI_OSABI]",
        format!("{}", elf.ident[7]),
    ));
    out.push(ann(8, 8, "header.e_ident.padding", "reserved, all zero"));
    out.push(ann(
        16,
        2,
        "header.e_type",
        match elf.e_type {
            ET_REL => "1 (ET_REL, a relocatable object)".to_string(),
            ET_EXEC => "2 (ET_EXEC, an executable)".to_string(),
            other => format!("{other}"),
        },
    ));
    out.push(ann(
        18,
        2,
        "header.e_machine",
        match elf.e_machine {
            EM_X86_64 => "62 (EM_X86_64)".to_string(),
            EM_386 => "3 (EM_386)".to_string(),
            other => format!("{other}"),
        },
    ));
    out.push(ann(20, 4, "header.e_version", format!("{}", elf.e_version)));
    out.push(ann(24, 8, "header.e_entry", format!("0x{:x}", elf.entry)));
    out.push(ann(32, 8, "header.e_phoff", format!("{}", elf.phoff)));
    out.push(ann(
        40,
        8,
        "header.e_shoff",
        format!("{} (the section header table)", elf.shoff),
    ));
    out.push(ann(48, 4, "header.e_flags", format!("0x{:x}", elf.e_flags)));
    out.push(ann(52, 2, "header.e_ehsize", format!("{}", elf.ehsize)));
    out.push(ann(
        54,
        2,
        "header.e_phentsize",
        format!("{}", elf.phentsize),
    ));
    out.push(ann(56, 2, "header.e_phnum", format!("{}", elf.phnum)));
    out.push(ann(
        58,
        2,
        "header.e_shentsize",
        format!("{}", elf.shentsize),
    ));
    out.push(ann(
        60,
        2,
        "header.e_shnum",
        format!("{} section headers", elf.shnum),
    ));
    out.push(ann(
        62,
        2,
        "header.e_shstrndx",
        format!("{} (the section name string table)", elf.shstrndx),
    ));

    // --- section headers ---------------------------------------------------------------
    let step = elf.shentsize.max(SHDR_SIZE) as u64;
    for s in elf.sections.iter().take(TABLE_LIMIT + 1) {
        let base = elf.shoff + (s.index as u64) * step;
        let name = if s.name.is_empty() {
            "(the null section)".to_string()
        } else {
            s.name.clone()
        };
        out.push(ann(
            base,
            4,
            format!("sections[{}].sh_name", s.index),
            format!("'{name}'"),
        ));
        out.push(ann(
            base + 4,
            4,
            format!("sections[{}].sh_type", s.index),
            section_type_name(s.sh_type),
        ));
        out.push(ann(
            base + 8,
            8,
            format!("sections[{}].sh_flags", s.index),
            section_flags(s.flags),
        ));
        out.push(ann(
            base + 24,
            8,
            format!("sections[{}].sh_offset", s.index),
            format!("{} (where '{name}' starts in the file)", s.offset),
        ));
        out.push(ann(
            base + 32,
            8,
            format!("sections[{}].sh_size", s.index),
            format!("{} bytes", s.size),
        ));
        out.push(ann(
            base + 48,
            8,
            format!("sections[{}].sh_addralign", s.index),
            format!("{}", s.align),
        ));
    }
    if elf.sections.len() > TABLE_LIMIT + 1 {
        let from = elf.shoff + ((TABLE_LIMIT + 1) as u64) * step;
        let count = elf.sections.len() - (TABLE_LIMIT + 1);
        out.push(ann(
            from,
            (count as u64) * step,
            "sections[..]",
            format!("{count} more section headers, same shape"),
        ));
    }

    // --- content ------------------------------------------------------------------------
    for s in &elf.sections {
        if s.size == 0 || s.sh_type == SHT_NOBITS || s.sh_type == SHT_NULL {
            continue;
        }
        match s.sh_type {
            SHT_SYMTAB => annotate_symbols(&elf, s.index, &mut out),
            SHT_RELA => annotate_relocations(&elf, s.index, &mut out),
            SHT_STRTAB => out.push(ann(
                s.offset,
                s.size,
                format!("{}.contents", s.name),
                format!("{} bytes of NUL-terminated strings", s.size),
            )),
            _ => out.push(ann(
                s.offset,
                s.size,
                format!("{}.contents", s.name),
                describe_content(&elf, s.index),
            )),
        }
    }

    out.retain(|a| a.offset + a.length <= bytes.len());
    out.sort_by_key(|a| a.offset);
    Ok(out)
}

fn section_flags(flags: u64) -> String {
    let mut parts = Vec::new();
    if flags & SHF_WRITE != 0 {
        parts.push("SHF_WRITE");
    }
    if flags & SHF_ALLOC != 0 {
        parts.push("SHF_ALLOC");
    }
    if flags & SHF_EXECINSTR != 0 {
        parts.push("SHF_EXECINSTR");
    }
    if flags & SHF_INFO_LINK != 0 {
        parts.push("SHF_INFO_LINK");
    }
    if parts.is_empty() {
        format!("0x{flags:x}")
    } else {
        format!("0x{flags:x} ({})", parts.join(" | "))
    }
}

fn describe_content(elf: &Elf, index: usize) -> String {
    let s = &elf.sections[index];
    let data = elf.slice(s.offset, s.size).unwrap_or_default();
    if s.is_exec() {
        return format!("{} bytes of machine code", s.size);
    }
    let printable = data.iter().all(|b| (0x20..0x7f).contains(b) || *b == b'\n');
    if printable {
        format!("{} bytes: {:?}", s.size, String::from_utf8_lossy(data))
    } else {
        format!("{} bytes of data", s.size)
    }
}

fn annotate_symbols(elf: &Elf, index: usize, out: &mut Vec<FieldAnn>) {
    let s = &elf.sections[index];
    let step = if s.entsize == 0 { SYM_SIZE } else { s.entsize };
    for (i, sym) in elf.symbols.iter().enumerate().take(TABLE_LIMIT + 1) {
        let base = s.offset + (i as u64) * step;
        let name = if sym.name.is_empty() {
            "(unnamed)".to_string()
        } else {
            format!("'{}'", sym.name)
        };
        out.push(ann(base, 4, format!("symtab[{i}].st_name"), name));
        out.push(ann(
            base + 4,
            1,
            format!("symtab[{i}].st_info"),
            format!("{} {}", bind_name(sym.bind), type_name(sym.stype)),
        ));
        out.push(ann(
            base + 5,
            1,
            format!("symtab[{i}].st_other"),
            visibility_name(sym.visibility).to_string(),
        ));
        out.push(ann(
            base + 6,
            2,
            format!("symtab[{i}].st_shndx"),
            match sym.shndx {
                SHN_UNDEF => "SHN_UNDEF (a reference, not a definition)".to_string(),
                SHN_ABS => "SHN_ABS (absolute, never relocated)".to_string(),
                SHN_COMMON => "SHN_COMMON (a tentative definition)".to_string(),
                n => format!(
                    "{n} ('{}')",
                    elf.sections
                        .get(n as usize)
                        .map(|x| x.name.as_str())
                        .unwrap_or("?")
                ),
            },
        ));
        out.push(ann(
            base + 8,
            8,
            format!("symtab[{i}].st_value"),
            format!("0x{:x}", sym.value),
        ));
        out.push(ann(
            base + 16,
            8,
            format!("symtab[{i}].st_size"),
            format!("{}", sym.size),
        ));
    }
    if elf.symbols.len() > TABLE_LIMIT + 1 {
        let from = s.offset + ((TABLE_LIMIT + 1) as u64) * step;
        let count = elf.symbols.len() - (TABLE_LIMIT + 1);
        out.push(ann(
            from,
            (count as u64) * step,
            "symtab[..]",
            format!("{count} more symbols, same shape"),
        ));
    }
}

fn annotate_relocations(elf: &Elf, index: usize, out: &mut Vec<FieldAnn>) {
    let s = &elf.sections[index];
    let target = elf
        .sections
        .get(s.info as usize)
        .map(|x| x.name.clone())
        .unwrap_or_default();
    let count = (s.size / RELA_SIZE) as usize;
    for i in 0..count.min(TABLE_LIMIT + 1) {
        let base = s.offset + (i as u64) * RELA_SIZE;
        let Ok(chunk) = elf.slice(base, RELA_SIZE) else {
            continue;
        };
        let mut o = [0u8; 8];
        o.copy_from_slice(&chunk[0..8]);
        let offset = u64::from_le_bytes(o);
        o.copy_from_slice(&chunk[8..16]);
        let info = u64::from_le_bytes(o);
        o.copy_from_slice(&chunk[16..24]);
        let addend = u64::from_le_bytes(o) as i64;
        let sym = r_sym(info) as usize;
        let sym_name = elf
            .symbols
            .get(sym)
            .map(|x| {
                if x.name.is_empty() {
                    format!(
                        "the section symbol of '{}'",
                        elf.sections
                            .get(x.shndx as usize)
                            .map(|s| s.name.as_str())
                            .unwrap_or("?")
                    )
                } else {
                    format!("'{}'", x.name)
                }
            })
            .unwrap_or_else(|| format!("symbol {sym}"));
        out.push(ann(
            base,
            8,
            format!("{}[{i}].r_offset", s.name),
            format!("0x{offset:x} — the field to patch, inside '{target}'"),
        ));
        out.push(ann(
            base + 8,
            8,
            format!("{}[{i}].r_info", s.name),
            format!(
                "symbol {sym} ({sym_name}), type {}",
                reloc_name(r_type(info))
            ),
        ));
        out.push(ann(
            base + 16,
            8,
            format!("{}[{i}].r_addend", s.name),
            format!("{addend} (the A in S + A - P)"),
        ));
    }
    if count > TABLE_LIMIT + 1 {
        let from = s.offset + ((TABLE_LIMIT + 1) as u64) * RELA_SIZE;
        let left = count - (TABLE_LIMIT + 1);
        out.push(ann(
            from,
            (left as u64) * RELA_SIZE,
            format!("{}[..]", s.name),
            format!("{left} more relocations, same shape"),
        ));
    }
}

fn bind_name(bind: u8) -> &'static str {
    match bind {
        STB_LOCAL => "STB_LOCAL",
        STB_GLOBAL => "STB_GLOBAL",
        STB_WEAK => "STB_WEAK",
        _ => "STB_?",
    }
}

fn type_name(stype: u8) -> &'static str {
    match stype {
        STT_NOTYPE => "STT_NOTYPE",
        STT_OBJECT => "STT_OBJECT",
        STT_FUNC => "STT_FUNC",
        STT_SECTION => "STT_SECTION",
        STT_FILE => "STT_FILE",
        STT_COMMON => "STT_COMMON",
        _ => "STT_?",
    }
}

fn visibility_name(v: u8) -> &'static str {
    match v {
        STV_DEFAULT => "STV_DEFAULT",
        STV_INTERNAL => "STV_INTERNAL",
        STV_HIDDEN => "STV_HIDDEN",
        STV_PROTECTED => "STV_PROTECTED",
        _ => "STV_?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stages::helpers;

    #[test]
    fn every_annotation_is_inside_the_file_and_labelled() {
        let bytes = helpers::caller("hi\n", "other").expect("fixture");
        let anns = object(&bytes).expect("annotate");
        assert!(anns.len() > 30, "a two-section object has plenty of fields");
        for a in &anns {
            assert!(
                a.offset + a.length <= bytes.len(),
                "{} runs past the end of the file",
                a.field
            );
            assert!(!a.field.is_empty() && !a.value.is_empty());
        }
    }

    #[test]
    fn the_relocation_entries_are_explained() {
        let bytes = helpers::caller("hi\n", "other").expect("fixture");
        let anns = object(&bytes).expect("annotate");
        let reloc: Vec<&FieldAnn> = anns
            .iter()
            .filter(|a| a.field.starts_with(".rela.text"))
            .collect();
        assert_eq!(reloc.len(), 6, "two relocations, three fields each");
        assert!(reloc
            .iter()
            .any(|a| a.value.contains("R_X86_64_PLT32") && a.value.contains("'other'")));
    }

    #[test]
    fn the_header_is_walked_from_the_first_byte() {
        let bytes = helpers::exit_only(0).expect("fixture");
        let anns = object(&bytes).expect("annotate");
        assert_eq!(anns[0].offset, 0);
        assert_eq!(anns[0].field, "header.e_ident.magic");
        assert!(anns.iter().any(|a| a.field == "header.e_shoff"));
    }
}
