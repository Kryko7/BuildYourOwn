//! Stage 32 — An archive on the command line.
//!
//! An archive is not a new kind of input, it is a *bag* of the inputs you already handle:
//! eight bytes of magic, then 60-byte ASCII headers each followed by a relocatable object.
//! Two of those members are special and come first — `/`, the symbol index that says which
//! member defines which global, and `//`, the table that holds any member name too long for
//! the 16-byte field. Everything this stage asks for follows from that: open the archive,
//! find the member that defines the symbol you are missing, and hand its bytes to the object
//! reader you already wrote. The one rule that is genuinely new — *only the members you need
//! are loaded* — is stage 33's whole subject; here every member either is needed or is
//! harmless, so a linker that loads all of them still passes. That is deliberate: get the
//! container right first.

use crate::asm::{Code, Reg, STDOUT};
use crate::assert::{Check, Failure};
use crate::elf::archive::{ArchiveBuilder, Member, ARCHIVE_MAGIC};
use crate::elf::write::{ObjectBuilder, SectionSpec, SymbolSpec};
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 32,
        slug: "archive_basics",
        name: "An archive on the command line",
        ext: false,
        hints: &[
            "Recognise an archive by its first eight bytes, `!<arch>\\n`, not by the `.a` on \
             the file name — the name is a convention and `-l` will hand you paths you did \
             not choose",
            "A member header is 60 bytes of space-padded ASCII; the only two fields you need \
             are the 16-byte name and the decimal size at offset 48, and every member starts \
             on an even offset, so round the size up when you step to the next one",
            "A name of `/<decimal>` is an offset into the `//` member, and a short name is \
             written with a trailing `/` that is not part of the name — strip it",
            "Once you have a member's bytes you are back in stage 01: it is an ordinary \
             ET_REL object and the reader you already have parses it unchanged",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "a one-member archive resolves an undefined symbol",
                one_member,
            ),
            Test::new(
                "an archive supplies two members that are both needed",
                two_members,
            ),
            Test::new(
                "an archive named by a relative path is found",
                named_by_path,
            ),
            Test::new(
                "a member name too long for the header is read from the long-name table",
                long_member_name,
            ),
            Test::new("an archive links beside plain objects", mixed_with_objects),
            Test::new("an archive nothing needs is not an error", nothing_needed),
            Test::new(
                "one member defining both a function and a datum supplies both",
                function_and_datum,
            ),
            Test::new(
                "an archive with no members at all is not an error",
                empty_archive,
            ),
            Test::new(
                "the archive's container bytes never reach the output",
                container_bytes_do_not_leak,
            ),
        ],
    }
}

link_test!(one_member, |ctx| {
    let main = caller("from an archive\n", "other")?;
    let lib = archive_of(vec![Member::new(
        "other.o",
        callee_returning("other", 7)?,
        &["other"],
    )])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .archive("libx.a", lib)
            .label("an object and a one-member archive"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "from an archive\n", 7)?;
    let mut c = Check::new("that the member's definition is what the call reached");
    let target = linked.address_of("other")?;
    let site = linked.address_of(DEFAULT_ENTRY)? + CALLER_CALL_DISP_OFFSET;
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[call].disp32",
        site,
        target,
        -4,
    );
    c.note(
        "the exit status alone proves the right body ran; the displacement proves it was \
         relocated against the address the linker itself gave the member",
    );
    c.finish()
});

link_test!(two_members, |ctx| {
    // `_start` calls alpha (6) and adds the datum alpha_word (30): 36 only if both members
    // were found, and the two symbols live in two different members.
    let main = needs_alpha_pair("two members\n")?;
    let lib = archive_of(vec![
        Member::new("alpha.o", callee_returning("alpha", 6)?, &["alpha"]),
        Member::new("word.o", data_word("alpha_word", 30)?, &["alpha_word"]),
    ])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .archive("libpair.a", lib)
            .label("an archive whose two members are both needed"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "two members\n", 36)?;
    Ok(())
});

link_test!(named_by_path, |ctx| {
    // The archive is not in the link's directory: the command line names it by a relative
    // path, which is what every real build system does.
    let main = caller("by path\n", "other")?;
    let lib = archive_of(vec![Member::new(
        "other.o",
        callee_returning("other", 12)?,
        &["other"],
    )])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .archive("build/lib/libx.a", lib)
            .label("an archive named by a relative path"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "by path\n", 12)?;
    Ok(())
});

link_test!(long_member_name, |ctx| {
    // 34 characters: far past the 16-byte name field, so the writer puts it in the `//`
    // member and the header carries `/<offset>` instead.
    let name = "a_really_long_member_name_indeed.o";
    let main = caller("long name\n", "other")?;
    let lib = archive_of(vec![Member::new(
        name,
        callee_returning("other", 21)?,
        &["other"],
    )])?;
    let mut c = Check::new("that the fixture really does exercise the long-name table");
    c.at_least("fixture.member_name.len", 16usize, name.len());
    c.finish()?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .archive("liblong.a", lib)
            .label("an archive whose member name lives in the // table"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "long name\n", 21)?;
    ctx.note(
        "a linker that never resolves `/<offset>` names still links this archive if it \
         ignores member names entirely — the names only ever matter for diagnostics",
    );
    Ok(())
});

link_test!(mixed_with_objects, |ctx| {
    // main.o -> mid.o -> the archive member: plain objects and an archive in one link.
    let main = caller("mixed\n", "mid")?;
    let mid = tail_call("mid", "other")?;
    let lib = archive_of(vec![Member::new(
        "other.o",
        callee_returning("other", 33)?,
        &["other"],
    )])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .object("mid.o", mid)
            .archive("libx.a", lib)
            .label("two objects and an archive"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "mixed\n", 33)?;
    Ok(())
});

link_test!(nothing_needed, |ctx| {
    // The object on the command line is a whole program already. The archive defines a
    // symbol nothing mentions, and the link must simply succeed.
    let main = print_and_exit("complete already\n", 4)?;
    let lib = archive_of(vec![Member::new(
        "spare.o",
        callee_returning("nobody_calls_this", 1)?,
        &["nobody_calls_this"],
    )])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .archive("libunused.a", lib)
            .label("an archive nothing in the link needs"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "complete already\n", 4)?;
    ctx.note(
        "an archive is an offer, not an input: nothing being taken from it is the normal \
         case, not an error",
    );
    Ok(())
});

link_test!(function_and_datum, |ctx| {
    // One member, two globals in two different sections: pulling it in has to bring both,
    // and the .data it carries has to be laid out like any other input's .data.
    let main = caller("both halves\n", "other")?;
    let mut code = Code::new();
    code.mov_r32_rip(Reg::Rax, "table", 0);
    code.ret();
    let member = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::data(".data", 31u32.to_le_bytes().to_vec()).align(4))
            .symbol(SymbolSpec::global("other", ".text", 0).func())
            .symbol(SymbolSpec::global("table", ".data", 0).object(4)),
    )?;
    let lib = archive_of(vec![Member::new("both.o", member, &["other", "table"])])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .archive("libboth.a", lib)
            .label("a member defining a function and a datum"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "both halves\n", 31)?;
    let mut c = Check::new("where the member's two symbols ended up");
    let found = (linked.elf.symbol("other"), linked.elf.symbol("table"));
    if let (Some(f), Some(d)) = found {
        c.that(
            "output.symtab['other'].st_shndx",
            "a defined symbol, not SHN_UNDEF",
            !f.is_undefined(),
            f.shndx,
        );
        c.that(
            "output.symtab['table'].st_shndx",
            "a defined symbol, not SHN_UNDEF",
            !d.is_undefined(),
            d.shndx,
        );
        c.observe(
            "output.symtab['other'].st_value",
            format!("0x{:x}", f.value),
        );
        c.observe(
            "output.symtab['table'].st_value",
            format!("0x{:x}", d.value),
        );
    } else {
        c.note(
            "the linker kept no symbol table entry for one of the member's globals; the exit \
             status above is then the only evidence, and it is enough",
        );
    }
    c.finish()
});

link_test!(empty_archive, |ctx| {
    // Eight bytes of magic and an index with nothing in it. A linker that assumes at least
    // one member walks off the end of the file here.
    let main = print_and_exit("no members\n", 6)?;
    let lib = archive_of(Vec::new())?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .archive("libempty.a", lib)
            .label("an archive with no members"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "no members\n", 6)?;
    Ok(())
});

link_test!(container_bytes_do_not_leak, |ctx| {
    // What goes into the output is the *member*, never the archive around it. A linker that
    // concatenates the archive file wholesale leaves its magic behind, and the run below
    // would still pass, so this is the check that catches it.
    let main = caller("no container\n", "other")?;
    let lib = archive_of(vec![Member::new(
        "other.o",
        callee_returning("other", 9)?,
        &["other"],
    )])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .archive("libx.a", lib)
            .label("an archive whose container must be discarded"),
    )?;
    ctx.expect_output(&linked, "no container\n", 9)?;
    let found = linked
        .bytes
        .windows(ARCHIVE_MAGIC.len())
        .any(|w| w == ARCHIVE_MAGIC);
    let mut c = Check::new("that the archive's own bytes are not in the executable");
    c.that(
        "output.bytes",
        "no `!<arch>\\n` anywhere in the linked file",
        !found,
        if found {
            "the archive magic is still there"
        } else {
            "absent"
        },
    );
    if !c.ok() {
        c.note(
            "an archive is a container: its magic, its member headers and its symbol index \
             are scaffolding the linker consumes and throws away",
        );
        c.block("linker command", linked.run.output.command_line());
    }
    c.finish()
});

/// A correct archive built from `members`, in the order given.
fn archive_of(members: Vec<Member>) -> Result<Vec<u8>, Failure> {
    let mut builder = ArchiveBuilder::new();
    for m in members {
        builder = builder.member(m);
    }
    builder
        .build()
        .map_err(|e| Failure::harness(format!("the suite could not build an archive: {e}")))
}

/// An object whose `.data` holds one 32-bit `value` under the global name `name`.
fn data_word(name: &str, value: u32) -> Result<Vec<u8>, Failure> {
    build(
        ObjectBuilder::new()
            .section(SectionSpec::data(".data", value.to_le_bytes().to_vec()).align(4))
            .symbol(SymbolSpec::global(name, ".data", 0).object(4)),
    )
}

/// An object defining `name` as a function that calls `next` and returns its result.
fn tail_call(name: &str, next: &str) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.call(next);
    code.ret();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(name, ".text", 0).func())
            .symbol(SymbolSpec::undefined(next)),
    )
}

/// An object that prints `message`, calls the undefined `alpha`, adds the undefined datum
/// `alpha_word` to the result and exits with the sum — two references, two members.
fn needs_alpha_pair(message: &str) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "message", message.len() as u32);
    code.call("alpha");
    code.add_r32_rip(Reg::Rax, "alpha_word", 0);
    code.sys_exit_eax();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::local("message", ".rodata", 0).object(message.len() as u64))
            .symbol(SymbolSpec::undefined("alpha"))
            .symbol(SymbolSpec::undefined("alpha_word")),
    )
}

/// Worked examples: the container, and the case where nothing is taken out of it.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::archive(
            "One archive, one member, one missing symbol",
            "ld -o prog main.o libx.a",
            || callee_returning("other", 7).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "libx.a is `!<arch>\\n`, a `/` symbol index whose single entry maps `other` to the \
             offset of the one member header, and then that member — the ET_REL object shown \
             here, which defines `other` as a function returning 7. main.o (not shown) prints \
             a line, calls `other` and exits with whatever it returns",
        )
        .response(
            "The linker opens libx.a, sees that `other` is the symbol it is missing, reads the \
             member at the offset the index gave it and links that object exactly as if it had \
             been named on the command line: the program prints its line and exits 7",
        )
        .note(
            "Nothing of the container survives: the magic, the 60-byte member header and the \
             symbol index are scaffolding. What reaches the output is the member's sections.",
        )
        .runs("from an archive\n", 7),
        ExampleSpec::archive(
            "An archive nobody needs",
            "ld -o prog main.o libunused.a",
            || callee_returning("nobody_calls_this", 1).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "main.o is already a whole program — it prints and exits 4 with no undefined \
             symbols at all. libunused.a holds the member shown here, which defines \
             `nobody_calls_this`",
        )
        .response(
            "A successful link whose output contains nothing from the archive; the program \
             prints its line and exits 4",
        )
        .note(
            "An archive is an offer. Refusing the link because an input contributed nothing \
             would break every real build, where most of libc is never touched.",
        )
        .runs("complete already\n", 4),
    ]
}
