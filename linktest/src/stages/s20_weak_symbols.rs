//! Stage 20 — Weak definitions and weak references.
//!
//! One bit in `st_info` turns "I define this" into "I define this unless somebody better
//! comes along", and "I need this" into "I would like this if it exists". Weak definitions
//! are how a library ships an overridable default; weak references are how a program asks
//! *whether* an optional component was linked in, by testing the symbol's address against
//! zero. Both rules live in the same place in the linker: the moment a definition or a
//! reference enters the global table.

use crate::asm::{Code, Reg, STDOUT};
use crate::assert::{Check, Failure};
use crate::elf::write::{ObjectBuilder, SectionSpec, SymbolSpec};
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 20,
        slug: "weak_symbols",
        name: "Weak definitions and weak references",
        ext: false,
        hints: &[
            "A strong definition always beats a weak one, whichever input it came from and in \
             whatever order the objects are named — the winner must not depend on the command \
             line",
            "A weak definition is still a definition: with no strong one in the link it is \
             used, and it never counts as a duplicate",
            "An unresolved weak *reference* is not an error; compute the relocation with S = \
             0 and let the program test for it",
            "Two weak definitions of one name are not an error either; pick one \
             deterministically (GNU ld takes the first it sees) and move on",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new("a strong definition beats a weak one", strong_beats_weak),
            Test::new(
                "a weak definition is used when it is the only one",
                weak_alone_is_used,
            ),
            Test::new(
                "two weak definitions resolve to one of the two",
                weak_beside_weak,
            ),
            Test::new(
                "an unresolved weak reference is zero and the code can branch on it",
                undefined_weak_is_zero,
            ),
            Test::new("a weak data symbol is used like any other", weak_data),
            Test::new(
                "the strong definition wins in either command-line order",
                order_does_not_decide,
            ),
            Test::new(
                "a weak reference a strong definition satisfies is not zero",
                weak_reference_resolved,
            ),
            Test::new(
                "every weak combination links without an error",
                nothing_here_is_an_error,
            ),
        ],
    }
}

link_test!(strong_beats_weak, |ctx| {
    let a = caller("weak vs strong\n", "dual")?;
    let weak = weak_callee_returning("dual", 11)?;
    let strong = callee_returning("dual", 66)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("weak.o", weak)
            .object("strong.o", strong)
            .label("a weak definition and a strong one"),
    )?;
    assert_runnable_layout(&linked)?;
    // 66 is the strong body; 11 would mean the weak one had won.
    ctx.expect_output(&linked, "weak vs strong\n", 66)?;
    let mut c = Check::new("which definition the call was patched to");
    let site = linked.address_of(DEFAULT_ENTRY)? + CALLER_CALL_DISP_OFFSET;
    let target = linked.address_of("dual")?;
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[call].disp32",
        site,
        target,
        -4,
    );
    c.finish()
});

link_test!(weak_alone_is_used, |ctx| {
    let a = caller("weak only\n", "dual")?;
    let weak = weak_callee_returning("dual", 29)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("weak.o", weak)
            .label("a weak definition and nothing else"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "weak only\n", 29)?;
    let mut c = Check::new("the weak definition in the output");
    match linked.elf.symbol("dual") {
        Some(s) => {
            c.that(
                "output.symtab['dual'].st_shndx",
                "a defined symbol — the weak definition was allocated like any other",
                !s.is_undefined(),
                format!("st_shndx {}", s.shndx),
            );
        }
        None => {
            c.observe(
                "output.symtab['dual']",
                "absent from the output symbol table",
            );
        }
    }
    c.finish()
});

link_test!(weak_beside_weak, |ctx| {
    let a = caller("two weaks\n", "dual")?;
    let first = weak_callee_returning("dual", 41)?;
    let second = weak_callee_returning("dual", 42)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("w1.o", first)
            .object("w2.o", second)
            .label("two weak definitions of one name"),
    )?;
    assert_runnable_layout(&linked)?;
    let out = ctx.run(&linked)?;
    let mut c = Check::new("which of the two weak definitions was chosen");
    c.eq(
        "program.stdout",
        "two weaks\n".to_string(),
        out.stdout.clone(),
    );
    c.that(
        "program.exit_status",
        "41 or 42 — with two weak definitions the ABI does not say which one wins, only that \
         it is one of them and that it is not an error",
        matches!(out.code, Some(41) | Some(42)),
        out.status_line(),
    );
    if !c.ok() {
        c.block("linker command", linked.run.output.command_line());
    }
    c.finish()?;
    match out.code {
        Some(41) => ctx.note(
            "with two weak definitions this linker took the first one on the command line \
             (exit 41); the ABI permits either, so the test only requires one of the two",
        ),
        Some(42) => ctx.note(
            "with two weak definitions this linker took the last one on the command line \
             (exit 42); the ABI permits either, so the test only requires one of the two",
        ),
        _ => {}
    }
    Ok(())
});

link_test!(undefined_weak_is_zero, |ctx| {
    // mov rax, <weak_absent>   ; R_X86_64_32S, S = 0 when nothing defines it
    // test rax, rax
    // je  +12                  ; skip the "it was there" exit
    // exit(1)
    // exit(77)
    let mut code = Code::new();
    let imm_site = code.len() + 3; // 48 c7 c0, then the imm32
    code.mov_r64_symbol_addr32s(Reg::Rax, "weak_absent", 0);
    code.test_r64_r64(Reg::Rax, Reg::Rax);
    code.je_rel8(12); // over exactly one sys_exit, which is 12 bytes
    code.sys_exit(1);
    code.sys_exit(77);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::weak_undefined("weak_absent")),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("a program that branches on an unresolved weak symbol"),
    )?;
    assert_runnable_layout(&linked)?;
    // 77 is the zero road; 1 would mean the address was not zero.
    ctx.expect_output(&linked, "", 77)?;
    let mut c = Check::new("the value the linker wrote for the unresolved weak symbol");
    let site = linked.address_of(DEFAULT_ENTRY)? + imm_site;
    match linked.elf.i32_at_vaddr(site) {
        Ok(got) => {
            c.eq("output.text[mov rax, weak_absent].imm32", 0i32, got);
        }
        Err(e) => {
            c.that(
                "output.text[mov rax, weak_absent].imm32",
                "a readable 32-bit field at the relocation site",
                false,
                e.to_string(),
            );
        }
    }
    if !c.ok() {
        c.block("linker command", linked.run.output.command_line());
        c.block("output program headers", linked.elf.program_header_table());
    }
    c.finish()
});

link_test!(weak_data, |ctx| {
    let a = read_global_and_exit("wval")?;
    let b = weak_word("wval", 88)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("a weak datum and nothing stronger"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "", 88)?;

    // Add a strong definition and the answer changes, without the link becoming an error.
    let a2 = read_global_and_exit("wval")?;
    let b2 = weak_word("wval", 88)?;
    let c2 = global_word("wval", 99)?;
    let overridden = ctx.link_ok(
        &Link::new()
            .object("a.o", a2)
            .object("b.o", b2)
            .object("c.o", c2)
            .out("overridden")
            .label("the same weak datum with a strong definition beside it"),
    )?;
    ctx.expect_output(&overridden, "", 99)?;
    Ok(())
});

link_test!(order_does_not_decide, |ctx| {
    let a = caller("either order\n", "dual")?;
    let weak = weak_callee_returning("dual", 12)?;
    let strong = callee_returning("dual", 34)?;
    let weak_first = ctx.link_ok(
        &Link::new()
            .object("a.o", a.clone())
            .object("weak.o", weak.clone())
            .object("strong.o", strong.clone())
            .label("weak first, then strong"),
    )?;
    ctx.expect_output(&weak_first, "either order\n", 34)?;
    let strong_first = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("strong.o", strong)
            .object("weak.o", weak)
            .out("strong_first")
            .label("strong first, then weak"),
    )?;
    assert_runnable_layout(&strong_first)?;
    ctx.expect_output(&strong_first, "either order\n", 34)?;
    let mut c = Check::new("that the command-line order did not choose the winner");
    c.note(
        "a linker that simply keeps the first definition it meets gets exit 12 for one of \
         these two links — the strong/weak rule has to beat first-come-first-served",
    );
    c.finish()
});

link_test!(weak_reference_resolved, |ctx| {
    // The reference is weak, but something does define it: the real address must be used.
    let mut code = Code::new();
    let imm_site = code.len() + 3;
    code.mov_r64_symbol_addr32s(Reg::Rax, "weak_ref", 0);
    code.test_r64_r64(Reg::Rax, Reg::Rax);
    code.je_rel8(12);
    code.sys_exit(52);
    code.sys_exit(0);
    let a = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::weak_undefined("weak_ref")),
    )?;
    let b = callee_returning("weak_ref", 0)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("a weak reference something defines"),
    )?;
    assert_runnable_layout(&linked)?;
    // 52 is the non-zero road: the weak reference found a real address.
    ctx.expect_output(&linked, "", 52)?;
    let mut c = Check::new("the address the weak reference resolved to");
    let site = linked.address_of(DEFAULT_ENTRY)? + imm_site;
    let target = linked.address_of("weak_ref")?;
    match linked.elf.u32_at_vaddr(site) {
        Ok(got) => {
            c.addr_eq(
                "output.text[mov rax, weak_ref].imm32",
                target,
                u64::from(got),
            );
        }
        Err(e) => {
            c.that(
                "output.text[mov rax, weak_ref].imm32",
                "a readable 32-bit field at the relocation site",
                false,
                e.to_string(),
            );
        }
    }
    c.finish()
});

link_test!(nothing_here_is_an_error, |ctx| {
    // Five shapes, none of which a linker may refuse.
    let cases: Vec<(&str, Link)> = vec![
        (
            "a weak definition on its own",
            Link::new()
                .object("a.o", caller("x\n", "dual")?)
                .object("w.o", weak_callee_returning("dual", 1)?)
                .out("weak_only"),
        ),
        (
            "a weak definition beside a strong one",
            Link::new()
                .object("a.o", caller("x\n", "dual")?)
                .object("w.o", weak_callee_returning("dual", 1)?)
                .object("s.o", callee_returning("dual", 2)?)
                .out("weak_and_strong"),
        ),
        (
            "two weak definitions",
            Link::new()
                .object("a.o", caller("x\n", "dual")?)
                .object("w1.o", weak_callee_returning("dual", 1)?)
                .object("w2.o", weak_callee_returning("dual", 2)?)
                .out("two_weak"),
        ),
        (
            "three weak definitions",
            Link::new()
                .object("a.o", caller("x\n", "dual")?)
                .object("w1.o", weak_callee_returning("dual", 1)?)
                .object("w2.o", weak_callee_returning("dual", 2)?)
                .object("w3.o", weak_callee_returning("dual", 3)?)
                .out("three_weak"),
        ),
        (
            "a weak definition and a weak reference",
            Link::new()
                .object("a.o", read_global_and_exit("wval")?)
                .object("w.o", weak_word("wval", 5)?)
                .out("weak_data"),
        ),
    ];
    let mut c = Check::new("that no weak combination is an error");
    for (what, link) in cases {
        let run = ctx.link(&link.label(what))?;
        c.that(
            &format!("linker.exit_status[{what}]"),
            "exit 0 — none of the weak rules makes a link fail",
            run.output.success(),
            run.output.status_line(),
        );
        if !run.output.success() {
            c.block(format!("linker output for {what}"), run.diagnostics());
        }
    }
    c.finish()
});

// ---------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------

/// `_start` loads the four bytes at `name` — defined elsewhere — and exits with them.
fn read_global_and_exit(name: &str) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.mov_r32_rip(Reg::Rax, name, 0);
    code.sys_exit_eax();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::undefined(name)),
    )
}

/// An object whose `.data` holds one *weak* 32-bit word.
fn weak_word(name: &str, value: u32) -> Result<Vec<u8>, Failure> {
    build(
        ObjectBuilder::new()
            .section(SectionSpec::data(".data", value.to_le_bytes().to_vec()).align(4))
            .symbol(SymbolSpec::weak(name, ".data", 0).object(4)),
    )
}

/// An object whose `.data` holds one strong 32-bit word.
fn global_word(name: &str, value: u32) -> Result<Vec<u8>, Failure> {
    build(
        ObjectBuilder::new()
            .section(SectionSpec::data(".data", value.to_le_bytes().to_vec()).align(4))
            .symbol(SymbolSpec::global(name, ".data", 0).object(4)),
    )
}

/// An object that prints a banner and returns, used only by an example.
fn banner(message: &str) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "message", message.len() as u32);
    code.ret();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .symbol(SymbolSpec::weak("banner", ".text", 0).func())
            .symbol(SymbolSpec::local("message", ".rodata", 0).object(message.len() as u64)),
    )
}

/// Worked examples: the overridable default and the optional component.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "A weak definition, waiting to be overridden",
            "ld -o prog a.o weak.o strong.o",
            || weak_callee_returning("dual", 11).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "weak.o defines `dual` with STB_WEAK (binding 2) in the top nibble of st_info; \
             everything else about the symbol — its section, st_value, st_size — is exactly \
             what a strong definition would carry",
        )
        .response(
            "With strong.o also in the link, the strong `dual` wins: the call site is patched \
             to the strong body's address and the program exits 66. Drop strong.o and the \
             very same link uses the weak body and exits 11",
        )
        .note(
            "The rule is a total order, not a race: strong beats weak no matter which object \
             came first on the command line. Checking only `is the slot already filled` gives \
             the right answer half the time.",
        )
        .runs("weak vs strong\n", 66),
        ExampleSpec::object(
            "A weak reference the program tests for",
            "ld -o prog a.o",
            || {
                let mut code = Code::new();
                code.mov_r64_symbol_addr32s(Reg::Rax, "weak_absent", 0);
                code.test_r64_r64(Reg::Rax, Reg::Rax);
                code.je_rel8(12);
                code.sys_exit(1);
                code.sys_exit(77);
                build(
                    ObjectBuilder::new()
                        .section(text_of(&code))
                        .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
                        .symbol(SymbolSpec::weak_undefined("weak_absent")),
                )
                .map_err(|f| f.messages.join("; "))
            },
        )
        .request(
            "a.o loads the *address* of `weak_absent` into rax with an R_X86_64_32S, then \
             `test rax, rax` and `je`. The symbol is STB_WEAK and SHN_UNDEF, and no other \
             input is given",
        )
        .response(
            "No error. S = 0, so the imm32 is patched to zero, ZF is set, the branch is taken \
             and the program exits 77 down the `it was not linked in` road",
        )
        .note(
            "This is the whole reason unresolved weak references are legal: the program can \
             see the difference at run time. Refusing the link takes that away.",
        )
        .runs("", 77),
        ExampleSpec::object(
            "A weak function nothing overrides",
            "ld -o prog a.o banner.o",
            || banner("default banner\n").map_err(|f| f.messages.join("; ")),
        )
        .request(
            "banner.o's `banner` is STB_WEAK and defined: a default implementation shipped \
             inside a library, with its string reached through a local symbol",
        )
        .response(
            "With nothing stronger in the link the weak definition is simply used — a weak \
             definition is a definition, and never counts towards a duplicate",
        )
        .note(
            "A linker that treats `weak` as `only if referenced` or as `never allocated` \
             breaks here: the section still has to be laid out and the symbol still has to get \
             an address.",
        ),
    ]
}
