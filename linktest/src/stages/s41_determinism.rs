//! Stage 41 — Determinism.
//!
//! The same inputs, linked again, must give the same bytes. No standard says so: an ELF file
//! is a set of constraints, and a linker that padded with the time of day would still be
//! producing conforming executables. Every build system on earth says so instead — `make`,
//! `ninja`, `ccache`, content-addressed caches, reproducible-build policies and the plain
//! human question "did my change do anything?" all rest on it, and GNU ld has given it for
//! decades.
//!
//! So this stage asserts byte-identity, and where a linker does not have it that is a real
//! finding, reported as the byte that differs. The usual causes are all the same shape:
//! something that is not part of the input leaked into the output — a hash map iterated in
//! address order, a timestamp, an uninitialised padding buffer, the name of the output file,
//! the path an input happened to be read from.

use crate::asm::{Code, Reg, STDOUT};
use crate::assert::{Check, Failure};
use crate::elf::archive::{ArchiveBuilder, IndexMode, Member};
use crate::elf::read::Elf;
use crate::elf::write::{ObjectBuilder, SectionSpec, SymbolSpec};
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 41,
        slug: "determinism",
        name: "Determinism",
        ext: true,
        hints: &[
            "Iterate over ordered containers only: a hash map walked in whatever order it \
             happens to be in is the single most common source of a non-reproducible linker",
            "Zero every byte of padding you write — an uninitialised buffer is a different \
             output every run and a security hole in the same breath",
            "Nothing that is not in the inputs may reach the output: not the time, not the \
             output file's name, not the path an input was read from, not a process id",
            "Truncate the output file when you open it; a relink over a longer previous \
             output must not leave the old tail behind",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "the same inputs linked twice are byte-identical",
                twice_is_identical,
            )
            .ext(),
            Test::new("five links in a row are all identical", five_in_a_row).ext(),
            Test::new(
                "the path the inputs were read from does not reach the output",
                input_path_does_not_leak,
            )
            .ext(),
            Test::new(
                "the output file name does not leak into the bytes",
                output_name_does_not_leak,
            )
            .ext(),
            Test::new(
                "a relink over an existing output file overwrites it cleanly",
                relink_overwrites,
            )
            .ext(),
            Test::new(
                "a program with .text, .rodata, .data and .bss is identical too",
                a_richer_program_is_identical,
            )
            .ext(),
            Test::new("an archive input links byte-identically too", archives_too).ext(),
            Test::new(
                "every symbol lands at the same address in both links",
                addresses_agree,
            )
            .ext(),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------

/// What the two-object program prints.
const MESSAGE: &str = "determinism\n";

/// What it exits with — the value the callee returns.
const STATUS: i32 = 12;

/// The caller and the callee of the two-object program, in command-line order.
fn duo() -> Result<Vec<(&'static str, Vec<u8>)>, Failure> {
    Ok(vec![
        ("main.o", caller(MESSAGE, "other")?),
        ("other.o", callee_returning("other", STATUS as u32)?),
    ])
}

/// A third object with initialised data and a `.bss`, so the richer link has every kind of
/// allocated section and a writable segment to lay out.
fn data_object() -> Result<Vec<u8>, Failure> {
    build(
        ObjectBuilder::new()
            .section(SectionSpec::data(".data", vec![0x11; 24]).align(8))
            .section(SectionSpec::bss(".bss", 512).align(32))
            .section(SectionSpec::rodata(".rodata", b"table\n".to_vec()))
            .symbol(SymbolSpec::global("table", ".data", 0).object(24))
            .symbol(SymbolSpec::global("slot", ".bss", 0).object(512))
            .symbol(SymbolSpec::local("table_name", ".rodata", 0).object(6)),
    )
}

/// A program that prints a string and exits with a value it loads out of `.data`, used where
/// the test needs one self-contained object.
fn solo_object(status: u32) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "message", MESSAGE.len() as u32);
    code.mov_r32_rip(Reg::Rax, "value", 0);
    code.sys_exit_eax();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", MESSAGE.as_bytes().to_vec()))
            .section(SectionSpec::data(".data", status.to_le_bytes().to_vec()).align(4))
            .section(SectionSpec::bss(".bss", 128).align(16))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::local("message", ".rodata", 0).object(MESSAGE.len() as u64))
            .symbol(SymbolSpec::local("value", ".data", 0).object(4))
            .symbol(SymbolSpec::global("pad", ".bss", 0).object(128)),
    )
}

/// The failure a differing relink produces: which byte, and the likely cause.
fn difference(c: &mut Check, path: &str, first: &[u8], second: &[u8]) {
    c.bytes_eq(path, first, second);
    if first != second {
        let at = first
            .iter()
            .zip(second.iter())
            .position(|(a, b)| a != b)
            .unwrap_or_else(|| first.len().min(second.len()));
        c.note(format!(
            "the two outputs first differ at byte 0x{at:x} ({at}); look for a hash map walked \
             in an unordered way, a padding buffer that was never zeroed, or a timestamp"
        ));
        c.note(
            "byte-identity is not required by the ELF standard — it is required by every \
             build system, and GNU ld has it",
        );
    }
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

link_test!(twice_is_identical, |ctx| {
    let objects = duo()?;
    let first = ctx.link_ok(
        &Link::new()
            .object(objects[0].0, objects[0].1.clone())
            .object(objects[1].0, objects[1].1.clone())
            .out("prog")
            .label("the first link"),
    )?;
    let second = ctx.link_ok(
        &Link::new()
            .object(objects[0].0, objects[0].1.clone())
            .object(objects[1].0, objects[1].1.clone())
            .out("prog")
            .label("the second link of the same inputs"),
    )?;
    ctx.expect_output(&first, MESSAGE, STATUS)?;
    ctx.expect_output(&second, MESSAGE, STATUS)?;

    let mut c = Check::new("two links of the same two objects");
    difference(&mut c, "output.bytes", &first.bytes, &second.bytes);
    c.finish()?;
    ctx.note(format!("{} bytes, identical both times", first.bytes.len()));
    Ok(())
});

link_test!(five_in_a_row, |ctx| {
    let objects = duo()?;
    let mut outputs: Vec<Vec<u8>> = Vec::new();
    for i in 0..5 {
        let linked = ctx.link_ok(
            &Link::new()
                .object(objects[0].0, objects[0].1.clone())
                .object(objects[1].0, objects[1].1.clone())
                .out("prog")
                .label(&format!("link number {}", i + 1)),
        )?;
        outputs.push(linked.bytes.clone());
    }
    let mut c = Check::new("five consecutive links of the same inputs");
    for (i, bytes) in outputs.iter().enumerate().skip(1) {
        difference(
            &mut c,
            &format!("output.bytes (link {})", i + 1),
            &outputs[0],
            bytes,
        );
        if !c.ok() {
            break;
        }
    }
    c.note(
        "a linker that is right four times out of five is not deterministic; the one run that \
         differs is the one that breaks a build cache",
    );
    c.finish()
});

link_test!(input_path_does_not_leak, |ctx| {
    let objects = duo()?;
    let flat = ctx.link_ok(
        &Link::new()
            .object("main.o", objects[0].1.clone())
            .object("other.o", objects[1].1.clone())
            .out("prog")
            .label("inputs in the link's own directory"),
    )?;
    let nested = ctx.link_ok(
        &Link::new()
            .object("build/x86_64/objects/main.o", objects[0].1.clone())
            .object("build/x86_64/objects/other.o", objects[1].1.clone())
            .out("prog")
            .label("the same inputs three directories down"),
    )?;
    ctx.expect_output(&nested, MESSAGE, STATUS)?;

    let mut c = Check::new("the same objects read from two different paths");
    difference(&mut c, "output.bytes", &flat.bytes, &nested.bytes);
    for needle in ["build/x86_64/objects", "x86_64/objects/main.o"] {
        c.that(
            "output.bytes",
            &format!("no trace of the input path '{needle}'"),
            !contains(&nested.bytes, needle.as_bytes()),
            needle,
        );
    }
    c.note(
        "a linker that records the input's file name — in a STT_FILE symbol, a .comment, or a \
         debug section — makes every build directory produce different binaries",
    );
    c.finish()
});

link_test!(output_name_does_not_leak, |ctx| {
    let obj = solo_object(STATUS as u32)?;
    let short = ctx.link_ok(
        &Link::new()
            .object("a.o", obj.clone())
            .out("p")
            .label("output named 'p'"),
    )?;
    let long_name = "a_considerably_longer_output_file_name";
    let long = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .out(long_name)
            .label("the same link, a much longer output name"),
    )?;
    ctx.expect_output(&long, MESSAGE, STATUS)?;

    let mut c = Check::new("the same link written under two different names");
    difference(&mut c, "output.bytes", &short.bytes, &long.bytes);
    c.that(
        "output.bytes",
        "no trace of the output file's own name",
        !contains(&long.bytes, long_name.as_bytes()),
        long_name,
    );
    c.finish()
});

link_test!(relink_overwrites, |ctx| {
    let obj = solo_object(STATUS as u32)?;
    let clean = ctx.link_ok(
        &Link::new()
            .object("a.o", obj.clone())
            .out("prog")
            .label("a link into a directory with no output file yet"),
    )?;
    // The same link, but something much larger is already sitting at the output path.
    let stale = vec![0xaau8; clean.bytes.len() * 2 + 4096];
    let stale_len = stale.len();
    let over = ctx.link_ok(
        &Link::new()
            .write_file("prog", stale)
            .object("a.o", obj)
            .out("prog")
            .label("a relink over a stale, larger output file"),
    )?;
    ctx.expect_output(&over, MESSAGE, STATUS)?;

    let mut c = Check::new("what is left at the output path after a relink");
    c.eq("output.size", clean.bytes.len(), over.bytes.len());
    difference(&mut c, "output.bytes", &clean.bytes, &over.bytes);
    c.that(
        "output.bytes",
        "no tail of the file that was there before",
        !over.bytes.windows(64).any(|w| w.iter().all(|b| *b == 0xaa)),
        format!("the stale file was {stale_len} bytes of 0xaa"),
    );
    c.note(
        "open the output with O_TRUNC, or write it to a temporary file and rename it; a \
         linker that only writes as far as it needs leaves the old tail behind",
    );
    c.finish()
});

link_test!(a_richer_program_is_identical, |ctx| {
    let objects = duo()?;
    let data = data_object()?;
    let build_link = |label: &str| {
        Link::new()
            .object("main.o", objects[0].1.clone())
            .object("other.o", objects[1].1.clone())
            .object("data.o", data.clone())
            .out("prog")
            .label(label)
    };
    let first = ctx.link_ok(&build_link("the first link of three objects"))?;
    let second = ctx.link_ok(&build_link("the second link of three objects"))?;
    ctx.expect_output(&first, MESSAGE, STATUS)?;

    let mut c = Check::new("two links of a program with every kind of allocated section");
    difference(&mut c, "output.bytes", &first.bytes, &second.bytes);
    c.that(
        "output.sections",
        "a .data and a .bss in the output, so the writable segment was laid out too",
        first.elf.section(".data").is_some() && first.elf.section(".bss").is_some(),
        first
            .elf
            .sections
            .iter()
            .map(|s| s.name.clone())
            .collect::<Vec<String>>()
            .join(" "),
    );
    c.finish()
});

link_test!(archives_too, |ctx| {
    let objects = duo()?;
    let archive = ArchiveBuilder::new()
        .member(Member::new("other.o", objects[1].1.clone(), &["other"]))
        .member(Member::new(
            "unused.o",
            callee_returning("unused", 1)?,
            &["unused"],
        ))
        .index(IndexMode::Correct)
        .build()
        .map_err(|e| Failure::harness(format!("the suite could not build an archive: {e}")))?;

    let build_link = |label: &str| {
        Link::new()
            .object("main.o", objects[0].1.clone())
            .archive("libother.a", archive.clone())
            .out("prog")
            .label(label)
    };
    let first = ctx.link_ok(&build_link("the first link against the archive"))?;
    let second = ctx.link_ok(&build_link("the second link against the archive"))?;
    ctx.expect_output(&first, MESSAGE, STATUS)?;

    let mut c = Check::new("two links that pull the same member out of the same archive");
    difference(&mut c, "output.bytes", &first.bytes, &second.bytes);
    c.that(
        "output.symtab[unused]",
        "the member nothing needed staying out of the output",
        first.elf.symbol("unused").is_none(),
        "the unused member was pulled in",
    );
    c.note(
        "an archive is where member order, the symbol index's order and the order members are \
         pulled in all become visible; all three have to be decided by the input, not by a \
         hash map",
    );
    c.finish()
});

link_test!(addresses_agree, |ctx| {
    let objects = duo()?;
    let data = data_object()?;
    let build_link = |label: &str| {
        Link::new()
            .object("main.o", objects[0].1.clone())
            .object("other.o", objects[1].1.clone())
            .object("data.o", data.clone())
            .out("prog")
            .label(label)
    };
    let first = ctx.link_ok(&build_link("the first link"))?;
    let second = ctx.link_ok(&build_link("the second link"))?;

    let mut c = Check::new("the addresses two links of the same inputs chose");
    c.addr_eq("output.e_entry", first.elf.entry, second.elf.entry);
    for name in [DEFAULT_ENTRY, "other", "table", "slot"] {
        match (first.elf.symbol(name), second.elf.symbol(name)) {
            (Some(a), Some(b)) => {
                c.addr_eq(&format!("output.symtab[{name}].st_value"), a.value, b.value);
            }
            (a, b) => {
                c.that(
                    &format!("output.symtab[{name}]"),
                    "present in both links",
                    a.is_some() && b.is_some(),
                    format!("first: {}, second: {}", a.is_some(), b.is_some()),
                );
            }
        }
    }
    let shape = |e: &Elf| {
        e.sections
            .iter()
            .map(|s| format!("{}@0x{:x}+0x{:x}", s.name, s.addr, s.size))
            .collect::<Vec<String>>()
    };
    c.that(
        "output.sections",
        "the same sections at the same addresses with the same sizes",
        shape(&first.elf) == shape(&second.elf),
        format!("{:?} vs {:?}", shape(&first.elf), shape(&second.elf)),
    );
    c.note(
        "this is the weaker half of byte-identity, and the half that usually fails first: if \
         the addresses already differ, the layout pass is the thing that is not deterministic",
    );
    c.finish()
});

/// True when `haystack` contains `needle`.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack.windows(needle.len()).any(|w| w == needle)
}

/// Worked examples: the two objects the whole stage relinks.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "The caller half of the link that has to come out the same twice",
            "ld -o prog main.o other.o   # then again, and diff the two",
            || caller(MESSAGE, "other").map_err(|f| f.messages.join("; ")),
        )
        .request(
            "main.o: a .rodata string, a .text that writes it, calls the undefined 'other' \
             through an R_X86_64_PLT32 and exits with what comes back in eax",
        )
        .response(
            "The same executable, byte for byte, however many times it is linked, from \
             whatever directory, under whatever output name",
        )
        .note(
            "Nothing in the ELF standard requires this. Every build system does. The cheapest \
             way to have it is to never iterate a hash map and always zero your padding.",
        )
        .runs(MESSAGE, STATUS),
        ExampleSpec::text(
            "A relink over a longer file that is already there",
            "ld -o prog a.o   # where prog already exists and is twice as long",
        )
        .request(
            "The same single object, linked to an output path where a 20 KiB file of 0xaa \
             bytes is already sitting",
        )
        .response(
            "An output file exactly as long as a link into an empty directory would have \
             produced, with no trace of the bytes that were there before",
        )
        .note(
            "O_WRONLY|O_CREAT without O_TRUNC is the bug, and it produces an executable that \
             runs perfectly — until the day the old tail is longer than the new file.",
        ),
    ]
}
