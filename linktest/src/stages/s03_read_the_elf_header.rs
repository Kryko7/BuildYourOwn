//! Stage 03 — Read the ELF header of an input.
//!
//! Sixty-four bytes, of which a static x86-64 linker uses four: `e_shoff`, `e_shentsize`,
//! `e_shnum` and `e_shstrndx`. The rest it has already checked (stage 02) or must
//! deliberately ignore — `EI_OSABI`, `EI_ABIVERSION`, the seven reserved padding bytes,
//! `e_flags`, and above all `e_entry` and `e_phoff`, which mean nothing in a relocatable
//! object and would be a disaster to believe.

use crate::assert::{Check, Failure};
use crate::elf::read::Elf;
use crate::elf::write::{HeaderOverrides, ObjectBuilder, SectionSpec, SymbolSpec};
use crate::elf::*;
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 3,
        slug: "read_the_elf_header",
        name: "Read the ELF header of an input",
        ext: false,
        hints: &[
            "Take e_shoff, e_shentsize and e_shnum out of the header and walk the section \
             header table from there: the table is wherever e_shoff says, not at a fixed \
             offset such as 0x40",
            "EI_OSABI, EI_ABIVERSION, the seven reserved e_ident padding bytes and e_flags \
             are all information an x86-64 static linker ignores; refusing them means \
             refusing real compiler output",
            "A relocatable object's e_entry and e_phoff are meaningless — the executable's \
             entry comes from the entry symbol you resolve, and its program headers are ones \
             you write yourself",
            "The table is exactly e_shnum entries of e_shentsize bytes; whatever follows it \
             in the file is not yours to read",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "the section header table is found through e_shoff, wherever it sits",
                shoff_is_authoritative,
            ),
            Test::new(
                "e_shnum bounds the walk: bytes after the table are never read",
                shnum_bounds_the_walk,
            ),
            Test::new(
                "a GNU/Linux EI_OSABI and a non-zero EI_ABIVERSION are still linkable",
                osabi_is_ignored,
            ),
            Test::new(
                "a non-zero e_flags is not a reason to refuse",
                e_flags_is_ignored,
            ),
            Test::new(
                "a relocatable object's e_entry never becomes the executable's entry",
                input_e_entry_is_meaningless,
            ),
            Test::new(
                "a relocatable object has no program headers and the executable gets its own",
                phnum_is_zero_on_the_way_in,
            ),
            Test::new(
                "the reserved e_ident padding bytes are ignored",
                ident_padding_is_reserved,
            ),
            Test::new(
                "an object with every ignorable header field set at once still runs",
                everything_ignorable_at_once,
            ),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------

/// What the stage's program prints.
const MESSAGE: &str = "header read\n";

/// The stage's one program, under a header the test may corrupt.
fn program(header: HeaderOverrides) -> Result<Vec<u8>, Failure> {
    let mut code = crate::asm::Code::new();
    code.sys_write(crate::asm::STDOUT, "message", MESSAGE.len() as u32);
    code.sys_exit(0);
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", MESSAGE.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::local("message", ".rodata", 0).object(MESSAGE.len() as u64))
            .header(header),
    )
}

/// Overwrite `len` bytes at `at`. Fails as a harness error rather than panicking, because a
/// fixture that does not fit its own file is the suite's bug, not the linker's.
fn poke(bytes: &mut [u8], at: usize, value: &[u8]) -> Result<(), Failure> {
    let end = at + value.len();
    if end > bytes.len() {
        return Err(Failure::harness(format!(
            "the fixture wanted to patch {} bytes at offset {at} of a {}-byte object",
            value.len(),
            bytes.len()
        )));
    }
    bytes[at..end].copy_from_slice(value);
    Ok(())
}

/// Move the section header table to the end of the file and fill the place it used to
/// occupy with 0xcc, so that a linker which assumes "the table is at the end" — or worse,
/// "the table is at 0x40" — reads rubbish instead.
fn move_section_header_table(mut bytes: Vec<u8>) -> Result<Vec<u8>, Failure> {
    let elf = parse_fixture(&bytes)?;
    let at = elf.shoff as usize;
    let size = elf.shnum as usize * elf.shentsize as usize;
    if at + size > bytes.len() {
        return Err(Failure::harness(
            "the fixture's own section header table does not fit in the fixture",
        ));
    }
    let table = bytes[at..at + size].to_vec();
    poke(&mut bytes, at, &vec![0xcc; size])?;
    bytes.extend_from_slice(&[0xcc; 24]);
    while !bytes.len().is_multiple_of(8) {
        bytes.push(0xcc);
    }
    let moved = bytes.len() as u64;
    bytes.extend_from_slice(&table);
    poke(&mut bytes, 0x28, &moved.to_le_bytes())?;
    Ok(bytes)
}

/// Parse a fixture the suite itself built; a parse error here is a harness bug.
fn parse_fixture(bytes: &[u8]) -> Result<Elf, Failure> {
    Elf::parse(bytes).map_err(|e| {
        Failure::harness(format!(
            "the suite built an object it cannot itself read: {e}"
        ))
    })
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

link_test!(shoff_is_authoritative, |ctx| {
    let obj = move_section_header_table(program(HeaderOverrides::default())?)?;
    let moved = parse_fixture(&obj)?;
    let mut c = Check::new("the fixture the linker is about to be given");
    c.at_least("input.e_shoff", 0x40u64, moved.shoff);
    c.that(
        "input.section_header_table",
        "the table sits at the very end of the file, after 0xcc filler where it used to be",
        moved.shoff as usize + moved.shnum as usize * 64 == obj.len(),
        format!(
            "shoff 0x{:x}, {} entries, file {} bytes",
            moved.shoff,
            moved.shnum,
            obj.len()
        ),
    );
    c.finish()?;

    let linked = ctx.link_ok(
        &Link::new()
            .object("moved.o", obj)
            .label("a moved section header table"),
    )?;
    assert_runnable_layout(&linked)?;
    assert_entry_is(&linked, DEFAULT_ENTRY)?;
    ctx.expect_output(&linked, MESSAGE, 0)?;
    ctx.note(
        "the bytes the table used to occupy are 0xcc, so a linker that reads the table from \
         a remembered offset instead of e_shoff cannot pass this",
    );
    Ok(())
});

link_test!(shnum_bounds_the_walk, |ctx| {
    // The writer puts the section header table last, so 64 bytes of junk appended to the
    // file look exactly like one more section header — to anyone who does not stop at
    // e_shnum entries.
    let mut obj = program(HeaderOverrides::default())?;
    let before = parse_fixture(&obj)?;
    obj.extend_from_slice(&[0xcc; SHDR_SIZE as usize]);
    let after = parse_fixture(&obj)?;
    let mut c = Check::new("the fixture: one section header's worth of junk after the table");
    c.eq("input.e_shnum", before.shnum, after.shnum);
    c.eq(
        "input.sections.len",
        before.sections.len(),
        after.sections.len(),
    );
    c.finish()?;

    let linked = ctx.link_ok(
        &Link::new()
            .object("trailing.o", obj)
            .label("junk after the table"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, MESSAGE, 0)?;
    Ok(())
});

link_test!(osabi_is_ignored, |ctx| {
    let mut obj = program(HeaderOverrides {
        osabi: Some(3),       // ELFOSABI_GNU: what a Linux toolchain writes as soon as it uses
        ..Default::default()  // an STT_GNU_IFUNC or a unique global symbol
    })?;
    poke(&mut obj, 8, &[7])?; // EI_ABIVERSION
    let parsed = parse_fixture(&obj)?;
    let mut c = Check::new("the fixture's e_ident");
    c.eq("input.e_ident[EI_OSABI]", 3u8, parsed.ident[7]);
    c.eq("input.e_ident[EI_ABIVERSION]", 7u8, parsed.ident[8]);
    c.finish()?;

    let linked = ctx.link_ok(&Link::new().object("gnu.o", obj).label("an EI_OSABI of 3"))?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, MESSAGE, 0)?;
    ctx.note(
        "EI_OSABI 3 (GNU/Linux) and a non-zero EI_ABIVERSION are ordinary in objects gcc \
         produces; GNU ld links them without a word",
    );
    Ok(())
});

link_test!(e_flags_is_ignored, |ctx| {
    // e_flags is processor-specific and unused on x86-64: there are no defined flags, so
    // whatever is in it must not change what the linker does.
    let mut obj = program(HeaderOverrides::default())?;
    poke(&mut obj, 0x30, &0x1234_5678u32.to_le_bytes())?;
    let parsed = parse_fixture(&obj)?;
    let mut c = Check::new("the fixture's e_flags");
    c.eq("input.e_flags", 0x1234_5678u32, parsed.e_flags);
    c.finish()?;

    let linked = ctx.link_ok(
        &Link::new()
            .object("flags.o", obj)
            .label("a non-zero e_flags"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, MESSAGE, 0)?;
    Ok(())
});

link_test!(input_e_entry_is_meaningless, |ctx| {
    const LIE: u64 = 0xdead_beef;
    let obj = program(HeaderOverrides {
        e_entry: Some(LIE),
        ..Default::default()
    })?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("entry.o", obj)
            .label("an input with e_entry set"),
    )?;
    assert_runnable_layout(&linked)?;
    assert_entry_is(&linked, DEFAULT_ENTRY)?;
    let mut c = Check::new("where the executable's entry point came from");
    c.ne("output.e_entry", LIE, linked.elf.entry);
    if !c.ok() {
        c.note(
            "the input's e_entry is not an address the linker may use: it is not even an address",
        );
        c.block("output program headers", linked.elf.program_header_table());
    }
    c.finish()?;
    ctx.expect_output(&linked, MESSAGE, 0)?;
    Ok(())
});

link_test!(phnum_is_zero_on_the_way_in, |ctx| {
    let obj = program(HeaderOverrides::default())?;
    let parsed = parse_fixture(&obj)?;
    let mut c = Check::new("a relocatable object's program header fields");
    c.eq("input.e_phnum", 0u16, parsed.phnum);
    c.eq("input.e_phoff", 0u64, parsed.phoff);
    c.eq("input.e_type", ET_REL, parsed.e_type);
    c.finish()?;

    let linked = ctx.link_ok(&Link::new().object("a.o", obj))?;
    let mut c = Check::new("the program headers the linker wrote for itself");
    c.at_least("output.e_phnum", 1u16, linked.elf.phnum);
    c.eq("output.e_phentsize", PHDR_SIZE, linked.elf.phentsize);
    c.at_least("output.e_phoff", EHDR_SIZE as u64, linked.elf.phoff);
    if !c.ok() {
        c.block("output program headers", linked.elf.program_header_table());
    }
    c.finish()?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, MESSAGE, 0)?;
    Ok(())
});

link_test!(ident_padding_is_reserved, |ctx| {
    let mut obj = program(HeaderOverrides::default())?;
    poke(&mut obj, 9, &[0xaa; 7])?; // EI_PAD .. EI_NIDENT
    let linked = ctx.link_ok(
        &Link::new()
            .object("pad.o", obj)
            .label("non-zero e_ident padding"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, MESSAGE, 0)?;
    ctx.note(
        "the ABI says the padding is reserved and set to zero, and also that a reader must \
         ignore it — the second half is the one a linker has to implement",
    );
    Ok(())
});

link_test!(everything_ignorable_at_once, |ctx| {
    let mut obj = program(HeaderOverrides {
        osabi: Some(3),
        e_entry: Some(0x4000_0000),
        ..Default::default()
    })?;
    poke(&mut obj, 8, &[7])?; // EI_ABIVERSION
    poke(&mut obj, 9, &[0xaa; 7])?; // EI_PAD
    poke(&mut obj, 0x20, &(EHDR_SIZE as u64).to_le_bytes())?; // e_phoff, with e_phnum still 0
    poke(&mut obj, 0x30, &3u32.to_le_bytes())?; // e_flags
    let obj = move_section_header_table(obj)?;

    let linked = ctx.link_ok(
        &Link::new()
            .object("odd-header.o", obj)
            .label("every ignorable header field at once"),
    )?;
    assert_runnable_layout(&linked)?;
    assert_entry_is(&linked, DEFAULT_ENTRY)?;
    ctx.expect_output(&linked, MESSAGE, 0)?;
    ctx.note(
        "e_phoff is non-zero while e_phnum is 0: there is no program header table to read, \
         and GNU ld does not look at either field of an ET_REL input",
    );
    Ok(())
});

// ---------------------------------------------------------------------------------------
// Examples
// ---------------------------------------------------------------------------------------

/// The fixture whose header is full of fields a linker must ignore.
fn example_ignorable() -> Result<Vec<u8>, String> {
    let mut obj = program(HeaderOverrides {
        osabi: Some(3),
        e_entry: Some(0x4000_0000),
        ..Default::default()
    })
    .map_err(|f| f.messages.join("; "))?;
    poke(&mut obj, 8, &[7]).map_err(|f| f.messages.join("; "))?;
    poke(&mut obj, 9, &[0xaa; 7]).map_err(|f| f.messages.join("; "))?;
    poke(&mut obj, 0x30, &3u32.to_le_bytes()).map_err(|f| f.messages.join("; "))?;
    Ok(obj)
}

/// The fixture whose section header table has been moved.
fn example_moved_table() -> Result<Vec<u8>, String> {
    let obj = program(HeaderOverrides::default()).map_err(|f| f.messages.join("; "))?;
    move_section_header_table(obj).map_err(|f| f.messages.join("; "))
}

/// Worked examples: the fields to ignore, and the one field to believe.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "A header full of fields to ignore",
            "ld -o prog odd-header.o",
            example_ignorable,
        )
        .request(
            "odd-header.o: EI_OSABI 3, EI_ABIVERSION 7, seven padding bytes of 0xaa, e_flags \
             0x3 and e_entry 0x40000000 — on an otherwise ordinary object that prints a \
             string and exits 0",
        )
        .response(
            "An executable that prints `header read` and exits 0, whose e_entry is the \
             address the linker gave `_start` — never 0x40000000, which was only ever a \
             number in an input that has no entry point",
        )
        .note(
            "Believing an input's e_entry produces a binary that jumps to an unmapped \
             address and dies with SIGSEGV before printing anything.",
        )
        .runs("header read\n", 0),
        ExampleSpec::object(
            "A section header table that is not where you expect",
            "ld -o prog moved.o",
            example_moved_table,
        )
        .request(
            "moved.o: the section header table has been moved to the end of the file and the \
             bytes it used to occupy overwritten with 0xcc; e_shoff points at the new place",
        )
        .response(
            "The same executable as for the untouched object. e_shoff, e_shentsize and \
             e_shnum are the only way to find the table, and all three are in the header",
        )
        .note(
            "Real toolchains do put the table last, which is exactly why a linker that \
             hard-codes the assumption passes every test until it meets a linker script or \
             an object built by something else.",
        )
        .runs("header read\n", 0),
    ]
}
