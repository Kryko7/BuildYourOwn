//! Stage 34 — Order on the command line is semantics.
//!
//! A linker reads its inputs left to right, once, and an archive is only asked to supply the
//! symbols that are undefined at the moment it is reached. That single sentence explains the
//! oldest confusing error message in the toolchain: `undefined reference to 'foo'` from a
//! link whose command line plainly contains a library defining `foo` — it was just listed
//! before the object that needed it.
//!
//! The same left-to-right rule has a quieter consequence for plain objects: they are all
//! loaded, so their order cannot change *whether* the link succeeds, but it does decide the
//! order their sections are concatenated in, and therefore every address in the output. Both
//! halves are tested here, and neither is the linker being clever: a linker that sorts its
//! inputs, or that keeps re-scanning archives it has already passed, is a different linker
//! with different answers.

use crate::asm::{Code, STDOUT};
use crate::assert::{Check, Failure};
use crate::elf::archive::{ArchiveBuilder, Member};
use crate::elf::write::{ObjectBuilder, SectionSpec, SymbolSpec};
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 34,
        slug: "link_order",
        name: "Order on the command line is semantics",
        ext: false,
        hints: &[
            "Walk the inputs strictly left to right and keep one set of still-undefined \
             symbols; an archive reached before the reference that needs it simply has \
             nothing to offer and is dropped",
            "Do not go back: once an archive has been scanned and taken nothing, a reference \
             made by a later input does not reopen it — `--start-group` is the opt-in for \
             that, and it is stage 36",
            "Two definitions of one symbol in two archives is not an error: the first \
             archive on the command line supplies it and the second is simply never asked",
            "Objects are concatenated in command-line order, so the first object's .text is \
             the one that gets the lowest address — sorting inputs by name or by size changes \
             every address in the output",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "an archive before the object that needs it resolves nothing",
                archive_before_object,
            ),
            Test::new(
                "the same archive after the object works",
                archive_after_object,
            ),
            Test::new(
                "a reference made after the archive has gone past is not resolved",
                reference_after_the_archive,
            ),
            Test::new(
                "two archives in dependency order link",
                two_archives_in_order,
            ),
            Test::new(
                "the same two archives in the wrong order do not",
                two_archives_reversed,
            ),
            Test::new(
                "a symbol defined in two archives is taken from the first one",
                first_archive_wins,
            ),
            Test::new(
                "the same archive listed twice is not an error",
                archive_twice,
            ),
            Test::new(
                "an object after the archive still satisfies what the member needed",
                object_after_archive,
            ),
            Test::new(
                "the order of the objects is the order their sections are concatenated in",
                objects_concatenate_in_order,
            ),
        ],
    }
}

link_test!(archive_before_object, |ctx| {
    // Nothing is undefined when libx.a is reached, so its member is not a candidate; main.o
    // is read afterwards and `other` stays undefined for good.
    let main = caller("never printed\n", "other")?;
    let lib = archive_of(vec![Member::new(
        "other.o",
        callee_returning("other", 7)?,
        &["other"],
    )])?;
    let run = ctx.link_fails(
        &Link::new()
            .archive("libx.a", lib)
            .object("main.o", main)
            .label("an archive listed before the object that needs it"),
        "linking an archive that comes before the reference it could satisfy",
    )?;
    let mut c = Check::new("the diagnostic for an archive that came too early");
    c.mentions("linker.stderr", "other", &run.diagnostics());
    if !c.ok() {
        c.note(
            "the name is the only actionable part: a learner reading it has to realise the \
             library is on the command line, just in the wrong place",
        );
        c.block("linker command", run.output.command_line());
    }
    c.finish()
});

link_test!(archive_after_object, |ctx| {
    // Byte-for-byte the same inputs as the test above, in the other order.
    let main = caller("ordered\n", "other")?;
    let lib = archive_of(vec![Member::new(
        "other.o",
        callee_returning("other", 7)?,
        &["other"],
    )])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .archive("libx.a", lib)
            .label("the same archive, after the object"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "ordered\n", 7)?;
    ctx.note(
        "the inputs are identical to the failing link above; only their position on the \
         command line changed",
    );
    Ok(())
});

link_test!(reference_after_the_archive, |ctx| {
    // main.o needs `late`, which late.o defines; late.o calls `other`, which only the
    // archive defines — but the archive was reached before late.o made that reference.
    let main = caller("late reference\n", "late")?;
    let late = defines_and_needs("late", "other")?;
    let lib = archive_of(vec![Member::new(
        "other.o",
        callee_returning("other", 7)?,
        &["other"],
    )])?;
    let run = ctx.link_fails(
        &Link::new()
            .object("main.o", main)
            .archive("libx.a", lib)
            .object("late.o", late)
            .label("an object whose reference is made after the archive"),
        "linking an object that needs an archive already gone past",
    )?;
    let mut c = Check::new("the diagnostic for a reference made too late");
    c.mentions("linker.stderr", "other", &run.diagnostics());
    if !c.ok() {
        c.note(
            "an archive is scanned once, at the position it occupies; a linker that keeps \
             every archive it has seen open is more forgiving than every real one",
        );
        c.block("linker command", run.output.command_line());
    }
    c.finish()
});

link_test!(two_archives_in_order, |ctx| {
    // libp's member needs `q_fn`, which libq supplies. libp comes first, so `q_fn` is
    // undefined by the time libq is reached.
    let main = caller("p then q\n", "p_fn")?;
    let libp = archive_of(vec![Member::new(
        "p.o",
        defines_and_needs("p_fn", "q_fn")?,
        &["p_fn"],
    )])?;
    let libq = archive_of(vec![Member::new(
        "q.o",
        callee_returning("q_fn", 12)?,
        &["q_fn"],
    )])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .archive("libp.a", libp)
            .archive("libq.a", libq)
            .label("two archives in dependency order"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "p then q\n", 12)?;
    Ok(())
});

link_test!(two_archives_reversed, |ctx| {
    // The same two archives, the other way round: when libq is scanned nothing needs `q_fn`
    // yet, and by the time libp makes it undefined libq is behind us.
    let main = caller("never printed\n", "p_fn")?;
    let libp = archive_of(vec![Member::new(
        "p.o",
        defines_and_needs("p_fn", "q_fn")?,
        &["p_fn"],
    )])?;
    let libq = archive_of(vec![Member::new(
        "q.o",
        callee_returning("q_fn", 12)?,
        &["q_fn"],
    )])?;
    let run = ctx.link_fails(
        &Link::new()
            .object("main.o", main)
            .archive("libq.a", libq)
            .archive("libp.a", libp)
            .label("two archives in the wrong order"),
        "linking two archives whose dependency runs backwards",
    )?;
    let mut c = Check::new("the diagnostic for archives in the wrong order");
    c.mentions("linker.stderr", "q_fn", &run.diagnostics());
    c.note(
        "this is why a real link line ends `-lfoo -lbar` with the dependencies last, and why \
         build systems sometimes list the same library twice",
    );
    if !c.ok() {
        c.block("linker command", run.output.command_line());
    }
    c.finish()
});

link_test!(first_archive_wins, |ctx| {
    // Two archives, one symbol, two different bodies. Not a duplicate definition: the second
    // archive is never asked, because by the time it is reached `other` is defined.
    let main = caller("first wins\n", "other")?;
    let first = archive_of(vec![Member::new(
        "other.o",
        callee_returning("other", 3)?,
        &["other"],
    )])?;
    let second = archive_of(vec![Member::new(
        "other.o",
        callee_returning("other", 4)?,
        &["other"],
    )])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .archive("lib1.a", first)
            .archive("lib2.a", second)
            .label("two archives both offering 'other'"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "first wins\n", 3)?;
    ctx.note(
        "exit 3 rather than 4 is the whole assertion: the first archive supplied the body, \
         and the second was not an error",
    );
    Ok(())
});

link_test!(archive_twice, |ctx| {
    // Naming the same archive twice is what build systems do to work around a circular
    // dependency. The second mention finds nothing left to take, and that is not an error —
    // unlike the same *object* twice, which is a duplicate definition.
    let main = caller("twice listed\n", "other")?;
    let lib = archive_of(vec![Member::new(
        "other.o",
        callee_returning("other", 8)?,
        &["other"],
    )])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .archive("libx.a", lib)
            .arg("libx.a")
            .label("the same archive named twice"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "twice listed\n", 8)?;
    ctx.note(
        "the member is pulled in by the first mention; the second finds `other` already \
         defined and takes nothing, so there is no duplicate definition",
    );
    Ok(())
});

link_test!(object_after_archive, |ctx| {
    // The member pulled out of libx.a needs `deeper`, and `deeper` is in a plain object
    // listed after the archive. Objects are always loaded, whatever their position, so this
    // one resolves the member's reference even though it comes later.
    let main = caller("object last\n", "other")?;
    let lib = archive_of(vec![Member::new(
        "other.o",
        defines_and_needs("other", "deeper")?,
        &["other"],
    )])?;
    let deeper = callee_returning("deeper", 11)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .archive("libx.a", lib)
            .object("deeper.o", deeper)
            .label("an object that satisfies what the archive member needed"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "object last\n", 11)?;
    ctx.note(
        "the asymmetry is the point: a *reference* made after an archive is too late, but a \
         *definition* given after one is not, because objects are never conditional",
    );
    Ok(())
});

link_test!(objects_concatenate_in_order, |ctx| {
    // Two objects with a .rodata each. a.o writes six bytes starting at its own three-byte
    // string, so what comes out tells you which .rodata was placed second.
    let a = spans_two_strings("first_msg", 6)?;
    let b = rodata_only("second_msg", "BB\n")?;
    let forward = ctx.link_ok(
        &Link::new()
            .object("a.o", a.clone())
            .object("b.o", b.clone())
            .label("a.o then b.o"),
    )?;
    assert_runnable_layout(&forward)?;
    ctx.expect_output(&forward, "AAABB\n", 0)?;

    let reverse = ctx.link_ok(
        &Link::new()
            .object("b.o", b)
            .object("a.o", a)
            .out("reversed")
            .label("b.o then a.o"),
    )?;
    assert_runnable_layout(&reverse)?;

    let mut c = Check::new("which object's .rodata was placed first");
    let fwd_first = forward.address_of("first_msg")?;
    let fwd_second = forward.address_of("second_msg")?;
    let rev_first = reverse.address_of("first_msg")?;
    let rev_second = reverse.address_of("second_msg")?;
    c.that(
        "output.symtab['first_msg'] < output.symtab['second_msg']",
        "a.o's .rodata below b.o's when a.o is named first",
        fwd_first < fwd_second,
        format!("0x{fwd_first:x} vs 0x{fwd_second:x}"),
    );
    c.that(
        "reversed.symtab['second_msg'] < reversed.symtab['first_msg']",
        "b.o's .rodata below a.o's when b.o is named first",
        rev_second < rev_first,
        format!("0x{rev_second:x} vs 0x{rev_first:x}"),
    );
    c.note(
        "the addresses themselves are the linker's own business; what is asserted is only \
         their order relative to the command line",
    );
    if !c.ok() {
        c.block("linker command", reverse.run.output.command_line());
        c.block("output section headers", reverse.elf.section_header_table());
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

/// An object defining `def` as a function that calls the undefined `undef` and returns its
/// result.
fn defines_and_needs(def: &str, undef: &str) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.call(undef);
    code.ret();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(def, ".text", 0).func())
            .symbol(SymbolSpec::undefined(undef)),
    )
}

/// An object whose `.rodata` is three bytes long but whose `write` asks for `len` of them,
/// so the output shows whatever the linker concatenated after it.
fn spans_two_strings(symbol: &str, len: u32) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, symbol, len);
    code.sys_exit(0);
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"AAA".to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global(symbol, ".rodata", 0).object(3)),
    )
}

/// An object with nothing but a `.rodata` string under a global name.
fn rodata_only(symbol: &str, text: &str) -> Result<Vec<u8>, Failure> {
    build(
        ObjectBuilder::new()
            .section(SectionSpec::rodata(".rodata", text.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(symbol, ".rodata", 0).object(text.len() as u64)),
    )
}

/// Worked examples: the classic wrong order, and the concatenation rule.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::error(
            "The library listed first",
            "ld -o prog libx.a main.o",
            || callee_returning("other", 7).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "libx.a holds exactly one member — the object shown here, which defines `other`. \
             main.o calls `other`. The only thing wrong with this command line is that the \
             archive is on the left of the object",
        )
        .response(
            "The linker refuses: when libx.a is reached nothing is undefined, so no member is \
             a candidate and the archive contributes nothing; main.o is read next and leaves \
             `other` undefined. GNU ld exits 1 with `undefined reference to \\`other'` and \
             writes no output file",
        )
        .note(
            "Swapping the two arguments makes the identical inputs link and run. This is the \
             single most common confusing linker error there is.",
        ),
        ExampleSpec::object("Two objects, one .rodata", "ld -o prog a.o b.o", || {
            spans_two_strings("first_msg", 6).map_err(|f| f.messages.join("; "))
        })
        .request(
            "a.o, shown here, holds `AAA` in .rodata under the global `first_msg` and asks \
             write(1, first_msg, 6) for six bytes. b.o holds `BB\\n` in its own .rodata under \
             `second_msg`",
        )
        .response(
            "The two `.rodata` sections are concatenated in command-line order, so \
             `second_msg` lands immediately after `first_msg` and the six bytes the program \
             writes are `AAABB\\n`. Name b.o first and the same program prints something else",
        )
        .note(
            "Reading past the end of a symbol is not something a real program should do; here \
             it is the cheapest possible probe for the order the linker chose.",
        )
        .runs("AAABB\n", 0),
    ]
}
