//! Stage 06 — Truncated and malformed objects.
//!
//! A build that ran out of disk, an object copied out of an archive by hand, a download that
//! stopped: a truncated `.o` is the most ordinary malformed input there is. Cut at any
//! length it is still a file the linker is asked to read, and at every length the answer is
//! the same — an exit status and a message, never a panic, never a hang, and never an
//! executable left behind for somebody to run.

use crate::asm::{Code, STDOUT};
use crate::assert::{Check, Failure};
use crate::elf::read::Elf;
use crate::elf::write::{ObjectBuilder, SectionSpec, SymbolSpec};
use crate::elf::*;
use crate::examples::ExampleSpec;
use crate::link::{Link, LinkRun};
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Ctx, Stage, Test};
use rand::Rng;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 6,
        slug: "truncated_objects",
        name: "Truncated and malformed objects",
        ext: true,
        hints: &[
            "Read the whole input into memory once and check every offset and length against \
             that buffer before using it — a truncated object is exactly a file whose \
             offsets point past its end",
            "The three places a truncation shows up are the 64-byte header, the section \
             contents and the section header table; a bounds check in one of the three is a \
             bounds check in none of them",
            "Refusing has to be an exit status with a message on stderr, not an index panic \
             and not a silent zero-filled read",
            "If you have already created the output file when you discover the problem, \
             remove it or never mark it executable: a failed link must not leave something \
             runnable behind",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "an object cut short inside the ELF header is refused",
                inside_header,
            )
            .ext(),
            Test::new(
                "an object cut to exactly 64 bytes is refused",
                exactly_the_header,
            )
            .ext(),
            Test::new(
                "an object cut inside its section contents is refused",
                inside_contents,
            )
            .ext(),
            Test::new(
                "an object cut inside the section header table is refused",
                inside_the_table,
            )
            .ext(),
            Test::new(
                "an object one byte short of complete is refused",
                one_byte_short,
            )
            .ext(),
            Test::new("an empty file never becomes a runnable program", empty_file).ext(),
            Test::new(
                "a truncated object next to a good one still fails the link",
                truncated_beside_good,
            )
            .ext(),
            Test::new(
                "many truncation lengths, none crashing, hanging or leaving a binary",
                every_length,
            )
            .ext()
            .tag("slow")
            .min_timeout_ms(30_000),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------

/// What the intact program would print, if it were ever allowed to.
const MESSAGE: &str = "never printed\n";

/// The stage's program, cut short at `bytes` bytes.
fn truncated(bytes: usize) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "message", MESSAGE.len() as u32);
    code.sys_exit(0);
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", MESSAGE.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::local("message", ".rodata", 0).object(MESSAGE.len() as u64))
            .truncate_to(bytes),
    )
}

/// The intact program, and the two lengths that matter about it: how long it is and where
/// its section header table starts.
fn intact() -> Result<(Vec<u8>, usize, usize), Failure> {
    let bytes = truncated(usize::MAX)?;
    let elf = Elf::parse(&bytes)
        .map_err(|e| Failure::harness(format!("the suite built an object it cannot read: {e}")))?;
    let shoff = elf.shoff as usize;
    Ok((bytes.clone(), bytes.len(), shoff))
}

/// True when the file looks like something the kernel would start.
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

/// Link one truncated object and insist it was refused cleanly.
fn refuses(ctx: &mut Ctx, length: usize, what: &str) -> Result<(), Failure> {
    let obj = truncated(length)?;
    let doing = format!("linking an object cut to {length} bytes ({what})");
    let run = ctx.link_fails(&Link::new().object("cut.o", obj).label(&doing), &doing)?;
    let mut c = Check::new(format!("the diagnostic and the aftermath of {doing}"));
    c.mentions("linker.stderr", "cut.o", &run.output.stderr);
    no_runnable_leftovers(&mut c, &run);
    if !c.ok() {
        c.block("linker command", run.output.command_line());
        c.block("linker output", run.diagnostics());
    }
    c.finish()
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

link_test!(inside_header, |ctx| {
    for length in [1usize, 4, 16, 40, 63] {
        refuses(ctx, length, "inside the ELF header")?;
    }
    ctx.note(
        "at 4 bytes there is a magic number and nothing else; at 63 every field but the last \
         two bytes of e_shstrndx is there, and the file is still unreadable",
    );
    Ok(())
});

link_test!(exactly_the_header, |ctx| {
    refuses(ctx, EHDR_SIZE as usize, "the header and nothing else")?;
    ctx.note(
        "the header is complete and self-consistent; it is e_shoff, pointing past the end of \
         a 64-byte file, that gives it away",
    );
    Ok(())
});

link_test!(inside_contents, |ctx| {
    for length in [100usize, 200] {
        refuses(ctx, length, "inside the section contents")?;
    }
    Ok(())
});

link_test!(inside_the_table, |ctx| {
    let (_, _, shoff) = intact()?;
    refuses(ctx, shoff + 32, "half of the first section header")?;
    refuses(
        ctx,
        shoff + SHDR_SIZE as usize * 3,
        "three of seven section headers",
    )?;
    ctx.note(format!(
        "the section header table of this fixture starts at offset {shoff}: e_shnum still \
         says seven, and only the file's length says otherwise"
    ));
    Ok(())
});

link_test!(one_byte_short, |ctx| {
    let (_, len, _) = intact()?;
    refuses(ctx, len - 1, "one byte short of the whole file")?;
    ctx.note(format!(
        "the intact object is {len} bytes; the missing byte is the last byte of the last \
         section header's sh_entsize"
    ));
    Ok(())
});

link_test!(empty_file, |ctx| {
    // GNU ld does not refuse a zero-byte input: it treats it as an empty linker script,
    // warns that _start cannot be found and exits 0 with a file that has no PT_LOAD and no
    // entry point. That file is not runnable, which is the part that matters.
    let run = ctx.link(
        &Link::new()
            .object("empty.o", Vec::new())
            .label("an empty input file"),
    )?;
    let mut c = Check::new("what happened to an empty input file");
    c.that(
        "linker.status",
        "an exit status, not a signal and not a hang",
        run.output.signal.is_none() && !run.output.timed_out,
        run.output.status_line(),
    );
    if run.succeeded() {
        let runnable = run
            .produced
            .as_ref()
            .and_then(|b| Elf::parse(b).ok())
            .map(|e| looks_runnable(&e))
            .unwrap_or(false);
        c.that(
            "output",
            "nothing runnable: an empty input cannot have produced a program",
            !runnable,
            "a runnable executable",
        );
        ctx.note(
            "GNU ld reads a zero-byte input as an empty linker script, warns that it cannot \
             find _start and exits 0; the file it writes has no PT_LOAD and cannot be \
             executed, so the suite allows it",
        );
    } else {
        no_runnable_leftovers(&mut c, &run);
    }
    if !c.ok() {
        c.block("linker command", run.output.command_line());
        c.block("linker output", run.diagnostics());
    }
    c.finish()
});

link_test!(truncated_beside_good, |ctx| {
    let good = print_and_exit("good\n", 0)?;
    let (_, len, _) = intact()?;
    let cut = truncated(len / 2)?;
    let run = ctx.link_fails(
        &Link::new()
            .object("good.o", good)
            .object("cut.o", cut)
            .label("a good object followed by a truncated one"),
        "linking a good object followed by a truncated one",
    )?;
    let mut c = Check::new("the diagnostic for the truncated second input");
    c.mentions("linker.stderr", "cut.o", &run.output.stderr);
    no_runnable_leftovers(&mut c, &run);
    if !c.ok() {
        c.block("linker output", run.diagnostics());
    }
    c.finish()?;
    ctx.note(
        "the first input is perfectly good and defines _start: a linker that validates \
         inputs lazily gets as far as writing an output before it notices",
    );
    Ok(())
});

link_test!(every_length, |ctx| {
    let (_, len, shoff) = intact()?;
    let mut lengths: Vec<usize> = vec![
        1,
        2,
        8,
        16,
        32,
        40,
        63,
        EHDR_SIZE as usize,
        EHDR_SIZE as usize + 1,
        100,
        shoff - 1,
        shoff,
        shoff + 1,
        shoff + SHDR_SIZE as usize / 2,
        shoff + SHDR_SIZE as usize * 4,
        len - 1,
    ];
    for _ in 0..4 {
        lengths.push(ctx.rng.random_range(1..len));
    }
    lengths.sort_unstable();
    lengths.dedup();

    let mut refused = 0usize;
    let mut tolerated: Vec<usize> = Vec::new();
    let mut c = Check::new("every truncation length, one link each");
    for length in &lengths {
        let obj = truncated(*length)?;
        let label = format!("a {length}-byte object");
        let run = ctx.link(&Link::new().object("cut.o", obj).label(&label))?;
        c.that(
            &format!("linker.status[{length}]"),
            "an exit status, not a signal and not a hang",
            run.output.signal.is_none() && !run.output.timed_out,
            run.output.status_line(),
        );
        if run.succeeded() {
            tolerated.push(*length);
            let runnable = run
                .produced
                .as_ref()
                .and_then(|b| Elf::parse(b).ok())
                .map(|e| looks_runnable(&e))
                .unwrap_or(false);
            c.that(
                &format!("output[{length}]"),
                "nothing runnable out of a truncated input",
                !runnable,
                "a runnable executable",
            );
        } else {
            refused += 1;
            no_runnable_leftovers(&mut c, &run);
        }
    }
    let total = lengths.len();
    ctx.note(format!(
        "{refused} of {total} truncation lengths were refused outright; the rest exited 0 \
         without producing anything runnable"
    ));
    if !tolerated.is_empty() {
        ctx.note(format!(
            "lengths tolerated with an exit status of 0: {tolerated:?}"
        ));
    }
    c.at_least("refused_lengths", 1usize, refused);
    c.observe("lengths_tried", total);
    c.finish()
});

// ---------------------------------------------------------------------------------------
// Examples
// ---------------------------------------------------------------------------------------

/// Worked examples: truncated bytes cannot be annotated, so these are described, not shown.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::text(
            "An object cut off inside its section header table",
            "ld -o prog cut.o",
        )
        .request(
            "cut.o: the first 360 bytes of a 776-byte object. The ELF header is intact and \
             says e_shoff 328, e_shnum 7, e_shentsize 64 — a table that would end at 776",
        )
        .response(
            "The linker exits non-zero with a message naming `cut.o`, and writes no output \
             file. 328 + 7 * 64 is past the end of a 360-byte file, and that is the whole \
             check",
        )
        .note(
            "Compute the end of every table in a width that cannot overflow and compare it \
             with the length of the buffer you read. `e_shnum * 64` in 32 bits is enough to \
             wrap for e_shnum near 65535.",
        ),
        ExampleSpec::text("An empty input file", "ld -o prog empty.o")
            .request("empty.o: zero bytes")
            .response(
                "A non-zero exit naming the file is the clearest answer. GNU ld instead reads it \
             as an empty linker script and exits 0 — with an output that has no PT_LOAD, no \
             entry point and cannot be executed, which is what the suite checks",
            )
            .note(
                "Whatever you decide, decide it: the one outcome that is always wrong is exit 0 \
             next to a file the shell will happily try to run.",
            ),
    ]
}
