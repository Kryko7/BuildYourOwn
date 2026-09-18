//! Stage 31 — GOTPCREL and its relaxation.
//!
//! `R_X86_64_GOTPCREL` means "load the address of this symbol out of the global offset
//! table". In a dynamic link the GOT is how a program reaches a symbol whose address is not
//! known until load time. In a *static* link every address is known at link time, so there
//! are two correct answers: build a one-entry GOT holding the address and point the load at
//! it, or notice that the address is a constant and rewrite the instruction to produce it
//! directly.
//!
//! GNU ld 2.47 takes the second road for all three spellings — it turns `mov rax,
//! [rip+GOT]` into `lea rax, [rip+sym]`, and a `REX_GOTPCRELX` into `mov rax, $sym`
//! (`48 c7 c0`) — and emits no `.got` at all. Both roads are accepted here. What is asserted
//! either way is the thing a program can feel: the register ends up holding the address of
//! the symbol.

use crate::asm::{Code, Reg, STDOUT};
use crate::assert::{Check, Failure};
use crate::elf::write::{ObjectBuilder, SectionSpec, SymbolSpec};
use crate::elf::{R_X86_64_GOTPCREL, R_X86_64_REX_GOTPCRELX};
use crate::examples::ExampleSpec;
use crate::link::{Link, Linked};
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 31,
        slug: "gotpcrel",
        name: "GOTPCREL and its relaxation",
        ext: true,
        hints: &[
            "The simple implementation is a real GOT: collect every symbol a GOTPCREL names, \
             give each one an eight-byte slot in a section you allocate, fill the slot with \
             S, and resolve the relocation to GOT_slot + A - P",
            "The other implementation is relaxation: in a static link S is a constant, so \
             `mov r64, [rip+GOT]` can become `lea r64, [rip+sym]` — same length, one fewer \
             memory reference, no GOT",
            "Whichever road you take, the register must end up holding the address of the \
             symbol and not the contents of it; a GOT slot holds an address, so the load \
             reads a pointer, not the object",
            "Only relax when you are sure: R_X86_64_GOTPCRELX and R_X86_64_REX_GOTPCRELX \
             exist precisely to tell the linker the instruction is safe to rewrite",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "a GOTPCREL load produces a usable address at run time",
                gotpcrel_address_is_usable,
            )
            .ext(),
            Test::new(
                "the linker either builds a GOT entry or relaxes the load",
                either_a_got_or_a_relaxation,
            )
            .ext(),
            Test::new(
                "R_X86_64_REX_GOTPCRELX takes the same road or a shorter one",
                rex_gotpcrelx,
            )
            .ext(),
            Test::new(
                "two references to the same symbol agree",
                two_references_agree,
            )
            .ext(),
            Test::new(
                "a GOT reference to a function can be called through",
                gotpcrel_to_a_function,
            )
            .ext(),
            Test::new(
                "a GOT reference to a symbol defined in another object",
                gotpcrel_across_objects,
            )
            .ext(),
            Test::new(
                "a GOT load and a lea of the same symbol agree at run time",
                got_and_lea_agree,
            )
            .ext(),
            Test::new(
                "a GOT reference to an undefined symbol names the symbol",
                gotpcrel_to_nothing,
            )
            .ext(),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Fixtures and the two-road assertion
// ---------------------------------------------------------------------------------------

/// One object: `_start` loads the address of `symbol` through `kind`, writes `len` bytes
/// through it and exits 0. Returns the object and the offset of the relocated field.
fn load_through_got(kind: u32, symbol: &str, message: &str) -> Result<(Vec<u8>, u64), Failure> {
    let mut code = Code::new();
    let site = code.len() + 3;
    code.mov_r64_rip(Reg::Rsi, symbol, kind, -4);
    code.sys_write_rsi(STDOUT, message.len() as u32);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global(symbol, ".rodata", 0).object(message.len() as u64)),
    )?;
    Ok((obj, site))
}

/// Which of the two legal roads the linker took for one GOT reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Road {
    /// The `mov` was left alone and its displacement points at a real GOT slot.
    GotEntry,
    /// The `mov` was rewritten as a `lea` of the symbol itself.
    RelaxedToLea,
    /// The `mov` was rewritten to take the address as a 32-bit immediate.
    RelaxedToImmediate,
}

/// Assert that the GOT reference at `site` makes the register hold the address of `target`,
/// whichever road the linker took, and say which road that was.
///
/// The three-byte opcode in front of the field is what distinguishes them: `48 8b` is the
/// original load, `48 8d` a `lea`, `48 c7` a `mov` of an immediate.
fn assert_got_reference(
    c: &mut Check,
    linked: &Linked,
    path: &str,
    site: u64,
    target: u64,
) -> Option<Road> {
    let opcode = match linked.elf.read_at_vaddr(site - 3, 3) {
        Ok(b) => [b[0], b[1], b[2]],
        Err(e) => {
            c.that(
                path,
                "a readable instruction at the GOT reference",
                false,
                e.to_string(),
            );
            return None;
        }
    };
    match (opcode[0], opcode[1]) {
        (0x48, 0x8b) => {
            // Untouched load: the displacement points at a GOT slot holding S.
            let Ok(disp) = linked.elf.i32_at_vaddr(site) else {
                c.that(path, "a readable displacement", false, "unreadable");
                return None;
            };
            let slot = (site as i64 + 4 + i64::from(disp)) as u64;
            match linked.elf.u64_at_vaddr(slot) {
                Ok(held) => {
                    c.that(
                        &format!("{path}.got_entry"),
                        &format!("a GOT slot at 0x{slot:x} holding 0x{target:x}"),
                        held == target,
                        format!("0x{held:x}"),
                    );
                }
                Err(e) => {
                    c.that(
                        &format!("{path}.got_entry"),
                        &format!("eight readable bytes at the GOT slot 0x{slot:x}"),
                        false,
                        e.to_string(),
                    );
                }
            }
            Some(Road::GotEntry)
        }
        (0x48, 0x8d) => {
            assert_pc32(
                c,
                &linked.elf,
                &format!("{path}.lea_disp32"),
                site,
                target,
                -4,
            );
            Some(Road::RelaxedToLea)
        }
        (0x48, 0xc7) => {
            match linked.elf.u32_at_vaddr(site) {
                Ok(got) => {
                    c.that(
                        &format!("{path}.imm32"),
                        &format!("the address of the symbol, 0x{target:x}"),
                        u64::from(got) == target,
                        format!("0x{got:x}"),
                    );
                }
                Err(e) => {
                    c.that(
                        &format!("{path}.imm32"),
                        "a readable immediate",
                        false,
                        e.to_string(),
                    );
                }
            }
            Some(Road::RelaxedToImmediate)
        }
        _ => {
            c.that(
                path,
                "either the original 48 8b load, a 48 8d lea or a 48 c7 immediate",
                false,
                format!("{:02x} {:02x} {:02x}", opcode[0], opcode[1], opcode[2]),
            );
            None
        }
    }
}

/// A one-line description of the road, for `ctx.note`.
fn describe(road: Option<Road>, linked: &Linked) -> String {
    let got = linked
        .elf
        .sections
        .iter()
        .find(|s| s.name == ".got" || s.name == ".got.plt")
        .map(|s| format!("{} at 0x{:x}, {} bytes", s.name, s.addr, s.size))
        .unwrap_or_else(|| "no .got section in the output".to_string());
    match road {
        Some(Road::GotEntry) => format!("the linker built a real GOT entry ({got})"),
        Some(Road::RelaxedToLea) => {
            format!("the linker relaxed the load to `lea` ({got})")
        }
        Some(Road::RelaxedToImmediate) => {
            format!("the linker relaxed the load to `mov $imm32` ({got})")
        }
        None => format!("the instruction at the GOT reference was not recognised ({got})"),
    }
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

link_test!(gotpcrel_address_is_usable, |ctx| {
    let message = "via the GOT\n";
    let (obj, site_offset) = load_through_got(R_X86_64_GOTPCREL, "gv", message)?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).label("a GOTPCREL load"))?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, message, 0)?;
    let mut c = Check::new("what the GOT reference put in rsi");
    let site = linked.address_of(DEFAULT_ENTRY)? + site_offset;
    let road = assert_got_reference(
        &mut c,
        &linked,
        "output.text[got]",
        site,
        linked.address_of("gv")?,
    );
    let note = describe(road, &linked);
    c.note(note.clone());
    c.finish()?;
    ctx.note(note);
    ctx.note(
        "the program wrote the string through the register the load filled, so the address \
         is right whichever road the linker took",
    );
    Ok(())
});

link_test!(either_a_got_or_a_relaxation, |ctx| {
    let message = "road\n";
    let (obj, site_offset) = load_through_got(R_X86_64_GOTPCREL, "gv", message)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("a GOTPCREL the linker may relax"),
    )?;
    ctx.expect_output(&linked, message, 0)?;
    let site = linked.address_of(DEFAULT_ENTRY)? + site_offset;
    let target = linked.address_of("gv")?;
    let mut c = Check::new("the road the linker took for a static GOT reference");
    let road = assert_got_reference(&mut c, &linked, "output.text[got]", site, target);
    let has_got = linked
        .elf
        .sections
        .iter()
        .any(|s| s.name == ".got" || s.name == ".got.plt");
    c.that(
        "output.got",
        "a GOT section exactly when the load was left in place",
        !(road == Some(Road::GotEntry)) || has_got,
        format!("road {road:?}, .got present: {has_got}"),
    );
    let note = describe(road, &linked);
    c.note(note.clone());
    c.finish()?;
    ctx.note(note);
    ctx.note(
        "both roads are conformant: the suite asserts the GOT slot's contents when a GOT \
         exists and the rewritten instruction's operand when one does not",
    );
    Ok(())
});

link_test!(rex_gotpcrelx, |ctx| {
    let message = "relaxable\n";
    let (obj, site_offset) = load_through_got(R_X86_64_REX_GOTPCRELX, "gv", message)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("an explicitly relaxable GOT load"),
    )?;
    ctx.expect_output(&linked, message, 0)?;
    let site = linked.address_of(DEFAULT_ENTRY)? + site_offset;
    let mut c = Check::new("an R_X86_64_REX_GOTPCRELX reference");
    let road = assert_got_reference(
        &mut c,
        &linked,
        "output.text[gotpcrelx]",
        site,
        linked.address_of("gv")?,
    );
    let note = describe(road, &linked);
    c.note(note.clone());
    c.finish()?;
    ctx.note(note);
    ctx.note(
        "the X spelling is the assembler promising the instruction is safe to rewrite; a \
         linker is still allowed to ignore the promise and build a GOT",
    );
    Ok(())
});

link_test!(two_references_agree, |ctx| {
    let mut code = Code::new();
    let first = code.len() + 3;
    code.mov_r64_rip(Reg::Rsi, "gv", R_X86_64_GOTPCREL, -4);
    code.sys_write_rsi(STDOUT, 5);
    let second = code.len() + 3;
    code.mov_r64_rip(Reg::Rsi, "gv", R_X86_64_GOTPCREL, -4);
    code.sys_write_rsi(STDOUT, 5);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"same\n".to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("gv", ".rodata", 0).object(5)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("two GOT references to one symbol"),
    )?;
    ctx.expect_output(&linked, "same\nsame\n", 0)?;
    let base = linked.address_of(DEFAULT_ENTRY)?;
    let target = linked.address_of("gv")?;
    let mut c = Check::new("two GOT references to the same symbol");
    let a = assert_got_reference(&mut c, &linked, "output.text[got #1]", base + first, target);
    let b = assert_got_reference(
        &mut c,
        &linked,
        "output.text[got #2]",
        base + second,
        target,
    );
    c.that(
        "output.text[got].road",
        "both references handled the same way",
        a == b,
        format!("{a:?} then {b:?}"),
    );
    let note = describe(a, &linked);
    c.finish()?;
    ctx.note(note);
    ctx.note(
        "a real GOT should hold one slot for a symbol however many times it is referenced, \
         but a linker that allocates two slots with the same contents is not wrong — the \
         test asserts only that both references produce the symbol's address",
    );
    Ok(())
});

link_test!(gotpcrel_to_a_function, |ctx| {
    let mut fun = Code::new();
    fun.mov_r32_imm32(Reg::Rax, 21);
    fun.ret();
    let fun_len = fun.len();
    let mut code = Code::new();
    let site_offset = code.len() + 3;
    code.mov_r64_rip(Reg::Rax, "fun", R_X86_64_GOTPCREL, -4);
    code.raw(&[0xff, 0xd0]); // call rax
    code.sys_exit_eax();
    let fun_offset = code.len();
    code.append(&fun);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(
                SymbolSpec::global("fun", ".text", fun_offset)
                    .func()
                    .size(fun_len),
            ),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("a GOT reference to a function"),
    )?;
    ctx.expect_output(&linked, "", 21)?;
    let site = linked.address_of(DEFAULT_ENTRY)? + site_offset;
    let mut c = Check::new("a GOT reference whose symbol is code");
    let road = assert_got_reference(
        &mut c,
        &linked,
        "output.text[got fun]",
        site,
        linked.address_of("fun")?,
    );
    c.finish()?;
    ctx.note(describe(road, &linked));
    ctx.note(
        "the program called through the register, so it held the entry point of the \
         function and not the first eight bytes of its code",
    );
    Ok(())
});

link_test!(gotpcrel_across_objects, |ctx| {
    let message = "elsewhere\n";
    let mut code = Code::new();
    let site_offset = code.len() + 3;
    code.mov_r64_rip(Reg::Rsi, "remote", R_X86_64_GOTPCREL, -4);
    code.sys_write_rsi(STDOUT, message.len() as u32);
    code.sys_exit(0);
    let a = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::undefined("remote")),
    )?;
    let b = build(
        ObjectBuilder::new()
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .symbol(SymbolSpec::global("remote", ".rodata", 0).object(message.len() as u64)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("a GOT reference resolved from another object"),
    )?;
    ctx.expect_output(&linked, message, 0)?;
    let site = linked.address_of(DEFAULT_ENTRY)? + site_offset;
    let mut c = Check::new("a GOT reference across objects");
    let road = assert_got_reference(
        &mut c,
        &linked,
        "output.text[got remote]",
        site,
        linked.address_of("remote")?,
    );
    c.finish()?;
    ctx.note(describe(road, &linked));
    Ok(())
});

link_test!(got_and_lea_agree, |ctx| {
    // rsi = GOT(gv), rdi = &gv, then exit with the difference: zero iff they agree.
    let message = "agree\n";
    let mut code = Code::new();
    let got_site = code.len() + 3;
    code.mov_r64_rip(Reg::Rsi, "gv", R_X86_64_GOTPCREL, -4);
    code.sys_write_rsi(STDOUT, message.len() as u32);
    code.mov_r64_rip(Reg::Rsi, "gv", R_X86_64_GOTPCREL, -4);
    let lea_site = code.len() + 3;
    code.lea_rip(Reg::Rdi, "gv", 0);
    code.sub_r32_r32(Reg::Rsi, Reg::Rdi);
    code.mov_r32_r32(Reg::Rax, Reg::Rsi);
    code.sys_exit_eax();
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("gv", ".rodata", 0).object(message.len() as u64)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("a GOT load next to a lea of the same symbol"),
    )?;
    // Exit status 0 means the two addresses were bit-for-bit identical at run time.
    ctx.expect_output(&linked, message, 0)?;
    let base = linked.address_of(DEFAULT_ENTRY)?;
    let target = linked.address_of("gv")?;
    let mut c = Check::new("a GOT reference and a plain PC-relative one");
    let road = assert_got_reference(&mut c, &linked, "output.text[got]", base + got_site, target);
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[lea].disp32",
        base + lea_site,
        target,
        -4,
    );
    c.finish()?;
    ctx.note(describe(road, &linked));
    ctx.note(
        "the program subtracts the two addresses and exits with the difference, so the \
         exit status 0 above is a run-time proof that the GOT road and the direct road \
         produced the same pointer",
    );
    Ok(())
});

link_test!(gotpcrel_to_nothing, |ctx| {
    let mut code = Code::new();
    code.mov_r64_rip(Reg::Rsi, "no_such_global", R_X86_64_GOTPCREL, -4);
    code.sys_write_rsi(STDOUT, 3);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::undefined("no_such_global")),
    )?;
    let run = ctx.link_fails(
        &Link::new()
            .object("a.o", obj)
            .label("a GOT reference to nothing"),
        "linking a GOTPCREL whose symbol nothing defines",
    )?;
    let mut c = Check::new("the diagnostic for an unresolvable GOT reference");
    c.mentions("linker.stderr", "no_such_global", &run.output.stderr);
    c.that(
        "linker.output_file",
        "no output file left behind",
        run.produced.is_none(),
        run.produced.is_some(),
    );
    c.note(
        "a GOT slot needs a value like any other relocation; there is nothing to put in it \
         and nothing to relax to",
    );
    c.finish()
});

/// Worked examples: the load, and the two shapes it may come out as.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "Loading an address out of the GOT",
            "ld -o prog got.o",
            || {
                load_through_got(R_X86_64_GOTPCREL, "gv", "via the GOT\n")
                    .map(|(bytes, _)| bytes)
                    .map_err(|f| f.messages.join("; "))
            },
        )
        .request(
            "got.o: `48 8b 35 00 00 00 00` — `mov rsi, [rip+disp32]` with an \
             R_X86_64_GOTPCREL naming `gv' and addend -4, then a write(2) that uses rsi as \
             the buffer",
        )
        .response(
            "Either a GOT section with one eight-byte slot holding the address of `gv', with \
             the displacement pointing at that slot; or the instruction rewritten as \
             `lea rsi, [rip+gv]` or `mov rsi, $gv` with no GOT at all. Both print the string",
        )
        .note(
            "The one thing that is never right is loading the *contents* of `gv`. A GOT slot \
             holds a pointer, so the untouched instruction reads a pointer out of memory — \
             which is exactly why the relaxed form is a `lea` and not a `mov` from memory.",
        )
        .runs("via the GOT\n", 0),
        ExampleSpec::text("Why a static link may skip the GOT", "ld -o prog gotx.o")
            .request(
                "The same instruction under R_X86_64_REX_GOTPCRELX, the spelling an assembler \
             uses to say the instruction may be rewritten",
            )
            .response(
                "GNU ld 2.47 emits `48 c7 c6 <addr>` — `mov rsi, $gv` — and no .got section. The \
             address is a link-time constant, so the indirection buys nothing",
            )
            .note(
                "Start with a real GOT: it is a dozen lines and it is correct for every \
             relocation kind in this family. Relaxation is an optimisation, and one that is \
             only safe for the instruction encodings the X relocations name.",
            ),
    ]
}
