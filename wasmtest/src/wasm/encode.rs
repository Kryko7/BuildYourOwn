//! LEB128, section framing and the module builder.

use std::fmt::Write as _;

// ---------------------------------------------------------------------------------------
// LEB128
// ---------------------------------------------------------------------------------------

/// Unsigned LEB128, the shortest encoding.
pub fn uleb(mut v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return out;
        }
        out.push(byte | 0x80);
    }
}

/// Unsigned LEB128 padded to exactly `len` bytes with redundant continuation bytes.
///
/// A decoder that stops at the first byte without the high bit set reads the same number;
/// one that hard-codes "one byte" does not. The spec allows up to `ceil(bits / 7)` bytes for
/// a value of that width, so a 5-byte encoding of `1` is legal for `u32` and a 6-byte one is
/// not — stage 02 tests both sides of that line.
pub fn uleb_padded(v: u64, len: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut v = v;
    for i in 0..len {
        let last = i + 1 == len;
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        out.push(if last { byte } else { byte | 0x80 });
    }
    out
}

/// Signed LEB128, the shortest encoding.
pub fn sleb(mut v: i64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        let sign_bit_set = byte & 0x40 != 0;
        if (v == 0 && !sign_bit_set) || (v == -1 && sign_bit_set) {
            out.push(byte);
            return out;
        }
        out.push(byte | 0x80);
    }
}

/// Signed LEB128 padded to exactly `len` bytes by repeating the sign.
pub fn sleb_padded(v: i64, len: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut v = v;
    for i in 0..len {
        let last = i + 1 == len;
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        out.push(if last { byte } else { byte | 0x80 });
    }
    out
}

/// A `vec(T)`: the element count as a uLEB128, then the elements.
pub fn vector(items: &[Vec<u8>]) -> Vec<u8> {
    let mut out = uleb(items.len() as u64);
    for i in items {
        out.extend_from_slice(i);
    }
    out
}

/// A `name`: its length as a uLEB128, then its UTF-8 bytes.
pub fn wasm_name(s: &str) -> Vec<u8> {
    let mut out = uleb(s.len() as u64);
    out.extend_from_slice(s.as_bytes());
    out
}

// ---------------------------------------------------------------------------------------
// Annotations
// ---------------------------------------------------------------------------------------

/// One annotated region of a module's bytes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Ann {
    /// Byte offset into the module.
    pub offset: usize,
    /// Length in bytes.
    pub length: usize,
    /// What this region is, as a dotted path: `header.magic`, `type[0].params`,
    /// `code[1].body`.
    pub field: String,
    /// The decoded value, written the way the report writes it.
    pub value: String,
}

/// A finished module: the exact bytes, and what every region of them means.
#[derive(Debug, Clone, Default)]
pub struct Module {
    /// The module bytes, exactly as they are written to disk.
    pub bytes: Vec<u8>,
    /// One entry per annotated region, in ascending offset order.
    pub anns: Vec<Ann>,
    /// A short human name, used as the temporary file's stem and in failure messages.
    pub label: String,
}

impl Module {
    /// Lower-case hex of the whole module, no separators.
    pub fn hex(&self) -> String {
        let mut s = String::with_capacity(self.bytes.len() * 2);
        for b in &self.bytes {
            let _ = write!(s, "{b:02x}");
        }
        s
    }

    /// The module's size in bytes.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// True when the module has no bytes at all.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// The annotated hex listing shown under a failure.
    ///
    /// One line per annotation: offset, the bytes, the field path and the decoded value.
    /// Regions the builder did not annotate are printed as an `unannotated` run, so the
    /// listing always adds up to the whole module.
    pub fn listing(&self, max_bytes: usize) -> String {
        hex_listing(&self.bytes, &self.anns, max_bytes)
    }

    /// A plain hex dump with the given byte ranges marked, for tests that point at a byte.
    pub fn dump(&self, marks: &[std::ops::Range<usize>], max_bytes: usize) -> String {
        super::hex::hexdump(&self.bytes, marks, max_bytes)
    }

    /// Replace the byte at `at`, keeping the annotations (stage 43 patches modules).
    pub fn with_byte(&self, at: usize, value: u8) -> Module {
        let mut m = self.clone();
        if let Some(b) = m.bytes.get_mut(at) {
            *b = value;
        }
        m
    }

    /// Keep only the first `keep` bytes (truncation tests).
    pub fn truncated(&self, keep: usize) -> Module {
        let mut m = self.clone();
        m.bytes.truncate(keep);
        m.anns.retain(|a| a.offset + a.length <= keep);
        m.label = format!("{}-truncated-{keep}", self.label);
        m
    }

    /// Give the module a different label (the temp file's stem).
    pub fn labelled(mut self, label: impl Into<String>) -> Module {
        self.label = label.into();
        self
    }

    /// The annotation covering `offset`, when there is one.
    pub fn ann_at(&self, offset: usize) -> Option<&Ann> {
        self.anns
            .iter()
            .find(|a| (a.offset..a.offset + a.length).contains(&offset))
    }
}

/// Render `bytes` as an annotated listing against `anns`.
pub fn hex_listing(bytes: &[u8], anns: &[Ann], max_bytes: usize) -> String {
    let mut sorted: Vec<&Ann> = anns.iter().collect();
    sorted.sort_by_key(|a| (a.offset, a.length));
    let mut out = String::new();
    let mut at = 0usize;
    let mut budget = max_bytes;
    let row = |out: &mut String, from: usize, len: usize, field: &str, value: &str| {
        let end = (from + len).min(bytes.len());
        let slice = &bytes[from.min(bytes.len())..end];
        let shown: String = slice
            .iter()
            .take(12)
            .map(|b| format!("{b:02x} "))
            .collect::<String>();
        let more = if slice.len() > 12 { "…" } else { "" };
        let _ = writeln!(
            out,
            "{from:04x}  {:<38} {field}{}{}",
            format!("{shown}{more}"),
            if value.is_empty() { "" } else { " = " },
            value
        );
    };
    for a in sorted {
        if a.offset > at {
            row(&mut out, at, a.offset - at, "(unannotated)", "");
        }
        if budget == 0 {
            let _ = writeln!(out, "      … {} more bytes", bytes.len().saturating_sub(at));
            return out;
        }
        row(&mut out, a.offset, a.length, &a.field, &a.value);
        budget = budget.saturating_sub(a.length.max(1));
        at = at.max(a.offset + a.length);
    }
    if at < bytes.len() {
        row(&mut out, at, bytes.len() - at, "(unannotated)", "");
    }
    out
}

// ---------------------------------------------------------------------------------------
// The low-level sink
// ---------------------------------------------------------------------------------------

/// A byte sink that records what each region it writes means.
///
/// This is the escape hatch the malformed-module stages use: it will happily write a section
/// size that does not match the section, two type sections, or a five-byte encoding of `0`.
#[derive(Debug, Default, Clone)]
pub struct Enc {
    buf: Vec<u8>,
    anns: Vec<Ann>,
}

impl Enc {
    /// An empty sink.
    pub fn new() -> Enc {
        Enc::default()
    }

    /// A sink that already holds the 8-byte module header.
    pub fn with_header() -> Enc {
        let mut e = Enc::new();
        e.put(b"\0asm", "header.magic", "\\0asm");
        e.put(&1u32.to_le_bytes(), "header.version", "1 (little-endian)");
        e
    }

    /// The current write position.
    pub fn at(&self) -> usize {
        self.buf.len()
    }

    /// The bytes written so far.
    pub fn bytes(&self) -> &[u8] {
        &self.buf
    }

    /// Append bytes without annotating them.
    pub fn raw(&mut self, bytes: &[u8]) -> &mut Enc {
        self.buf.extend_from_slice(bytes);
        self
    }

    /// Append bytes and annotate the region they occupy.
    pub fn put(
        &mut self,
        bytes: &[u8],
        field: impl Into<String>,
        value: impl Into<String>,
    ) -> &mut Enc {
        let at = self.buf.len();
        self.buf.extend_from_slice(bytes);
        self.anns.push(Ann {
            offset: at,
            length: bytes.len(),
            field: field.into(),
            value: value.into(),
        });
        self
    }

    /// Annotate a region that was already written (after a nested build, say).
    pub fn annotate(
        &mut self,
        offset: usize,
        length: usize,
        field: impl Into<String>,
        value: impl Into<String>,
    ) -> &mut Enc {
        self.anns.push(Ann {
            offset,
            length,
            field: field.into(),
            value: value.into(),
        });
        self
    }

    /// Append another sink's bytes, shifting its annotations into place.
    pub fn splice(&mut self, other: &Enc) -> &mut Enc {
        let base = self.buf.len();
        self.buf.extend_from_slice(&other.buf);
        for a in &other.anns {
            self.anns.push(Ann {
                offset: a.offset + base,
                length: a.length,
                field: a.field.clone(),
                value: a.value.clone(),
            });
        }
        self
    }

    /// A whole section: the id byte, the size as a uLEB128, then the body.
    pub fn section(&mut self, id: u8, body: &[u8]) -> &mut Enc {
        self.put(&[id], format!("section[{id}].id"), section_name(id));
        self.put(
            &uleb(body.len() as u64),
            format!("section[{id}].size"),
            format!("{} bytes", body.len()),
        );
        self.raw(body)
    }

    /// A section whose size field is written exactly as given — the "size that lies" path.
    pub fn section_with_size(&mut self, id: u8, size: &[u8], body: &[u8], why: &str) -> &mut Enc {
        self.put(&[id], format!("section[{id}].id"), section_name(id));
        self.put(size, format!("section[{id}].size"), why.to_string());
        self.raw(body)
    }

    /// Turn the sink into a module.
    pub fn finish(self, label: impl Into<String>) -> Module {
        Module {
            bytes: self.buf,
            anns: self.anns,
            label: label.into(),
        }
    }
}

/// The spec's name for a section id, used in every annotation and failure message.
pub fn section_name(id: u8) -> &'static str {
    match id {
        0 => "custom section",
        1 => "type section",
        2 => "import section",
        3 => "function section",
        4 => "table section",
        5 => "memory section",
        6 => "global section",
        7 => "export section",
        8 => "start section",
        9 => "element section",
        10 => "code section",
        11 => "data section",
        12 => "data count section",
        _ => "an id no section uses",
    }
}

/// Section ids, in the order a well-formed module writes them.
pub mod section {
    /// Custom section, legal anywhere.
    pub const CUSTOM: u8 = 0;
    /// Type section.
    pub const TYPE: u8 = 1;
    /// Import section.
    pub const IMPORT: u8 = 2;
    /// Function section.
    pub const FUNCTION: u8 = 3;
    /// Table section.
    pub const TABLE: u8 = 4;
    /// Memory section.
    pub const MEMORY: u8 = 5;
    /// Global section.
    pub const GLOBAL: u8 = 6;
    /// Export section.
    pub const EXPORT: u8 = 7;
    /// Start section.
    pub const START: u8 = 8;
    /// Element section.
    pub const ELEMENT: u8 = 9;
    /// Code section.
    pub const CODE: u8 = 10;
    /// Data section.
    pub const DATA: u8 = 11;
    /// Data count section (comes *before* code, despite its id).
    pub const DATA_COUNT: u8 = 12;
}

// ---------------------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------------------

/// A WebAssembly value type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValType {
    /// 32-bit integer.
    I32,
    /// 64-bit integer.
    I64,
    /// 32-bit float.
    F32,
    /// 64-bit float.
    F64,
    /// A reference to a function.
    FuncRef,
    /// A reference to a host value.
    ExternRef,
}

impl ValType {
    /// The single byte the binary format writes for this type.
    pub fn code(self) -> u8 {
        match self {
            ValType::I32 => 0x7f,
            ValType::I64 => 0x7e,
            ValType::F32 => 0x7d,
            ValType::F64 => 0x7c,
            ValType::FuncRef => 0x70,
            ValType::ExternRef => 0x6f,
        }
    }

    /// The spec's name for this type.
    pub fn name(self) -> &'static str {
        match self {
            ValType::I32 => "i32",
            ValType::I64 => "i64",
            ValType::F32 => "f32",
            ValType::F64 => "f64",
            ValType::FuncRef => "funcref",
            ValType::ExternRef => "externref",
        }
    }
}

fn types_text(ts: &[ValType]) -> String {
    ts.iter().map(|t| t.name()).collect::<Vec<_>>().join(" ")
}

/// A function type: what it takes and what it gives back.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FuncType {
    /// Parameter types, left to right.
    pub params: Vec<ValType>,
    /// Result types; more than one needs the multi-value proposal (stage 27).
    pub results: Vec<ValType>,
}

/// `ftype(&[I32, I32], &[I32])` — the readable way to write a [`FuncType`].
pub fn ftype(params: &[ValType], results: &[ValType]) -> FuncType {
    FuncType {
        params: params.to_vec(),
        results: results.to_vec(),
    }
}

impl FuncType {
    /// `(i32 i32) -> (i32)`, how every failure message writes a signature.
    pub fn text(&self) -> String {
        format!(
            "({}) -> ({})",
            types_text(&self.params),
            types_text(&self.results)
        )
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = vec![0x60];
        out.extend_from_slice(&uleb(self.params.len() as u64));
        out.extend(self.params.iter().map(|t| t.code()));
        out.extend_from_slice(&uleb(self.results.len() as u64));
        out.extend(self.results.iter().map(|t| t.code()));
        out
    }
}

/// The `{min, max}` pair a memory or a table carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Minimum size, in pages (memory) or elements (table).
    pub min: u32,
    /// Maximum size, when the module declares one.
    pub max: Option<u32>,
}

impl Limits {
    /// `min..` with no declared maximum.
    pub fn min(min: u32) -> Limits {
        Limits { min, max: None }
    }

    /// `min..=max`.
    pub fn range(min: u32, max: u32) -> Limits {
        Limits {
            min,
            max: Some(max),
        }
    }

    fn encode(&self) -> Vec<u8> {
        match self.max {
            None => {
                let mut out = vec![0x00];
                out.extend_from_slice(&uleb(self.min as u64));
                out
            }
            Some(m) => {
                let mut out = vec![0x01];
                out.extend_from_slice(&uleb(self.min as u64));
                out.extend_from_slice(&uleb(m as u64));
                out
            }
        }
    }

    fn text(&self) -> String {
        match self.max {
            None => format!("min {}, no max", self.min),
            Some(m) => format!("min {}, max {m}", self.min),
        }
    }
}

/// A table's type: what it holds and how big it is.
#[derive(Debug, Clone, Copy)]
pub struct TableType {
    /// `funcref` or `externref`.
    pub elem: ValType,
    /// How many elements.
    pub limits: Limits,
}

/// One global: its type, whether it can be written, and how it starts out.
#[derive(Debug, Clone)]
pub struct Global {
    /// The value type.
    pub ty: ValType,
    /// True when `global.set` may target it.
    pub mutable: bool,
    /// The initialiser, a constant expression **including** its terminating `end`.
    pub init: Vec<u8>,
    /// How the initialiser reads, for the annotation.
    pub init_text: String,
}

/// What an import brings in.
#[derive(Debug, Clone)]
pub enum ImportKind {
    /// A function of the given type index.
    Func(u32),
    /// A table.
    Table(TableType),
    /// A memory.
    Memory(Limits),
    /// A global.
    Global {
        /// Its value type.
        ty: ValType,
        /// Whether it is mutable.
        mutable: bool,
    },
}

/// One entry of the import section.
#[derive(Debug, Clone)]
pub struct Import {
    /// The module the import comes from (`wasi_snapshot_preview1`).
    pub module: String,
    /// The name inside that module (`fd_write`).
    pub name: String,
    /// What is being imported.
    pub kind: ImportKind,
}

/// What an export points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportKind {
    /// A function.
    Func,
    /// A table.
    Table,
    /// A memory.
    Memory,
    /// A global.
    Global,
}

impl ExportKind {
    fn code(self) -> u8 {
        match self {
            ExportKind::Func => 0,
            ExportKind::Table => 1,
            ExportKind::Memory => 2,
            ExportKind::Global => 3,
        }
    }

    fn name(self) -> &'static str {
        match self {
            ExportKind::Func => "func",
            ExportKind::Table => "table",
            ExportKind::Memory => "memory",
            ExportKind::Global => "global",
        }
    }
}

/// One entry of the export section.
#[derive(Debug, Clone)]
pub struct Export {
    /// The name the host sees.
    pub name: String,
    /// What kind of thing it names.
    pub kind: ExportKind,
    /// Its index in the relevant index space.
    pub index: u32,
}

/// One function body: its locals and its instructions.
#[derive(Debug, Clone)]
pub struct Func {
    /// Local declarations as `(count, type)` runs, exactly as the binary format groups them.
    pub locals: Vec<(u32, ValType)>,
    /// The instruction bytes, **including** the terminating `end` (`0x0b`).
    pub body: Vec<u8>,
    /// How the body reads, for the annotation.
    pub text: String,
}

/// How an element segment reaches its table.
#[derive(Debug, Clone)]
pub enum ElemMode {
    /// Copied into a table at instantiation time.
    Active {
        /// The table index.
        table: u32,
        /// The offset, a constant expression including its `end`.
        offset: Vec<u8>,
    },
    /// Available to `table.init` until it is dropped.
    Passive,
    /// Declared only so `ref.func` may name its functions.
    Declarative,
}

/// One element segment.
#[derive(Debug, Clone)]
pub struct Elem {
    /// Where the segment goes.
    pub mode: ElemMode,
    /// The element type; `funcref` for everything the func-index form can express.
    pub ty: ValType,
    /// The function indices, for the compact form.
    pub funcs: Vec<u32>,
    /// Constant expressions instead of indices, for the `ref.null` / `ref.func` form.
    pub exprs: Vec<Vec<u8>>,
}

/// How a data segment reaches memory.
#[derive(Debug, Clone)]
pub enum DataMode {
    /// Copied into a memory at instantiation time.
    Active {
        /// The memory index (always 0 today).
        memory: u32,
        /// The offset, a constant expression including its `end`.
        offset: Vec<u8>,
    },
    /// Available to `memory.init` until it is dropped.
    Passive,
}

/// One data segment.
#[derive(Debug, Clone)]
pub struct Data {
    /// Where the bytes go.
    pub mode: DataMode,
    /// The bytes themselves.
    pub bytes: Vec<u8>,
}

/// A custom section and where to put it.
#[derive(Debug, Clone)]
pub struct Custom {
    /// The section's name (`name`, `producers`, anything).
    pub name: String,
    /// Its payload, which a runtime must not try to understand.
    pub payload: Vec<u8>,
    /// The section id this custom section is written *after*; `None` means "first, right
    /// after the header".
    pub after: Option<u8>,
}

// ---------------------------------------------------------------------------------------
// The module builder
// ---------------------------------------------------------------------------------------

/// Builds a well-formed module and annotates every section it writes.
///
/// ```ignore
/// let m = ModuleBuilder::new("add")
///     .func_type(ftype(&[I32, I32], &[I32]))
///     .function(0, Func::new(Expr::new().local_get(0).local_get(1).op(op::I32_ADD)))
///     .export_func("add", 0)
///     .build();
/// ```
#[derive(Debug, Clone, Default)]
pub struct ModuleBuilder {
    label: String,
    types: Vec<FuncType>,
    imports: Vec<Import>,
    funcs: Vec<u32>,
    tables: Vec<TableType>,
    memories: Vec<Limits>,
    globals: Vec<Global>,
    exports: Vec<Export>,
    start: Option<u32>,
    elems: Vec<Elem>,
    code: Vec<Func>,
    data: Vec<Data>,
    force_data_count: bool,
    customs: Vec<Custom>,
}

impl ModuleBuilder {
    /// A builder for a module that will be written to `<label>.wasm`.
    pub fn new(label: impl Into<String>) -> ModuleBuilder {
        ModuleBuilder {
            label: label.into(),
            ..Default::default()
        }
    }

    /// Add a function type and return its index.
    pub fn add_type(&mut self, t: FuncType) -> u32 {
        if let Some(i) = self.types.iter().position(|x| *x == t) {
            return i as u32;
        }
        self.types.push(t);
        self.types.len() as u32 - 1
    }

    /// Add a function type, chaining.
    pub fn func_type(mut self, t: FuncType) -> ModuleBuilder {
        self.add_type(t);
        self
    }

    /// Add an import; imported functions come before defined ones in the index space.
    pub fn import(mut self, module: &str, name: &str, kind: ImportKind) -> ModuleBuilder {
        self.imports.push(Import {
            module: module.to_string(),
            name: name.to_string(),
            kind,
        });
        self
    }

    /// How many functions the imports bring in, i.e. the index of the first defined one.
    pub fn imported_funcs(&self) -> u32 {
        self.imports
            .iter()
            .filter(|i| matches!(i.kind, ImportKind::Func(_)))
            .count() as u32
    }

    /// Define a function of type index `ty` with the given body, and return its index.
    pub fn add_func(&mut self, ty: u32, body: Func) -> u32 {
        self.funcs.push(ty);
        self.code.push(body);
        self.imported_funcs() + self.funcs.len() as u32 - 1
    }

    /// Define a function, chaining.
    pub fn function(mut self, ty: u32, body: Func) -> ModuleBuilder {
        self.add_func(ty, body);
        self
    }

    /// Define a function whose type is created on the fly, and return its index.
    pub fn add_typed_func(&mut self, sig: FuncType, body: Func) -> u32 {
        let ty = self.add_type(sig);
        self.add_func(ty, body)
    }

    /// Add a table.
    pub fn table(mut self, t: TableType) -> ModuleBuilder {
        self.tables.push(t);
        self
    }

    /// Add a memory.
    pub fn memory(mut self, limits: Limits) -> ModuleBuilder {
        self.memories.push(limits);
        self
    }

    /// Add a global.
    pub fn global(mut self, g: Global) -> ModuleBuilder {
        self.globals.push(g);
        self
    }

    /// Export anything by index.
    pub fn export(mut self, name: &str, kind: ExportKind, index: u32) -> ModuleBuilder {
        self.exports.push(Export {
            name: name.to_string(),
            kind,
            index,
        });
        self
    }

    /// Export a function.
    pub fn export_func(self, name: &str, index: u32) -> ModuleBuilder {
        self.export(name, ExportKind::Func, index)
    }

    /// Export memory 0 as `memory` — every WASI module needs this.
    pub fn export_memory(self) -> ModuleBuilder {
        self.export("memory", ExportKind::Memory, 0)
    }

    /// Set the start function.
    pub fn start(mut self, index: u32) -> ModuleBuilder {
        self.start = Some(index);
        self
    }

    /// Add an element segment.
    pub fn elem(mut self, e: Elem) -> ModuleBuilder {
        self.elems.push(e);
        self
    }

    /// Add an active element segment putting `funcs` into table 0 at `offset`.
    pub fn elem_active(self, offset: i32, funcs: &[u32]) -> ModuleBuilder {
        self.elem(Elem {
            mode: ElemMode::Active {
                table: 0,
                offset: const_i32(offset),
            },
            ty: ValType::FuncRef,
            funcs: funcs.to_vec(),
            exprs: Vec::new(),
        })
    }

    /// Add a data segment.
    pub fn data(mut self, d: Data) -> ModuleBuilder {
        self.data.push(d);
        self
    }

    /// Add an active data segment writing `bytes` at `offset` of memory 0.
    pub fn data_active(self, offset: i32, bytes: &[u8]) -> ModuleBuilder {
        self.data(Data {
            mode: DataMode::Active {
                memory: 0,
                offset: const_i32(offset),
            },
            bytes: bytes.to_vec(),
        })
    }

    /// Add a passive data segment (`memory.init` reads it).
    pub fn data_passive(self, bytes: &[u8]) -> ModuleBuilder {
        self.data(Data {
            mode: DataMode::Passive,
            bytes: bytes.to_vec(),
        })
    }

    /// Emit a data count section even when no segment strictly needs one.
    pub fn with_data_count(mut self) -> ModuleBuilder {
        self.force_data_count = true;
        self
    }

    /// Add a custom section, placed after the section with id `after` (or first).
    pub fn custom(mut self, name: &str, payload: &[u8], after: Option<u8>) -> ModuleBuilder {
        self.customs.push(Custom {
            name: name.to_string(),
            payload: payload.to_vec(),
            after,
        });
        self
    }

    /// True when a data count section is required: any `memory.init` or `data.drop` in the
    /// module makes it mandatory, and a passive segment is always accompanied by one here.
    fn needs_data_count(&self) -> bool {
        self.force_data_count
            || self
                .data
                .iter()
                .any(|d| matches!(d.mode, DataMode::Passive))
    }

    /// Encode the module.
    pub fn build(&self) -> Module {
        let mut e = Enc::with_header();
        let customs_after = |e: &mut Enc, id: Option<u8>| {
            for c in self.customs.iter().filter(|c| c.after == id) {
                let mut body = wasm_name(&c.name);
                body.extend_from_slice(&c.payload);
                let at = e.at();
                e.section(section::CUSTOM, &body);
                e.annotate(
                    at + 2,
                    body.len(),
                    format!("custom[{}]", c.name),
                    format!("name '{}', {} payload bytes", c.name, c.payload.len()),
                );
            }
        };

        customs_after(&mut e, None);

        if !self.types.is_empty() {
            let items: Vec<Vec<u8>> = self.types.iter().map(FuncType::encode).collect();
            let body = vector(&items);
            let at = e.at();
            e.section(section::TYPE, &body);
            let mut pos = at + 1 + uleb(body.len() as u64).len() + uleb(items.len() as u64).len();
            for (i, (t, bytes)) in self.types.iter().zip(items.iter()).enumerate() {
                e.annotate(pos, bytes.len(), format!("type[{i}]"), t.text());
                pos += bytes.len();
            }
        }

        if !self.imports.is_empty() {
            let items: Vec<Vec<u8>> = self.imports.iter().map(encode_import).collect();
            let body = vector(&items);
            let at = e.at();
            e.section(section::IMPORT, &body);
            let mut pos = at + 1 + uleb(body.len() as u64).len() + uleb(items.len() as u64).len();
            for (i, (imp, bytes)) in self.imports.iter().zip(items.iter()).enumerate() {
                e.annotate(
                    pos,
                    bytes.len(),
                    format!("import[{i}]"),
                    format!(
                        "{}::{} — {}",
                        imp.module,
                        imp.name,
                        import_text(&imp.kind, self)
                    ),
                );
                pos += bytes.len();
            }
        }

        if !self.funcs.is_empty() {
            let items: Vec<Vec<u8>> = self.funcs.iter().map(|t| uleb(*t as u64)).collect();
            let body = vector(&items);
            let at = e.at();
            e.section(section::FUNCTION, &body);
            let mut pos = at + 1 + uleb(body.len() as u64).len() + uleb(items.len() as u64).len();
            let base = self.imported_funcs();
            for (i, (ty, bytes)) in self.funcs.iter().zip(items.iter()).enumerate() {
                e.annotate(
                    pos,
                    bytes.len(),
                    format!("function[{}]", base + i as u32),
                    format!(
                        "type {ty} {}",
                        self.types
                            .get(*ty as usize)
                            .map(FuncType::text)
                            .unwrap_or_else(|| "(no such type)".to_string())
                    ),
                );
                pos += bytes.len();
            }
        }

        if !self.tables.is_empty() {
            let items: Vec<Vec<u8>> = self
                .tables
                .iter()
                .map(|t| {
                    let mut v = vec![t.elem.code()];
                    v.extend_from_slice(&t.limits.encode());
                    v
                })
                .collect();
            let body = vector(&items);
            let at = e.at();
            e.section(section::TABLE, &body);
            let mut pos = at + 1 + uleb(body.len() as u64).len() + uleb(items.len() as u64).len();
            for (i, (t, bytes)) in self.tables.iter().zip(items.iter()).enumerate() {
                e.annotate(
                    pos,
                    bytes.len(),
                    format!("table[{i}]"),
                    format!("{}, {}", t.elem.name(), t.limits.text()),
                );
                pos += bytes.len();
            }
        }

        if !self.memories.is_empty() {
            let items: Vec<Vec<u8>> = self.memories.iter().map(Limits::encode).collect();
            let body = vector(&items);
            let at = e.at();
            e.section(section::MEMORY, &body);
            let mut pos = at + 1 + uleb(body.len() as u64).len() + uleb(items.len() as u64).len();
            for (i, (m, bytes)) in self.memories.iter().zip(items.iter()).enumerate() {
                e.annotate(
                    pos,
                    bytes.len(),
                    format!("memory[{i}]"),
                    format!("{} (64 KiB pages)", m.text()),
                );
                pos += bytes.len();
            }
        }

        if !self.globals.is_empty() {
            let items: Vec<Vec<u8>> = self
                .globals
                .iter()
                .map(|g| {
                    let mut v = vec![g.ty.code(), u8::from(g.mutable)];
                    v.extend_from_slice(&g.init);
                    v
                })
                .collect();
            let body = vector(&items);
            let at = e.at();
            e.section(section::GLOBAL, &body);
            let mut pos = at + 1 + uleb(body.len() as u64).len() + uleb(items.len() as u64).len();
            for (i, (g, bytes)) in self.globals.iter().zip(items.iter()).enumerate() {
                e.annotate(
                    pos,
                    bytes.len(),
                    format!("global[{i}]"),
                    format!(
                        "{} {}, init {}",
                        if g.mutable { "mut" } else { "const" },
                        g.ty.name(),
                        g.init_text
                    ),
                );
                pos += bytes.len();
            }
        }

        if !self.exports.is_empty() {
            let items: Vec<Vec<u8>> = self
                .exports
                .iter()
                .map(|x| {
                    let mut v = wasm_name(&x.name);
                    v.push(x.kind.code());
                    v.extend_from_slice(&uleb(x.index as u64));
                    v
                })
                .collect();
            let body = vector(&items);
            let at = e.at();
            e.section(section::EXPORT, &body);
            let mut pos = at + 1 + uleb(body.len() as u64).len() + uleb(items.len() as u64).len();
            for (i, (x, bytes)) in self.exports.iter().zip(items.iter()).enumerate() {
                e.annotate(
                    pos,
                    bytes.len(),
                    format!("export[{i}]"),
                    format!("'{}' → {} {}", x.name, x.kind.name(), x.index),
                );
                pos += bytes.len();
            }
        }

        if let Some(s) = self.start {
            let body = uleb(s as u64);
            let at = e.at();
            e.section(section::START, &body);
            e.annotate(
                at + 1 + uleb(body.len() as u64).len(),
                body.len(),
                "start.func",
                format!("function {s} runs before any export is callable"),
            );
        }

        if !self.elems.is_empty() {
            let items: Vec<Vec<u8>> = self.elems.iter().map(encode_elem).collect();
            let body = vector(&items);
            let at = e.at();
            e.section(section::ELEMENT, &body);
            let mut pos = at + 1 + uleb(body.len() as u64).len() + uleb(items.len() as u64).len();
            for (i, (el, bytes)) in self.elems.iter().zip(items.iter()).enumerate() {
                e.annotate(pos, bytes.len(), format!("elem[{i}]"), elem_text(el));
                pos += bytes.len();
            }
        }

        if self.needs_data_count() {
            let body = uleb(self.data.len() as u64);
            let at = e.at();
            e.section(section::DATA_COUNT, &body);
            e.annotate(
                at + 1 + uleb(body.len() as u64).len(),
                body.len(),
                "data_count.count",
                format!(
                    "{} data segments, declared before the code section",
                    self.data.len()
                ),
            );
        }

        if !self.code.is_empty() {
            let items: Vec<Vec<u8>> = self.code.iter().map(encode_func).collect();
            let body = vector(&items);
            let at = e.at();
            e.section(section::CODE, &body);
            let mut pos = at + 1 + uleb(body.len() as u64).len() + uleb(items.len() as u64).len();
            let base = self.imported_funcs();
            for (i, (f, bytes)) in self.code.iter().zip(items.iter()).enumerate() {
                let size_len = uleb((bytes.len() - uleb(bytes.len() as u64).len()) as u64).len();
                e.annotate(
                    pos,
                    size_len,
                    format!("code[{}].size", base + i as u32),
                    format!("{} bytes", bytes.len() - size_len),
                );
                e.annotate(
                    pos + size_len,
                    bytes.len() - size_len,
                    format!("code[{}].body", base + i as u32),
                    if f.locals.is_empty() {
                        f.text.clone()
                    } else {
                        format!("locals {}; {}", locals_text(&f.locals), f.text)
                    },
                );
                pos += bytes.len();
            }
        }

        if !self.data.is_empty() {
            let items: Vec<Vec<u8>> = self.data.iter().map(encode_data).collect();
            let body = vector(&items);
            let at = e.at();
            e.section(section::DATA, &body);
            let mut pos = at + 1 + uleb(body.len() as u64).len() + uleb(items.len() as u64).len();
            for (i, (d, bytes)) in self.data.iter().zip(items.iter()).enumerate() {
                e.annotate(pos, bytes.len(), format!("data[{i}]"), data_text(d));
                pos += bytes.len();
            }
        }

        for id in [
            section::TYPE,
            section::IMPORT,
            section::FUNCTION,
            section::TABLE,
            section::MEMORY,
            section::GLOBAL,
            section::EXPORT,
            section::START,
            section::ELEMENT,
            section::CODE,
            section::DATA,
        ] {
            customs_after(&mut e, Some(id));
        }

        e.finish(self.label.clone())
    }
}

fn locals_text(locals: &[(u32, ValType)]) -> String {
    locals
        .iter()
        .map(|(n, t)| format!("{n}×{}", t.name()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn import_text(kind: &ImportKind, m: &ModuleBuilder) -> String {
    match kind {
        ImportKind::Func(t) => format!(
            "func type {t} {}",
            m.types
                .get(*t as usize)
                .map(FuncType::text)
                .unwrap_or_else(|| "(no such type)".to_string())
        ),
        ImportKind::Table(t) => format!("table {}, {}", t.elem.name(), t.limits.text()),
        ImportKind::Memory(l) => format!("memory {}", l.text()),
        ImportKind::Global { ty, mutable } => format!(
            "global {} {}",
            if *mutable { "mut" } else { "const" },
            ty.name()
        ),
    }
}

fn encode_import(i: &Import) -> Vec<u8> {
    let mut out = wasm_name(&i.module);
    out.extend_from_slice(&wasm_name(&i.name));
    match &i.kind {
        ImportKind::Func(t) => {
            out.push(0x00);
            out.extend_from_slice(&uleb(*t as u64));
        }
        ImportKind::Table(t) => {
            out.push(0x01);
            out.push(t.elem.code());
            out.extend_from_slice(&t.limits.encode());
        }
        ImportKind::Memory(l) => {
            out.push(0x02);
            out.extend_from_slice(&l.encode());
        }
        ImportKind::Global { ty, mutable } => {
            out.push(0x03);
            out.push(ty.code());
            out.push(u8::from(*mutable));
        }
    }
    out
}

fn encode_func(f: &Func) -> Vec<u8> {
    let mut inner = uleb(f.locals.len() as u64);
    for (n, t) in &f.locals {
        inner.extend_from_slice(&uleb(*n as u64));
        inner.push(t.code());
    }
    inner.extend_from_slice(&f.body);
    let mut out = uleb(inner.len() as u64);
    out.extend_from_slice(&inner);
    out
}

fn encode_elem(e: &Elem) -> Vec<u8> {
    let use_exprs = !e.exprs.is_empty();
    let mut out = Vec::new();
    match (&e.mode, use_exprs) {
        (ElemMode::Active { table: 0, offset }, false) => {
            out.push(0x00);
            out.extend_from_slice(offset);
            out.extend_from_slice(&vector(
                &e.funcs.iter().map(|f| uleb(*f as u64)).collect::<Vec<_>>(),
            ));
        }
        (ElemMode::Passive, false) => {
            out.push(0x01);
            out.push(0x00);
            out.extend_from_slice(&vector(
                &e.funcs.iter().map(|f| uleb(*f as u64)).collect::<Vec<_>>(),
            ));
        }
        (ElemMode::Active { table, offset }, false) => {
            out.push(0x02);
            out.extend_from_slice(&uleb(*table as u64));
            out.extend_from_slice(offset);
            out.push(0x00);
            out.extend_from_slice(&vector(
                &e.funcs.iter().map(|f| uleb(*f as u64)).collect::<Vec<_>>(),
            ));
        }
        (ElemMode::Declarative, false) => {
            out.push(0x03);
            out.push(0x00);
            out.extend_from_slice(&vector(
                &e.funcs.iter().map(|f| uleb(*f as u64)).collect::<Vec<_>>(),
            ));
        }
        (ElemMode::Active { table: 0, offset }, true) => {
            out.push(0x04);
            out.extend_from_slice(offset);
            out.extend_from_slice(&vector(&e.exprs));
        }
        (ElemMode::Passive, true) => {
            out.push(0x05);
            out.push(e.ty.code());
            out.extend_from_slice(&vector(&e.exprs));
        }
        (ElemMode::Active { table, offset }, true) => {
            out.push(0x06);
            out.extend_from_slice(&uleb(*table as u64));
            out.extend_from_slice(offset);
            out.push(e.ty.code());
            out.extend_from_slice(&vector(&e.exprs));
        }
        (ElemMode::Declarative, true) => {
            out.push(0x07);
            out.push(e.ty.code());
            out.extend_from_slice(&vector(&e.exprs));
        }
    }
    out
}

fn elem_text(e: &Elem) -> String {
    let what = if e.exprs.is_empty() {
        format!("{} funcs {:?}", e.funcs.len(), e.funcs)
    } else {
        format!("{} element expressions", e.exprs.len())
    };
    match &e.mode {
        ElemMode::Active { table, .. } => format!("active into table {table}, {what}"),
        ElemMode::Passive => format!("passive, {what}"),
        ElemMode::Declarative => format!("declarative, {what}"),
    }
}

fn encode_data(d: &Data) -> Vec<u8> {
    let mut out = Vec::new();
    match &d.mode {
        DataMode::Active { memory: 0, offset } => {
            out.push(0x00);
            out.extend_from_slice(offset);
        }
        DataMode::Passive => out.push(0x01),
        DataMode::Active { memory, offset } => {
            out.push(0x02);
            out.extend_from_slice(&uleb(*memory as u64));
            out.extend_from_slice(offset);
        }
    }
    out.extend_from_slice(&uleb(d.bytes.len() as u64));
    out.extend_from_slice(&d.bytes);
    out
}

fn data_text(d: &Data) -> String {
    let preview: String = d
        .bytes
        .iter()
        .take(16)
        .map(|b| {
            if (0x20..0x7f).contains(b) {
                char::from(*b)
            } else {
                '.'
            }
        })
        .collect();
    match &d.mode {
        DataMode::Active { memory, .. } => format!(
            "active into memory {memory}, {} bytes '{preview}'",
            d.bytes.len()
        ),
        DataMode::Passive => format!("passive, {} bytes '{preview}'", d.bytes.len()),
    }
}

/// The constant expression `i32.const v; end`, as an initialiser or an offset.
pub fn const_i32(v: i32) -> Vec<u8> {
    let mut out = vec![0x41];
    out.extend_from_slice(&sleb(v as i64));
    out.push(0x0b);
    out
}

/// The constant expression `i64.const v; end`.
pub fn const_i64(v: i64) -> Vec<u8> {
    let mut out = vec![0x42];
    out.extend_from_slice(&sleb(v));
    out.push(0x0b);
    out
}

/// The constant expression `f32.const v; end`.
pub fn const_f32(v: f32) -> Vec<u8> {
    let mut out = vec![0x43];
    out.extend_from_slice(&v.to_le_bytes());
    out.push(0x0b);
    out
}

/// The constant expression `f64.const v; end`.
pub fn const_f64(v: f64) -> Vec<u8> {
    let mut out = vec![0x44];
    out.extend_from_slice(&v.to_le_bytes());
    out.push(0x0b);
    out
}

/// The constant expression `global.get n; end`, the only non-literal initialiser there is.
pub fn const_global_get(n: u32) -> Vec<u8> {
    let mut out = vec![0x23];
    out.extend_from_slice(&uleb(n as u64));
    out.push(0x0b);
    out
}

/// The constant expression `ref.null t; end`.
pub fn const_ref_null(t: ValType) -> Vec<u8> {
    vec![0xd0, t.code(), 0x0b]
}

/// The constant expression `ref.func n; end`.
pub fn const_ref_func(n: u32) -> Vec<u8> {
    let mut out = vec![0xd2];
    out.extend_from_slice(&uleb(n as u64));
    out.push(0x0b);
    out
}

/// An immutable global with an `i32.const` initialiser.
pub fn global_i32(v: i32, mutable: bool) -> Global {
    Global {
        ty: ValType::I32,
        mutable,
        init: const_i32(v),
        init_text: format!("i32.const {v}"),
    }
}

/// An immutable global with an `i64.const` initialiser.
pub fn global_i64(v: i64, mutable: bool) -> Global {
    Global {
        ty: ValType::I64,
        mutable,
        init: const_i64(v),
        init_text: format!("i64.const {v}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wasm::instr::{op, Expr};

    #[test]
    fn uleb_matches_the_spec_examples() {
        assert_eq!(uleb(0), vec![0x00]);
        assert_eq!(uleb(1), vec![0x01]);
        assert_eq!(uleb(127), vec![0x7f]);
        assert_eq!(uleb(128), vec![0x80, 0x01]);
        assert_eq!(uleb(624_485), vec![0xe5, 0x8e, 0x26]);
        assert_eq!(uleb(u32::MAX as u64), vec![0xff, 0xff, 0xff, 0xff, 0x0f]);
    }

    #[test]
    fn sleb_matches_the_spec_examples() {
        assert_eq!(sleb(0), vec![0x00]);
        assert_eq!(sleb(-1), vec![0x7f]);
        // 63 still fits in the six value bits of one byte; 64 sets the sign bit, so it
        // needs a second byte to say "positive".
        assert_eq!(sleb(63), vec![0x3f]);
        assert_eq!(sleb(64), vec![0xc0, 0x00]);
        assert_eq!(sleb(-64), vec![0x40]);
        assert_eq!(sleb(-123_456), vec![0xc0, 0xbb, 0x78]);
        assert_eq!(
            sleb(i32::MIN as i64),
            vec![0x80, 0x80, 0x80, 0x80, 0x78],
            "i32::MIN is five bytes"
        );
    }

    #[test]
    fn padded_encodings_decode_to_the_same_number() {
        // A five-byte encoding of 1 is still 1 to any decoder that honours the high bit.
        assert_eq!(uleb_padded(1, 5), vec![0x81, 0x80, 0x80, 0x80, 0x00]);
        assert_eq!(uleb_padded(0, 2), vec![0x80, 0x00]);
        assert_eq!(sleb_padded(-1, 3), vec![0xff, 0xff, 0x7f]);
    }

    #[test]
    fn a_module_starts_with_the_magic_and_version() {
        let m = ModuleBuilder::new("empty").build();
        assert_eq!(m.bytes, b"\0asm\x01\x00\x00\x00");
        assert_eq!(m.anns.len(), 2);
        assert_eq!(m.anns[0].field, "header.magic");
        assert_eq!(m.anns[1].field, "header.version");
    }

    #[test]
    fn sections_come_out_in_the_order_the_spec_fixes() {
        let m = ModuleBuilder::new("ordered")
            .func_type(ftype(&[], &[ValType::I32]))
            .memory(Limits::min(1))
            .function(0, Func::new(Expr::new().i32_const(7)))
            .export_func("seven", 0)
            .build();
        let ids: Vec<u8> = m
            .anns
            .iter()
            .filter(|a| a.field.ends_with(".id"))
            .map(|a| m.bytes[a.offset])
            .collect();
        assert_eq!(
            ids,
            vec![
                section::TYPE,
                section::FUNCTION,
                section::MEMORY,
                section::EXPORT,
                section::CODE
            ]
        );
    }

    #[test]
    fn every_annotation_lands_inside_the_module() {
        let m = ModuleBuilder::new("anns")
            .func_type(ftype(&[ValType::I32], &[ValType::I32]))
            .memory(Limits::range(1, 2))
            .global(global_i32(3, true))
            .function(
                0,
                Func::new(Expr::new().local_get(0).i32_const(1).op(op::I32_ADD)),
            )
            .export_func("inc", 0)
            .data_active(0, b"hello")
            .build();
        for a in &m.anns {
            assert!(
                a.offset + a.length <= m.bytes.len(),
                "annotation {a:?} runs past the module ({} bytes)",
                m.bytes.len()
            );
            assert!(!a.field.is_empty());
        }
        assert!(m.listing(4096).contains("code[0].body"));
    }

    #[test]
    fn a_lying_size_field_is_written_exactly_as_asked() {
        let mut e = Enc::with_header();
        e.section_with_size(
            section::TYPE,
            &uleb(99),
            &[0x00],
            "99, but only 1 byte follows",
        );
        let m = e.finish("liar");
        assert_eq!(m.bytes.len(), 8 + 1 + 1 + 1);
        assert_eq!(m.bytes[9], 99);
    }
}
