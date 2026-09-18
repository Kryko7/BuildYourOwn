//! Stage 01 — Link one object and run it.

use crate::assert::Check;
use crate::elf::read::Elf;
use crate::elf::*;
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 1,
        slug: "link_one_object",
        name: "Link one object and run it",
        ext: false,
        hints: &[
            "Read the 64-byte ELF header, then the section header table it points at, and \
             keep the SHF_ALLOC sections",
            "Give .text an address, write one PT_LOAD segment that covers it, and set \
             e_entry to the address of _start",
            "e_type is ET_EXEC (2), e_machine EM_X86_64 (62); the output has to be chmod \
             0755 or the kernel will not run it",
            "Nothing here needs relocation yet: one object with no .rela sections is the \
             whole job",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new("a one-object program links", links_at_all),
            Test::new("the linked program runs and prints", runs_and_prints),
            Test::new("the exit status is the one the code asked for", exit_status),
            Test::new(
                "the output is an ET_EXEC x86-64 executable",
                is_an_executable,
            ),
            Test::new("the output is marked executable", is_chmod_x),
            Test::new("the output satisfies every layout invariant", layout),
            Test::new("a second link of the same object behaves the same", twice),
            Test::new("an object with no .rodata at all still links", no_rodata),
        ],
    }
}

link_test!(links_at_all, |ctx| {
    let obj = print_and_exit("hello, linker\n", 0)?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).label("one object"))?;
    let mut c = Check::new("that the linker produced something");
    c.that(
        "output.size",
        "a non-empty file",
        !linked.bytes.is_empty(),
        linked.bytes.len(),
    );
    c.finish()
});

link_test!(runs_and_prints, |ctx| {
    let obj = print_and_exit("hello, linker\n", 0)?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj))?;
    ctx.expect_output(&linked, "hello, linker\n", 0)?;
    Ok(())
});

link_test!(exit_status, |ctx| {
    let obj = print_and_exit("bye\n", 42)?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj))?;
    ctx.expect_output(&linked, "bye\n", 42)?;
    Ok(())
});

link_test!(is_an_executable, |ctx| {
    let obj = print_and_exit("x\n", 0)?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj))?;
    let mut c = Check::new("the ELF header of the output");
    c.eq("output.e_ident[EI_CLASS]", ELFCLASS64, linked.elf.ident[4]);
    c.eq("output.e_ident[EI_DATA]", ELFDATA2LSB, linked.elf.ident[5]);
    c.eq("output.e_type", ET_EXEC, linked.elf.e_type);
    c.eq("output.e_machine", EM_X86_64, linked.elf.e_machine);
    c.ne("output.e_entry", 0u64, linked.elf.entry);
    if !c.ok() {
        let e_type = 16..20;
        c.hex(
            "the first 64 bytes of the output",
            &linked.bytes,
            std::slice::from_ref(&e_type),
        );
    }
    c.finish()
});

link_test!(is_chmod_x, |ctx| {
    let obj = exit_only(3)?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj))?;
    let mut c = Check::new("the permissions of the output file");
    c.that(
        "output.mode",
        "at least one execute bit set — the kernel refuses to exec anything else",
        crate::stages::is_executable(&linked.path),
        mode_of(&linked.path),
    );
    c.finish()?;
    ctx.expect_output(&linked, "", 3)?;
    Ok(())
});

link_test!(layout, |ctx| {
    let obj = print_and_exit("layout\n", 0)?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj))?;
    assert_runnable_layout(&linked)?;
    assert_entry_is(&linked, DEFAULT_ENTRY)
});

link_test!(twice, |ctx| {
    let obj = print_and_exit("twice\n", 9)?;
    let first = ctx.link_ok(
        &Link::new()
            .object("a.o", obj.clone())
            .label("the first link"),
    )?;
    ctx.expect_output(&first, "twice\n", 9)?;
    let second = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .out("second")
            .label("the second link"),
    )?;
    ctx.expect_output(&second, "twice\n", 9)?;
    let mut c = Check::new("that two links of the same input agree");
    c.addr_eq("output.e_entry", first.elf.entry, second.elf.entry);
    c.finish()
});

link_test!(no_rodata, |ctx| {
    let obj = exit_only(17)?;
    let linked = ctx.link_ok(&Link::new().object("only-text.o", obj))?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "", 17)?;
    Ok(())
});

/// The file mode as a string, for a failure row.
fn mode_of(path: &std::path::Path) -> String {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(m) => format!("0{:o}", m.permissions().mode() & 0o7777),
        Err(e) => format!("cannot stat: {e}"),
    }
}

/// Worked examples: the smallest program there is.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object("One object, one syscall pair", "ld -o prog hello.o", || {
            print_and_exit("hello, linker\n", 0).map_err(|f| f.messages.join("; "))
        })
        .request(
            "hello.o: a .text with write(1, message, 14) then exit(0), a .rodata holding the \
             string, and one R_X86_64_PC32 relocation for the lea that finds it",
        )
        .response(
            "An ET_EXEC ELF64 file with at least one PT_LOAD covering .text, e_entry equal to \
             the address the linker gave _start, and the lea's displacement patched to \
             S + A - P so that rsi ends up pointing at the string",
        )
        .note(
            "The whole program is two syscalls. If it prints nothing, the displacement is \
             wrong; if it segfaults, the segment holding .text is not executable or e_entry \
             does not point at code.",
        )
        .runs("hello, linker\n", 0),
        ExampleSpec::object(
            "An object with nothing but code",
            "ld -o prog exit17.o",
            || exit_only(17).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "exit17.o: one .text section, one global symbol _start, no .rodata, no .data, no \
             relocations at all",
        )
        .response(
            "A runnable executable whose only PT_LOAD is R+X; the program writes nothing and \
             exits 17",
        )
        .note(
            "Start here. A linker that handles this one input correctly already has to read \
             a section header table, allocate an address, write a program header and set \
             e_entry.",
        )
        .runs("", 17),
    ]
}

/// Re-exported so the doc-comment above can point at the parser the tests use.
#[allow(dead_code)]
type ParsedOutput = Elf;
