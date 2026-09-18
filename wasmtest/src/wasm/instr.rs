//! The instruction builder.
//!
//! [`Expr`] is a byte buffer that also remembers the mnemonics it wrote, so a function body
//! can be annotated with what it actually does (`local.get 0, local.get 1, i32.add, end`)
//! rather than with a length. That text is what the failure block and the catalog examples
//! show next to the hex.
//!
//! Single-byte opcodes are [`Op`] constants in [`op`]; the `0xfc`-prefixed ones (saturating
//! truncation and the bulk-memory family) are [`Op2`] constants in [`op2`]. Anything the
//! suite has no constant for can still be written with [`Expr::bytes`].

use super::encode::{sleb, uleb, Func, ValType};

/// A single-byte opcode and its spec mnemonic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Op(pub u8, pub &'static str);

/// A `0xfc`-prefixed opcode (its sub-index is a uLEB128) and its spec mnemonic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Op2(pub u32, pub &'static str);

/// The block type of a `block`, `loop` or `if`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockType {
    /// No result.
    Empty,
    /// One result of this type.
    Value(ValType),
    /// The function type at this index: block parameters and multiple results (stage 27).
    Type(u32),
}

impl BlockType {
    fn encode(self) -> Vec<u8> {
        match self {
            BlockType::Empty => vec![0x40],
            BlockType::Value(t) => vec![t.code()],
            // A type index in a block type is a *signed* LEB128, which is what keeps it
            // apart from the 0x40/0x7f.. single-byte forms.
            BlockType::Type(i) => sleb(i as i64),
        }
    }

    fn text(self) -> String {
        match self {
            BlockType::Empty => String::new(),
            BlockType::Value(t) => format!(" (result {})", t.name()),
            BlockType::Type(i) => format!(" (type {i})"),
        }
    }
}

/// A sequence of instructions, and what it says.
#[derive(Debug, Clone, Default)]
pub struct Expr {
    bytes: Vec<u8>,
    text: Vec<String>,
}

impl Expr {
    /// An empty instruction sequence.
    pub fn new() -> Expr {
        Expr::default()
    }

    /// The instruction bytes, **without** a terminating `end`.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The instruction bytes plus the terminating `end` (`0x0b`).
    pub fn into_body(mut self) -> Vec<u8> {
        self.bytes.push(0x0b);
        self.bytes
    }

    /// The mnemonics, comma separated, as they go into an annotation.
    pub fn text(&self) -> String {
        self.text.join(", ")
    }

    /// The constant expression form: the bytes plus `end`, for an initialiser or an offset.
    pub fn into_const(self) -> Vec<u8> {
        self.into_body()
    }

    fn push(mut self, bytes: &[u8], text: impl Into<String>) -> Expr {
        self.bytes.extend_from_slice(bytes);
        self.text.push(text.into());
        self
    }

    /// Append raw bytes with a description — for anything the suite has no helper for.
    pub fn bytes(self, bytes: &[u8], text: impl Into<String>) -> Expr {
        self.push(bytes, text)
    }

    /// Append another expression's bytes and text.
    pub fn then(mut self, other: Expr) -> Expr {
        self.bytes.extend_from_slice(&other.bytes);
        self.text.extend(other.text);
        self
    }

    /// Repeat an expression `n` times.
    pub fn repeat(mut self, n: usize, other: &Expr) -> Expr {
        for _ in 0..n {
            self.bytes.extend_from_slice(&other.bytes);
        }
        if n > 0 {
            self.text.push(format!("[{} × {}]", n, other.text()));
        }
        self
    }

    // -- constants ----------------------------------------------------------------------

    /// `i32.const v`.
    pub fn i32_const(self, v: i32) -> Expr {
        let mut b = vec![0x41];
        b.extend_from_slice(&sleb(v as i64));
        self.push(&b, format!("i32.const {v}"))
    }

    /// `i64.const v`.
    pub fn i64_const(self, v: i64) -> Expr {
        let mut b = vec![0x42];
        b.extend_from_slice(&sleb(v));
        self.push(&b, format!("i64.const {v}"))
    }

    /// `f32.const v` — four little-endian bytes, never a LEB128.
    pub fn f32_const(self, v: f32) -> Expr {
        let mut b = vec![0x43];
        b.extend_from_slice(&v.to_le_bytes());
        self.push(&b, format!("f32.const {v:?}"))
    }

    /// `f32.const` from a raw bit pattern, so a test can name an exact NaN payload.
    pub fn f32_bits(self, bits: u32) -> Expr {
        let mut b = vec![0x43];
        b.extend_from_slice(&bits.to_le_bytes());
        self.push(&b, format!("f32.const 0x{bits:08x} (bit pattern)"))
    }

    /// `f64.const v`.
    pub fn f64_const(self, v: f64) -> Expr {
        let mut b = vec![0x44];
        b.extend_from_slice(&v.to_le_bytes());
        self.push(&b, format!("f64.const {v:?}"))
    }

    /// `f64.const` from a raw bit pattern.
    pub fn f64_bits(self, bits: u64) -> Expr {
        let mut b = vec![0x44];
        b.extend_from_slice(&bits.to_le_bytes());
        self.push(&b, format!("f64.const 0x{bits:016x} (bit pattern)"))
    }

    // -- locals and globals -------------------------------------------------------------

    /// `local.get n`.
    pub fn local_get(self, n: u32) -> Expr {
        let mut b = vec![0x20];
        b.extend_from_slice(&uleb(n as u64));
        self.push(&b, format!("local.get {n}"))
    }

    /// `local.set n`.
    pub fn local_set(self, n: u32) -> Expr {
        let mut b = vec![0x21];
        b.extend_from_slice(&uleb(n as u64));
        self.push(&b, format!("local.set {n}"))
    }

    /// `local.tee n` — set, and leave the value on the stack.
    pub fn local_tee(self, n: u32) -> Expr {
        let mut b = vec![0x22];
        b.extend_from_slice(&uleb(n as u64));
        self.push(&b, format!("local.tee {n}"))
    }

    /// `global.get n`.
    pub fn global_get(self, n: u32) -> Expr {
        let mut b = vec![0x23];
        b.extend_from_slice(&uleb(n as u64));
        self.push(&b, format!("global.get {n}"))
    }

    /// `global.set n`.
    pub fn global_set(self, n: u32) -> Expr {
        let mut b = vec![0x24];
        b.extend_from_slice(&uleb(n as u64));
        self.push(&b, format!("global.set {n}"))
    }

    // -- plain opcodes ------------------------------------------------------------------

    /// Any single-byte opcode from [`op`].
    pub fn op(self, o: Op) -> Expr {
        self.push(&[o.0], o.1)
    }

    /// Any `0xfc`-prefixed opcode from [`op2`] with no immediates.
    pub fn op2(self, o: Op2) -> Expr {
        let mut b = vec![0xfc];
        b.extend_from_slice(&uleb(o.0 as u64));
        self.push(&b, o.1)
    }

    /// `nop`.
    pub fn nop(self) -> Expr {
        self.op(op::NOP)
    }

    /// `drop`.
    pub fn drop(self) -> Expr {
        self.op(op::DROP)
    }

    /// `unreachable`.
    pub fn unreachable(self) -> Expr {
        self.op(op::UNREACHABLE)
    }

    /// `return`.
    pub fn return_(self) -> Expr {
        self.op(op::RETURN)
    }

    /// `select` — untyped: the two operands must be numeric.
    pub fn select(self) -> Expr {
        self.op(op::SELECT)
    }

    /// `select t` — the typed form, which is what reference types need.
    pub fn select_typed(self, types: &[ValType]) -> Expr {
        let mut b = vec![0x1c];
        b.extend_from_slice(&uleb(types.len() as u64));
        b.extend(types.iter().map(|t| t.code()));
        let names: Vec<&str> = types.iter().map(|t| t.name()).collect();
        self.push(&b, format!("select ({})", names.join(" ")))
    }

    // -- control flow -------------------------------------------------------------------

    /// `block bt … end`.
    pub fn block(self, bt: BlockType, body: Expr) -> Expr {
        let mut b = vec![0x02];
        b.extend_from_slice(&bt.encode());
        let text = format!("block{} {{ {} }}", bt.text(), body.text());
        let mut e = self.push(&b, text);
        e.bytes.extend_from_slice(&body.bytes);
        e.bytes.push(0x0b);
        e
    }

    /// `loop bt … end`.
    pub fn loop_(self, bt: BlockType, body: Expr) -> Expr {
        let mut b = vec![0x03];
        b.extend_from_slice(&bt.encode());
        let text = format!("loop{} {{ {} }}", bt.text(), body.text());
        let mut e = self.push(&b, text);
        e.bytes.extend_from_slice(&body.bytes);
        e.bytes.push(0x0b);
        e
    }

    /// `if bt … end` with no `else`.
    pub fn if_(self, bt: BlockType, then: Expr) -> Expr {
        let mut b = vec![0x04];
        b.extend_from_slice(&bt.encode());
        let text = format!("if{} {{ {} }}", bt.text(), then.text());
        let mut e = self.push(&b, text);
        e.bytes.extend_from_slice(&then.bytes);
        e.bytes.push(0x0b);
        e
    }

    /// `if bt … else … end`.
    pub fn if_else(self, bt: BlockType, then: Expr, otherwise: Expr) -> Expr {
        let mut b = vec![0x04];
        b.extend_from_slice(&bt.encode());
        let text = format!(
            "if{} {{ {} }} else {{ {} }}",
            bt.text(),
            then.text(),
            otherwise.text()
        );
        let mut e = self.push(&b, text);
        e.bytes.extend_from_slice(&then.bytes);
        e.bytes.push(0x05);
        e.bytes.extend_from_slice(&otherwise.bytes);
        e.bytes.push(0x0b);
        e
    }

    /// `br l` — a jump to the end of the `l`-th enclosing block (or the top of a `loop`).
    pub fn br(self, label: u32) -> Expr {
        let mut b = vec![0x0c];
        b.extend_from_slice(&uleb(label as u64));
        self.push(&b, format!("br {label}"))
    }

    /// `br_if l`.
    pub fn br_if(self, label: u32) -> Expr {
        let mut b = vec![0x0d];
        b.extend_from_slice(&uleb(label as u64));
        self.push(&b, format!("br_if {label}"))
    }

    /// `br_table l* ld` — a jump table, with the default label last.
    pub fn br_table(self, labels: &[u32], default: u32) -> Expr {
        let mut b = vec![0x0e];
        b.extend_from_slice(&uleb(labels.len() as u64));
        for l in labels {
            b.extend_from_slice(&uleb(*l as u64));
        }
        b.extend_from_slice(&uleb(default as u64));
        self.push(&b, format!("br_table {labels:?} default {default}"))
    }

    /// `call n`.
    pub fn call(self, n: u32) -> Expr {
        let mut b = vec![0x10];
        b.extend_from_slice(&uleb(n as u64));
        self.push(&b, format!("call {n}"))
    }

    /// `call_indirect ty tbl` — the table index comes off the stack.
    pub fn call_indirect(self, ty: u32, table: u32) -> Expr {
        let mut b = vec![0x11];
        b.extend_from_slice(&uleb(ty as u64));
        b.extend_from_slice(&uleb(table as u64));
        self.push(&b, format!("call_indirect (type {ty}) (table {table})"))
    }

    // -- memory -------------------------------------------------------------------------

    /// A load or a store: `align` is the *power of two*, `offset` the static displacement.
    pub fn mem(self, o: Op, align: u32, offset: u32) -> Expr {
        let mut b = vec![o.0];
        b.extend_from_slice(&uleb(align as u64));
        b.extend_from_slice(&uleb(offset as u64));
        self.push(&b, format!("{} align={} offset={offset}", o.1, 1u32 << align))
    }

    /// `i32.load` with the natural alignment and no offset.
    pub fn i32_load(self, offset: u32) -> Expr {
        self.mem(op::I32_LOAD, 2, offset)
    }

    /// `i32.store` with the natural alignment and no offset.
    pub fn i32_store(self, offset: u32) -> Expr {
        self.mem(op::I32_STORE, 2, offset)
    }

    /// `i64.load` with the natural alignment.
    pub fn i64_load(self, offset: u32) -> Expr {
        self.mem(op::I64_LOAD, 3, offset)
    }

    /// `i64.store` with the natural alignment.
    pub fn i64_store(self, offset: u32) -> Expr {
        self.mem(op::I64_STORE, 3, offset)
    }

    /// `memory.size` (memory 0).
    pub fn memory_size(self) -> Expr {
        self.push(&[0x3f, 0x00], "memory.size")
    }

    /// `memory.grow` (memory 0): pages off the stack, the old size back or -1.
    pub fn memory_grow(self) -> Expr {
        self.push(&[0x40, 0x00], "memory.grow")
    }

    /// `memory.init d` — copy from passive data segment `d`.
    pub fn memory_init(self, seg: u32) -> Expr {
        let mut b = vec![0xfc];
        b.extend_from_slice(&uleb(8));
        b.extend_from_slice(&uleb(seg as u64));
        b.push(0x00);
        self.push(&b, format!("memory.init {seg}"))
    }

    /// `data.drop d`.
    pub fn data_drop(self, seg: u32) -> Expr {
        let mut b = vec![0xfc];
        b.extend_from_slice(&uleb(9));
        b.extend_from_slice(&uleb(seg as u64));
        self.push(&b, format!("data.drop {seg}"))
    }

    /// `memory.copy` — destination, source, length off the stack.
    pub fn memory_copy(self) -> Expr {
        let mut b = vec![0xfc];
        b.extend_from_slice(&uleb(10));
        b.extend_from_slice(&[0x00, 0x00]);
        self.push(&b, "memory.copy")
    }

    /// `memory.fill` — destination, byte value, length off the stack.
    pub fn memory_fill(self) -> Expr {
        let mut b = vec![0xfc];
        b.extend_from_slice(&uleb(11));
        b.push(0x00);
        self.push(&b, "memory.fill")
    }

    // -- tables and references ----------------------------------------------------------

    /// `table.get t`.
    pub fn table_get(self, table: u32) -> Expr {
        let mut b = vec![0x25];
        b.extend_from_slice(&uleb(table as u64));
        self.push(&b, format!("table.get {table}"))
    }

    /// `table.set t`.
    pub fn table_set(self, table: u32) -> Expr {
        let mut b = vec![0x26];
        b.extend_from_slice(&uleb(table as u64));
        self.push(&b, format!("table.set {table}"))
    }

    /// `table.size t`.
    pub fn table_size(self, table: u32) -> Expr {
        let mut b = vec![0xfc];
        b.extend_from_slice(&uleb(16));
        b.extend_from_slice(&uleb(table as u64));
        self.push(&b, format!("table.size {table}"))
    }

    /// `table.grow t`.
    pub fn table_grow(self, table: u32) -> Expr {
        let mut b = vec![0xfc];
        b.extend_from_slice(&uleb(15));
        b.extend_from_slice(&uleb(table as u64));
        self.push(&b, format!("table.grow {table}"))
    }

    /// `table.fill t`.
    pub fn table_fill(self, table: u32) -> Expr {
        let mut b = vec![0xfc];
        b.extend_from_slice(&uleb(17));
        b.extend_from_slice(&uleb(table as u64));
        self.push(&b, format!("table.fill {table}"))
    }

    /// `table.copy dst src`.
    pub fn table_copy(self, dst: u32, src: u32) -> Expr {
        let mut b = vec![0xfc];
        b.extend_from_slice(&uleb(14));
        b.extend_from_slice(&uleb(dst as u64));
        b.extend_from_slice(&uleb(src as u64));
        self.push(&b, format!("table.copy {dst} {src}"))
    }

    /// `table.init e t` — copy from passive element segment `e` into table `t`.
    pub fn table_init(self, seg: u32, table: u32) -> Expr {
        let mut b = vec![0xfc];
        b.extend_from_slice(&uleb(12));
        b.extend_from_slice(&uleb(seg as u64));
        b.extend_from_slice(&uleb(table as u64));
        self.push(&b, format!("table.init {seg} {table}"))
    }

    /// `elem.drop e`.
    pub fn elem_drop(self, seg: u32) -> Expr {
        let mut b = vec![0xfc];
        b.extend_from_slice(&uleb(13));
        b.extend_from_slice(&uleb(seg as u64));
        self.push(&b, format!("elem.drop {seg}"))
    }

    /// `ref.null t`.
    pub fn ref_null(self, t: ValType) -> Expr {
        self.push(&[0xd0, t.code()], format!("ref.null {}", t.name()))
    }

    /// `ref.is_null`.
    pub fn ref_is_null(self) -> Expr {
        self.push(&[0xd1], "ref.is_null")
    }

    /// `ref.func n`.
    pub fn ref_func(self, n: u32) -> Expr {
        let mut b = vec![0xd2];
        b.extend_from_slice(&uleb(n as u64));
        self.push(&b, format!("ref.func {n}"))
    }
}

impl Func {
    /// A function body with no locals.
    pub fn new(e: Expr) -> Func {
        Func {
            locals: Vec::new(),
            text: e.text(),
            body: e.into_body(),
        }
    }

    /// A function body with local declarations.
    pub fn with_locals(locals: &[(u32, ValType)], e: Expr) -> Func {
        Func {
            locals: locals.to_vec(),
            text: e.text(),
            body: e.into_body(),
        }
    }

    /// A function body written as raw bytes — including its own terminating `end`.
    ///
    /// The malformed-body stages need this: a body that ends early, one with a trailing
    /// byte, one holding an opcode no version of the format defines.
    pub fn raw(body: &[u8], text: impl Into<String>) -> Func {
        Func {
            locals: Vec::new(),
            body: body.to_vec(),
            text: text.into(),
        }
    }

    /// A function body with raw local declarations and raw bytes.
    pub fn raw_with_locals(locals: &[(u32, ValType)], body: &[u8], text: impl Into<String>) -> Func {
        Func {
            locals: locals.to_vec(),
            body: body.to_vec(),
            text: text.into(),
        }
    }
}

/// Single-byte opcodes, named as the spec names them.
#[allow(missing_docs)]
pub mod op {
    use super::Op;

    pub const UNREACHABLE: Op = Op(0x00, "unreachable");
    pub const NOP: Op = Op(0x01, "nop");
    pub const ELSE: Op = Op(0x05, "else");
    pub const END: Op = Op(0x0b, "end");
    pub const RETURN: Op = Op(0x0f, "return");
    pub const DROP: Op = Op(0x1a, "drop");
    pub const SELECT: Op = Op(0x1b, "select");

    // Loads and stores. The immediates (align, offset) are written by `Expr::mem`.
    pub const I32_LOAD: Op = Op(0x28, "i32.load");
    pub const I64_LOAD: Op = Op(0x29, "i64.load");
    pub const F32_LOAD: Op = Op(0x2a, "f32.load");
    pub const F64_LOAD: Op = Op(0x2b, "f64.load");
    pub const I32_LOAD8_S: Op = Op(0x2c, "i32.load8_s");
    pub const I32_LOAD8_U: Op = Op(0x2d, "i32.load8_u");
    pub const I32_LOAD16_S: Op = Op(0x2e, "i32.load16_s");
    pub const I32_LOAD16_U: Op = Op(0x2f, "i32.load16_u");
    pub const I64_LOAD8_S: Op = Op(0x30, "i64.load8_s");
    pub const I64_LOAD8_U: Op = Op(0x31, "i64.load8_u");
    pub const I64_LOAD16_S: Op = Op(0x32, "i64.load16_s");
    pub const I64_LOAD16_U: Op = Op(0x33, "i64.load16_u");
    pub const I64_LOAD32_S: Op = Op(0x34, "i64.load32_s");
    pub const I64_LOAD32_U: Op = Op(0x35, "i64.load32_u");
    pub const I32_STORE: Op = Op(0x36, "i32.store");
    pub const I64_STORE: Op = Op(0x37, "i64.store");
    pub const F32_STORE: Op = Op(0x38, "f32.store");
    pub const F64_STORE: Op = Op(0x39, "f64.store");
    pub const I32_STORE8: Op = Op(0x3a, "i32.store8");
    pub const I32_STORE16: Op = Op(0x3b, "i32.store16");
    pub const I64_STORE8: Op = Op(0x3c, "i64.store8");
    pub const I64_STORE16: Op = Op(0x3d, "i64.store16");
    pub const I64_STORE32: Op = Op(0x3e, "i64.store32");

    // i32 comparisons.
    pub const I32_EQZ: Op = Op(0x45, "i32.eqz");
    pub const I32_EQ: Op = Op(0x46, "i32.eq");
    pub const I32_NE: Op = Op(0x47, "i32.ne");
    pub const I32_LT_S: Op = Op(0x48, "i32.lt_s");
    pub const I32_LT_U: Op = Op(0x49, "i32.lt_u");
    pub const I32_GT_S: Op = Op(0x4a, "i32.gt_s");
    pub const I32_GT_U: Op = Op(0x4b, "i32.gt_u");
    pub const I32_LE_S: Op = Op(0x4c, "i32.le_s");
    pub const I32_LE_U: Op = Op(0x4d, "i32.le_u");
    pub const I32_GE_S: Op = Op(0x4e, "i32.ge_s");
    pub const I32_GE_U: Op = Op(0x4f, "i32.ge_u");

    // i64 comparisons.
    pub const I64_EQZ: Op = Op(0x50, "i64.eqz");
    pub const I64_EQ: Op = Op(0x51, "i64.eq");
    pub const I64_NE: Op = Op(0x52, "i64.ne");
    pub const I64_LT_S: Op = Op(0x53, "i64.lt_s");
    pub const I64_LT_U: Op = Op(0x54, "i64.lt_u");
    pub const I64_GT_S: Op = Op(0x55, "i64.gt_s");
    pub const I64_GT_U: Op = Op(0x56, "i64.gt_u");
    pub const I64_LE_S: Op = Op(0x57, "i64.le_s");
    pub const I64_LE_U: Op = Op(0x58, "i64.le_u");
    pub const I64_GE_S: Op = Op(0x59, "i64.ge_s");
    pub const I64_GE_U: Op = Op(0x5a, "i64.ge_u");

    // f32 / f64 comparisons.
    pub const F32_EQ: Op = Op(0x5b, "f32.eq");
    pub const F32_NE: Op = Op(0x5c, "f32.ne");
    pub const F32_LT: Op = Op(0x5d, "f32.lt");
    pub const F32_GT: Op = Op(0x5e, "f32.gt");
    pub const F32_LE: Op = Op(0x5f, "f32.le");
    pub const F32_GE: Op = Op(0x60, "f32.ge");
    pub const F64_EQ: Op = Op(0x61, "f64.eq");
    pub const F64_NE: Op = Op(0x62, "f64.ne");
    pub const F64_LT: Op = Op(0x63, "f64.lt");
    pub const F64_GT: Op = Op(0x64, "f64.gt");
    pub const F64_LE: Op = Op(0x65, "f64.le");
    pub const F64_GE: Op = Op(0x66, "f64.ge");

    // i32 arithmetic.
    pub const I32_CLZ: Op = Op(0x67, "i32.clz");
    pub const I32_CTZ: Op = Op(0x68, "i32.ctz");
    pub const I32_POPCNT: Op = Op(0x69, "i32.popcnt");
    pub const I32_ADD: Op = Op(0x6a, "i32.add");
    pub const I32_SUB: Op = Op(0x6b, "i32.sub");
    pub const I32_MUL: Op = Op(0x6c, "i32.mul");
    pub const I32_DIV_S: Op = Op(0x6d, "i32.div_s");
    pub const I32_DIV_U: Op = Op(0x6e, "i32.div_u");
    pub const I32_REM_S: Op = Op(0x6f, "i32.rem_s");
    pub const I32_REM_U: Op = Op(0x70, "i32.rem_u");
    pub const I32_AND: Op = Op(0x71, "i32.and");
    pub const I32_OR: Op = Op(0x72, "i32.or");
    pub const I32_XOR: Op = Op(0x73, "i32.xor");
    pub const I32_SHL: Op = Op(0x74, "i32.shl");
    pub const I32_SHR_S: Op = Op(0x75, "i32.shr_s");
    pub const I32_SHR_U: Op = Op(0x76, "i32.shr_u");
    pub const I32_ROTL: Op = Op(0x77, "i32.rotl");
    pub const I32_ROTR: Op = Op(0x78, "i32.rotr");

    // i64 arithmetic.
    pub const I64_CLZ: Op = Op(0x79, "i64.clz");
    pub const I64_CTZ: Op = Op(0x7a, "i64.ctz");
    pub const I64_POPCNT: Op = Op(0x7b, "i64.popcnt");
    pub const I64_ADD: Op = Op(0x7c, "i64.add");
    pub const I64_SUB: Op = Op(0x7d, "i64.sub");
    pub const I64_MUL: Op = Op(0x7e, "i64.mul");
    pub const I64_DIV_S: Op = Op(0x7f, "i64.div_s");
    pub const I64_DIV_U: Op = Op(0x80, "i64.div_u");
    pub const I64_REM_S: Op = Op(0x81, "i64.rem_s");
    pub const I64_REM_U: Op = Op(0x82, "i64.rem_u");
    pub const I64_AND: Op = Op(0x83, "i64.and");
    pub const I64_OR: Op = Op(0x84, "i64.or");
    pub const I64_XOR: Op = Op(0x85, "i64.xor");
    pub const I64_SHL: Op = Op(0x86, "i64.shl");
    pub const I64_SHR_S: Op = Op(0x87, "i64.shr_s");
    pub const I64_SHR_U: Op = Op(0x88, "i64.shr_u");
    pub const I64_ROTL: Op = Op(0x89, "i64.rotl");
    pub const I64_ROTR: Op = Op(0x8a, "i64.rotr");

    // f32 arithmetic.
    pub const F32_ABS: Op = Op(0x8b, "f32.abs");
    pub const F32_NEG: Op = Op(0x8c, "f32.neg");
    pub const F32_CEIL: Op = Op(0x8d, "f32.ceil");
    pub const F32_FLOOR: Op = Op(0x8e, "f32.floor");
    pub const F32_TRUNC: Op = Op(0x8f, "f32.trunc");
    pub const F32_NEAREST: Op = Op(0x90, "f32.nearest");
    pub const F32_SQRT: Op = Op(0x91, "f32.sqrt");
    pub const F32_ADD: Op = Op(0x92, "f32.add");
    pub const F32_SUB: Op = Op(0x93, "f32.sub");
    pub const F32_MUL: Op = Op(0x94, "f32.mul");
    pub const F32_DIV: Op = Op(0x95, "f32.div");
    pub const F32_MIN: Op = Op(0x96, "f32.min");
    pub const F32_MAX: Op = Op(0x97, "f32.max");
    pub const F32_COPYSIGN: Op = Op(0x98, "f32.copysign");

    // f64 arithmetic.
    pub const F64_ABS: Op = Op(0x99, "f64.abs");
    pub const F64_NEG: Op = Op(0x9a, "f64.neg");
    pub const F64_CEIL: Op = Op(0x9b, "f64.ceil");
    pub const F64_FLOOR: Op = Op(0x9c, "f64.floor");
    pub const F64_TRUNC: Op = Op(0x9d, "f64.trunc");
    pub const F64_NEAREST: Op = Op(0x9e, "f64.nearest");
    pub const F64_SQRT: Op = Op(0x9f, "f64.sqrt");
    pub const F64_ADD: Op = Op(0xa0, "f64.add");
    pub const F64_SUB: Op = Op(0xa1, "f64.sub");
    pub const F64_MUL: Op = Op(0xa2, "f64.mul");
    pub const F64_DIV: Op = Op(0xa3, "f64.div");
    pub const F64_MIN: Op = Op(0xa4, "f64.min");
    pub const F64_MAX: Op = Op(0xa5, "f64.max");
    pub const F64_COPYSIGN: Op = Op(0xa6, "f64.copysign");

    // Conversions.
    pub const I32_WRAP_I64: Op = Op(0xa7, "i32.wrap_i64");
    pub const I32_TRUNC_F32_S: Op = Op(0xa8, "i32.trunc_f32_s");
    pub const I32_TRUNC_F32_U: Op = Op(0xa9, "i32.trunc_f32_u");
    pub const I32_TRUNC_F64_S: Op = Op(0xaa, "i32.trunc_f64_s");
    pub const I32_TRUNC_F64_U: Op = Op(0xab, "i32.trunc_f64_u");
    pub const I64_EXTEND_I32_S: Op = Op(0xac, "i64.extend_i32_s");
    pub const I64_EXTEND_I32_U: Op = Op(0xad, "i64.extend_i32_u");
    pub const I64_TRUNC_F32_S: Op = Op(0xae, "i64.trunc_f32_s");
    pub const I64_TRUNC_F32_U: Op = Op(0xaf, "i64.trunc_f32_u");
    pub const I64_TRUNC_F64_S: Op = Op(0xb0, "i64.trunc_f64_s");
    pub const I64_TRUNC_F64_U: Op = Op(0xb1, "i64.trunc_f64_u");
    pub const F32_CONVERT_I32_S: Op = Op(0xb2, "f32.convert_i32_s");
    pub const F32_CONVERT_I32_U: Op = Op(0xb3, "f32.convert_i32_u");
    pub const F32_CONVERT_I64_S: Op = Op(0xb4, "f32.convert_i64_s");
    pub const F32_CONVERT_I64_U: Op = Op(0xb5, "f32.convert_i64_u");
    pub const F32_DEMOTE_F64: Op = Op(0xb6, "f32.demote_f64");
    pub const F64_CONVERT_I32_S: Op = Op(0xb7, "f64.convert_i32_s");
    pub const F64_CONVERT_I32_U: Op = Op(0xb8, "f64.convert_i32_u");
    pub const F64_CONVERT_I64_S: Op = Op(0xb9, "f64.convert_i64_s");
    pub const F64_CONVERT_I64_U: Op = Op(0xba, "f64.convert_i64_u");
    pub const F64_PROMOTE_F32: Op = Op(0xbb, "f64.promote_f32");
    pub const I32_REINTERPRET_F32: Op = Op(0xbc, "i32.reinterpret_f32");
    pub const I64_REINTERPRET_F64: Op = Op(0xbd, "i64.reinterpret_f64");
    pub const F32_REINTERPRET_I32: Op = Op(0xbe, "f32.reinterpret_i32");
    pub const F64_REINTERPRET_I64: Op = Op(0xbf, "f64.reinterpret_i64");

    // Sign extension (the sign-extension-ops proposal, in the standard since 2.0).
    pub const I32_EXTEND8_S: Op = Op(0xc0, "i32.extend8_s");
    pub const I32_EXTEND16_S: Op = Op(0xc1, "i32.extend16_s");
    pub const I64_EXTEND8_S: Op = Op(0xc2, "i64.extend8_s");
    pub const I64_EXTEND16_S: Op = Op(0xc3, "i64.extend16_s");
    pub const I64_EXTEND32_S: Op = Op(0xc4, "i64.extend32_s");
}

/// `0xfc`-prefixed opcodes that take no immediates: the saturating truncations.
#[allow(missing_docs)]
pub mod op2 {
    use super::Op2;

    pub const I32_TRUNC_SAT_F32_S: Op2 = Op2(0, "i32.trunc_sat_f32_s");
    pub const I32_TRUNC_SAT_F32_U: Op2 = Op2(1, "i32.trunc_sat_f32_u");
    pub const I32_TRUNC_SAT_F64_S: Op2 = Op2(2, "i32.trunc_sat_f64_s");
    pub const I32_TRUNC_SAT_F64_U: Op2 = Op2(3, "i32.trunc_sat_f64_u");
    pub const I64_TRUNC_SAT_F32_S: Op2 = Op2(4, "i64.trunc_sat_f32_s");
    pub const I64_TRUNC_SAT_F32_U: Op2 = Op2(5, "i64.trunc_sat_f32_u");
    pub const I64_TRUNC_SAT_F64_S: Op2 = Op2(6, "i64.trunc_sat_f64_s");
    pub const I64_TRUNC_SAT_F64_U: Op2 = Op2(7, "i64.trunc_sat_f64_u");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_expression_records_both_bytes_and_mnemonics() {
        let e = Expr::new().local_get(0).local_get(1).op(op::I32_ADD);
        assert_eq!(e.as_bytes(), &[0x20, 0x00, 0x20, 0x01, 0x6a]);
        assert_eq!(e.text(), "local.get 0, local.get 1, i32.add");
        assert_eq!(*e.clone().into_body().last().unwrap_or(&0), 0x0b);
    }

    #[test]
    fn constants_use_the_right_encoding() {
        assert_eq!(Expr::new().i32_const(-1).as_bytes(), &[0x41, 0x7f]);
        assert_eq!(Expr::new().i64_const(-1).as_bytes(), &[0x42, 0x7f]);
        // Floats are four or eight raw little-endian bytes, never a LEB128.
        assert_eq!(
            Expr::new().f32_const(1.0).as_bytes(),
            &[0x43, 0x00, 0x00, 0x80, 0x3f]
        );
        assert_eq!(Expr::new().f64_const(0.0).as_bytes().len(), 9);
    }

    #[test]
    fn blocks_close_themselves() {
        let e = Expr::new().block(BlockType::Empty, Expr::new().br(0));
        assert_eq!(e.as_bytes(), &[0x02, 0x40, 0x0c, 0x00, 0x0b]);
        let e = Expr::new().if_else(
            BlockType::Value(ValType::I32),
            Expr::new().i32_const(1),
            Expr::new().i32_const(2),
        );
        assert_eq!(
            e.as_bytes(),
            &[0x04, 0x7f, 0x41, 0x01, 0x05, 0x41, 0x02, 0x0b]
        );
    }

    #[test]
    fn a_block_type_index_is_a_signed_leb() {
        // 64 would be 0xc0 0x00 as an sLEB, never the single byte 0x40 that means "empty".
        assert_eq!(BlockType::Type(64).encode(), vec![0xc0, 0x00]);
        assert_eq!(BlockType::Type(0).encode(), vec![0x00]);
    }

    #[test]
    fn memory_immediates_are_align_then_offset() {
        let e = Expr::new().mem(op::I32_LOAD, 2, 4);
        assert_eq!(e.as_bytes(), &[0x28, 0x02, 0x04]);
        assert!(e.text().contains("align=4"), "{}", e.text());
    }

    #[test]
    fn bulk_memory_ops_carry_their_prefix() {
        assert_eq!(
            Expr::new().memory_copy().as_bytes(),
            &[0xfc, 0x0a, 0x00, 0x00]
        );
        assert_eq!(Expr::new().memory_fill().as_bytes(), &[0xfc, 0x0b, 0x00]);
        assert_eq!(
            Expr::new().op2(op2::I32_TRUNC_SAT_F64_S).as_bytes(),
            &[0xfc, 0x02]
        );
    }
}
