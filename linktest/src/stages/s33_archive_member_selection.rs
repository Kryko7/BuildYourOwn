//! Stage 33 — Only the members that are needed.
//!
//! This is the rule that makes archives worth having, and it is the one place where a
//! linker's behaviour is not a function of its inputs alone but of the *set of symbols still
//! undefined* at the moment the archive is reached. An object named on the command line is
//! loaded whether or not anything wants it. A member of an archive is loaded only when it
//! defines a symbol that is undefined *right now* — and loading it can make more symbols
//! undefined, so the archive has to be scanned again until a pass takes nothing.
//!
//! Every test below proves the negative the only way it can be proved: by putting something
//! *fatal* inside the member that must not be loaded — a duplicate of a symbol the program
//! already defines, or a reference to a symbol nothing anywhere defines — so that a
//! successful link is itself the evidence that the member stayed shut.

use crate::asm::{Code, Reg, STDOUT};
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
        number: 33,
        slug: "archive_member_selection",
        name: "Only the members that are needed",
        ext: false,
        hints: &[
            "Keep the set of still-undefined symbols as you go; when you reach an archive, \
             load exactly the members that define one of them and nothing else",
            "Loading a member adds its own undefined symbols to that set, so loop over the \
             archive until a whole pass pulls nothing in — one pass is not enough when a \
             member needs a member that sits earlier in the same file",
            "A symbol that is already defined does not make a member needed, and neither \
             does an undefined *weak* reference: a weak reference resolves to zero rather \
             than dragging a definition in",
            "Never validate a member you did not load — its duplicate symbols and its \
             undefined references are somebody else's problem until the day it is needed",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "a duplicate symbol inside an unused member is not a duplicate definition",
                unused_duplicate_is_harmless,
            ),
            Test::new(
                "an unused member's own undefined symbol is not an error",
                unused_undefined_is_harmless,
            ),
            Test::new(
                "a member already defined elsewhere is never pulled in",
                already_defined_wins,
            ),
            Test::new(
                "a member is pulled in transitively by another member's reference",
                transitive_pull,
            ),
            Test::new(
                "a member earlier in the archive is still found on a second pass",
                rescan_finds_earlier_member,
            ),
            Test::new("a member is pulled in for a data symbol", data_symbol_pull),
            Test::new(
                "an undefined weak reference does not pull a member in",
                weak_reference_pulls_nothing,
            ),
            Test::new(
                "a member needed for two symbols is pulled in only once",
                pulled_once,
            ),
            Test::new(
                "the pulled members' symbols are in the output and the unused ones are not",
                symtab_shows_only_what_was_pulled,
            ),
        ],
    }
}

link_test!(unused_duplicate_is_harmless, |ctx| {
    // The second member offers `never_referenced`, which nothing in the link mentions — but
    // it also defines `_start`, which main.o defines too. If the linker loads members it has
    // no reason to load, this link dies with "multiple definition of `_start'"; a successful
    // link is the proof that it did not.
    let main = caller("selective\n", "other")?;
    let lib = archive_of(vec![
        Member::new("used.o", callee_returning("other", 7)?, &["other"]),
        Member::new("decoy.o", decoy("never_referenced")?, &["never_referenced"]),
    ])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .archive("libx.a", lib)
            .label("an archive whose unused member duplicates _start"),
    )?;
    assert_runnable_layout(&linked)?;
    assert_entry_is(&linked, DEFAULT_ENTRY)?;
    ctx.expect_output(&linked, "selective\n", 7)?;
    ctx.note(
        "the unused member's `_start` would collide with main.o's, and its entry point would \
         be the wrong one; the link succeeding and printing is the assertion that the member \
         was never loaded",
    );
    Ok(())
});

link_test!(unused_undefined_is_harmless, |ctx| {
    // The unused member calls a symbol that exists nowhere in the link. Loading it would
    // turn the link into an undefined-reference error.
    let main = caller("still fine\n", "other")?;
    let lib = archive_of(vec![
        Member::new("used.o", callee_returning("other", 13)?, &["other"]),
        Member::new(
            "broken.o",
            defines_and_needs("never_needed", "nowhere_at_all")?,
            &["never_needed"],
        ),
    ])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .archive("libx.a", lib)
            .label("an archive whose unused member is itself unlinkable"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "still fine\n", 13)?;
    let mut c = Check::new("that nothing from the unused member reached the output");
    c.that(
        "output.symtab['nowhere_at_all']",
        "no trace of the unused member's undefined symbol",
        linked.elf.symbol("nowhere_at_all").is_none(),
        linked
            .elf
            .symbol("nowhere_at_all")
            .map(|s| format!("shndx {}", s.shndx)),
    );
    c.finish()
});

link_test!(already_defined_wins, |ctx| {
    // `other` is defined by a plain object that is loaded before the archive is reached, so
    // by the time the archive is scanned `other` is not undefined any more and the member
    // that also defines it is not a candidate. If it were loaded, it would be both a
    // duplicate definition and an undefined reference.
    let main = caller("from the object\n", "other")?;
    let real = callee_returning("other", 40)?;
    let lib = archive_of(vec![Member::new(
        "shadow.o",
        defines_and_needs("other", "nowhere_at_all")?,
        &["other"],
    )])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .object("other.o", real)
            .archive("libx.a", lib)
            .label("an archive offering a symbol an object already defines"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "from the object\n", 40)?;
    ctx.note(
        "the member is a candidate only while the symbol is still undefined; an archive \
         never overrides a definition that is already in the link",
    );
    Ok(())
});

link_test!(transitive_pull, |ctx| {
    // main.o needs `other`; the member that defines `other` needs `deeper`, which lives in a
    // second member nothing mentioned on the command line.
    let main = caller("two deep\n", "other")?;
    let lib = archive_of(vec![
        Member::new("first.o", defines_and_needs("other", "deeper")?, &["other"]),
        Member::new("second.o", callee_returning("deeper", 11)?, &["deeper"]),
    ])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .archive("libx.a", lib)
            .label("an archive whose needed member needs another member"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "two deep\n", 11)?;
    Ok(())
});

link_test!(rescan_finds_earlier_member, |ctx| {
    // The same two members, in the other order: the member that satisfies the new reference
    // sits *before* the member that made it, so a single forward pass misses it and the
    // archive has to be scanned again.
    let main = caller("rescanned\n", "other")?;
    let lib = archive_of(vec![
        Member::new("second.o", callee_returning("deeper", 19)?, &["deeper"]),
        Member::new("first.o", defines_and_needs("other", "deeper")?, &["other"]),
    ])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .archive("libx.a", lib)
            .label("an archive that has to be scanned twice"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "rescanned\n", 19)?;
    ctx.note(
        "GNU ld loops over one archive until a pass adds nothing; a linker that walks the \
         members once, front to back, fails exactly this input",
    );
    Ok(())
});

link_test!(data_symbol_pull, |ctx| {
    // Nothing is *called* here: the only reference into the archive is a load of a datum,
    // and it has to pull its member in just as a call would.
    let main = reads_word("a datum\n", "counter")?;
    let lib = archive_of(vec![
        Member::new("data.o", data_word("counter", 23)?, &["counter"]),
        Member::new(
            "code.o",
            callee_returning("unused_code", 1)?,
            &["unused_code"],
        ),
    ])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .archive("libx.a", lib)
            .label("an archive member needed only for a datum"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "a datum\n", 23)?;
    Ok(())
});

link_test!(weak_reference_pulls_nothing, |ctx| {
    // main.o's reference to `other` is STB_WEAK and undefined. A weak reference is a
    // question ("is this linked in?"), not a demand, so it must not pull the member in — and
    // the member is one that could not be linked if it were.
    let main = weak_probe("weak\n", "other", 5)?;
    let lib = archive_of(vec![Member::new(
        "other.o",
        defines_and_needs("other", "nowhere_at_all")?,
        &["other"],
    )])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .archive("libx.a", lib)
            .label("a weak reference and an archive that could satisfy it"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "weak\n", 5)?;
    let mut c = Check::new("that the weak reference left the member alone");
    let pulled = linked
        .elf
        .symbol("other")
        .map(|s| !s.is_undefined())
        .unwrap_or(false);
    c.that(
        "output.symtab['other']",
        "absent, or still undefined — the member was not loaded",
        !pulled,
        linked
            .elf
            .symbol("other")
            .map(|s| format!("shndx {} value 0x{:x}", s.shndx, s.value)),
    );
    c.note(
        "GNU ld 2.47 leaves the member shut and resolves the weak reference to zero; this is \
         the behaviour `__attribute__((weak))` feature tests depend on",
    );
    if !c.ok() {
        c.block("linker command", linked.run.output.command_line());
    }
    c.finish()
});

link_test!(pulled_once, |ctx| {
    // Both references land in the same member. A linker that loads the member once per
    // satisfied symbol ends up with two copies of `alpha` and `alpha_word` and has to call
    // that a duplicate definition.
    let main = needs_alpha_pair("once only\n")?;
    let mut code = Code::new();
    code.mov_r32_imm32(Reg::Rax, 6);
    code.ret();
    let member = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::data(".data", 30u32.to_le_bytes().to_vec()).align(4))
            .symbol(SymbolSpec::global("alpha", ".text", 0).func())
            .symbol(SymbolSpec::global("alpha_word", ".data", 0).object(4)),
    )?;
    let lib = archive_of(vec![Member::new(
        "pair.o",
        member,
        &["alpha", "alpha_word"],
    )])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .archive("libpair.a", lib)
            .label("one member satisfying two references"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "once only\n", 36)?;
    let mut c = Check::new("that the member's code was not copied in twice");
    let alphas = linked
        .elf
        .symbols
        .iter()
        .filter(|s| s.name == "alpha" && !s.is_undefined())
        .count();
    if alphas > 0 {
        c.at_most("output.symtab['alpha'].definitions", 1usize, alphas);
    } else {
        c.note("the output has no symbol table entry for 'alpha'; the exit status is the proof");
    }
    c.finish()
});

link_test!(symtab_shows_only_what_was_pulled, |ctx| {
    let main = caller("selected\n", "other")?;
    let lib = archive_of(vec![
        Member::new("used.o", callee_returning("other", 7)?, &["other"]),
        Member::new("spare.o", callee_returning("spare", 9)?, &["spare"]),
    ])?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .archive("libx.a", lib)
            .label("one needed member and one spare"),
    )?;
    ctx.expect_output(&linked, "selected\n", 7)?;
    let mut c = Check::new("which of the archive's symbols are in the output");
    let spare = linked.elf.symbol("spare");
    c.that(
        "output.symtab['spare']",
        "absent, or at least not a definition — that member was never needed",
        spare.map(|s| s.is_undefined()).unwrap_or(true),
        spare.map(|s| format!("shndx {} value 0x{:x}", s.shndx, s.value)),
    );
    if let Some(used) = linked.elf.symbol("other") {
        c.that(
            "output.symtab['other']",
            "a definition — that member was needed",
            !used.is_undefined(),
            used.shndx,
        );
    } else {
        c.note(
            "the linker keeps no symbol table for the pulled member either, so only the \
             absence of 'spare' can be checked",
        );
    }
    c.note(
        "a linker that keeps more symbols than GNU ld is not wrong; a linker that keeps a \
         *definition* of 'spare' has loaded a member it had no reason to",
    );
    if !c.ok() {
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

/// An object with two globals at the same place: the name the archive index advertises for
/// it, and a second one that collides with the program's own entry point. Loading it when it
/// was not needed is a duplicate definition; leaving it alone costs nothing.
fn decoy(advertised: &str) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.mov_r32_imm32(Reg::Rax, 0);
    code.ret();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(advertised, ".text", 0).func())
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func()),
    )
}

/// An object defining `def` as a function that calls the undefined `undef` and returns its
/// result — harmless when it is loaded together with a definition of `undef`, fatal when it
/// is loaded on its own.
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

/// An object whose `.data` holds one 32-bit `value` under the global name `name`.
fn data_word(name: &str, value: u32) -> Result<Vec<u8>, Failure> {
    build(
        ObjectBuilder::new()
            .section(SectionSpec::data(".data", value.to_le_bytes().to_vec()).align(4))
            .symbol(SymbolSpec::global(name, ".data", 0).object(4)),
    )
}

/// An object that prints `message` and exits with the 32-bit datum held at the undefined
/// symbol `word` — a reference that is a load, not a call.
fn reads_word(message: &str, word: &str) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "message", message.len() as u32);
    code.mov_r32_rip(Reg::Rax, word, 0);
    code.sys_exit_eax();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::local("message", ".rodata", 0).object(message.len() as u64))
            .symbol(SymbolSpec::undefined(word)),
    )
}

/// An object that prints `message`, mentions `symbol` through an *undefined weak* reference
/// it never calls, and exits with `status` whatever the symbol resolves to.
fn weak_probe(message: &str, symbol: &str, status: u32) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "message", message.len() as u32);
    code.mov_r64_symbol_addr32s(Reg::Rax, symbol, 0);
    code.sys_exit(status);
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::local("message", ".rodata", 0).object(message.len() as u64))
            .symbol(SymbolSpec::weak_undefined(symbol)),
    )
}

/// An object that prints `message`, calls the undefined `alpha`, adds the undefined datum
/// `alpha_word` and exits with the sum.
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

/// Worked examples: the member that must stay shut, and the one that has to be found twice.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::archive(
            "The member that must not be loaded",
            "ld -o prog main.o libx.a",
            || decoy("never_referenced").map_err(|f| f.messages.join("; ")),
        )
        .request(
            "libx.a holds two members. The first defines `other`, which main.o calls. The \
             second is the object shown here: the symbol index offers it as the definition of \
             `never_referenced`, which nothing in the link mentions — but it also defines \
             `_start`, and main.o defines `_start` too",
        )
        .response(
            "A successful link. The first member is read and its `other` is relocated into \
             main.o's call site; the second member is never opened, so its `_start` never \
             becomes a second definition and `multiple definition of \\`_start'` never happens",
        )
        .note(
            "There is no way to observe a member that was not loaded, so the test is built \
             the other way round: put something fatal in it and let the link's success be the \
             proof.",
        )
        .runs("selective\n", 7),
        ExampleSpec::archive(
            "A member found on the second pass",
            "ld -o prog main.o libx.a",
            || defines_and_needs("other", "deeper").map_err(|f| f.messages.join("; ")),
        )
        .request(
            "libx.a's members are, in order: one defining `deeper`, then the object shown \
             here, which defines `other` and calls `deeper`. main.o calls `other` and nothing \
             calls `deeper`",
        )
        .response(
            "The scan pulls the second member in for `other`, which makes `deeper` undefined. \
             That member sits *earlier* in the archive, so a front-to-back pass has already \
             gone past it: the archive must be scanned again before the link can succeed and \
             the program exit 19",
        )
        .note(
            "`while (a pass loaded something) scan again;` is the whole fix, and it is why an \
             archive's member order does not matter while the order of archives on the \
             command line does.",
        )
        .runs("rescanned\n", 19),
    ]
}
