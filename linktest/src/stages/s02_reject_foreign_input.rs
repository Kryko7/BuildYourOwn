//! Stage 02 — Reject what is not an x86-64 relocatable object.
//!
//! The first thing a linker does with an input is decide whether it can read it at all.
//! Sixteen bytes of `e_ident` and two 16-bit fields settle it: the magic, ELFCLASS64,
//! little-endian, `ET_REL` and `EM_X86_64`. Everything else — a 32-bit object, a big-endian
//! object, an i386 or AArch64 object, a finished executable someone passed by mistake — has
//! to be refused with a diagnostic that names the file, not linked in and left to explode at
//! run time.

use crate::assert::{Check, Failure};
use crate::elf::read::Elf;
use crate::elf::write::{HeaderOverrides, ObjectBuilder, SectionSpec, SymbolSpec};
use crate::elf::*;
use crate::examples::ExampleSpec;
use crate::link::{Link, LinkRun};
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 2,
        slug: "reject_foreign_input",
        name: "Reject what is not an x86-64 relocatable object",
        ext: false,
        hints: &[
            "Check e_ident first — the four magic bytes, EI_CLASS 2 (ELFCLASS64), EI_DATA 1 \
             (little-endian) — then e_type ET_REL and e_machine EM_X86_64, and refuse \
             anything else before reading a single section header",
            "A refusal is an exit status, not a panic: write one line to stderr that names \
             the offending input file, exit non-zero, and leave no runnable output behind",
            "An executable and a shared object are ELF too, and a static linker that only \
             understands relocatable objects has to say so rather than folding their headers \
             into its own output",
            "Validate every input before allocating anything, so that a bad third input does \
             not leave a half-written executable in place of the old one",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new("a good relocatable object still links and runs", control),
            Test::new("a file whose ELF magic is wrong is refused", bad_magic),
            Test::new("an ELFCLASS32 object is refused", class32),
            Test::new("a big-endian object is refused", big_endian),
            Test::new("an i386 object is refused", i386),
            Test::new("an aarch64 object is refused", aarch64),
            Test::new("an ET_EXEC input is refused", et_exec),
            Test::new(
                "an ET_DYN input is not linked in as if it were a relocatable object",
                et_dyn,
            ),
            Test::new(
                "a foreign input is refused even when a good object comes first",
                good_then_bad,
            ),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------

/// The stage's one program: print `hello, linker` and exit 0, under a header the test is
/// free to corrupt.
fn hello(header: HeaderOverrides) -> Result<Vec<u8>, Failure> {
    const MESSAGE: &str = "hello, linker\n";
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

/// The refusal every test in this stage expects: a non-zero exit, a diagnostic that names
/// the input file, and nothing runnable left in the output's place.
fn refused(ctx: &mut crate::stages::Ctx, obj: Vec<u8>, doing: &str) -> Result<(), Failure> {
    let run = ctx.link_fails(&Link::new().object("bad.o", obj).label(doing), doing)?;
    let mut c = Check::new(format!("the diagnostic and the aftermath of {doing}"));
    c.mentions("linker.stderr", "bad.o", &run.output.stderr);
    no_runnable_leftovers(&mut c, &run);
    if !c.ok() {
        c.block("linker command", run.output.command_line());
        c.block("linker output", run.diagnostics());
    }
    c.finish()
}

/// After a refused link there must be no file the shell could run: either no output file at
/// all (what GNU ld does) or something that is plainly not an executable.
fn no_runnable_leftovers(c: &mut Check, run: &LinkRun) {
    let Some(bytes) = run.produced.as_ref() else {
        return;
    };
    let runnable = match Elf::parse(bytes) {
        Ok(exe) => {
            exe.e_type == ET_EXEC
                && exe.entry != 0
                && exe.loads().any(|s| s.contains(exe.entry) && s.executable())
        }
        Err(_) => false,
    };
    c.that(
        "output",
        "no runnable executable left behind by a link that failed",
        !runnable,
        format!("{} bytes that look runnable", bytes.len()),
    );
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

link_test!(control, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", hello(HeaderOverrides::default())?))?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "hello, linker\n", 0)?;
    Ok(())
});

link_test!(bad_magic, |ctx| {
    let obj = hello(HeaderOverrides {
        magic: Some(*b"\x7fELG"),
        ..Default::default()
    })?;
    ctx.note("the fourth magic byte is 'G': everything after it is a perfectly good object");
    refused(ctx, obj, "linking a file whose ELF magic is wrong")
});

link_test!(class32, |ctx| {
    let obj = hello(HeaderOverrides {
        class: Some(ELFCLASS32),
        ..Default::default()
    })?;
    ctx.note(
        "EI_CLASS says ELFCLASS32, so every structure that follows would have a different \
         size and layout — the header alone settles it",
    );
    refused(ctx, obj, "linking an object that claims to be ELFCLASS32")
});

link_test!(big_endian, |ctx| {
    let obj = hello(HeaderOverrides {
        endianness: Some(ELFDATA2MSB),
        ..Default::default()
    })?;
    refused(ctx, obj, "linking an object that claims to be big-endian")
});

link_test!(i386, |ctx| {
    let obj = hello(HeaderOverrides {
        e_machine: Some(EM_386),
        ..Default::default()
    })?;
    ctx.note(
        "GNU ld gets as far as the relocations before it gives up here ('Relocations in \
         generic ELF (EM: 3)'); a linker that checks e_machine up front refuses sooner, and \
         both are correct",
    );
    refused(ctx, obj, "linking an i386 object")
});

link_test!(aarch64, |ctx| {
    let obj = hello(HeaderOverrides {
        e_machine: Some(EM_AARCH64),
        ..Default::default()
    })?;
    refused(ctx, obj, "linking an aarch64 object")
});

link_test!(et_exec, |ctx| {
    let obj = hello(HeaderOverrides {
        e_type: Some(ET_EXEC),
        ..Default::default()
    })?;
    ctx.note("GNU ld says: cannot use executable file 'bad.o' as input to a link");
    refused(
        ctx,
        obj,
        "linking a finished executable as if it were an object",
    )
});

link_test!(et_dyn, |ctx| {
    // GNU ld does *not* refuse an ET_DYN input: it treats it as a shared library, which is
    // exactly what ET_DYN means, and produces a dynamically linked output that needs an
    // interpreter. A static linker with no dynamic support has nothing to do with such an
    // input, so the invariant both satisfy is: whatever comes out, it is never a static
    // executable built out of the shared object's sections.
    let obj = hello(HeaderOverrides {
        e_type: Some(ET_DYN),
        ..Default::default()
    })?;
    let run = ctx.link(&Link::new().object("bad.o", obj).label("an ET_DYN input"))?;
    let mut c = Check::new("what happened to an ET_DYN input");
    c.that(
        "linker.status",
        "an exit status, not a signal and not a hang",
        run.output.signal.is_none() && !run.output.timed_out,
        run.output.status_line(),
    );
    if run.succeeded() {
        ctx.note(
            "GNU ld accepts an ET_DYN input as a shared library and exits 0; the output it \
             writes is a dynamic image with no entry point, not a static executable",
        );
        let bytes = run.produced.clone().unwrap_or_default();
        match Elf::parse(&bytes) {
            Ok(exe) => {
                let dynamic = exe
                    .segments
                    .iter()
                    .any(|s| s.p_type == PT_INTERP || s.p_type == PT_DYNAMIC);
                c.that(
                    "output",
                    "not a runnable static executable: a shared object is not a relocatable \
                     input, so either the link is refused or the output is a dynamic image",
                    dynamic || exe.entry == 0,
                    format!("e_type {} entry 0x{:x}", exe.e_type, exe.entry),
                );
            }
            Err(e) => {
                c.observe("output.parse", e.to_string());
            }
        }
    } else {
        c.that(
            "linker.stderr",
            "a diagnostic naming the input that was refused",
            run.output.stderr.contains("bad.o"),
            crate::assert::one_line(&run.output.stderr, 200),
        );
        no_runnable_leftovers(&mut c, &run);
    }
    if !c.ok() {
        c.block("linker command", run.output.command_line());
        c.block("linker output", run.diagnostics());
    }
    c.finish()
});

link_test!(good_then_bad, |ctx| {
    let good = hello(HeaderOverrides::default())?;
    let bad = hello(HeaderOverrides {
        e_machine: Some(EM_386),
        ..Default::default()
    })?;
    let link = Link::new()
        .object("good.o", good)
        .object("bad.o", bad)
        .label("a good object followed by an i386 one");
    let run = ctx.link_fails(&link, "linking a good object followed by an i386 one")?;
    let mut c = Check::new("the diagnostic for the second, foreign input");
    c.mentions("linker.stderr", "bad.o", &run.output.stderr);
    no_runnable_leftovers(&mut c, &run);
    if !c.ok() {
        c.block("linker command", run.output.command_line());
        c.block("linker output", run.diagnostics());
    }
    c.finish()?;
    // The failure must be about the input, not about the suite: the same good object on its
    // own has to link.
    let linked = ctx.link_ok(
        &Link::new()
            .object("good.o", hello(HeaderOverrides::default())?)
            .out("good")
            .label("the good object on its own"),
    )?;
    ctx.expect_output(&linked, "hello, linker\n", 0)?;
    Ok(())
});

// ---------------------------------------------------------------------------------------
// Examples
// ---------------------------------------------------------------------------------------

/// The i386 fixture, for the worked example.
fn example_i386() -> Result<Vec<u8>, String> {
    hello(HeaderOverrides {
        e_machine: Some(EM_386),
        ..Default::default()
    })
    .map_err(|f| f.messages.join("; "))
}

/// The ET_EXEC fixture, for the worked example.
fn example_et_exec() -> Result<Vec<u8>, String> {
    hello(HeaderOverrides {
        e_type: Some(ET_EXEC),
        ..Default::default()
    })
    .map_err(|f| f.messages.join("; "))
}

/// Worked examples: two inputs that have to be refused.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::error(
            "An object for the wrong machine",
            "ld -o prog i386.o",
            example_i386,
        )
        .request(
            "i386.o: a well-formed ELF64 relocatable object in every respect except \
             e_machine, which is 3 (EM_386) instead of 62 (EM_X86_64)",
        )
        .response(
            "No output file. The linker exits non-zero and writes a line to stderr naming \
             `i386.o`; the x86-64 relocation types in it mean nothing on another machine, so \
             linking it would produce a binary that cannot work",
        )
        .note(
            "The trap is reading the section headers first and only noticing e_machine when \
             a relocation type comes out unknown. Two bytes at offset 18 decide this.",
        ),
        ExampleSpec::error(
            "A finished executable passed as an input",
            "ld -o prog already-linked.o",
            example_et_exec,
        )
        .request(
            "already-linked.o: the same object with e_type 2 (ET_EXEC) instead of 1 (ET_REL) \
             — what you get by passing the linker its own output by mistake",
        )
        .response(
            "The linker exits non-zero naming the file. Only ET_REL inputs can be combined: \
             an executable's sections already have addresses and its relocations are gone",
        )
        .note(
            "e_type is two bytes at offset 16. Refusing it here is what keeps `ld -o a.out \
             a.out` from quietly destroying the file it is reading.",
        ),
    ]
}
