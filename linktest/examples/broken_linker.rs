//! A deliberately wrong linker, used to show what a red `linktest` run looks like.
//!
//! It does most of a static link correctly — it reads relocatable objects, concatenates
//! their sections, gives them addresses, resolves symbols, writes an `ET_EXEC` with three
//! `PT_LOAD` segments and a symbol table, and chmods the result — and then gets one thing
//! wrong: **it ignores the relocation addend**. So `S + A - P` comes out as `S - P`, every
//! RIP-relative reference lands four bytes past the thing it meant to name, and the linked
//! program prints the wrong string while still running perfectly happily.
//!
//! That is deliberate: it is the single most common bug in a first linker, and it produces
//! exactly the sort of failure the suite has to be able to explain.
//!
//! ```text
//! cargo build --release --example broken_linker
//! ./target/release/linktest --linker broken --until 5
//! ```
//!
//! It is **not** a reference implementation and it is not complete: no archives, no `-L`/`-l`,
//! no common symbols, no overflow checks. Stages past the first few fail for that reason too.

use linktest::elf::read::Elf;
use linktest::elf::*;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

/// Where the three output segments are placed. Nothing about these addresses is required;
/// they are simply page-aligned and out of the way.
const TEXT_ADDR: u64 = 0x40_1000;
const RODATA_ADDR: u64 = 0x40_2000;
const DATA_ADDR: u64 = 0x40_3000;
const TEXT_OFF: u64 = 0x1000;
const RODATA_OFF: u64 = 0x2000;
const DATA_OFF: u64 = 0x3000;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("broken_linker: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Which output section an input section is folded into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Text,
    Rodata,
    Data,
    Bss,
}

impl Kind {
    fn of(name: &str, flags: u64, sh_type: u32) -> Option<Kind> {
        if flags & SHF_ALLOC == 0 {
            return None;
        }
        if sh_type == SHT_NOBITS {
            return Some(Kind::Bss);
        }
        if flags & SHF_EXECINSTR != 0 {
            return Some(Kind::Text);
        }
        if flags & SHF_WRITE != 0 {
            return Some(Kind::Data);
        }
        let _ = name;
        Some(Kind::Rodata)
    }

    fn base(self) -> u64 {
        match self {
            Kind::Text => TEXT_ADDR,
            Kind::Rodata => RODATA_ADDR,
            Kind::Data | Kind::Bss => DATA_ADDR,
        }
    }
}

/// One input section that made it into the output.
struct Piece {
    object: usize,
    section: usize,
    kind: Kind,
    /// Offset inside the output section of this kind.
    offset: u64,
}

fn run() -> Result<(), String> {
    let mut out = PathBuf::from("a.out");
    let mut entry_name = "_start".to_string();
    let mut inputs: Vec<PathBuf> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-o" => out = PathBuf::from(args.next().ok_or("-o needs a file name")?),
            "-e" => entry_name = args.next().ok_or("-e needs a symbol name")?,
            _ if a.starts_with("--entry=") => entry_name = a["--entry=".len()..].to_string(),
            "-L" | "-l" => {
                let _ = args.next();
                return Err("this linker does not do archives".to_string());
            }
            _ if a.starts_with('-') => return Err(format!("unknown option {a}")),
            _ => inputs.push(PathBuf::from(a)),
        }
    }
    if inputs.is_empty() {
        return Err("no input files".to_string());
    }

    // --- read every input -----------------------------------------------------------
    let mut objects = Vec::new();
    for path in &inputs {
        let bytes =
            std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let elf = Elf::parse(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        if elf.e_type != ET_REL {
            return Err(format!("{}: not a relocatable object", path.display()));
        }
        if elf.e_machine != EM_X86_64 {
            return Err(format!("{}: not an x86-64 object", path.display()));
        }
        objects.push(elf);
    }

    // --- lay the sections out --------------------------------------------------------
    let mut pieces: Vec<Piece> = Vec::new();
    let mut cursor: BTreeMap<Kind, u64> = BTreeMap::new();
    for kind in [Kind::Text, Kind::Rodata, Kind::Data, Kind::Bss] {
        for (oi, elf) in objects.iter().enumerate() {
            for s in &elf.sections {
                if Kind::of(&s.name, s.flags, s.sh_type) != Some(kind) || s.size == 0 {
                    continue;
                }
                let align = s.align.max(1);
                let at = cursor.entry(kind).or_insert(0);
                *at = at.next_multiple_of(align);
                pieces.push(Piece {
                    object: oi,
                    section: s.index,
                    kind,
                    offset: *at,
                });
                *at += s.size;
            }
        }
    }
    let size_of = |k: Kind| cursor.get(&k).copied().unwrap_or(0);
    // `.bss` follows `.data` in memory but costs no file space.
    let bss_base = DATA_ADDR + size_of(Kind::Data).next_multiple_of(16);
    let piece_addr = |p: &Piece| match p.kind {
        Kind::Bss => bss_base + p.offset,
        k => k.base() + p.offset,
    };

    // --- resolve symbols -------------------------------------------------------------
    let mut globals: BTreeMap<String, u64> = BTreeMap::new();
    let mut locals: Vec<BTreeMap<usize, u64>> = vec![BTreeMap::new(); objects.len()];
    for (oi, elf) in objects.iter().enumerate() {
        for sym in &elf.symbols {
            if sym.is_undefined() || sym.name.is_empty() && sym.stype != STT_SECTION {
                continue;
            }
            let address = if sym.is_absolute() {
                sym.value
            } else {
                let Some(p) = pieces
                    .iter()
                    .find(|p| p.object == oi && p.section == sym.shndx as usize)
                else {
                    continue;
                };
                piece_addr(p) + sym.value
            };
            locals[oi].insert(sym.index, address);
            if sym.bind == STB_GLOBAL && !sym.name.is_empty() {
                if globals.contains_key(&sym.name) {
                    return Err(format!("multiple definition of `{}'", sym.name));
                }
                globals.insert(sym.name.clone(), address);
            } else if sym.bind == STB_WEAK && !sym.name.is_empty() {
                globals.entry(sym.name.clone()).or_insert(address);
            }
        }
    }

    // --- build the output sections ----------------------------------------------------
    let mut text = vec![0u8; size_of(Kind::Text) as usize];
    let mut rodata = vec![0u8; size_of(Kind::Rodata) as usize];
    let mut data = vec![0u8; size_of(Kind::Data) as usize];
    for p in &pieces {
        if p.kind == Kind::Bss {
            continue;
        }
        let elf = &objects[p.object];
        let s = &elf.sections[p.section];
        let bytes = elf
            .slice(s.offset, s.size)
            .map_err(|e| format!("{}: {e}", inputs[p.object].display()))?;
        let dst = match p.kind {
            Kind::Text => &mut text,
            Kind::Rodata => &mut rodata,
            _ => &mut data,
        };
        let at = p.offset as usize;
        dst[at..at + bytes.len()].copy_from_slice(bytes);
    }

    // --- apply relocations, forgetting the addend -------------------------------------
    for p in &pieces {
        if p.kind == Kind::Bss {
            continue;
        }
        let elf = &objects[p.object];
        for r in elf
            .relocations
            .iter()
            .filter(|r| r.target_section == p.section)
        {
            let sym = elf.symbols.get(r.sym as usize).ok_or_else(|| {
                format!("relocation against symbol {} which does not exist", r.sym)
            })?;
            let s_value = if sym.is_undefined() {
                *globals
                    .get(&sym.name)
                    .ok_or_else(|| format!("undefined reference to `{}'", sym.name))?
            } else if let Some(v) = locals[p.object].get(&sym.index) {
                *v
            } else if let Some(v) = globals.get(&sym.name) {
                *v
            } else {
                return Err(format!("undefined reference to `{}'", sym.name));
            };
            let site = piece_addr(p) + r.offset;
            let field = (p.offset + r.offset) as usize;
            let dst = match p.kind {
                Kind::Text => &mut text,
                Kind::Rodata => &mut rodata,
                _ => &mut data,
            };
            // THE BUG: the ABI says S + A - P (and S + A). This drops A.
            match r.kind {
                R_X86_64_PC32
                | R_X86_64_PLT32
                | R_X86_64_GOTPCREL
                | R_X86_64_GOTPCRELX
                | R_X86_64_REX_GOTPCRELX => {
                    let v = (s_value as i64 - site as i64) as i32;
                    dst[field..field + 4].copy_from_slice(&v.to_le_bytes());
                }
                R_X86_64_64 => {
                    dst[field..field + 8].copy_from_slice(&s_value.to_le_bytes());
                }
                R_X86_64_32 | R_X86_64_32S => {
                    dst[field..field + 4].copy_from_slice(&(s_value as u32).to_le_bytes());
                }
                other => return Err(format!("unsupported relocation type {other}")),
            }
        }
    }

    let entry = *globals
        .get(&entry_name)
        .ok_or_else(|| format!("cannot find entry symbol {entry_name}"))?;

    // --- write it out -----------------------------------------------------------------
    let bss_size = size_of(Kind::Bss);
    let file = assemble(&text, &rodata, &data, bss_size, bss_base, entry, &globals)?;
    std::fs::write(&out, &file).map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("cannot chmod {}: {e}", out.display()))?;
    Ok(())
}

/// Put the ELF header, three program headers, the section contents and a symbol table into
/// one buffer.
fn assemble(
    text: &[u8],
    rodata: &[u8],
    data: &[u8],
    bss_size: u64,
    bss_base: u64,
    entry: u64,
    globals: &BTreeMap<String, u64>,
) -> Result<Vec<u8>, String> {
    let mut phdrs: Vec<[u64; 7]> = Vec::new(); // type|flags, offset, vaddr, filesz, memsz, align
    let mut push = |flags: u32, offset: u64, vaddr: u64, filesz: u64, memsz: u64| {
        phdrs.push([
            PT_LOAD as u64,
            flags as u64,
            offset,
            vaddr,
            filesz,
            memsz,
            PAGE_SIZE,
        ]);
    };
    if !text.is_empty() {
        push(
            PF_R | PF_X,
            TEXT_OFF,
            TEXT_ADDR,
            text.len() as u64,
            text.len() as u64,
        );
    }
    if !rodata.is_empty() {
        push(
            PF_R,
            RODATA_OFF,
            RODATA_ADDR,
            rodata.len() as u64,
            rodata.len() as u64,
        );
    }
    if !data.is_empty() || bss_size > 0 {
        let memsz = (bss_base - DATA_ADDR) + bss_size;
        push(
            PF_R | PF_W,
            DATA_OFF,
            DATA_ADDR,
            data.len() as u64,
            memsz.max(data.len() as u64),
        );
    }

    let mut file = vec![0u8; DATA_OFF as usize + data.len().max(1)];
    file[TEXT_OFF as usize..TEXT_OFF as usize + text.len()].copy_from_slice(text);
    file[RODATA_OFF as usize..RODATA_OFF as usize + rodata.len()].copy_from_slice(rodata);
    file[DATA_OFF as usize..DATA_OFF as usize + data.len()].copy_from_slice(data);

    // A minimal .symtab/.strtab so a debugger — and linktest — can find the symbols.
    let mut strtab = vec![0u8];
    let mut symtab = vec![0u8; SYM_SIZE as usize];
    for (name, addr) in globals {
        let name_off = strtab.len() as u32;
        strtab.extend_from_slice(name.as_bytes());
        strtab.push(0);
        symtab.extend_from_slice(&name_off.to_le_bytes());
        symtab.push(st_info(STB_GLOBAL, STT_NOTYPE));
        symtab.push(STV_DEFAULT);
        symtab.extend_from_slice(&1u16.to_le_bytes()); // pretend everything is in .text
        symtab.extend_from_slice(&addr.to_le_bytes());
        symtab.extend_from_slice(&0u64.to_le_bytes());
    }
    let shstr = b"\0.text\0.rodata\0.data\0.bss\0.symtab\0.strtab\0.shstrtab\0";

    let symtab_off = file.len().next_multiple_of(8) as u64;
    file.resize(symtab_off as usize, 0);
    file.extend_from_slice(&symtab);
    let strtab_off = file.len() as u64;
    file.extend_from_slice(&strtab);
    let shstr_off = file.len() as u64;
    file.extend_from_slice(shstr);
    let shoff = file.len().next_multiple_of(8) as u64;
    file.resize(shoff as usize, 0);

    let shdr = |name: u32,
                sh_type: u32,
                flags: u64,
                addr: u64,
                offset: u64,
                size: u64,
                link: u32,
                info: u32,
                align: u64,
                entsize: u64| {
        let mut b = Vec::with_capacity(SHDR_SIZE as usize);
        b.extend_from_slice(&name.to_le_bytes());
        b.extend_from_slice(&sh_type.to_le_bytes());
        b.extend_from_slice(&flags.to_le_bytes());
        b.extend_from_slice(&addr.to_le_bytes());
        b.extend_from_slice(&offset.to_le_bytes());
        b.extend_from_slice(&size.to_le_bytes());
        b.extend_from_slice(&link.to_le_bytes());
        b.extend_from_slice(&info.to_le_bytes());
        b.extend_from_slice(&align.to_le_bytes());
        b.extend_from_slice(&entsize.to_le_bytes());
        b
    };
    let mut table = Vec::new();
    table.extend_from_slice(&shdr(0, SHT_NULL, 0, 0, 0, 0, 0, 0, 0, 0));
    table.extend_from_slice(&shdr(
        1,
        SHT_PROGBITS,
        SHF_ALLOC | SHF_EXECINSTR,
        TEXT_ADDR,
        TEXT_OFF,
        text.len() as u64,
        0,
        0,
        16,
        0,
    ));
    table.extend_from_slice(&shdr(
        7,
        SHT_PROGBITS,
        SHF_ALLOC,
        RODATA_ADDR,
        RODATA_OFF,
        rodata.len() as u64,
        0,
        0,
        1,
        0,
    ));
    table.extend_from_slice(&shdr(
        15,
        SHT_PROGBITS,
        SHF_ALLOC | SHF_WRITE,
        DATA_ADDR,
        DATA_OFF,
        data.len() as u64,
        0,
        0,
        1,
        0,
    ));
    table.extend_from_slice(&shdr(
        21,
        SHT_NOBITS,
        SHF_ALLOC | SHF_WRITE,
        bss_base,
        DATA_OFF + data.len() as u64,
        bss_size,
        0,
        0,
        16,
        0,
    ));
    table.extend_from_slice(&shdr(
        26,
        SHT_SYMTAB,
        0,
        0,
        symtab_off,
        symtab.len() as u64,
        6,
        1,
        8,
        SYM_SIZE,
    ));
    table.extend_from_slice(&shdr(
        34,
        SHT_STRTAB,
        0,
        0,
        strtab_off,
        strtab.len() as u64,
        0,
        0,
        1,
        0,
    ));
    table.extend_from_slice(&shdr(
        42,
        SHT_STRTAB,
        0,
        0,
        shstr_off,
        shstr.len() as u64,
        0,
        0,
        1,
        0,
    ));
    let shnum = (table.len() / SHDR_SIZE as usize) as u16;
    file.extend_from_slice(&table);

    let mut hdr = Vec::with_capacity(EHDR_SIZE as usize);
    hdr.extend_from_slice(&ELF_MAGIC);
    hdr.push(ELFCLASS64);
    hdr.push(ELFDATA2LSB);
    hdr.push(EV_CURRENT);
    hdr.push(ELFOSABI_SYSV);
    hdr.extend_from_slice(&[0u8; 8]);
    hdr.extend_from_slice(&ET_EXEC.to_le_bytes());
    hdr.extend_from_slice(&EM_X86_64.to_le_bytes());
    hdr.extend_from_slice(&(EV_CURRENT as u32).to_le_bytes());
    hdr.extend_from_slice(&entry.to_le_bytes());
    hdr.extend_from_slice(&(EHDR_SIZE as u64).to_le_bytes());
    hdr.extend_from_slice(&shoff.to_le_bytes());
    hdr.extend_from_slice(&0u32.to_le_bytes());
    hdr.extend_from_slice(&EHDR_SIZE.to_le_bytes());
    hdr.extend_from_slice(&PHDR_SIZE.to_le_bytes());
    hdr.extend_from_slice(&(phdrs.len() as u16).to_le_bytes());
    hdr.extend_from_slice(&SHDR_SIZE.to_le_bytes());
    hdr.extend_from_slice(&shnum.to_le_bytes());
    hdr.extend_from_slice(&(shnum - 1).to_le_bytes());
    if hdr.len() != EHDR_SIZE as usize {
        return Err("internal: bad header size".to_string());
    }
    file[..EHDR_SIZE as usize].copy_from_slice(&hdr);

    let mut at = EHDR_SIZE as usize;
    for p in &phdrs {
        file[at..at + 4].copy_from_slice(&(p[0] as u32).to_le_bytes());
        file[at + 4..at + 8].copy_from_slice(&(p[1] as u32).to_le_bytes());
        file[at + 8..at + 16].copy_from_slice(&p[2].to_le_bytes());
        file[at + 16..at + 24].copy_from_slice(&p[3].to_le_bytes());
        file[at + 24..at + 32].copy_from_slice(&p[3].to_le_bytes()); // p_paddr = p_vaddr
        file[at + 32..at + 40].copy_from_slice(&p[4].to_le_bytes());
        file[at + 40..at + 48].copy_from_slice(&p[5].to_le_bytes());
        file[at + 48..at + 56].copy_from_slice(&p[6].to_le_bytes());
        at += PHDR_SIZE as usize;
    }
    Ok(file)
}
