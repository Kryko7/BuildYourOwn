//! Stage 24 — R_X86_64_PC32 and R_X86_64_PLT32.
//!
//! The first relocation a linker ever applies, and the one nearly every instruction in a
//! real program needs. Both kinds compute the same arithmetic — `S + A - P`, stored in a
//! signed 32-bit field — and in a static link with no procedure linkage table they are the
//! same relocation under two names: `PLT32` is what an assembler emits for a call to a
//! global it cannot see, and a static linker resolves it straight to the symbol.

use crate::asm::{Code, Reg, STDOUT};
use crate::assert::{Check, Failure};
use crate::elf::write::{ObjectBuilder, SectionSpec, SymbolSpec};
use crate::elf::{R_X86_64_PC32, R_X86_64_PLT32};
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 24,
        slug: "pc32_and_plt32",
        name: "R_X86_64_PC32 and R_X86_64_PLT32",
        ext: false,
        hints: &[
            "Walk every SHT_RELA section of every input and compute S + A - P for each \
             entry: S is the final address of the symbol, A is r_addend, P is the final \
             address of the four-byte field being patched",
            "P is the address of the field, not of the instruction — the -4 the assembler \
             already folded into the addend is what turns that into the end of the \
             instruction the processor measures displacements from",
            "Treat R_X86_64_PLT32 exactly like R_X86_64_PC32: with no PLT to build, a static \
             link resolves both straight to the symbol, and a linker that handles only one of \
             them dies on the first real compiler output it sees",
            "Write the result into the output image as a little-endian signed 32-bit value, \
             never back into the input object, and check that it fits before storing it",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "a cross-object call is patched to S + A - P",
                plt32_call_across_objects,
            ),
            Test::new(
                "the same call encoded as R_X86_64_PC32 behaves identically",
                pc32_call_is_the_same_thing,
            ),
            Test::new("a lea reaches a string in .rodata", lea_to_rodata),
            Test::new(
                "a backward reference gets a negative displacement",
                backward_reference,
            ),
            Test::new(
                "two relocations in one section are both patched",
                two_relocations_one_section,
            ),
            Test::new(
                "a relocation living in the second object is patched too",
                relocation_in_the_second_object,
            ),
            Test::new(
                "a jmp reaches a label in another object",
                jmp_across_objects,
            ),
            Test::new(
                "a PC32 against a symbol nothing defines names the symbol",
                undefined_pc32_target,
            ),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------

/// A caller whose `call <callee>` is encoded with `kind`, plus the offset of the
/// displacement field inside its `.text`.
///
/// With `R_X86_64_PLT32` this is byte-for-byte what a compiler emits for a call to an extern
/// function; with `R_X86_64_PC32` it is the same instruction under the other relocation
/// name, which a static link has to resolve the same way.
fn caller_encoded(kind: u32, message: &str, callee: &str) -> Result<(Vec<u8>, u64), Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "message", message.len() as u32);
    let site = code.len() + 1;
    if kind == R_X86_64_PLT32 {
        code.call(callee);
    } else {
        code.call_pc32(callee);
    }
    code.sys_exit_eax();
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("message", ".rodata", 0).object(message.len() as u64))
            .symbol(SymbolSpec::undefined(callee)),
    )?;
    Ok((obj, site))
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

link_test!(plt32_call_across_objects, |ctx| {
    let (obj, site_offset) = caller_encoded(R_X86_64_PLT32, "calling\n", "other")?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .object("b.o", callee_returning("other", 7)?)
            .label("a caller and its callee"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "calling\n", 7)?;
    let site = linked.address_of(DEFAULT_ENTRY)? + site_offset;
    let target = linked.address_of("other")?;
    let mut c = Check::new("the displacement of an R_X86_64_PLT32 call");
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[call].disp32",
        site,
        target,
        -4,
    );
    if !c.ok() {
        c.note(format!(
            "the call's disp32 sits at 0x{site:x} and 'other' was placed at 0x{target:x}"
        ));
        c.block("linker command", linked.run.output.command_line());
    }
    c.finish()
});

link_test!(pc32_call_is_the_same_thing, |ctx| {
    let (plt, plt_site) = caller_encoded(R_X86_64_PLT32, "twice\n", "other")?;
    let (pc, pc_site) = caller_encoded(R_X86_64_PC32, "twice\n", "other")?;
    let with_plt = ctx.link_ok(
        &Link::new()
            .object("a.o", plt)
            .object("b.o", callee_returning("other", 11)?)
            .label("the call encoded as R_X86_64_PLT32"),
    )?;
    let with_pc = ctx.link_ok(
        &Link::new()
            .object("a.o", pc)
            .object("b.o", callee_returning("other", 11)?)
            .out("pc32")
            .label("the same call encoded as R_X86_64_PC32"),
    )?;
    ctx.expect_output(&with_plt, "twice\n", 11)?;
    ctx.expect_output(&with_pc, "twice\n", 11)?;

    let mut c = Check::new("that PLT32 and PC32 resolve the same way");
    assert_pc32(
        &mut c,
        &with_plt.elf,
        "output(PLT32).text[call].disp32",
        with_plt.address_of(DEFAULT_ENTRY)? + plt_site,
        with_plt.address_of("other")?,
        -4,
    );
    assert_pc32(
        &mut c,
        &with_pc.elf,
        "output(PC32).text[call].disp32",
        with_pc.address_of(DEFAULT_ENTRY)? + pc_site,
        with_pc.address_of("other")?,
        -4,
    );
    c.note(
        "each field is compared against the addresses its own link chose, never against the \
         other link's addresses: the claim is that the two relocation kinds mean the same \
         arithmetic, not that two links lay memory out the same way",
    );
    c.finish()
});

link_test!(lea_to_rodata, |ctx| {
    let message = "rodata\n";
    let mut code = Code::new();
    let site_offset = code.len() + WRITE_LEA_DISP_OFFSET;
    code.sys_write(STDOUT, "message", message.len() as u32);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("message", ".rodata", 0).object(message.len() as u64)),
    )?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).label("a lea into .rodata"))?;
    ctx.expect_output(&linked, message, 0)?;
    let site = linked.address_of(DEFAULT_ENTRY)? + site_offset;
    let target = linked.address_of("message")?;
    let mut c = Check::new("the displacement of the lea that finds the string");
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[lea].disp32",
        site,
        target,
        -4,
    );
    c.note(
        ".text and .rodata almost always end up in different segments, so this displacement \
         is the one that proves the linker computed across a section boundary",
    );
    c.finish()
});

link_test!(backward_reference, |ctx| {
    // The callee is laid out *before* _start, so S < P and the displacement is negative.
    let mut helper = Code::new();
    helper.mov_r32_imm32(Reg::Rax, 23);
    helper.ret();
    let helper_len = helper.len();
    let mut code = Code::new();
    code.append(&helper);
    let start_offset = code.len();
    let site_offset = code.len() + 1;
    code.call("back");
    code.sys_exit_eax();
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(
                SymbolSpec::global("back", ".text", 0)
                    .func()
                    .size(helper_len),
            )
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", start_offset).func()),
    )?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).label("a backward call"))?;
    ctx.expect_output(&linked, "", 23)?;
    // 'back' sits at offset 0 of this object's .text, so it is the base of the block.
    let target = linked.address_of("back")?;
    let site = target + site_offset;
    let mut c = Check::new("the displacement of a backward call");
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[call].disp32",
        site,
        target,
        -4,
    );
    match linked.elf.i32_at_vaddr(site) {
        Ok(disp) => {
            c.that(
                "output.text[call].disp32",
                "a negative displacement — the target is behind the call",
                disp < 0,
                disp,
            );
        }
        Err(e) => {
            c.that(
                "output.text[call].disp32",
                "a readable 32-bit field at the call site",
                false,
                e.to_string(),
            );
        }
    }
    c.finish()
});

link_test!(two_relocations_one_section, |ctx| {
    let mut code = Code::new();
    let first = code.len() + WRITE_LEA_DISP_OFFSET;
    code.sys_write(STDOUT, "one", 4);
    let second = code.len() + WRITE_LEA_DISP_OFFSET;
    code.sys_write(STDOUT, "two", 4);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"one\ntwo\n".to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("one", ".rodata", 0).object(4))
            .symbol(SymbolSpec::global("two", ".rodata", 4).object(4)),
    )?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).label("two lea references"))?;
    ctx.expect_output(&linked, "one\ntwo\n", 0)?;
    let base = linked.address_of(DEFAULT_ENTRY)?;
    let mut c = Check::new("both displacements inside one .text");
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[lea 'one'].disp32",
        base + first,
        linked.address_of("one")?,
        -4,
    );
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[lea 'two'].disp32",
        base + second,
        linked.address_of("two")?,
        -4,
    );
    c.finish()
});

link_test!(relocation_in_the_second_object, |ctx| {
    let mut first = Code::new();
    first.call("second");
    first.sys_exit_eax();
    let a = build(
        ObjectBuilder::new()
            .section(text_of(&first))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::undefined("second")),
    )?;
    let mut code = Code::new();
    let site_offset = code.len() + WRITE_LEA_DISP_OFFSET;
    code.sys_write(STDOUT, "tail", 5);
    code.mov_r32_imm32(Reg::Rax, 31);
    code.ret();
    let b = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"tail\n".to_vec()))
            .symbol(SymbolSpec::global("second", ".text", 0).func())
            .symbol(SymbolSpec::global("tail", ".rodata", 0).object(5)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("a relocation that lives in the second input"),
    )?;
    ctx.expect_output(&linked, "tail\n", 31)?;
    let site = linked.address_of("second")? + site_offset;
    let mut c = Check::new("the displacement of a lea inside the second object");
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[b.o lea].disp32",
        site,
        linked.address_of("tail")?,
        -4,
    );
    c.note(
        "a relocation's r_offset is relative to its own object's section, so the site is \
         that object's base plus the offset — not the output section's base plus the offset",
    );
    c.finish()
});

link_test!(jmp_across_objects, |ctx| {
    let mut first = Code::new();
    let site_offset = first.len() + 1;
    first.jmp("target");
    first.ud2();
    let a = build(
        ObjectBuilder::new()
            .section(text_of(&first))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::undefined("target")),
    )?;
    let mut tail = Code::new();
    tail.sys_write(STDOUT, "msg", 7);
    tail.sys_exit(5);
    let b = build(
        ObjectBuilder::new()
            .section(text_of(&tail))
            .section(SectionSpec::rodata(".rodata", b"jumped\n".to_vec()))
            .symbol(SymbolSpec::global("target", ".text", 0).func())
            .symbol(SymbolSpec::global("msg", ".rodata", 0).object(7)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("a jmp into another object"),
    )?;
    ctx.expect_output(&linked, "jumped\n", 5)?;
    let site = linked.address_of(DEFAULT_ENTRY)? + site_offset;
    let mut c = Check::new("the displacement of a cross-object jmp");
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[jmp].disp32",
        site,
        linked.address_of("target")?,
        -4,
    );
    c.note(
        "the ud2 after the jmp never executes: if the program exits 5 having printed the \
         string, control really did leave this object",
    );
    c.finish()
});

link_test!(undefined_pc32_target, |ctx| {
    let (obj, _) = caller_encoded(R_X86_64_PC32, "never\n", "nosuchfunction")?;
    let run = ctx.link_fails(
        &Link::new()
            .object("a.o", obj)
            .label("a PC32 with no definition"),
        "linking a PC32 reference to a symbol nothing defines",
    )?;
    let mut c = Check::new("the diagnostic for an unresolvable PC32");
    c.mentions("linker.stderr", "nosuchfunction", &run.output.stderr);
    c.note(
        "there is no value of S to compute with; the linker has to name the symbol it is \
         missing rather than patch a zero and hand over a binary that calls into nothing",
    );
    c.finish()
});

/// Worked examples: the call and the reference that has nothing to point at.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "A call into another object",
            "ld -o prog caller.o callee.o",
            || caller("calling\n", "other").map_err(|f| f.messages.join("; ")),
        )
        .request(
            "caller.o: a .text that writes a string and then `e8 00 00 00 00` — a call whose \
             disp32 is zero and whose R_X86_64_PLT32 relocation, addend -4, names the \
             undefined symbol `other'; callee.o defines `other' as `mov eax, 7; ret'",
        )
        .response(
            "The disp32 at the call site holds S + A - P: the address the linker gave \
             `other', minus four, minus the address of the disp32 field itself. The program \
             prints its string and exits with whatever the callee left in eax",
        )
        .note(
            "P is the field, not the instruction. The assembler already folded the four \
             bytes between the field and the end of the instruction into the addend, so a \
             linker that subtracts four again lands four bytes short of the callee.",
        )
        .runs("calling\n", 0),
        ExampleSpec::error(
            "A PC32 with nothing to point at",
            "ld -o prog caller.o",
            || caller("never\n", "nosuchfunction").map_err(|f| f.messages.join("; ")),
        )
        .request(
            "The same caller.o linked on its own: the relocation names `nosuchfunction' and \
             no input defines it",
        )
        .response(
            "The linker exits non-zero with a diagnostic naming `nosuchfunction' and leaves \
             no output file behind",
        )
        .note(
            "There is no sensible S here. Patching a zero produces a binary that calls \
             address 0 and dies with SIGSEGV, which is exactly the bug the error prevents.",
        ),
    ]
}
