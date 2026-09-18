//! Stage 36 — Groups and broken symbol indexes.
//!
//! Two things that only come up once archives work at all. The first is `--start-group` /
//! `--end-group`, the escape hatch for a dependency that runs in a circle: the archives
//! between the two flags are rescanned as a set until a whole pass takes nothing, which is
//! exactly stage 33's fixpoint loop lifted one level up. The second is what happens when the
//! `/` symbol index is wrong — stale, missing, or naming symbols no member defines — which is
//! a real thing that happens when `ar q` is used without `s`, or when a build system copies
//! members between archives.
//!
//! The invariant that runs through all of it: **a broken input is refused with a diagnostic
//! or survived correctly, and nothing crashes, loops or hangs.** Where GNU ld and a careful
//! hand-written linker could reasonably differ — a stale index is the interesting case — the
//! test accepts either outcome and says so in a note, because the index is a cache and
//! whether you trust a cache is a design decision.

use crate::asm::Code;
use crate::assert::{Check, Failure};
use crate::elf::archive::{ArchiveBuilder, IndexMode, Member};
use crate::elf::write::{ObjectBuilder, SymbolSpec};
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// The symbol [`IndexMode::Phantom`] invents: it is in the index and in no member.
const PHANTOM: &str = "linktest_phantom_symbol";

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 36,
        slug: "archive_edge_cases",
        name: "Groups and broken symbol indexes",
        ext: true,
        hints: &[
            "`--start-group`/`--end-group` bracket a set of archives that is rescanned as a \
             whole until a pass pulls nothing in; the loop must terminate on the first \
             barren pass, not on a fixed number of tries",
            "Treat the `/` index as a hint you verify, not as the truth: after opening the \
             member it points at, check that the member really does define the symbol before \
             you count it as resolved",
            "An archive with no `/` member at all is still a perfectly good archive — read \
             each member's own symbol table instead",
            "Every malformed archive here has to end in a diagnostic and a non-zero exit, \
             never a panic, an infinite loop or a half-written output file",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "two archives with a circular dependency do not link on their own",
                circular_without_a_group,
            )
            .ext(),
            Test::new(
                "the same two archives inside --start-group/--end-group link",
                circular_inside_a_group,
            )
            .ext(),
            Test::new(
                "a group from which nothing is needed still terminates",
                empty_group_terminates,
            )
            .ext(),
            Test::new(
                "an archive with no symbol index at all still resolves its members",
                missing_index,
            )
            .ext(),
            Test::new(
                "a stale symbol index is either seen through or refused by name",
                stale_index,
            )
            .ext(),
            Test::new(
                "an index entry no member backs is harmless while nothing references it",
                phantom_index_entry,
            )
            .ext(),
            Test::new(
                "referencing the phantom symbol is an ordinary undefined reference",
                phantom_symbol_referenced,
            )
            .ext(),
            Test::new("a truncated archive is refused", truncated_archive).ext(),
            Test::new("an archive with broken magic is refused", broken_magic).ext(),
        ],
    }
}

link_test!(circular_without_a_group, |ctx| {
    // liba's a1.o needs b_fn from libb; libb's b1.o needs a_other, which is a *second* member
    // of liba. By the time that reference is made, liba has been scanned and passed.
    let inputs = circular_trio("never printed\n")?;
    let run = ctx.link_fails(
        &Link::new()
            .object("main.o", inputs.main)
            .archive("liba.a", inputs.liba)
            .archive("libb.a", inputs.libb)
            .label("two archives that need each other, with no group"),
        "linking two mutually dependent archives without a group",
    )?;
    let mut c = Check::new("the diagnostic for an unresolved circular dependency");
    c.mentions("linker.stderr", "a_other", &run.diagnostics());
    if !c.ok() {
        c.note(
            "the reference that cannot be satisfied is the one libb's member makes back into \
             liba, after liba is already behind the scan",
        );
        c.block("linker command", run.output.command_line());
    }
    c.finish()
});

link_test!(circular_inside_a_group, |ctx| {
    // The same three inputs, with the two archives bracketed. The group is rescanned, liba
    // is reopened, and its second member supplies a_other.
    let inputs = circular_trio("grouped\n")?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", inputs.main)
            .arg("--start-group")
            .archive("liba.a", inputs.liba)
            .archive("libb.a", inputs.libb)
            .arg("--end-group")
            .label("the same two archives inside a group"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "grouped\n", 21)?;
    ctx.note(
        "the inputs are identical to the failing link above; only the two flags around them \
         changed",
    );
    Ok(())
});

link_test!(empty_group_terminates, |ctx| {
    // Nothing inside the group is needed by anything. The rescan loop has to notice that its
    // first pass took nothing and stop; a loop that only stops when the archives are
    // exhausted never stops at all.
    let main = print_and_exit("nothing taken\n", 3)?;
    let liba = archive_of(vec![Member::new(
        "a.o",
        callee_returning("unused_a", 1)?,
        &["unused_a"],
    )])?;
    let libb = archive_of(vec![Member::new(
        "b.o",
        callee_returning("unused_b", 2)?,
        &["unused_b"],
    )])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .arg("--start-group")
            .archive("liba.a", liba)
            .archive("libb.a", libb)
            .arg("--end-group")
            .label("a group nothing needs"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "nothing taken\n", 3)?;
    Ok(())
});

link_test!(missing_index, |ctx| {
    // No `/` member at all. The member that defines `other` is the *second* one, so a linker
    // that falls back to "the first member" rather than to the members' own symbol tables
    // gets this wrong.
    let main = caller("no index\n", "other")?;
    let lib = indexed_archive(
        IndexMode::Omitted,
        vec![
            Member::new("spare.o", callee_returning("spare", 1)?, &["spare"]),
            Member::new("other.o", callee_returning("other", 7)?, &["other"]),
        ],
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .archive("libnoindex.a", lib)
            .label("an archive with no symbol index"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "no index\n", 7)?;
    ctx.note(
        "verified against GNU ld 2.47: an archive without a `/` member is accepted and its \
         members' own symbol tables are read — `ar` writes the index as a convenience, not as \
         part of the format's meaning",
    );
    Ok(())
});

link_test!(stale_index, |ctx| {
    // Every entry in this index points at the *first* member, which is what `ar q` leaves
    // behind once members have moved. `other` is really in the second member, so the index
    // is a lie: following it lands on a member that does not define what was asked for.
    let main = caller("stale\n", "other")?;
    let lib = indexed_archive(
        IndexMode::Stale,
        vec![
            Member::new("spare.o", callee_returning("spare", 1)?, &["spare"]),
            Member::new("other.o", callee_returning("other", 7)?, &["other"]),
        ],
    )?;
    let link = Link::new()
        .object("main.o", main)
        .archive("libstale.a", lib)
        .label("an archive whose index points at the wrong members");
    let run = ctx.link(&link)?;
    let mut c = Check::new("what a stale symbol index led to");
    c.that(
        "linker.signal",
        "a linker that finished by itself, whatever it decided",
        run.output.signal.is_none() && !run.output.timed_out,
        run.output.status_line(),
    );
    c.finish()?;
    if run.succeeded() {
        ctx.note(
            "this linker distrusts the index and found the member by its own symbol table; \
             that is the stronger behaviour and the program below proves it landed on the \
             right member",
        );
        let linked = ctx.link_ok(&link)?;
        assert_runnable_layout(&linked)?;
        ctx.expect_output(&linked, "stale\n", 7)?;
    } else {
        let mut c = Check::new("the diagnostic for a stale symbol index");
        c.that(
            "linker.stderr",
            "a diagnostic saying something",
            !run.output.stderr.trim().is_empty(),
            "(empty)",
        );
        c.mentions("linker.stderr", "other", &run.diagnostics());
        if !c.ok() {
            c.block("linker command", run.output.command_line());
            c.block("linker output", run.diagnostics());
        }
        c.finish()?;
        ctx.note(
            "verified against GNU ld 2.47: ld trusts the index, opens the first member, does \
             not find `other` there and reports `undefined reference to 'other'`. Both \
             answers are defensible — the index is a cache — so this test accepts either, and \
             only insists the linker terminates and names the symbol when it refuses",
        );
    }
    Ok(())
});

link_test!(phantom_index_entry, |ctx| {
    // The index advertises a symbol no member defines. Nothing in this link asks for it, so
    // it must cost nothing: a linker that pre-loads every member the index names, or that
    // validates the index up front, fails a link that has nothing wrong with it.
    let main = caller("phantom\n", "other")?;
    let lib = indexed_archive(
        IndexMode::Phantom,
        vec![Member::new(
            "other.o",
            callee_returning("other", 7)?,
            &["other"],
        )],
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .archive("libphantom.a", lib)
            .label("an archive whose index names a symbol no member has"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "phantom\n", 7)?;
    ctx.note(format!(
        "the index claims '{PHANTOM}' is in the first member and it is not; GNU ld 2.47 links \
         this without a word because nothing ever asks for it"
    ));
    Ok(())
});

link_test!(phantom_symbol_referenced, |ctx| {
    // Now something does ask for it. The index says yes, the member says no, and the answer
    // has to be the ordinary undefined-symbol error — not a crash, and not a silent zero.
    let main = caller("never printed\n", PHANTOM)?;
    let lib = indexed_archive(
        IndexMode::Phantom,
        vec![Member::new(
            "other.o",
            callee_returning("other", 7)?,
            &["other"],
        )],
    )?;
    let run = ctx.link_fails(
        &Link::new()
            .object("main.o", main)
            .archive("libphantom.a", lib)
            .label("a reference to a symbol only the index believes in"),
        "linking a reference to a symbol the index invents",
    )?;
    let mut c = Check::new("the diagnostic for a symbol only the index has");
    c.mentions("linker.stderr", PHANTOM, &run.diagnostics());
    if !c.ok() {
        c.note(
            "opening the member the index named and finding nothing there is not an internal \
             error: the symbol is simply undefined, and the message is the usual one",
        );
        c.block("linker command", run.output.command_line());
    }
    c.finish()
});

link_test!(truncated_archive, |ctx| {
    // Cut eight bytes off the end, which lands inside the last member's payload: the member
    // header promises more bytes than the file has.
    let main = caller("never printed\n", "other")?;
    let whole = archive_of(vec![Member::new(
        "other.o",
        callee_returning("other", 7)?,
        &["other"],
    )])?;
    let cut = whole.len().saturating_sub(8);
    let lib = truncated_archive_bytes(
        vec![Member::new(
            "other.o",
            callee_returning("other", 7)?,
            &["other"],
        )],
        cut,
    )?;
    let mut c = Check::new("that the fixture really is short");
    c.that(
        "fixture.archive.len",
        "fewer bytes than the members' headers promise",
        lib.len() < whole.len(),
        format!("{} of {}", lib.len(), whole.len()),
    );
    c.finish()?;
    let run = ctx.link_fails(
        &Link::new()
            .object("main.o", main)
            .archive("libcut.a", lib)
            .label("an archive cut short inside its last member"),
        "linking a truncated archive",
    )?;
    let said = run.diagnostics();
    let mut c = Check::new("the diagnostic for a truncated archive");
    c.that(
        "linker.stderr",
        "a message naming the archive or the symbol that could not be resolved",
        said.contains("libcut.a") || said.contains("other"),
        crate::assert::one_line(&said, 200),
    );
    c.note(
        "verified against GNU ld 2.47: it refuses with `libcut.a: error adding symbols: file \
         format not recognized`. Either half of the message is enough — what is not allowed \
         is a crash, a hang, or a link that quietly succeeds on a file that stops mid-member",
    );
    if !c.ok() {
        c.block("linker command", run.output.command_line());
    }
    c.finish()
});

link_test!(broken_magic, |ctx| {
    // `!<arch>!` instead of `!<arch>\n`. Nothing else about the file is wrong, which is the
    // point: a linker that dispatches on the `.a` in the file name rather than on the magic
    // sails straight past this.
    let main = caller("never printed\n", "other")?;
    let lib = ArchiveBuilder::new()
        .member(Member::new(
            "other.o",
            callee_returning("other", 7)?,
            &["other"],
        ))
        .magic_override(*b"!<arch>!")
        .build()
        .map_err(|e| Failure::harness(format!("the suite could not build an archive: {e}")))?;
    let run = ctx.link_fails(
        &Link::new()
            .object("main.o", main)
            .archive("libbad.a", lib)
            .label("an archive whose magic is wrong"),
        "linking a file that claims to be an archive and is not",
    )?;
    let said = run.diagnostics();
    let mut c = Check::new("the diagnostic for a file with broken archive magic");
    c.that(
        "linker.stderr",
        "a message naming the file or the symbol it was supposed to supply",
        said.contains("libbad.a") || said.contains("other"),
        crate::assert::one_line(&said, 200),
    );
    c.note(
        "verified against GNU ld 2.47: `libbad.a: file format not recognized; treating as \
         linker script`, then a syntax error, exit 1 — it decides what a file is from its \
         first bytes and never from its name",
    );
    if !c.ok() {
        c.block("linker command", run.output.command_line());
    }
    c.finish()
});

/// A correct archive built from `members`, in the order given.
fn archive_of(members: Vec<Member>) -> Result<Vec<u8>, Failure> {
    indexed_archive(IndexMode::Correct, members)
}

/// The same, with a deliberately broken (or absent) symbol index.
fn indexed_archive(mode: IndexMode, members: Vec<Member>) -> Result<Vec<u8>, Failure> {
    let mut builder = ArchiveBuilder::new().index(mode);
    for m in members {
        builder = builder.member(m);
    }
    builder
        .build()
        .map_err(|e| Failure::harness(format!("the suite could not build an archive: {e}")))
}

/// A correct archive cut off after `bytes` bytes.
fn truncated_archive_bytes(members: Vec<Member>, bytes: usize) -> Result<Vec<u8>, Failure> {
    let mut builder = ArchiveBuilder::new().truncate_to(bytes);
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

/// The three inputs of the circular-dependency tests.
struct Circular {
    /// The object that calls `a_fn`.
    main: Vec<u8>,
    /// Two members: `a_fn`, which calls `b_fn`, and `a_other`.
    liba: Vec<u8>,
    /// One member: `b_fn`, which calls `a_other` back inside `liba`.
    libb: Vec<u8>,
}

/// Build them: `main.o` calls `a_fn`; `liba.a`'s first member defines `a_fn` and calls
/// `b_fn`; `libb.a`'s only member defines `b_fn` and calls `a_other`, which is in `liba.a`'s
/// *second* member. The cycle runs a → b → a.
fn circular_trio(message: &str) -> Result<Circular, Failure> {
    let main = caller(message, "a_fn")?;
    let liba = archive_of(vec![
        Member::new("a1.o", defines_and_needs("a_fn", "b_fn")?, &["a_fn"]),
        Member::new("a2.o", callee_returning("a_other", 21)?, &["a_other"]),
    ])?;
    let libb = archive_of(vec![Member::new(
        "b1.o",
        defines_and_needs("b_fn", "a_other")?,
        &["b_fn"],
    )])?;
    Ok(Circular { main, liba, libb })
}

/// Worked examples: the circle, and the index that lies.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::archive(
            "A dependency that runs in a circle",
            "ld -o prog main.o --start-group liba.a libb.a --end-group",
            || defines_and_needs("b_fn", "a_other").map_err(|f| f.messages.join("; ")),
        )
        .request(
            "liba.a has two members: one defines `a_fn` and calls `b_fn`, the other defines \
             `a_other`. libb.a has the single member shown here, which defines `b_fn` and \
             calls `a_other` — back into liba.a. main.o calls `a_fn`",
        )
        .response(
            "Inside the group the two archives are rescanned as a set: the first pass takes \
             a1.o and b1.o and leaves `a_other` undefined, the second pass reopens liba.a and \
             takes a2.o, the third takes nothing and the loop stops. The program exits 21. \
             Without the two flags the same inputs fail with `undefined reference to \
             \\`a_other'`",
        )
        .note(
            "The loop must stop on the first pass that pulls nothing in. Counting iterations, \
             or stopping when the archives are 'exhausted', either quits too early or never \
             quits.",
        )
        .runs("grouped\n", 21),
        ExampleSpec::archive(
            "An index that points at the wrong member",
            "ld -o prog main.o libstale.a",
            || callee_returning("other", 7).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "libstale.a holds two members: `spare.o` and then the object shown here, which \
             defines `other`. Its `/` index lists both symbols but gives both of them the \
             offset of the *first* member — the state `ar q` leaves an archive in when a \
             member is appended without rebuilding the index",
        )
        .response(
            "Either answer is accepted. GNU ld 2.47 follows the index, opens spare.o, does not \
             find `other` and exits 1 with `undefined reference to \\`other'`. A linker that \
             treats the index as a hint and verifies the member — or that ignores the index \
             and reads each member's symbol table — links the program and it exits 7. What is \
             not acceptable is a crash, a hang, or a silently wrong symbol value",
        )
        .note(
            "The index is a cache of information that is already in the members. Trusting it \
             blindly is fast and fragile; verifying it after the lookup costs one symbol table \
             read and cannot be wrong.",
        ),
    ]
}
