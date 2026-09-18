//! Stage 40 — Fuzz: seeded mutations of valid objects.
//!
//! Every input here starts life as a perfectly good relocatable object and then has one
//! piece of its *metadata* corrupted: a field of the ELF header, an entry of the section
//! header table, a symbol, a relocation, or the end of the file. The contents of `.text` are
//! never touched — a mutated instruction stream would make a legitimately produced binary
//! crash, and a crashing program is not what this stage is about.
//!
//! There are only three rules, and none of them is about which inputs get refused:
//!
//! 1. the linker is **never killed by a signal** and **never hangs**;
//! 2. when it refuses an input it exits **non-zero** and says something;
//! 3. when it accepts one, the file it wrote still **parses as ELF64**.
//!
//! Which of the two roads a given mutation takes is the linker's business: GNU ld accepts
//! roughly half of them, because half of the fields it is handed are ones it never has to
//! look at. The stage reports the split with [`crate::stages::Ctx::note`] and asserts only
//! the three rules. Everything is driven by `--seed`, so a failing run is reproducible.

use crate::asm::{Code, Reg, STDOUT};
use crate::assert::{Check, Failure, FailureKind};
use crate::elf::read::Elf;
use crate::elf::write::{ObjectBuilder, SectionSpec, SymbolSpec};
use crate::elf::*;
use crate::examples::ExampleSpec;
use crate::exec::signal_name;
use crate::link::{Link, LinkRun};
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Ctx, Stage, Test};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 40,
        slug: "fuzz",
        name: "Fuzz: seeded mutations of valid objects",
        ext: true,
        hints: &[
            "Validate before you trust: every offset, size and index read out of an input has \
             to be checked against the length of the file before it is used to slice it",
            "Use checked arithmetic on everything that came from the file — sh_offset + \
             sh_size overflowing a u64 is a one-line panic and a panic is a signal",
            "Refusing an input is always allowed and accepting a harmless lie is always \
             allowed; being killed, hanging, or exiting 0 with a broken output file never is",
            "Never allocate a buffer of a size an input told you — sh_size can say sixteen \
             exabytes, and the answer is a diagnostic, not a memory request",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "four hundred seeded mutations never crash or hang the linker",
                four_hundred_mutations,
            )
            .ext()
            .tag("slow")
            .min_timeout_ms(120_000),
            Test::new(
                "a refused mutation exits non-zero and says something",
                refusals_are_diagnosed,
            )
            .ext()
            .tag("slow")
            .min_timeout_ms(120_000),
            Test::new(
                "an accepted mutation still produces a file that parses",
                acceptances_still_parse,
            )
            .ext()
            .tag("slow")
            .min_timeout_ms(120_000),
            Test::new("every truncation of a valid object is handled", truncations)
                .ext()
                .tag("slow")
                .min_timeout_ms(120_000),
            Test::new(
                "a corrupt section header table is never fatal to the linker",
                corrupt_section_table,
            )
            .ext()
            .tag("slow")
            .min_timeout_ms(120_000),
            Test::new(
                "a corrupt symbol table or relocation entry is never fatal",
                corrupt_symbols_and_relocations,
            )
            .ext()
            .tag("slow")
            .min_timeout_ms(120_000),
            Test::new(
                "a clean object still links and runs after the fuzz run",
                clean_object_after_the_storm,
            )
            .ext(),
            Test::new(
                "the same seed produces the same sequence of mutations",
                the_seed_is_the_whole_input,
            )
            .ext(),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// The base object every mutation starts from
// ---------------------------------------------------------------------------------------

/// What the unmutated program prints.
const BASE_STDOUT: &str = "fuzz\n";

/// What the unmutated program exits with — the `u32` it loads out of its own `.data`.
const BASE_STATUS: i32 = 29;

/// A small but complete program: `.text` with two relocations, `.rodata`, `.data`, `.bss`,
/// a local, a global, an absolute symbol and a section symbol per section.
///
/// Deliberately ordinary. Everything the fuzzer does is to take this object and change one
/// number in one of its tables.
fn base_object() -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "message", BASE_STDOUT.len() as u32);
    code.mov_r32_rip(Reg::Rax, "value", 0);
    code.sys_exit_eax();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(
                ".rodata",
                BASE_STDOUT.as_bytes().to_vec(),
            ))
            .section(
                SectionSpec::data(".data", (BASE_STATUS as u32).to_le_bytes().to_vec()).align(4),
            )
            .section(SectionSpec::bss(".bss", 64).align(16))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::local("message", ".rodata", 0).object(BASE_STDOUT.len() as u64))
            .symbol(SymbolSpec::local("value", ".data", 0).object(4))
            .symbol(SymbolSpec::global("scratch", ".bss", 0).object(64))
            .symbol(SymbolSpec::absolute("answer", 42)),
    )
}

// ---------------------------------------------------------------------------------------
// The mutator
// ---------------------------------------------------------------------------------------

/// The families of corruption the stage generates. `.text` content is in none of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Cut the file short at a random length.
    Truncate,
    /// Randomize one byte of the 64-byte ELF header.
    HeaderByte,
    /// Replace one named ELF header field with a deliberately bogus value.
    HeaderField,
    /// Randomize one byte of the section header table.
    TableByte,
    /// Replace one field of one section header with a bogus offset, size or index.
    SectionField,
    /// Randomize one byte of `.symtab`, `.strtab`, `.shstrtab` or a `.rela` section.
    MetaByte,
    /// Replace one field of one symbol table entry.
    SymbolField,
    /// Replace one field of one relocation entry.
    RelocField,
    /// Make a section header point at itself — `sh_link` and `sh_info` both its own index.
    Cyclic,
}

/// Every family, for the broad run.
const ALL_KINDS: [Kind; 9] = [
    Kind::Truncate,
    Kind::HeaderByte,
    Kind::HeaderField,
    Kind::TableByte,
    Kind::SectionField,
    Kind::MetaByte,
    Kind::SymbolField,
    Kind::RelocField,
    Kind::Cyclic,
];

/// The families that lie about sections.
const TABLE_KINDS: [Kind; 3] = [Kind::TableByte, Kind::SectionField, Kind::Cyclic];

/// The families that lie about symbols and relocations.
const SYMBOL_KINDS: [Kind; 3] = [Kind::MetaByte, Kind::SymbolField, Kind::RelocField];

/// One mutated input, with the sentence that describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Mutation {
    /// What was changed — printed when the linker misbehaves on this input.
    label: String,
    /// The mutated object.
    bytes: Vec<u8>,
}

/// Where the base object keeps its metadata, so the mutator can aim at it and never at code.
struct Shape {
    /// Length of the pristine object.
    len: usize,
    /// `e_shoff`.
    shoff: usize,
    /// `e_shnum`.
    shnum: usize,
    /// `(file offset, entry count)` of `.symtab`.
    symtab: Option<(usize, usize)>,
    /// `(file offset, entry count)` of every `.rela` section.
    relas: Vec<(usize, usize)>,
    /// `(file offset, length)` of every metadata region: the tables and the string tables.
    meta: Vec<(usize, usize)>,
}

/// Read the base object back with the suite's own parser and note where its tables are.
fn shape(base: &[u8]) -> Result<Shape, Failure> {
    let elf = Elf::parse(base).map_err(|e| {
        Failure::harness(format!(
            "the suite built a base object it cannot parse: {e}"
        ))
    })?;
    let mut meta = Vec::new();
    let mut symtab = None;
    let mut relas = Vec::new();
    for s in &elf.sections {
        let (off, size) = (s.offset as usize, s.size as usize);
        if off + size > base.len() {
            continue;
        }
        match s.sh_type {
            SHT_SYMTAB => {
                symtab = Some((off, size / SYM_SIZE as usize));
                meta.push((off, size));
            }
            SHT_RELA => {
                relas.push((off, size / RELA_SIZE as usize));
                meta.push((off, size));
            }
            SHT_STRTAB => meta.push((off, size)),
            _ => {}
        }
    }
    meta.push((elf.shoff as usize, elf.shnum as usize * SHDR_SIZE as usize));
    meta.retain(|(off, len)| *len > 0 && off + len <= base.len());
    if meta.is_empty() {
        return Err(Failure::harness(
            "the base object has no metadata to mutate, which cannot happen",
        ));
    }
    Ok(Shape {
        len: base.len(),
        shoff: elf.shoff as usize,
        shnum: elf.shnum as usize,
        symtab,
        relas,
        meta,
    })
}

/// Overwrite `value.len()` bytes at `off`, if they are inside the file.
fn put(bytes: &mut [u8], off: usize, value: &[u8]) -> bool {
    match bytes.get_mut(off..off + value.len()) {
        Some(slot) => {
            slot.copy_from_slice(value);
            true
        }
        None => false,
    }
}

/// A number that is wrong in an interesting way: zero, one, a plausible offset, a value just
/// past the end of the file, two gigabytes, four gigabytes, or the whole 64-bit space.
///
/// Deliberately *not* only `u64::MAX`: a linker that special-cases the obvious value and
/// still multiplies the plausible one is the bug this stage is looking for.
fn bogus_u64(rng: &mut StdRng, len: usize) -> u64 {
    let choices = [
        0u64,
        1,
        len as u64,
        len as u64 + 1,
        len as u64 * 4,
        0xffff,
        0x7fff_ffff,
        0xffff_f000,
        0xffff_ffff_ffff_ffff,
    ];
    choices[rng.random_range(0..choices.len())]
}

/// Generate `count` mutations of `base`, drawing only from `kinds`.
///
/// The RNG is seeded from `seed` alone, so the same seed produces the same sequence in the
/// same order — which is what makes a failure reproducible with `--seed`, and what the last
/// test of the stage asserts directly.
fn mutations(
    base: &[u8],
    seed: u64,
    count: usize,
    kinds: &[Kind],
) -> Result<Vec<Mutation>, Failure> {
    let shape = shape(base)?;
    let mut rng = StdRng::seed_from_u64(seed);
    let mut out = Vec::with_capacity(count);
    let mut attempts = 0;
    while out.len() < count {
        attempts += 1;
        if attempts > count * 16 {
            return Err(Failure::harness(
                "the mutator could not produce enough distinct mutations",
            ));
        }
        let kind = kinds[rng.random_range(0..kinds.len())];
        if let Some(m) = one_mutation(base, &shape, kind, &mut rng) {
            out.push(m);
        }
    }
    Ok(out)
}

/// Apply one mutation of `kind`, or `None` when this object has nothing of that sort to
/// corrupt (an object with no relocations, say).
fn one_mutation(base: &[u8], shape: &Shape, kind: Kind, rng: &mut StdRng) -> Option<Mutation> {
    let mut bytes = base.to_vec();
    let label = match kind {
        Kind::Truncate => {
            let at = rng.random_range(1..shape.len);
            bytes.truncate(at);
            format!("truncated to {at} of {} bytes", shape.len)
        }
        Kind::HeaderByte => {
            let at = rng.random_range(0..EHDR_SIZE as usize);
            let v: u8 = rng.random();
            bytes[at] = v;
            format!("ELF header byte {at} set to 0x{v:02x}")
        }
        Kind::HeaderField => {
            // (name, offset, width)
            let fields: [(&str, usize, usize); 8] = [
                ("e_type", 16, 2),
                ("e_machine", 18, 2),
                ("e_version", 20, 4),
                ("e_phoff", 32, 8),
                ("e_shoff", 40, 8),
                ("e_shentsize", 58, 2),
                ("e_shnum", 60, 2),
                ("e_shstrndx", 62, 2),
            ];
            let (name, off, width) = fields[rng.random_range(0..fields.len())];
            let v = bogus_u64(rng, shape.len);
            let raw = v.to_le_bytes();
            put(&mut bytes, off, &raw[..width]);
            format!("{name} set to 0x{:x}", v & mask(width))
        }
        Kind::TableByte => {
            let span = shape.shnum * SHDR_SIZE as usize;
            if span == 0 {
                return None;
            }
            let at = shape.shoff + rng.random_range(0..span);
            let v: u8 = rng.random();
            if !put(&mut bytes, at, &[v]) {
                return None;
            }
            format!("section header table byte {at} set to 0x{v:02x}")
        }
        Kind::SectionField => {
            if shape.shnum == 0 {
                return None;
            }
            let index = rng.random_range(0..shape.shnum);
            // (name, offset inside the header, width)
            let fields: [(&str, usize, usize); 8] = [
                ("sh_type", 4, 4),
                ("sh_flags", 8, 8),
                ("sh_offset", 24, 8),
                ("sh_size", 32, 8),
                ("sh_link", 40, 4),
                ("sh_info", 44, 4),
                ("sh_addralign", 48, 8),
                ("sh_entsize", 56, 8),
            ];
            let (name, off, width) = fields[rng.random_range(0..fields.len())];
            let v = bogus_u64(rng, shape.len);
            let raw = v.to_le_bytes();
            if !put(
                &mut bytes,
                shape.shoff + index * SHDR_SIZE as usize + off,
                &raw[..width],
            ) {
                return None;
            }
            format!("section {index}'s {name} set to 0x{:x}", v & mask(width))
        }
        Kind::MetaByte => {
            let (off, len) = shape.meta[rng.random_range(0..shape.meta.len())];
            let at = off + rng.random_range(0..len);
            let v: u8 = rng.random();
            if !put(&mut bytes, at, &[v]) {
                return None;
            }
            format!("metadata byte {at} set to 0x{v:02x}")
        }
        Kind::SymbolField => {
            let (off, count) = shape.symtab?;
            if count < 2 {
                return None;
            }
            let index = rng.random_range(1..count);
            let fields: [(&str, usize, usize); 5] = [
                ("st_name", 0, 4),
                ("st_info", 4, 1),
                ("st_shndx", 6, 2),
                ("st_value", 8, 8),
                ("st_size", 16, 8),
            ];
            let (name, at, width) = fields[rng.random_range(0..fields.len())];
            let v = bogus_u64(rng, shape.len);
            let raw = v.to_le_bytes();
            if !put(
                &mut bytes,
                off + index * SYM_SIZE as usize + at,
                &raw[..width],
            ) {
                return None;
            }
            format!("symbol {index}'s {name} set to 0x{:x}", v & mask(width))
        }
        Kind::RelocField => {
            if shape.relas.is_empty() {
                return None;
            }
            let (off, count) = shape.relas[rng.random_range(0..shape.relas.len())];
            if count == 0 {
                return None;
            }
            let index = rng.random_range(0..count);
            let base_at = off + index * RELA_SIZE as usize;
            let which = rng.random_range(0..4);
            let (name, at, raw): (&str, usize, [u8; 8]) = match which {
                0 => ("r_offset", 0, bogus_u64(rng, shape.len).to_le_bytes()),
                1 => (
                    "r_info (symbol index)",
                    8,
                    r_info(rng.random_range(1u32..0x0010_0000), R_X86_64_PC32).to_le_bytes(),
                ),
                2 => (
                    "r_info (type)",
                    8,
                    r_info(1, rng.random_range(64u32..256)).to_le_bytes(),
                ),
                _ => ("r_addend", 16, bogus_u64(rng, shape.len).to_le_bytes()),
            };
            if !put(&mut bytes, base_at + at, &raw) {
                return None;
            }
            format!("relocation {index}'s {name} rewritten")
        }
        Kind::Cyclic => {
            if shape.shnum < 2 {
                return None;
            }
            let index = rng.random_range(1..shape.shnum);
            let at = shape.shoff + index * SHDR_SIZE as usize;
            let me = (index as u32).to_le_bytes();
            if !put(&mut bytes, at + 40, &me) || !put(&mut bytes, at + 44, &me) {
                return None;
            }
            format!("section {index}'s sh_link and sh_info both point at section {index}")
        }
    };
    Some(Mutation { label, bytes })
}

/// The low `width` bytes, for printing a truncated field value.
fn mask(width: usize) -> u64 {
    match width {
        1 => 0xff,
        2 => 0xffff,
        4 => 0xffff_ffff,
        _ => u64::MAX,
    }
}

// ---------------------------------------------------------------------------------------
// The three rules, applied to one link
// ---------------------------------------------------------------------------------------

/// What one mutated link did, once the two fatal outcomes have been ruled out.
enum Verdict {
    /// The linker exited 0 and left a file behind.
    Accepted(Vec<u8>),
    /// The linker exited non-zero.
    Refused,
}

/// Run one mutated input and apply rule one: never a signal, never a hang.
fn judge(ctx: &mut Ctx, m: &Mutation, out: &str) -> Result<(Verdict, LinkRun), Failure> {
    let run = ctx.link(
        &Link::new()
            .object("fuzz.o", m.bytes.clone())
            .out(out)
            .label(&m.label),
    )?;
    if run.output.timed_out {
        return Err(run.attach(
            Failure::new(
                FailureKind::Timeout,
                format!("the linker never finished on an object whose {}", m.label),
            )
            .note(
                "a malformed input must produce a diagnostic, not an unbounded loop; a size or \
                 a count read out of the file was probably used as a loop bound",
            ),
        ));
    }
    if let Some(sig) = run.output.signal {
        return Err(run
            .attach(Failure::new(
                FailureKind::LinkerCrash,
                format!(
                    "the linker was killed by signal {sig} ({}) on an object whose {}",
                    signal_name(sig),
                    m.label
                ),
            ))
            .note(
                "refusing this input is fine and accepting it is fine; dying on it is not — \
                 check every offset and size against the length of the file before using it",
            )
            .hex("the mutated object", &m.bytes, &[]));
    }
    let verdict = if run.output.success() {
        Verdict::Accepted(run.produced.clone().unwrap_or_default())
    } else {
        Verdict::Refused
    };
    Ok((verdict, run))
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

link_test!(four_hundred_mutations, |ctx| {
    let base = base_object()?;
    let muts = mutations(&base, ctx.seed, 400, &ALL_KINDS)?;
    let mut accepted = 0usize;
    let mut refused = 0usize;
    for m in &muts {
        match judge(ctx, m, "fz")?.0 {
            Verdict::Accepted(_) => accepted += 1,
            Verdict::Refused => refused += 1,
        }
    }
    ctx.note(format!(
        "400 seeded mutations from seed 0x{:x}: {accepted} accepted, {refused} refused, 0 \
         crashes, 0 hangs",
        ctx.seed
    ));
    ctx.note(
        "which road a mutation takes is the linker's choice — half of these fields are ones a \
         static linker never has to read",
    );
    let mut c = Check::new("that four hundred malformed objects left the linker alive");
    c.eq("linker.mutations_seen", muts.len(), accepted + refused);
    c.finish()
});

link_test!(refusals_are_diagnosed, |ctx| {
    let base = base_object()?;
    let muts = mutations(&base, ctx.seed ^ 0x5151, 120, &ALL_KINDS)?;
    let mut c = Check::new("what the linker says when it refuses a mutated object");
    let mut silent = Vec::new();
    let mut refused = 0usize;
    for m in &muts {
        let (verdict, run) = judge(ctx, m, "fz")?;
        if matches!(verdict, Verdict::Refused) {
            refused += 1;
            if run.output.stderr.trim().is_empty() && run.output.stdout.trim().is_empty() {
                silent.push(m.label.clone());
            }
        }
    }
    c.that(
        "linker.stderr",
        "a diagnostic on every refusal — a bare non-zero exit tells the learner nothing",
        silent.is_empty(),
        format!("{} silent refusals, first few: {:?}", silent.len(), {
            let mut head = silent.clone();
            head.truncate(5);
            head
        }),
    );
    ctx.note(format!(
        "{refused} of {} mutations were refused, every one of them with a message",
        muts.len()
    ));
    c.finish()
});

link_test!(acceptances_still_parse, |ctx| {
    let base = base_object()?;
    let muts = mutations(&base, ctx.seed ^ 0xa1a1, 120, &ALL_KINDS)?;
    let mut c = Check::new("what the linker writes when it accepts a mutated object");
    let mut broken = Vec::new();
    let mut accepted = 0usize;
    for m in &muts {
        if let (Verdict::Accepted(bytes), _) = judge(ctx, m, "fz")? {
            accepted += 1;
            if let Err(e) = Elf::parse(&bytes) {
                broken.push(format!("{}: {e}", m.label));
            }
        }
    }
    c.that(
        "output",
        "every output of a successful link parsing as ELF64",
        broken.is_empty(),
        format!("{} unparsable, first few: {:?}", broken.len(), {
            let mut head = broken.clone();
            head.truncate(3);
            head
        }),
    );
    ctx.note(format!(
        "{accepted} of {} mutations were accepted, and every output was a readable ELF64 file",
        muts.len()
    ));
    c.finish()
});

link_test!(truncations, |ctx| {
    let base = base_object()?;
    let muts = mutations(&base, ctx.seed ^ 0x7c7c, 70, &[Kind::Truncate])?;
    let mut accepted = 0usize;
    let mut broken = Vec::new();
    for m in &muts {
        if let (Verdict::Accepted(bytes), _) = judge(ctx, m, "fz")? {
            accepted += 1;
            if let Err(e) = Elf::parse(&bytes) {
                broken.push(format!("{}: {e}", m.label));
            }
        }
    }
    // Plus the boundaries a random length is unlikely to hit exactly.
    let interesting = [
        1usize,
        4,
        EHDR_SIZE as usize - 1,
        EHDR_SIZE as usize,
        EHDR_SIZE as usize + 1,
        base.len() - 1,
    ];
    for at in interesting {
        let m = Mutation {
            label: format!("truncated to exactly {at} bytes"),
            bytes: base[..at.min(base.len())].to_vec(),
        };
        if let (Verdict::Accepted(bytes), _) = judge(ctx, &m, "fz")? {
            accepted += 1;
            if let Err(e) = Elf::parse(&bytes) {
                broken.push(format!("{}: {e}", m.label));
            }
        }
    }
    let mut c = Check::new("what happens to an object that stops in the middle");
    c.that(
        "output",
        "every accepted truncation still yielding a readable ELF64 file",
        broken.is_empty(),
        format!("{broken:?}"),
    );
    ctx.note(format!(
        "{} truncations, {accepted} of them accepted; a truncated file is the one malformed \
         input a linker meets by accident",
        muts.len() + interesting.len()
    ));
    c.finish()
});

link_test!(corrupt_section_table, |ctx| {
    let base = base_object()?;
    let muts = mutations(&base, ctx.seed ^ 0x3b3b, 80, &TABLE_KINDS)?;
    let mut accepted = 0usize;
    let mut refused = 0usize;
    let mut broken = Vec::new();
    for m in &muts {
        match judge(ctx, m, "fz")?.0 {
            Verdict::Accepted(bytes) => {
                accepted += 1;
                if let Err(e) = Elf::parse(&bytes) {
                    broken.push(format!("{}: {e}", m.label));
                }
            }
            Verdict::Refused => refused += 1,
        }
    }
    let mut c = Check::new("a section header table that lies about itself");
    c.that(
        "output",
        "no accepted lie producing an unreadable output",
        broken.is_empty(),
        format!("{broken:?}"),
    );
    ctx.note(format!(
        "80 section-table mutations (bogus sh_offset, sh_size, sh_link, self-referential \
         sh_link, random bytes): {accepted} accepted, {refused} refused"
    ));
    ctx.note(
        "a sh_size of 0xffffffffffffffff is the one to try by hand: the answer is a \
         diagnostic, never an allocation",
    );
    c.finish()
});

link_test!(corrupt_symbols_and_relocations, |ctx| {
    let base = base_object()?;
    let muts = mutations(&base, ctx.seed ^ 0x9d9d, 80, &SYMBOL_KINDS)?;
    let mut accepted = 0usize;
    let mut refused = 0usize;
    let mut broken = Vec::new();
    for m in &muts {
        match judge(ctx, m, "fz")?.0 {
            Verdict::Accepted(bytes) => {
                accepted += 1;
                if let Err(e) = Elf::parse(&bytes) {
                    broken.push(format!("{}: {e}", m.label));
                }
            }
            Verdict::Refused => refused += 1,
        }
    }
    let mut c = Check::new("a symbol table and a relocation table that lie");
    c.that(
        "output",
        "no accepted lie producing an unreadable output",
        broken.is_empty(),
        format!("{broken:?}"),
    );
    ctx.note(format!(
        "80 symbol and relocation mutations (st_name past the end of .strtab, st_shndx out of \
         range, a relocation against symbol 0x100000, an unknown relocation type): {accepted} \
         accepted, {refused} refused"
    ));
    c.finish()
});

link_test!(clean_object_after_the_storm, |ctx| {
    let base = base_object()?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("clean.o", base)
            .out("clean")
            .label("the pristine object the mutations all started from"),
    )?;
    assert_runnable_layout(&linked)?;
    assert_entry_is(&linked, DEFAULT_ENTRY)?;
    ctx.expect_output(&linked, BASE_STDOUT, BASE_STATUS)?;
    ctx.note(
        "the fuzz corpus is one good object with one number changed; if this test fails the \
         rest of the stage is measuring the wrong thing",
    );
    Ok(())
});

link_test!(the_seed_is_the_whole_input, |ctx| {
    let base = base_object()?;
    let first = mutations(&base, ctx.seed, 64, &ALL_KINDS)?;
    let second = mutations(&base, ctx.seed, 64, &ALL_KINDS)?;
    let other = mutations(&base, ctx.seed ^ 0xffff, 64, &ALL_KINDS)?;

    let mut c = Check::new("that a fuzz run is reproducible from its seed alone");
    c.eq("mutations.len", first.len(), second.len());
    let labels_a: Vec<&str> = first.iter().map(|m| m.label.as_str()).collect();
    let labels_b: Vec<&str> = second.iter().map(|m| m.label.as_str()).collect();
    c.that(
        "mutations.labels",
        "the same seed producing the same sequence of mutations",
        labels_a == labels_b,
        labels_a
            .iter()
            .zip(labels_b.iter())
            .position(|(a, b)| a != b)
            .map(|i| {
                format!(
                    "they diverge at mutation {i}: {} vs {}",
                    labels_a[i], labels_b[i]
                )
            })
            .unwrap_or_else(|| "identical".to_string()),
    );
    for (i, (a, b)) in first.iter().zip(second.iter()).enumerate() {
        if a.bytes != b.bytes {
            c.bytes_eq(&format!("mutations[{i}].bytes"), &a.bytes, &b.bytes);
            break;
        }
    }
    let labels_c: Vec<&str> = other.iter().map(|m| m.label.as_str()).collect();
    c.that(
        "mutations.labels",
        "a different seed producing a different sequence",
        labels_a != labels_c,
        "two different seeds produced the same 64 mutations",
    );
    ctx.note(format!(
        "seed 0x{:x} starts with: {}",
        ctx.seed,
        labels_a
            .iter()
            .take(3)
            .copied()
            .collect::<Vec<&str>>()
            .join("; ")
    ));
    c.finish()
});

/// Worked examples: the pristine object, and one mutation of it.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "The object every mutation starts from",
            "ld -o clean clean.o",
            || base_object().map_err(|f| f.messages.join("; ")),
        )
        .request(
            "clean.o: a .text with a write and a four-byte load, a .rodata string, a .data \
             holding 29, a 64-byte .bss, two PC32 relocations and five symbols including an \
             SHN_ABS one",
        )
        .response(
            "A runnable executable that prints \"fuzz\\n\" and exits 29 — this has to keep \
             working after the fuzz run, or the stage is measuring nothing",
        )
        .note(
            "Every input in this stage is these bytes with one number changed. That is the \
             point: the difference between a linker that says 'sh_offset 0x1f400 is past the \
             end of a 1128-byte file' and one that segfaults is a single bounds check.",
        )
        .runs(BASE_STDOUT, BASE_STATUS),
        ExampleSpec::error(
            "The same object with one section's sh_offset past the end of the file",
            "ld -o out mutated.o",
            || {
                let base = base_object().map_err(|f| f.messages.join("; "))?;
                let shape = shape(&base).map_err(|f| f.messages.join("; "))?;
                let mut bytes = base.clone();
                let at = shape.shoff + SHDR_SIZE as usize + 24;
                put(&mut bytes, at, &(shape.len as u64 * 4).to_le_bytes());
                Ok(bytes)
            },
        )
        .request(
            "The same clean.o with one number changed: section 1's sh_offset now points four \
             file-lengths past the end of the file, so any linker that slices the file at that \
             offset reads bytes that are not there",
        )
        .response(
            "Either a non-zero exit with a diagnostic naming the section, or — if the linker \
             never needed that section's contents — a successful link whose output still \
             parses as ELF64. What is forbidden is a signal, a hang, or an exit status of 0 \
             over a broken file",
        )
        .note(
            "This is the whole stage in one input. There is no single right answer, only \
             three wrong ones.",
        ),
    ]
}
