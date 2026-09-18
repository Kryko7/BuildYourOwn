//! The ELF64 relocatable-object writer.
//!
//! Every input the suite hands the linker under test is built here, byte by byte, so a test
//! can construct exactly the case it wants — an odd alignment, a section header table that
//! lies about its own offset, a relocation whose addend points into the middle of a symbol —
//! without needing an assembler on the machine.
//!
//! ```ignore
//! let obj = ObjectBuilder::new()
//!     .section(SectionSpec::text(".text", code).align(16))
//!     .symbol(SymbolSpec::global("_start", ".text", 0).func())
//!     .build()?;
//! ```
//!
//! The layout it produces is deliberately plain: the file header, then the section contents
//! in declaration order, then `.rela*`, `.symtab`, `.strtab`, `.shstrtab`, then the section
//! header table. Nothing about that layout is required by the ABI, which is exactly why the
//! reader ([`super::read`]) never assumes it.

use super::*;

/// What a symbol's `st_shndx` says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymSection {
    /// `SHN_UNDEF`: a reference the linker has to resolve elsewhere.
    Undefined,
    /// `SHN_ABS`: the value is absolute and must not be relocated.
    Absolute,
    /// `SHN_COMMON`: a tentative definition; `value` is the alignment, `size` the size.
    Common,
    /// Defined inside the named section of this object.
    Section(String),
    /// A raw section index, for fixtures that deliberately lie.
    Raw(u16),
}

/// What a relocation points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelTarget {
    /// A named symbol of this object (defined or undefined).
    Symbol(String),
    /// The section symbol of a named section — `S` is then the section's address.
    Section(String),
    /// A raw symbol table index, for fixtures that deliberately lie.
    Raw(u32),
}

/// One `Elf64_Rela` waiting for a symbol index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reloc {
    /// Offset of the field to patch, inside the section the relocation belongs to.
    pub offset: u64,
    /// The symbol (or section symbol) the relocation is against.
    pub target: RelTarget,
    /// One of the `R_X86_64_*` constants.
    pub kind: u32,
    /// The explicit addend `A`.
    pub addend: i64,
}

impl Reloc {
    /// A relocation of `kind` against the named symbol.
    pub fn sym(offset: u64, name: &str, kind: u32, addend: i64) -> Reloc {
        Reloc {
            offset,
            target: RelTarget::Symbol(name.to_string()),
            kind,
            addend,
        }
    }

    /// A relocation of `kind` against a section symbol.
    pub fn section(offset: u64, section: &str, kind: u32, addend: i64) -> Reloc {
        Reloc {
            offset,
            target: RelTarget::Section(section.to_string()),
            kind,
            addend,
        }
    }
}

/// One section of the object being written.
#[derive(Debug, Clone)]
pub struct SectionSpec {
    /// Section name, `.text` and friends.
    pub name: String,
    /// `sh_type`.
    pub sh_type: u32,
    /// `sh_flags`.
    pub flags: u64,
    /// `sh_addralign`; 0 and 1 both mean "no constraint".
    pub align: u64,
    /// The bytes (empty for `SHT_NOBITS`).
    pub data: Vec<u8>,
    /// `sh_size` for `SHT_NOBITS` sections, which have no bytes in the file.
    pub nobits_size: u64,
    /// `sh_entsize`.
    pub entsize: u64,
    /// `sh_addr`; always 0 in a well-formed relocatable object.
    pub addr: u64,
    /// The relocations that apply to this section; written into a `.rela<name>` section.
    pub relocs: Vec<Reloc>,
    /// Force `sh_offset` to a value the layout would not have chosen (malformed fixtures).
    pub offset_override: Option<u64>,
    /// Force `sh_size` (malformed fixtures).
    pub size_override: Option<u64>,
    /// Force `sh_link` (malformed fixtures).
    pub link_override: Option<u32>,
    /// Force `sh_info` (malformed fixtures).
    pub info_override: Option<u32>,
}

impl SectionSpec {
    /// A section with explicit type and flags.
    pub fn new(name: &str, sh_type: u32, flags: u64, data: Vec<u8>) -> SectionSpec {
        SectionSpec {
            name: name.to_string(),
            sh_type,
            flags,
            align: 1,
            data,
            nobits_size: 0,
            entsize: 0,
            addr: 0,
            relocs: Vec::new(),
            offset_override: None,
            size_override: None,
            link_override: None,
            info_override: None,
        }
    }

    /// `SHF_ALLOC | SHF_EXECINSTR` progbits: code.
    pub fn text(name: &str, data: Vec<u8>) -> SectionSpec {
        SectionSpec::new(name, SHT_PROGBITS, SHF_ALLOC | SHF_EXECINSTR, data).align(16)
    }

    /// `SHF_ALLOC` progbits: read-only data.
    pub fn rodata(name: &str, data: Vec<u8>) -> SectionSpec {
        SectionSpec::new(name, SHT_PROGBITS, SHF_ALLOC, data)
    }

    /// `SHF_ALLOC | SHF_WRITE` progbits: initialized writable data.
    pub fn data(name: &str, data: Vec<u8>) -> SectionSpec {
        SectionSpec::new(name, SHT_PROGBITS, SHF_ALLOC | SHF_WRITE, data)
    }

    /// `SHF_ALLOC | SHF_WRITE` nobits: zero-initialized data that costs no file space.
    pub fn bss(name: &str, size: u64) -> SectionSpec {
        let mut s = SectionSpec::new(name, SHT_NOBITS, SHF_ALLOC | SHF_WRITE, Vec::new());
        s.nobits_size = size;
        s
    }

    /// Set `sh_addralign`.
    pub fn align(mut self, align: u64) -> SectionSpec {
        self.align = align;
        self
    }

    /// Set `sh_entsize`.
    pub fn entsize(mut self, entsize: u64) -> SectionSpec {
        self.entsize = entsize;
        self
    }

    /// Attach one relocation.
    pub fn reloc(mut self, r: Reloc) -> SectionSpec {
        self.relocs.push(r);
        self
    }

    /// Attach several relocations.
    pub fn relocs(mut self, rs: impl IntoIterator<Item = Reloc>) -> SectionSpec {
        self.relocs.extend(rs);
        self
    }

    /// Lie about `sh_offset`.
    pub fn offset_override(mut self, v: u64) -> SectionSpec {
        self.offset_override = Some(v);
        self
    }

    /// Lie about `sh_size`.
    pub fn size_override(mut self, v: u64) -> SectionSpec {
        self.size_override = Some(v);
        self
    }

    /// Lie about `sh_link`.
    pub fn link_override(mut self, v: u32) -> SectionSpec {
        self.link_override = Some(v);
        self
    }

    /// The size this section reports.
    fn size(&self) -> u64 {
        self.size_override.unwrap_or(if self.sh_type == SHT_NOBITS {
            self.nobits_size
        } else {
            self.data.len() as u64
        })
    }
}

/// One symbol table entry of the object being written.
#[derive(Debug, Clone)]
pub struct SymbolSpec {
    /// Symbol name; the empty name is allowed (section symbols use it).
    pub name: String,
    /// `STB_LOCAL`, `STB_GLOBAL` or `STB_WEAK`.
    pub bind: u8,
    /// `STT_NOTYPE`, `STT_OBJECT`, `STT_FUNC`, …
    pub stype: u8,
    /// `st_other`: one of the `STV_*` visibilities.
    pub visibility: u8,
    /// Where the symbol lives.
    pub section: SymSection,
    /// `st_value`: the offset inside the section (or the alignment, for a common symbol).
    pub value: u64,
    /// `st_size`.
    pub size: u64,
}

impl SymbolSpec {
    /// A global symbol defined at `value` inside `section`.
    pub fn global(name: &str, section: &str, value: u64) -> SymbolSpec {
        SymbolSpec {
            name: name.to_string(),
            bind: STB_GLOBAL,
            stype: STT_NOTYPE,
            visibility: STV_DEFAULT,
            section: SymSection::Section(section.to_string()),
            value,
            size: 0,
        }
    }

    /// A local symbol defined at `value` inside `section`.
    pub fn local(name: &str, section: &str, value: u64) -> SymbolSpec {
        SymbolSpec {
            bind: STB_LOCAL,
            ..SymbolSpec::global(name, section, value)
        }
    }

    /// A weak symbol defined at `value` inside `section`.
    pub fn weak(name: &str, section: &str, value: u64) -> SymbolSpec {
        SymbolSpec {
            bind: STB_WEAK,
            ..SymbolSpec::global(name, section, value)
        }
    }

    /// An undefined global: a reference the linker must resolve from another input.
    pub fn undefined(name: &str) -> SymbolSpec {
        SymbolSpec {
            name: name.to_string(),
            bind: STB_GLOBAL,
            stype: STT_NOTYPE,
            visibility: STV_DEFAULT,
            section: SymSection::Undefined,
            value: 0,
            size: 0,
        }
    }

    /// An undefined *weak* symbol: unresolved, it is simply 0.
    pub fn weak_undefined(name: &str) -> SymbolSpec {
        SymbolSpec {
            bind: STB_WEAK,
            ..SymbolSpec::undefined(name)
        }
    }

    /// An absolute symbol (`SHN_ABS`): the value is final, never relocated.
    pub fn absolute(name: &str, value: u64) -> SymbolSpec {
        SymbolSpec {
            name: name.to_string(),
            bind: STB_GLOBAL,
            stype: STT_NOTYPE,
            visibility: STV_DEFAULT,
            section: SymSection::Absolute,
            value,
            size: 0,
        }
    }

    /// A common symbol (`SHN_COMMON`): `st_size` bytes with `st_value` alignment, allocated
    /// once by the linker with the largest size and the largest alignment of all the
    /// tentative definitions it saw.
    pub fn common(name: &str, size: u64, align: u64) -> SymbolSpec {
        SymbolSpec {
            name: name.to_string(),
            bind: STB_GLOBAL,
            stype: STT_OBJECT,
            visibility: STV_DEFAULT,
            section: SymSection::Common,
            value: align,
            size,
        }
    }

    /// Mark the symbol `STT_FUNC`.
    pub fn func(mut self) -> SymbolSpec {
        self.stype = STT_FUNC;
        self
    }

    /// Mark the symbol `STT_OBJECT`.
    pub fn object(mut self, size: u64) -> SymbolSpec {
        self.stype = STT_OBJECT;
        self.size = size;
        self
    }

    /// Set `st_size`.
    pub fn size(mut self, size: u64) -> SymbolSpec {
        self.size = size;
        self
    }

    /// Set `st_other` (visibility).
    pub fn visibility(mut self, v: u8) -> SymbolSpec {
        self.visibility = v;
        self
    }

    /// Make an otherwise global symbol weak.
    pub fn weaken(mut self) -> SymbolSpec {
        self.bind = STB_WEAK;
        self
    }
}

/// Fields of the ELF header a fixture can deliberately get wrong.
#[derive(Debug, Clone, Default)]
pub struct HeaderOverrides {
    /// Replace the four magic bytes.
    pub magic: Option<[u8; 4]>,
    /// Replace `e_ident[EI_CLASS]`.
    pub class: Option<u8>,
    /// Replace `e_ident[EI_DATA]`.
    pub endianness: Option<u8>,
    /// Replace `e_ident[EI_VERSION]`.
    pub ident_version: Option<u8>,
    /// Replace `e_ident[EI_OSABI]`.
    pub osabi: Option<u8>,
    /// Replace `e_type`.
    pub e_type: Option<u16>,
    /// Replace `e_machine`.
    pub e_machine: Option<u16>,
    /// Replace `e_version`.
    pub e_version: Option<u32>,
    /// Replace `e_ehsize`.
    pub e_ehsize: Option<u16>,
    /// Replace `e_shoff`.
    pub e_shoff: Option<u64>,
    /// Replace `e_shnum`.
    pub e_shnum: Option<u16>,
    /// Replace `e_shentsize`.
    pub e_shentsize: Option<u16>,
    /// Replace `e_shstrndx`.
    pub e_shstrndx: Option<u16>,
    /// Replace `e_entry` (meaningless in a relocatable object; a fixture sets it anyway).
    pub e_entry: Option<u64>,
}

/// Builds one relocatable object file.
#[derive(Debug, Clone, Default)]
pub struct ObjectBuilder {
    sections: Vec<SectionSpec>,
    symbols: Vec<SymbolSpec>,
    header: HeaderOverrides,
    truncate_to: Option<usize>,
    rela_before_target: bool,
    symtab_first: bool,
    omit_section_symbols: bool,
}

impl ObjectBuilder {
    /// An empty object: just a header and a section header table.
    pub fn new() -> ObjectBuilder {
        ObjectBuilder::default()
    }

    /// Add a section. Order is preserved, and it is the order the section header table has.
    pub fn section(mut self, s: SectionSpec) -> ObjectBuilder {
        self.sections.push(s);
        self
    }

    /// Add a symbol. Locals and globals may be declared in any order; the writer sorts them
    /// as the ABI requires and sets `sh_info` accordingly.
    pub fn symbol(mut self, s: SymbolSpec) -> ObjectBuilder {
        self.symbols.push(s);
        self
    }

    /// Add several symbols.
    pub fn symbols(mut self, s: impl IntoIterator<Item = SymbolSpec>) -> ObjectBuilder {
        self.symbols.extend(s);
        self
    }

    /// Deliberately wrong header fields.
    pub fn header(mut self, h: HeaderOverrides) -> ObjectBuilder {
        self.header = h;
        self
    }

    /// Cut the finished file short at `bytes` (truncation fixtures).
    pub fn truncate_to(mut self, bytes: usize) -> ObjectBuilder {
        self.truncate_to = Some(bytes);
        self
    }

    /// Emit each `.rela<name>` *before* the section it applies to, so a linker that assumes
    /// "relocations come later" breaks.
    pub fn rela_before_target(mut self) -> ObjectBuilder {
        self.rela_before_target = true;
        self
    }

    /// Emit `.symtab`/`.strtab` before the content sections, so a linker that assumes the
    /// symbol table is last breaks.
    pub fn symtab_first(mut self) -> ObjectBuilder {
        self.symtab_first = true;
        self
    }

    /// Do not emit a `STT_SECTION` symbol per section. Relocations against a section then
    /// have nothing to point at, so this is only for "what does a minimal object look like"
    /// fixtures.
    pub fn without_section_symbols(mut self) -> ObjectBuilder {
        self.omit_section_symbols = true;
        self
    }

    /// The finished object file.
    pub fn build(&self) -> Result<Vec<u8>, String> {
        Layout::new(self)?.emit()
    }
}

/// A string table being built: bytes plus the offset each string landed at.
#[derive(Default)]
struct StrTab {
    bytes: Vec<u8>,
}

impl StrTab {
    fn new() -> StrTab {
        StrTab { bytes: vec![0] }
    }

    fn add(&mut self, s: &str) -> u32 {
        if s.is_empty() {
            return 0;
        }
        let at = self.bytes.len() as u32;
        self.bytes.extend_from_slice(s.as_bytes());
        self.bytes.push(0);
        at
    }
}

/// One entry of the section header table being emitted.
struct Shdr {
    name: String,
    sh_type: u32,
    flags: u64,
    addr: u64,
    size: u64,
    link: u32,
    info: u32,
    align: u64,
    entsize: u64,
    data: Vec<u8>,
    offset_override: Option<u64>,
    occupies_file_space: bool,
}

/// Turns an [`ObjectBuilder`] into the concrete tables and then into bytes.
struct Layout<'a> {
    b: &'a ObjectBuilder,
    /// Final section header entries, index 0 being the null entry.
    shdrs: Vec<Shdr>,
    /// Index in `shdrs` of each user section, by name.
    index_of: Vec<(String, u32)>,
    /// `sh_name` of each entry of `shdrs`, into `.shstrtab`.
    name_offsets: Vec<u32>,
    /// Index of `.shstrtab`, which `e_shstrndx` names.
    shstrtab_index: u16,
}

impl<'a> Layout<'a> {
    fn new(b: &'a ObjectBuilder) -> Result<Layout<'a>, String> {
        let mut l = Layout {
            b,
            shdrs: Vec::new(),
            index_of: Vec::new(),
            name_offsets: Vec::new(),
            shstrtab_index: 0,
        };
        l.plan()?;
        Ok(l)
    }

    fn section_index(&self, name: &str) -> Result<u32, String> {
        self.index_of
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, i)| *i)
            .ok_or_else(|| format!("the object has no section named '{name}'"))
    }

    /// Work out every table: symbol order, symbol indices, relocation contents, names.
    fn plan(&mut self) -> Result<(), String> {
        let b = self.b;
        if b.rela_before_target && b.symtab_first {
            return Err(
                "rela_before_target and symtab_first cannot be combined: both rewrite the \
                 section header indices"
                    .to_string(),
            );
        }

        // --- section header indices -------------------------------------------------
        // 0 is the null entry. User sections come next (the `.rela` ones are interleaved
        // or appended depending on the builder), then .symtab, .strtab, .shstrtab.
        let n_user = b.sections.len() as u32;
        let n_rela = b.sections.iter().filter(|s| !s.relocs.is_empty()).count() as u32;
        let content_base: u32 = if b.symtab_first { 4 } else { 1 };
        let mut index_of = Vec::new();
        {
            let mut next = content_base;
            for s in &b.sections {
                if b.rela_before_target && !s.relocs.is_empty() {
                    next += 1; // the .rela section sits in front of this one
                }
                index_of.push((s.name.clone(), next));
                next += 1;
            }
        }
        self.index_of = index_of;
        let symtab_index: u32 = if b.symtab_first {
            1
        } else {
            content_base + n_user + n_rela
        };
        let strtab_index = symtab_index + 1;
        let shstrtab_index = symtab_index + 2;
        self.shstrtab_index = shstrtab_index as u16;

        // --- symbols ----------------------------------------------------------------
        // ABI order: the null symbol, every local (section symbols first, as every
        // assembler emits them), then the globals and weaks. `sh_info` is the index of the
        // first non-local.
        let mut strtab = StrTab::new();
        let mut syms: Vec<(String, Vec<u8>)> = Vec::new(); // (name, 24 bytes)
        let mut sym_index: Vec<(String, u32)> = Vec::new();
        let mut section_sym_index: Vec<(String, u32)> = Vec::new();
        syms.push((String::new(), vec![0u8; SYM_SIZE as usize]));

        if !b.omit_section_symbols {
            for s in &b.sections {
                let idx = syms.len() as u32;
                let shndx = self.section_index(&s.name)?;
                syms.push((
                    s.name.clone(),
                    sym_bytes(0, st_info(STB_LOCAL, STT_SECTION), STV_DEFAULT, shndx as u16, 0, 0),
                ));
                section_sym_index.push((s.name.clone(), idx));
            }
        }

        let push_sym = |spec: &SymbolSpec,
                            syms: &mut Vec<(String, Vec<u8>)>,
                            sym_index: &mut Vec<(String, u32)>,
                            strtab: &mut StrTab,
                            this: &Layout|
         -> Result<(), String> {
            let shndx = match &spec.section {
                SymSection::Undefined => SHN_UNDEF,
                SymSection::Absolute => SHN_ABS,
                SymSection::Common => SHN_COMMON,
                SymSection::Raw(v) => *v,
                SymSection::Section(name) => this.section_index(name)? as u16,
            };
            let name_off = strtab.add(&spec.name);
            let idx = syms.len() as u32;
            syms.push((
                spec.name.clone(),
                sym_bytes(
                    name_off,
                    st_info(spec.bind, spec.stype),
                    spec.visibility,
                    shndx,
                    spec.value,
                    spec.size,
                ),
            ));
            if !spec.name.is_empty() {
                sym_index.push((spec.name.clone(), idx));
            }
            Ok(())
        };

        for spec in b.symbols.iter().filter(|s| s.bind == STB_LOCAL) {
            push_sym(spec, &mut syms, &mut sym_index, &mut strtab, self)?;
        }
        let first_global = syms.len() as u32;
        for spec in b.symbols.iter().filter(|s| s.bind != STB_LOCAL) {
            push_sym(spec, &mut syms, &mut sym_index, &mut strtab, self)?;
        }

        let symtab_bytes: Vec<u8> = syms.iter().flat_map(|(_, b)| b.clone()).collect();

        // --- relocations ------------------------------------------------------------
        let mut rela_for: Vec<(String, Vec<u8>)> = Vec::new();
        for s in &b.sections {
            if s.relocs.is_empty() {
                continue;
            }
            let mut bytes = Vec::with_capacity(s.relocs.len() * RELA_SIZE as usize);
            for r in &s.relocs {
                let sym = match &r.target {
                    RelTarget::Raw(i) => *i,
                    RelTarget::Symbol(name) => sym_index
                        .iter()
                        .find(|(n, _)| n == name)
                        .map(|(_, i)| *i)
                        .ok_or_else(|| {
                            format!(
                                "section '{}' has a relocation against '{name}', which the \
                                 object never declares as a symbol",
                                s.name
                            )
                        })?,
                    RelTarget::Section(name) => section_sym_index
                        .iter()
                        .find(|(n, _)| n == name)
                        .map(|(_, i)| *i)
                        .ok_or_else(|| {
                            format!(
                                "section '{}' has a relocation against the section symbol of \
                                 '{name}', which this object does not have",
                                s.name
                            )
                        })?,
                };
                bytes.extend_from_slice(&r.offset.to_le_bytes());
                bytes.extend_from_slice(&r_info(sym, r.kind).to_le_bytes());
                bytes.extend_from_slice(&r.addend.to_le_bytes());
            }
            rela_for.push((s.name.clone(), bytes));
        }

        // --- section header entries -------------------------------------------------
        let mut entries: Vec<(u32, Shdr)> = Vec::new(); // (index, header)
        entries.push((
            0,
            Shdr {
                name: String::new(),
                sh_type: SHT_NULL,
                flags: 0,
                addr: 0,
                size: 0,
                link: 0,
                info: 0,
                align: 0,
                entsize: 0,
                data: Vec::new(),
                offset_override: None,
                occupies_file_space: false,
            },
        ));

        for s in &b.sections {
            let idx = self.section_index(&s.name)?;
            if let Some((_, bytes)) = rela_for.iter().find(|(n, _)| n == &s.name) {
                let rela_index = if b.rela_before_target {
                    idx - 1
                } else {
                    // Appended after the content sections, in declaration order.
                    let position = rela_for
                        .iter()
                        .position(|(n, _)| n == &s.name)
                        .unwrap_or_default() as u32;
                    content_base + n_user + position
                };
                entries.push((
                    rela_index,
                    Shdr {
                        name: format!(".rela{}", s.name),
                        sh_type: SHT_RELA,
                        flags: SHF_INFO_LINK,
                        addr: 0,
                        size: bytes.len() as u64,
                        link: symtab_index,
                        info: idx,
                        align: 8,
                        entsize: RELA_SIZE,
                        data: bytes.clone(),
                        offset_override: None,
                        occupies_file_space: true,
                    },
                ));
            }
            entries.push((
                idx,
                Shdr {
                    name: s.name.clone(),
                    sh_type: s.sh_type,
                    flags: s.flags,
                    addr: s.addr,
                    size: s.size(),
                    link: s.link_override.unwrap_or(0),
                    info: s.info_override.unwrap_or(0),
                    align: s.align,
                    entsize: s.entsize,
                    data: s.data.clone(),
                    offset_override: s.offset_override,
                    occupies_file_space: s.sh_type != SHT_NOBITS,
                },
            ));
        }

        entries.push((
            symtab_index,
            Shdr {
                name: ".symtab".into(),
                sh_type: SHT_SYMTAB,
                flags: 0,
                addr: 0,
                size: symtab_bytes.len() as u64,
                link: strtab_index,
                info: first_global,
                align: 8,
                entsize: SYM_SIZE,
                data: symtab_bytes,
                offset_override: None,
                occupies_file_space: true,
            },
        ));
        entries.push((
            strtab_index,
            Shdr {
                name: ".strtab".into(),
                sh_type: SHT_STRTAB,
                flags: 0,
                addr: 0,
                size: strtab.bytes.len() as u64,
                link: 0,
                info: 0,
                align: 1,
                entsize: 0,
                data: strtab.bytes.clone(),
                offset_override: None,
                occupies_file_space: true,
            },
        ));
        entries.push((
            shstrtab_index,
            Shdr {
                name: ".shstrtab".into(),
                sh_type: SHT_STRTAB,
                flags: 0,
                addr: 0,
                size: 0, // filled in once the names are known
                link: 0,
                info: 0,
                align: 1,
                entsize: 0,
                data: Vec::new(),
                offset_override: None,
                occupies_file_space: true,
            },
        ));

        entries.sort_by_key(|(i, _)| *i);
        let expected: Vec<u32> = (0..entries.len() as u32).collect();
        let got: Vec<u32> = entries.iter().map(|(i, _)| *i).collect();
        if got != expected {
            return Err(format!(
                "internal: section indices came out as {got:?}, expected {expected:?}"
            ));
        }
        self.shdrs = entries.into_iter().map(|(_, s)| s).collect();

        // `.shstrtab` holds every section name, itself included.
        let mut shstr = StrTab::new();
        let name_offsets: Vec<u32> = self.shdrs.iter().map(|s| shstr.add(&s.name)).collect();
        if let Some(last) = self.shdrs.last_mut() {
            last.data = shstr.bytes.clone();
            last.size = shstr.bytes.len() as u64;
        }
        // Stash the offsets in `entsize`-free space: emit() reads them from here.
        self.name_offsets = name_offsets;
        Ok(())
    }
}

// Emission is a separate `impl` block purely to keep `plan()` — which decides every index —
// readable next to it.
impl Layout<'_> {
    /// Assemble the bytes.
    fn emit(&self) -> Result<Vec<u8>, String> {
        let b = self.b;
        let mut out: Vec<u8> = vec![0; EHDR_SIZE as usize];

        // Section contents, in section header order, each at its own alignment.
        let mut offsets: Vec<u64> = Vec::with_capacity(self.shdrs.len());
        for s in &self.shdrs {
            if s.sh_type == SHT_NULL {
                offsets.push(0);
                continue;
            }
            let align = s.align.max(1);
            while !(out.len() as u64).is_multiple_of(align) {
                out.push(0);
            }
            let at = out.len() as u64;
            offsets.push(s.offset_override.unwrap_or(at));
            if s.occupies_file_space {
                out.extend_from_slice(&s.data);
            }
        }

        // The section header table is 8-aligned, as every toolchain writes it.
        while !(out.len() as u64).is_multiple_of(8) {
            out.push(0);
        }
        let shoff = out.len() as u64;
        for (i, s) in self.shdrs.iter().enumerate() {
            let name_off = self.name_offsets.get(i).copied().unwrap_or(0);
            out.extend_from_slice(&name_off.to_le_bytes());
            out.extend_from_slice(&s.sh_type.to_le_bytes());
            out.extend_from_slice(&s.flags.to_le_bytes());
            out.extend_from_slice(&s.addr.to_le_bytes());
            out.extend_from_slice(&offsets[i].to_le_bytes());
            out.extend_from_slice(&s.size.to_le_bytes());
            out.extend_from_slice(&s.link.to_le_bytes());
            out.extend_from_slice(&s.info.to_le_bytes());
            out.extend_from_slice(&s.align.to_le_bytes());
            out.extend_from_slice(&s.entsize.to_le_bytes());
        }

        // The file header, last, because it names the section header table.
        let h = &b.header;
        let mut ident = [0u8; EI_NIDENT];
        ident[..4].copy_from_slice(&h.magic.unwrap_or(ELF_MAGIC));
        ident[4] = h.class.unwrap_or(ELFCLASS64);
        ident[5] = h.endianness.unwrap_or(ELFDATA2LSB);
        ident[6] = h.ident_version.unwrap_or(EV_CURRENT);
        ident[7] = h.osabi.unwrap_or(ELFOSABI_SYSV);
        let shstrndx = h.e_shstrndx.unwrap_or(self.shstrtab_index);

        let mut hdr: Vec<u8> = Vec::with_capacity(EHDR_SIZE as usize);
        hdr.extend_from_slice(&ident);
        hdr.extend_from_slice(&h.e_type.unwrap_or(ET_REL).to_le_bytes());
        hdr.extend_from_slice(&h.e_machine.unwrap_or(EM_X86_64).to_le_bytes());
        hdr.extend_from_slice(&h.e_version.unwrap_or(EV_CURRENT as u32).to_le_bytes());
        hdr.extend_from_slice(&h.e_entry.unwrap_or(0).to_le_bytes());
        hdr.extend_from_slice(&0u64.to_le_bytes()); // e_phoff: a relocatable object has none
        hdr.extend_from_slice(&h.e_shoff.unwrap_or(shoff).to_le_bytes());
        hdr.extend_from_slice(&0u32.to_le_bytes()); // e_flags
        hdr.extend_from_slice(&h.e_ehsize.unwrap_or(EHDR_SIZE).to_le_bytes());
        hdr.extend_from_slice(&0u16.to_le_bytes()); // e_phentsize
        hdr.extend_from_slice(&0u16.to_le_bytes()); // e_phnum
        hdr.extend_from_slice(&h.e_shentsize.unwrap_or(SHDR_SIZE).to_le_bytes());
        hdr.extend_from_slice(
            &h.e_shnum
                .unwrap_or(self.shdrs.len() as u16)
                .to_le_bytes(),
        );
        hdr.extend_from_slice(&shstrndx.to_le_bytes());
        if hdr.len() != EHDR_SIZE as usize {
            return Err(format!(
                "internal: the file header came out {} bytes, not {EHDR_SIZE}",
                hdr.len()
            ));
        }
        out[..EHDR_SIZE as usize].copy_from_slice(&hdr);

        if let Some(n) = b.truncate_to {
            out.truncate(n);
        }
        Ok(out)
    }
}

/// One 24-byte `Elf64_Sym`.
fn sym_bytes(name: u32, info: u8, other: u8, shndx: u16, value: u64, size: u64) -> Vec<u8> {
    let mut b = Vec::with_capacity(SYM_SIZE as usize);
    b.extend_from_slice(&name.to_le_bytes());
    b.push(info);
    b.push(other);
    b.extend_from_slice(&shndx.to_le_bytes());
    b.extend_from_slice(&value.to_le_bytes());
    b.extend_from_slice(&size.to_le_bytes());
    b
}
