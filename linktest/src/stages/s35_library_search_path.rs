//! Stage 35 — `-L` and `-l`.
//!
//! `-l x` is not a file name, it is a request: *find something called `libx.a` in the
//! directories I gave you with `-L`, and treat it as if I had typed its path here*. That is
//! the whole feature. It is worth doing properly because it is the last piece of command
//! line handling a static linker needs, and because every part of it has a failure mode that
//! is easy to get almost right: joining the name the wrong way round (`xlib.a`), matching by
//! prefix so `-l x` picks up `libxy.a`, taking the last directory that matches instead of the
//! first, or reporting "no such file" without saying which library it was looking for.
//!
//! The `-l` still occupies a position on the command line: the archive it resolves to is
//! linked *there*, with all of stage 34's ordering rules. Only the *search* is independent of
//! position — see the note on the `-L`-after-`-l` test.

use crate::asm::Code;
use crate::assert::{Check, Failure};
use crate::elf::archive::{ArchiveBuilder, Member};
use crate::elf::write::{ObjectBuilder, SymbolSpec};
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 35,
        slug: "library_search_path",
        name: "-L and -l",
        ext: false,
        hints: &[
            "`-l <name>` means `lib<name>.a`, matched exactly — `libxy.a` is not a match for \
             `-l x`, and the file name is built by wrapping the name, never by searching for \
             it",
            "Search the `-L` directories in the order they were given and take the first \
             directory that has the file, not the last and not the best",
            "When nothing matches, exit non-zero with a message that contains the library's \
             name; `cannot find -lfoo` is the wording GNU ld uses and the name is the part \
             that matters",
            "Resolving `-l` to a path is all this option does: the archive it found is linked \
             at the position the `-l` occupied, under the ordering rules of stage 34",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "-L dir -l x finds libx.a in that directory",
                finds_the_library,
            ),
            Test::new(
                "the first -L directory that has the library wins",
                first_directory_wins,
            ),
            Test::new(
                "-l with no -L at all fails and names the library",
                no_search_path_at_all,
            ),
            Test::new(
                "a library that is in none of the -L directories is an error naming it",
                library_not_found,
            ),
            Test::new(
                "-L given after the -l that needs it still finds the library",
                search_path_after_the_l,
            ),
            Test::new("a relative -L path is resolved", relative_search_path),
            Test::new("-l links beside plain objects", library_beside_objects),
            Test::new(
                "-l x does not match libxy.a and -l xy does not match libx.a",
                prefix_names_do_not_collide,
            ),
            Test::new(
                "-l x and the same archive named by path produce the same program",
                same_as_naming_the_path,
            ),
        ],
    }
}

link_test!(finds_the_library, |ctx| {
    // libx.a is written into the link's directory but is *not* on the command line: the only
    // way the linker can reach it is by joining "libs" and "lib" + "x" + ".a".
    let main = caller("found it\n", "other")?;
    let lib = archive_of(vec![Member::new(
        "other.o",
        callee_returning("other", 7)?,
        &["other"],
    )])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .write_file("libs/libx.a", lib)
            .lib_dir("libs")
            .lib("x")
            .label("-L libs -l x"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "found it\n", 7)?;
    Ok(())
});

link_test!(first_directory_wins, |ctx| {
    // Both directories hold a libx.a; they differ only in the constant their member returns,
    // so the exit status says which one the linker opened.
    let main = caller("first dir\n", "other")?;
    let early = archive_of(vec![Member::new(
        "other.o",
        callee_returning("other", 7)?,
        &["other"],
    )])?;
    let late = archive_of(vec![Member::new(
        "other.o",
        callee_returning("other", 60)?,
        &["other"],
    )])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .write_file("d1/libx.a", early)
            .write_file("d2/libx.a", late)
            .lib_dir("d1")
            .lib_dir("d2")
            .lib("x")
            .label("-L d1 -L d2 -l x, with a libx.a in both"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "first dir\n", 7)?;
    ctx.note(
        "exit 7 rather than 60 is the assertion: d1 was searched first and the search stopped \
         at the first directory that had the file",
    );
    Ok(())
});

link_test!(no_search_path_at_all, |ctx| {
    // The archive exists, in a directory the linker was never told about. Note that the
    // link's own working directory is not a search path either.
    let main = caller("never printed\n", "other")?;
    let lib = archive_of(vec![Member::new(
        "other.o",
        callee_returning("other", 7)?,
        &["other"],
    )])?;
    let run = ctx.link_fails(
        &Link::new()
            .object("main.o", main)
            .write_file("libs/libonlyhere.a", lib)
            .lib("onlyhere")
            .label("-l onlyhere with no -L"),
        "linking with a -l and no search path",
    )?;
    let mut c = Check::new("the diagnostic for a library with nowhere to look");
    c.mentions("linker.stderr", "onlyhere", &run.diagnostics());
    if !c.ok() {
        c.note(
            "GNU ld says `cannot find -lonlyhere`; any wording works as long as the name the \
             learner typed is in it",
        );
        c.block("linker command", run.output.command_line());
    }
    c.finish()
});

link_test!(library_not_found, |ctx| {
    // A -L directory that exists and holds an archive — just not this one.
    let main = caller("never printed\n", "other")?;
    let decoy = archive_of(vec![Member::new(
        "other.o",
        callee_returning("other", 7)?,
        &["other"],
    )])?;
    let run = ctx.link_fails(
        &Link::new()
            .object("main.o", main)
            .write_file("libs/libdecoy.a", decoy)
            .lib_dir("libs")
            .lib("absentlib")
            .label("-L libs -l absentlib"),
        "linking against a library that is in none of the search directories",
    )?;
    let mut c = Check::new("the diagnostic for a library that is nowhere");
    c.mentions("linker.stderr", "absentlib", &run.diagnostics());
    if !c.ok() {
        c.note(
            "the directory is there and readable and holds a different archive; only \
             libabsentlib.a is missing",
        );
        c.block("linker command", run.output.command_line());
    }
    c.finish()
});

link_test!(search_path_after_the_l, |ctx| {
    // GNU ld collects every -L before it resolves any -l, so this links. A linker that
    // resolves each -l against the directories seen so far would report "cannot find -lx".
    let main = caller("late -L\n", "other")?;
    let lib = archive_of(vec![Member::new(
        "other.o",
        callee_returning("other", 7)?,
        &["other"],
    )])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .write_file("libs/libx.a", lib)
            .lib("x")
            .lib_dir("libs")
            .label("-l x before the -L that finds it"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "late -L\n", 7)?;
    ctx.note(
        "verified against GNU ld 2.47: the search path is gathered from the whole command \
         line before any -l is resolved, so -L may come after the -l that needs it — collect \
         every -L in a first pass and the position of -l then matters only for link order",
    );
    Ok(())
});

link_test!(relative_search_path, |ctx| {
    // A path with a `./` and more than one component: nothing special, but a linker that
    // concatenates strings without a separator, or that only accepts a bare directory name,
    // trips here.
    let main = caller("relative\n", "other")?;
    let lib = archive_of(vec![Member::new(
        "other.o",
        callee_returning("other", 17)?,
        &["other"],
    )])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .write_file("deep/nested/libs/libx.a", lib)
            .lib_dir("./deep/nested/libs")
            .lib("x")
            .label("-L ./deep/nested/libs -l x"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "relative\n", 17)?;
    Ok(())
});

link_test!(library_beside_objects, |ctx| {
    // main.o -> mid.o -> the member inside the archive that -l found.
    let main = caller("mixed -l\n", "mid")?;
    let mid = tail_call("mid", "other")?;
    let lib = archive_of(vec![Member::new(
        "other.o",
        callee_returning("other", 29)?,
        &["other"],
    )])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .object("mid.o", mid)
            .write_file("libs/libx.a", lib)
            .lib_dir("libs")
            .lib("x")
            .label("two objects and -L libs -l x"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "mixed -l\n", 29)?;
    Ok(())
});

link_test!(prefix_names_do_not_collide, |ctx| {
    // One directory, two archives whose names share a prefix. Both define `other`, with
    // different constants, so a linker that matches loosely picks the wrong one and says so
    // in the exit status.
    let short = archive_of(vec![Member::new(
        "other.o",
        callee_returning("other", 7)?,
        &["other"],
    )])?;
    let long = archive_of(vec![Member::new(
        "other.o",
        callee_returning("other", 99)?,
        &["other"],
    )])?;
    let pick_short = ctx.link_ok(
        &Link::new()
            .object("main.o", caller("short name\n", "other")?)
            .write_file("libs/libx.a", short.clone())
            .write_file("libs/libxy.a", long.clone())
            .lib_dir("libs")
            .lib("x")
            .label("-l x with libx.a and libxy.a both present"),
    )?;
    ctx.expect_output(&pick_short, "short name\n", 7)?;

    let pick_long = ctx.link_ok(
        &Link::new()
            .object("main.o", caller("long name\n", "other")?)
            .write_file("libs/libx.a", short)
            .write_file("libs/libxy.a", long)
            .lib_dir("libs")
            .lib("xy")
            .out("prog_xy")
            .label("-l xy with libx.a and libxy.a both present"),
    )?;
    ctx.expect_output(&pick_long, "long name\n", 99)?;
    ctx.note(
        "`lib` + name + `.a` is an exact file name, not a pattern; the two exit statuses are \
         what separate a join from a search",
    );
    Ok(())
});

link_test!(same_as_naming_the_path, |ctx| {
    // `-L libs -l x` is defined to be a way of writing `libs/libx.a`. Both links are run and
    // both programs have to behave identically.
    let lib = archive_of(vec![Member::new(
        "other.o",
        callee_returning("other", 44)?,
        &["other"],
    )])?;
    let by_path = ctx.link_ok(
        &Link::new()
            .object("main.o", caller("same thing\n", "other")?)
            .archive("libs/libx.a", lib.clone())
            .label("the archive named by path"),
    )?;
    ctx.expect_output(&by_path, "same thing\n", 44)?;

    let by_flag = ctx.link_ok(
        &Link::new()
            .object("main.o", caller("same thing\n", "other")?)
            .write_file("libs/libx.a", lib)
            .lib_dir("libs")
            .lib("x")
            .out("prog_l")
            .label("the same archive through -L and -l"),
    )?;
    ctx.expect_output(&by_flag, "same thing\n", 44)?;

    let mut c = Check::new("that the two ways of naming one archive agree");
    let a = by_path.address_of("other")?;
    let b = by_flag.address_of("other")?;
    c.that(
        "output.symtab['other'].st_shndx",
        "a defined symbol in both links",
        by_path.elf.symbol("other").map(|s| !s.is_undefined()) == Some(true)
            && by_flag.elf.symbol("other").map(|s| !s.is_undefined()) == Some(true),
        format!("0x{a:x} and 0x{b:x}"),
    );
    c.note(
        "the two outputs need not be byte-identical — the inputs' file names differ — but the \
         program they contain is the same one",
    );
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

/// Worked examples: the successful search, and the message when there is nothing to find.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::archive(
            "A library found by name",
            "ld -o prog main.o -L libs -l x",
            || callee_returning("other", 7).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "The command line never mentions a `.a` file. `libs/libx.a` exists on disk and \
             holds one member — the object shown here, which defines `other`. main.o calls \
             `other`",
        )
        .response(
            "The linker turns `-l x` into the file name `libx.a`, finds it in the one \
             directory `-L` named, and links that archive at the position the `-l` occupied: \
             the program prints its line and exits 7",
        )
        .note(
            "Build the name, do not search for it: `lib` + the argument + `.a`. Matching \
             loosely is how `-l x` ends up linking `libxy.a`.",
        )
        .runs("found it\n", 7),
        ExampleSpec::text("Nothing to find", "ld -o prog main.o -L libs -l absentlib")
            .request(
                "`libs/` exists and holds `libdecoy.a`, but there is no `libs/libabsentlib.a` and \
             no other `-L` directory",
            )
            .response(
                "The linker exits non-zero with a diagnostic that contains the string \
             `absentlib`, and writes no output file. GNU ld says `cannot find -labsentlib: No \
             such file or directory`",
            )
            .note(
                "The name in the message is the whole value of it: without it the learner has a \
             command line with six libraries on it and no idea which one is missing.",
            ),
    ]
}
