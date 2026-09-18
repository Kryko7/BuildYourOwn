//! Stage 05 — A section header table that lies.
//!
//! Every field of a section header is a number somebody else wrote. `e_shoff` can point past
//! the end of the file, `e_shnum` can claim sixty-five thousand sections, `sh_offset` and
//! `sh_size` can describe a region that does not exist, `e_shentsize` can say a header is 32
//! bytes, `e_shstrndx` can name a section that is not there, and `.symtab`'s `sh_link` can
//! point at `.text`. None of that may crash the linker, hang it, or produce an executable
//! that is quietly wrong: refuse the input, or tolerate it and still be right.

use crate::asm::{Code, STDOUT};
use crate::assert::{Check, Failure};
use crate::elf::read::Elf;
use crate::elf::write::{HeaderOverrides, ObjectBuilder, SectionSpec, SymbolSpec};
use crate::elf::*;
use crate::examples::ExampleSpec;
use crate::link::{Link, LinkRun, Linked};
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Ctx, Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 5,
        slug: "section_header_table",
        name: "A section header table that lies",
        ext: true,
        hints: &[
            "Bounds-check before you dereference: e_shoff + e_shnum * e_shentsize must fit \
             in the file, and so must sh_offset + sh_size of every section that occupies \
             file space",
            "An ELF64 section header is exactly 64 bytes, so an e_shentsize that says \
             anything else means this is not a file you can read",
            "Check that .symtab's sh_link is in range and really names a SHT_STRTAB before \
             reading any symbol name through it",
            "Refusing is right and tolerating can be right, but crashing, hanging and \
             silently writing a broken executable never are",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "an e_shoff past the end of the file is refused",
                shoff_past_end,
            )
            .ext(),
            Test::new("an absurd e_shnum is refused", shnum_absurd).ext(),
            Test::new(
                "a section whose sh_offset is past the end of the file is refused",
                sh_offset_past_end,
            )
            .ext(),
            Test::new(
                "a section whose sh_size is absurd is refused",
                sh_size_absurd,
            )
            .ext(),
            Test::new("an e_shentsize of 32 is refused", shentsize_32).ext(),
            Test::new(
                "an e_shstrndx out of range is never silently accepted",
                shstrndx_out_of_range,
            )
            .ext(),
            Test::new(
                "a .symtab whose sh_link is not a string table is refused",
                symtab_link_not_a_strtab,
            )
            .ext(),
            Test::new(
                "a .symtab whose sh_link is out of range is refused",
                symtab_link_out_of_range,
            )
            .ext(),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------

/// What the stage's program prints when it is allowed to run at all.
const MESSAGE: &str = "table\n";

/// The stage's program, with a header the test may corrupt and a `.text` the test may
/// describe wrongly.
fn program(
    header: HeaderOverrides,
    text: fn(SectionSpec) -> SectionSpec,
) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "message", MESSAGE.len() as u32);
    code.sys_exit(0);
    build(
        ObjectBuilder::new()
            .section(text(text_of(&code)))
            .section(SectionSpec::rodata(".rodata", MESSAGE.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::local("message", ".rodata", 0).object(MESSAGE.len() as u64))
            .header(header),
    )
}

/// `.text` exactly as the writer laid it out.
fn honest(s: SectionSpec) -> SectionSpec {
    s
}

/// `.text` claiming to live a megabyte into a file that is under a kilobyte.
fn offset_past_end(s: SectionSpec) -> SectionSpec {
    s.offset_override(1_000_000)
}

/// `.text` claiming to be two gigabytes long.
fn size_absurd(s: SectionSpec) -> SectionSpec {
    s.size_override(0x7fff_ffff)
}

/// Rewrite `sh_link` of the named section.
fn set_sh_link(mut bytes: Vec<u8>, section: &str, value: u32) -> Result<Vec<u8>, Failure> {
    let elf = Elf::parse(&bytes)
        .map_err(|e| Failure::harness(format!("the suite built an object it cannot read: {e}")))?;
    let index = elf
        .section(section)
        .map(|s| s.index)
        .ok_or_else(|| Failure::harness(format!("the fixture has no '{section}' section")))?;
    let at = elf.shoff as usize + index * SHDR_SIZE as usize + 40;
    if at + 4 > bytes.len() {
        return Err(Failure::harness(
            "the fixture's section header table is short",
        ));
    }
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    Ok(bytes)
}

/// The refusal this stage expects: a non-zero exit, a diagnostic naming the input, and
/// nothing runnable left behind.
fn refused(ctx: &mut Ctx, obj: Vec<u8>, doing: &str) -> Result<(), Failure> {
    let run = ctx.link_fails(&Link::new().object("a.o", obj).label(doing), doing)?;
    let mut c = Check::new(format!("the diagnostic and the aftermath of {doing}"));
    c.mentions("linker.stderr", "a.o", &run.output.stderr);
    no_runnable_leftovers(&mut c, &run);
    if !c.ok() {
        c.block("linker command", run.output.command_line());
        c.block("linker output", run.diagnostics());
    }
    c.finish()
}

/// True when the file looks like something the kernel would actually start: an `ET_EXEC`
/// with an entry point inside an executable `PT_LOAD`.
fn looks_runnable(exe: &Elf) -> bool {
    exe.e_type == ET_EXEC
        && exe.entry != 0
        && exe.loads().any(|s| s.contains(exe.entry) && s.executable())
}

/// A failed link may leave no output, or a stub; it may not leave a runnable executable.
fn no_runnable_leftovers(c: &mut Check, run: &LinkRun) {
    let Some(bytes) = run.produced.as_ref() else {
        return;
    };
    let runnable = Elf::parse(bytes)
        .map(|e| looks_runnable(&e))
        .unwrap_or(false);
    c.that(
        "output",
        "no runnable executable left behind by a link that failed",
        !runnable,
        format!("{} bytes that look runnable", bytes.len()),
    );
}

/// Turn a successful [`LinkRun`] into a [`Linked`] so the produced file can be run.
fn as_linked(run: LinkRun) -> Option<Linked> {
    let bytes = run.produced.clone()?;
    let elf = Elf::parse(&bytes).ok()?;
    Some(Linked {
        path: run.out_path.clone(),
        bytes,
        elf,
        run,
    })
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

link_test!(shoff_past_end, |ctx| {
    let obj = program(
        HeaderOverrides {
            e_shoff: Some(1_000_000),
            ..Default::default()
        },
        honest,
    )?;
    ctx.note("e_shoff is 1000000 in a file of well under a kilobyte");
    refused(
        ctx,
        obj,
        "linking an object whose e_shoff is past the end of the file",
    )
});

link_test!(shnum_absurd, |ctx| {
    let obj = program(
        HeaderOverrides {
            e_shnum: Some(0xfff0),
            ..Default::default()
        },
        honest,
    )?;
    ctx.note(
        "65520 section headers would be 4 MB of table in a file of a few hundred bytes; the \
         multiplication has to be done before the read, and in a width that cannot overflow",
    );
    refused(ctx, obj, "linking an object claiming 65520 sections")
});

link_test!(sh_offset_past_end, |ctx| {
    let obj = program(HeaderOverrides::default(), offset_past_end)?;
    ctx.note(
        "GNU ld warns 'a.o has a section extending past end of file' and then refuses the \
         file outright",
    );
    refused(
        ctx,
        obj,
        "linking an object whose .text lives past the end of the file",
    )
});

link_test!(sh_size_absurd, |ctx| {
    let obj = program(HeaderOverrides::default(), size_absurd)?;
    refused(
        ctx,
        obj,
        "linking an object whose .text claims to be 2 GB long",
    )
});

link_test!(shentsize_32, |ctx| {
    let obj = program(
        HeaderOverrides {
            e_shentsize: Some(32),
            ..Default::default()
        },
        honest,
    )?;
    ctx.note(
        "32 is the ELF32 section header size; believing it here walks the table at half \
         stride and reads every field from the wrong place",
    );
    refused(ctx, obj, "linking an object whose e_shentsize is 32")
});

link_test!(shstrndx_out_of_range, |ctx| {
    // Section names are not, strictly, needed to link: a linker that keys on sh_flags and
    // sh_type alone could ignore the broken index entirely and still be right. GNU ld takes
    // the third road — it warns, drops every section of the input, and exits 0 with an
    // output that has no PT_LOAD at all. So the invariant is the weakest of the three that
    // is still worth anything: never silently produce a binary that pretends to be the
    // program.
    let obj = program(
        HeaderOverrides {
            e_shstrndx: Some(99),
            ..Default::default()
        },
        honest,
    )?;
    let run = ctx.link(
        &Link::new()
            .object("a.o", obj)
            .label("an object whose e_shstrndx is out of range"),
    )?;
    let mut c = Check::new("what happened to an out-of-range e_shstrndx");
    c.that(
        "linker.status",
        "an exit status, not a signal and not a hang",
        run.output.signal.is_none() && !run.output.timed_out,
        run.output.status_line(),
    );
    if !run.succeeded() {
        c.that(
            "linker.stderr",
            "a diagnostic naming the input that was refused",
            !run.output.stderr.trim().is_empty(),
            "(empty)",
        );
        no_runnable_leftovers(&mut c, &run);
        c.finish()?;
        return Ok(());
    }

    let diagnosed = !run.output.stderr.trim().is_empty();
    c.finish()?;
    let mut c = Check::new("the output of a link that tolerated an out-of-range e_shstrndx");
    match as_linked(run) {
        Some(linked) if looks_runnable(&linked.elf) => {
            ctx.note(
                "the linker tolerated the broken index and produced a working program, which \
                 is allowed: section names are not needed to lay out SHF_ALLOC sections",
            );
            assert_runnable_layout(&linked)?;
            ctx.expect_output(&linked, MESSAGE, 0)?;
            Ok(())
        }
        Some(linked) => {
            c.that(
                "linker.stderr",
                "a warning or an error: an output this broken may not be produced in silence",
                diagnosed,
                "(empty)",
            );
            c.observe(
                "output",
                format!(
                    "e_type {} entry 0x{:x}, {} PT_LOAD",
                    linked.elf.e_type,
                    linked.elf.entry,
                    linked.elf.loads().count()
                ),
            );
            if !c.ok() {
                c.block("linker output", linked.run.diagnostics());
                c.block("output section headers", linked.elf.section_header_table());
            }
            c.finish()?;
            ctx.note(
                "GNU ld warns 'a.o has a corrupt string table index', drops the input's \
                 sections and exits 0 with an unrunnable file; a linker that refuses the \
                 object instead is equally correct",
            );
            Ok(())
        }
        None => {
            c.that(
                "output",
                "either a file this parser can read or no file at all",
                diagnosed,
                "an unparseable output produced with no diagnostic",
            );
            c.finish()
        }
    }
});

link_test!(symtab_link_not_a_strtab, |ctx| {
    let obj = set_sh_link(program(HeaderOverrides::default(), honest)?, ".symtab", 1)?;
    ctx.note(
        "sh_link now points at section 1, which is .text: reading symbol names through it \
         yields machine code, and GNU ld says 'attempt to load strings from a non-string \
         section'",
    );
    refused(
        ctx,
        obj,
        "linking an object whose .symtab points at .text for its names",
    )
});

link_test!(symtab_link_out_of_range, |ctx| {
    let obj = set_sh_link(program(HeaderOverrides::default(), honest)?, ".symtab", 99)?;
    refused(
        ctx,
        obj,
        "linking an object whose .symtab names section 99 as its string table",
    )
});

// ---------------------------------------------------------------------------------------
// Examples
// ---------------------------------------------------------------------------------------

/// The `.text`-past-the-end fixture, for the worked example.
fn example_offset_past_end() -> Result<Vec<u8>, String> {
    program(HeaderOverrides::default(), offset_past_end).map_err(|f| f.messages.join("; "))
}

/// The out-of-range `e_shstrndx` fixture, for the worked example.
fn example_shstrndx() -> Result<Vec<u8>, String> {
    program(
        HeaderOverrides {
            e_shstrndx: Some(99),
            ..Default::default()
        },
        honest,
    )
    .map_err(|f| f.messages.join("; "))
}

/// Worked examples: one header that has to be refused, one that may be survived.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::error(
            "A section that is not in the file",
            "ld -o prog liar.o",
            example_offset_past_end,
        )
        .request(
            "liar.o: an ordinary object except that .text's sh_offset is 1000000 while the \
             file is a few hundred bytes long",
        )
        .response(
            "The linker exits non-zero naming `liar.o`. Every sh_offset + sh_size is checked \
             against the length of the file before a single byte is copied out of it",
        )
        .note(
            "This is the check that keeps a malformed object from turning into a panic, an \
             out-of-bounds read, or — worst — a few hundred kilobytes of whatever followed \
             the file in memory copied into the executable.",
        ),
        ExampleSpec::object(
            "A string table index that is not a section",
            "ld -o prog badnames.o",
            example_shstrndx,
        )
        .request(
            "badnames.o: e_shstrndx is 99 and the object has seven sections, so there is no \
             table to read section names from",
        )
        .response(
            "Either a refusal naming the file, or a link that ignores section names \
             altogether and still produces the working program — but never a silent exit 0 \
             with an executable that cannot run",
        )
        .note(
            "GNU ld takes a third road: it warns about the corrupt index, drops the input's \
             sections and writes an output with no PT_LOAD. The suite accepts that, and \
             insists only that something was said.",
        ),
    ]
}
