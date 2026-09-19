//! Stage 44 — `--gc-sections`: deleting what nothing can reach.
//!
//! Compile with `-ffunction-sections` and every function lands in its own section:
//! `.text.parse`, `.text.print`, `.text.main`. That on its own does nothing useful. What it
//! enables is `--gc-sections`, where the linker treats sections as nodes in a graph,
//! relocations as edges, marks everything reachable from a set of roots, and deletes the
//! rest. It is how a binary that links a large library ends up carrying only the parts it
//! called.
//!
//! The algorithm is a mark and sweep and the interesting part is choosing the roots. The
//! entry point's section is obviously one. Less obviously, so is `.init_array` — nothing
//! *references* a constructor, that is the whole point of a constructor, so a collector
//! that marks only from the entry point deletes every constructor in the program and the
//! failure appears at run time as initialisation that silently did not happen. The same
//! goes for anything a linker script has marked `KEEP`.
//!
//! Reachability is transitive and it runs through data as well as code: a function that
//! loads a string keeps the section that string is in, and a function reachable only from a
//! section that was itself deleted goes with it.

use crate::asm::{Code, Reg, STDOUT};
use crate::assert::{Check, Failure};
use crate::elf::write::{ObjectBuilder, Reloc, SectionSpec, SymbolSpec};
use crate::elf::*;
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 44,
        slug: "gc_sections",
        name: "--gc-sections: deleting what nothing reaches",
        ext: true,
        hints: &[
            "Treat allocated sections as nodes and relocations as edges: mark everything \
             reachable from the roots, then drop every section that was not marked",
            "The entry point's section is a root, and so is `.init_array` — nothing references \
             a constructor, so marking only from the entry deletes them all",
            "Reachability is transitive and runs through data too: a function that references a \
             string keeps the section holding the string",
            "Without the flag nothing is dropped, so the same inputs must link both ways and \
             differ only in what came out",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "without the flag, an unreachable section is kept",
                kept_without_the_flag,
            )
            .ext(),
            Test::new(
                "with it, the unreachable section is gone",
                dropped_with_the_flag,
            )
            .ext(),
            Test::new(
                "what the entry point calls is kept",
                the_entry_point_is_a_root,
            )
            .ext(),
            Test::new(
                "and what that calls, transitively",
                reachability_is_transitive,
            )
            .ext(),
            Test::new(
                "a section reachable only from a dropped one goes too",
                unreachable_through_a_dropped_section,
            )
            .ext(),
            Test::new(
                "data a kept function references is kept",
                data_is_reachable_too,
            )
            .ext(),
            Test::new(
                ".init_array is a root, so constructors survive",
                init_array_is_a_root,
            )
            .ext(),
            Test::new("the collected program still runs", the_program_still_runs).ext(),
            Test::new(
                "collecting does not change what the program does",
                gc_changes_nothing_observable,
            )
            .ext(),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------------------

/// A function in its own section, optionally calling another.
fn func_section(section: &str, calls: Option<&str>) -> SectionSpec {
    let mut code = Code::new();
    if let Some(target) = calls {
        code.call(target);
    }
    code.ret();
    text_named(section, &code)
}

/// One object: `_start` in its own section, plus whatever else the caller asks for.
fn program(
    start_calls: Option<&str>,
    extras: Vec<(SectionSpec, SymbolSpec)>,
) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    if let Some(target) = start_calls {
        code.call(target);
    }
    code.sys_exit(0);
    let mut b = ObjectBuilder::new()
        .section(text_named(".text._start", &code))
        .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text._start", 0).func());
    for (section, symbol) in extras {
        b = b.section(section).symbol(symbol);
    }
    build(b)
}

fn stage_examples() -> Vec<ExampleSpec> {
    vec![ExampleSpec::object(
        "A constructor nothing calls, under --gc-sections",
        "ld --gc-sections -o prog a.o",
        || {
            let mut ctor = Code::new();
            ctor.ret();
            let mut start = Code::new();
            start.sys_exit(0);
            build(
                ObjectBuilder::new()
                    .section(text_named(".text._start", &start))
                    .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text._start", 0).func())
                    .section(text_named(".text.ctor", &ctor))
                    .symbol(SymbolSpec::global("ctor", ".text.ctor", 0).func())
                    .section(
                        SectionSpec::new(
                            ".init_array",
                            SHT_INIT_ARRAY,
                            SHF_ALLOC | SHF_WRITE,
                            vec![0; 8],
                        )
                        .align(8)
                        .entsize(8)
                        .reloc(Reloc::sym(0, "ctor", R_X86_64_64, 0)),
                    ),
            )
            .map_err(|f| f.messages.join("; "))
        },
    )
    .request(
        "a.o: a _start in .text._start, a ctor in .text.ctor that nothing calls, and an \
         .init_array whose single entry is relocated to ctor",
    )
    .response("Both .init_array and ctor are in the output, and nothing else is dropped either")
    .note(
        "Marking only from the entry point deletes ctor — nothing references it, because \
         nothing ever references a constructor. .init_array has to be a root in its own \
         right, and the entry inside it is the edge that keeps the function alive.",
    )]
}

/// Is a symbol in the output?
fn has_symbol(linked: &crate::stages::Linked, name: &str) -> bool {
    linked.elf.symbol(name).is_some()
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

link_test!(kept_without_the_flag, |ctx| {
    let obj = program(
        None,
        vec![(
            func_section(".text.orphan", None),
            SymbolSpec::global("orphan", ".text.orphan", 0).func(),
        )],
    )?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj))?;
    let mut c = Check::new("an unreferenced section, with no collection asked for");
    c.note(
        "Nothing calls `orphan` and it is in the output anyway. Garbage collection is opt-in \
         because dropping a section is only safe when the linker can see every reference, \
         and it cannot always — hand-written assembly and linker scripts both reach sections \
         in ways no relocation records.",
    );
    c.that(
        "output.symbol['orphan']",
        "present",
        has_symbol(&linked, "orphan"),
        "missing",
    );
    c.finish()
});

link_test!(dropped_with_the_flag, |ctx| {
    let obj = program(
        None,
        vec![(
            func_section(".text.orphan", None),
            SymbolSpec::global("orphan", ".text.orphan", 0).func(),
        )],
    )?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).arg("--gc-sections"))?;
    let mut c = Check::new("the same inputs, collected");
    c.note(
        "One flag, and a section nothing can reach is not in the output. The two tests \
         together are the whole feature: same inputs, same linker, and the only difference \
         is whether the mark-and-sweep ran.",
    );
    c.that(
        "output.symbol['orphan']",
        "gone",
        !has_symbol(&linked, "orphan"),
        "still present",
    );
    c.finish()
});

link_test!(the_entry_point_is_a_root, |ctx| {
    let obj = program(
        Some("reached"),
        vec![(
            func_section(".text.reached", None),
            SymbolSpec::global("reached", ".text.reached", 0).func(),
        )],
    )?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).arg("--gc-sections"))?;
    let mut c = Check::new("a function the entry point calls");
    c.note(
        "The entry point's own section is the first root — a collector that starts from an \
         empty root set deletes the whole program and reports success.",
    );
    c.that(
        "output.symbol['reached']",
        "present",
        has_symbol(&linked, "reached"),
        "missing",
    );
    c.finish()
});

link_test!(reachability_is_transitive, |ctx| {
    let obj = program(
        Some("middle"),
        vec![
            (
                func_section(".text.middle", Some("deep")),
                SymbolSpec::global("middle", ".text.middle", 0).func(),
            ),
            (
                func_section(".text.deep", None),
                SymbolSpec::global("deep", ".text.deep", 0).func(),
            ),
        ],
    )?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).arg("--gc-sections"))?;
    let mut c = Check::new("a call two levels down");
    c.note(
        "`_start` calls `middle` and `middle` calls `deep`. A collector that marks only the \
         direct neighbours of its roots keeps `middle` and deletes `deep`, and the program \
         jumps into a hole.",
    );
    for name in ["middle", "deep"] {
        c.that(
            &format!("output.symbol['{name}']"),
            "present",
            has_symbol(&linked, name),
            "missing",
        );
    }
    c.finish()
});

link_test!(unreachable_through_a_dropped_section, |ctx| {
    // `orphan` calls `only_from_orphan`, and nothing calls `orphan`.
    let obj = program(
        None,
        vec![
            (
                func_section(".text.orphan", Some("only_from_orphan")),
                SymbolSpec::global("orphan", ".text.orphan", 0).func(),
            ),
            (
                func_section(".text.only", None),
                SymbolSpec::global("only_from_orphan", ".text.only", 0).func(),
            ),
        ],
    )?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).arg("--gc-sections"))?;
    let mut c = Check::new("a function reachable only from dead code");
    c.note(
        "Being referenced is not the test — being referenced *from something reachable* is. \
         A collector that keeps any section with an incoming relocation keeps both of these \
         and collects almost nothing in a real program.",
    );
    for name in ["orphan", "only_from_orphan"] {
        c.that(
            &format!("output.symbol['{name}']"),
            "gone",
            !has_symbol(&linked, name),
            "still present",
        );
    }
    c.finish()
});

link_test!(data_is_reachable_too, |ctx| {
    // `_start` calls a function that references a string in its own section.
    let mut code = Code::new();
    code.lea_rip(Reg::Rsi, "kept_string", 0);
    code.ret();
    let obj = program(
        Some("uses_data"),
        vec![
            (
                text_named(".text.uses_data", &code),
                SymbolSpec::global("uses_data", ".text.uses_data", 0).func(),
            ),
            (
                SectionSpec::rodata(".rodata.kept", b"kept\n".to_vec()),
                SymbolSpec::global("kept_string", ".rodata.kept", 0),
            ),
            (
                SectionSpec::rodata(".rodata.dead", b"dead\n".to_vec()),
                SymbolSpec::global("dead_string", ".rodata.dead", 0),
            ),
        ],
    )?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).arg("--gc-sections"))?;
    let mut c = Check::new("data sections either side of the reachability line");
    c.note(
        "The graph runs through data, not just code. One string is referenced by a function \
         that survives and the other by nobody, and only the first is in the output — which \
         is most of where the size saving in a real binary comes from.",
    );
    c.that(
        "output.symbol['kept_string']",
        "present",
        has_symbol(&linked, "kept_string"),
        "missing",
    );
    c.that(
        "output.symbol['dead_string']",
        "gone",
        !has_symbol(&linked, "dead_string"),
        "still present",
    );
    c.finish()
});

link_test!(init_array_is_a_root, |ctx| {
    // A constructor that nothing calls, referenced only by an .init_array entry.
    let mut ctor = Code::new();
    ctor.ret();
    let obj = build(
        ObjectBuilder::new()
            .section({
                let mut code = Code::new();
                code.sys_exit(0);
                text_named(".text._start", &code)
            })
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text._start", 0).func())
            .section(text_named(".text.ctor", &ctor))
            .symbol(SymbolSpec::global("ctor", ".text.ctor", 0).func())
            .section(
                SectionSpec::new(
                    ".init_array",
                    SHT_INIT_ARRAY,
                    SHF_ALLOC | SHF_WRITE,
                    vec![0; 8],
                )
                .align(8)
                .entsize(8)
                .reloc(Reloc::sym(0, "ctor", R_X86_64_64, 0)),
            ),
    )?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).arg("--gc-sections"))?;
    let mut c = Check::new("a constructor under garbage collection");
    c.note(
        "Nothing calls `ctor` — nothing ever calls a constructor, that is what makes it one. \
         It survives because `.init_array` is a root and the entry in it is an edge to the \
         function. A collector that misses this produces a program whose initialisation \
         silently never happens, which is a miserable bug to find.",
    );
    c.that(
        "output.section['.init_array']",
        "present",
        linked.elf.section(".init_array").is_some(),
        "missing",
    );
    c.that(
        "output.symbol['ctor']",
        "present",
        has_symbol(&linked, "ctor"),
        "collected away",
    );
    c.finish()
});

link_test!(the_program_still_runs, |ctx| {
    let mut code = Code::new();
    code.sys_write(STDOUT, "message", 6);
    code.ret();
    let obj = program(
        Some("say"),
        vec![
            (
                text_named(".text.say", &code),
                SymbolSpec::global("say", ".text.say", 0).func(),
            ),
            (
                SectionSpec::rodata(".rodata.msg", b"alive\n".to_vec()),
                SymbolSpec::global("message", ".rodata.msg", 0),
            ),
            (
                func_section(".text.orphan", None),
                SymbolSpec::global("orphan", ".text.orphan", 0).func(),
            ),
        ],
    )?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).arg("--gc-sections"))?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "alive\n", 0)?;
    let mut c = Check::new("the collected program, executed");
    c.note(
        "Three legs of the same verification: the section is gone from the output, the \
         program still starts, and it still prints what it printed before. A collector that \
         drops a section but leaves a relocation pointing into it passes the first and fails \
         the other two.",
    );
    c.that(
        "the orphan is gone",
        "collected",
        !has_symbol(&linked, "orphan"),
        "still present",
    );
    c.finish()
});

link_test!(gc_changes_nothing_observable, |ctx| {
    let build_one = || -> Result<Vec<u8>, Failure> {
        let mut code = Code::new();
        code.sys_write(STDOUT, "message", 6);
        code.ret();
        program(
            Some("say"),
            vec![
                (
                    text_named(".text.say", &code),
                    SymbolSpec::global("say", ".text.say", 0).func(),
                ),
                (
                    SectionSpec::rodata(".rodata.msg", b"alive\n".to_vec()),
                    SymbolSpec::global("message", ".rodata.msg", 0),
                ),
                (
                    func_section(".text.orphan", None),
                    SymbolSpec::global("orphan", ".text.orphan", 0).func(),
                ),
            ],
        )
    };
    let plain = ctx.link_ok(
        &Link::new()
            .label("without --gc-sections")
            .object("a.o", build_one()?),
    )?;
    ctx.expect_output(&plain, "alive\n", 0)?;
    let collected = ctx.link_ok(
        &Link::new()
            .label("with --gc-sections")
            .object("a.o", build_one()?)
            .arg("--gc-sections"),
    )?;
    ctx.expect_output(&collected, "alive\n", 0)?;
    let mut c = Check::new("the same source, linked both ways");
    c.note(
        "Identical behaviour, smaller output. That is the only promise garbage collection \
         makes, and the reason it is safe to turn on by default in a build that controls all \
         of its own inputs.",
    );
    c.that(
        "the orphan survives without the flag",
        "present",
        has_symbol(&plain, "orphan"),
        "missing",
    );
    c.that(
        "and not with it",
        "collected",
        !has_symbol(&collected, "orphan"),
        "still present",
    );
    c.finish()
});
