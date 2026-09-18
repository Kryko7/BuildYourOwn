//! Stage 27 — Relocation overflow is a hard error.
//!
//! `R_X86_64_32`, `R_X86_64_32S` and `R_X86_64_PC32` all store their result in four bytes,
//! and the result is not always four bytes wide. When it is not, there is no correct value
//! to write: truncating produces a binary that reads or jumps somewhere arbitrary, and the
//! only honest answer is to refuse the link and name the symbol.
//!
//! Forcing the overflow needs a symbol whose value the linker cannot move, which is what
//! `SHN_ABS` is: an absolute symbol of 0x1_0000_0000 is 0x1_0000_0000 wherever the image
//! ends up.

use crate::asm::{Code, Reg};
use crate::assert::{Check, Failure};
use crate::elf::read::Elf;
use crate::elf::write::{ObjectBuilder, Reloc, SectionSpec, SymbolSpec};
use crate::elf::{R_X86_64_32, R_X86_64_32S};
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Ctx, Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 27,
        slug: "relocation_overflow",
        name: "Relocation overflow is a hard error",
        ext: true,
        hints: &[
            "After computing the value of a relocation, check it fits the field before you \
             store it: R_X86_64_32 needs 0 <= v <= 0xffffffff, R_X86_64_32S and \
             R_X86_64_PC32 need -0x80000000 <= v <= 0x7fffffff",
            "The check is on S + A, not on S: an addend can carry a perfectly ordinary \
             symbol past the end of the range",
            "Run the check over every relocated section, data included — an overflowing \
             pointer word in .data is as fatal as one in .text",
            "Refusing means exiting non-zero with the symbol's name on stderr and leaving no \
             output file behind; a truncated value produces a binary that fails far from the \
             mistake",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "an R_X86_64_32 against an absolute symbol above 4 GB is refused",
                abs32_above_four_gb,
            )
            .ext(),
            Test::new(
                "an R_X86_64_32 against 0xffffffff is accepted and stored whole",
                abs32_at_the_top_of_its_range,
            )
            .ext(),
            Test::new(
                "an R_X86_64_32S against 0x80000000 is refused",
                abs32s_just_past_its_range,
            )
            .ext(),
            Test::new(
                "an R_X86_64_32S against 0x7fffffff is accepted",
                abs32s_at_the_top_of_its_range,
            )
            .ext(),
            Test::new(
                "an R_X86_64_32S reaches the bottom of its signed range",
                abs32s_at_the_bottom_of_its_range,
            )
            .ext(),
            Test::new(
                "an addend that carries a legal symbol out of range is refused",
                addend_causes_the_overflow,
            )
            .ext(),
            Test::new(
                "a PC32 displacement that does not fit 32 bits is refused",
                pc32_displacement_too_far,
            )
            .ext(),
            Test::new(
                "an overflow inside .data is caught as well",
                overflow_in_a_data_section,
            )
            .ext(),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Fixtures and assertions
// ---------------------------------------------------------------------------------------

/// One object: `mov <reg>, $bigsym + addend` under `kind`, then `exit(0)`.
///
/// `bigsym` is `SHN_ABS` with `value`, so its final value is fixed by the input and the
/// linker has no layout choice that could make the relocation fit.
fn absolute_symbol_reference(
    kind: u32,
    value: u64,
    addend: i64,
) -> Result<(Vec<u8>, u64), Failure> {
    let mut code = Code::new();
    let site = if kind == R_X86_64_32 {
        let at = code.len() + 1;
        code.mov_r32_symbol_addr32(Reg::Rax, "bigsym", addend);
        at
    } else {
        let at = code.len() + 3;
        code.mov_r64_symbol_addr32s(Reg::Rax, "bigsym", addend);
        at
    };
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::absolute("bigsym", value)),
    )?;
    Ok((obj, site))
}

/// The link must be refused, must name `symbol`, and must leave no output file behind.
fn assert_refused(ctx: &mut Ctx, link: &Link, doing: &str, symbol: &str) -> Result<(), Failure> {
    let run = ctx.link_fails(link, doing)?;
    let mut c = Check::new(format!("the diagnostic for {doing}"));
    c.mentions("linker.stderr", symbol, &run.output.stderr);
    c.that(
        "linker.output_file",
        "no output file — a link that failed must not leave a half-relocated binary behind",
        run.produced.is_none(),
        match &run.produced {
            Some(b) => format!("{} bytes at {}", b.len(), run.out_path.display()),
            None => "absent".to_string(),
        },
    );
    if !c.ok() {
        c.block("linker output", run.diagnostics());
    }
    c.finish()
}

/// Assert the four bytes at `site` hold the low 32 bits of `value`.
///
/// The comparison is on the stored bits rather than on the whole 64-bit value, because
/// `R_X86_64_32S` legitimately stores a negative number whose 64-bit form has every high bit
/// set.
fn assert_stored32(c: &mut Check, exe: &Elf, path: &str, site: u64, value: u64) {
    let want = value as u32;
    match exe.u32_at_vaddr(site) {
        Ok(got) => {
            c.that(
                path,
                &format!("the low 32 bits of 0x{value:x}, that is 0x{want:x}"),
                got == want,
                format!("0x{got:x}"),
            );
        }
        Err(e) => {
            c.that(
                path,
                "a readable 32-bit field at the relocation site",
                false,
                e.to_string(),
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

link_test!(abs32_above_four_gb, |ctx| {
    let (obj, _) = absolute_symbol_reference(R_X86_64_32, 0x1_0000_0000, 0)?;
    assert_refused(
        ctx,
        &Link::new()
            .object("a.o", obj)
            .label("a 33-bit address in 32 bits"),
        "relocating an R_X86_64_32 against an absolute symbol of 0x100000000",
        "bigsym",
    )?;
    ctx.note(
        "GNU ld says 'relocation truncated to fit: R_X86_64_32 against symbol `bigsym'' and \
         exits 1; the suite asserts only the non-zero exit and that the symbol is named",
    );
    Ok(())
});

link_test!(abs32_at_the_top_of_its_range, |ctx| {
    let (obj, site_offset) = absolute_symbol_reference(R_X86_64_32, 0xffff_ffff, 0)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("the largest value an R_X86_64_32 can hold"),
    )?;
    ctx.expect_output(&linked, "", 0)?;
    let mut c = Check::new("an R_X86_64_32 at the very top of its range");
    let value = linked.address_of("bigsym")?;
    c.addr_eq("output.symtab[bigsym].st_value", 0xffff_ffff, value);
    assert_stored32(
        &mut c,
        &linked.elf,
        "output.text[mov].imm32",
        linked.address_of(DEFAULT_ENTRY)? + site_offset,
        value,
    );
    c.note(
        "R_X86_64_32 is unsigned, so 0xffffffff fits exactly; a linker that range-checks it \
         as a signed value rejects a legal link",
    );
    c.finish()
});

link_test!(abs32s_just_past_its_range, |ctx| {
    let (obj, _) = absolute_symbol_reference(R_X86_64_32S, 0x8000_0000, 0)?;
    assert_refused(
        ctx,
        &Link::new()
            .object("a.o", obj)
            .label("0x80000000 in a signed 32-bit field"),
        "relocating an R_X86_64_32S against an absolute symbol of 0x80000000",
        "bigsym",
    )?;
    ctx.note(
        "0x80000000 fits an R_X86_64_32 and does not fit an R_X86_64_32S: this is the one \
         value that proves the two range checks are actually different code",
    );
    Ok(())
});

link_test!(abs32s_at_the_top_of_its_range, |ctx| {
    let (obj, site_offset) = absolute_symbol_reference(R_X86_64_32S, 0x7fff_ffff, 0)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("the largest positive value an R_X86_64_32S can hold"),
    )?;
    ctx.expect_output(&linked, "", 0)?;
    let mut c = Check::new("an R_X86_64_32S one below the boundary");
    let value = linked.address_of("bigsym")?;
    c.addr_eq("output.symtab[bigsym].st_value", 0x7fff_ffff, value);
    assert_stored32(
        &mut c,
        &linked.elf,
        "output.text[mov].imm32",
        linked.address_of(DEFAULT_ENTRY)? + site_offset,
        value,
    );
    c.finish()
});

link_test!(abs32s_at_the_bottom_of_its_range, |ctx| {
    // -0x80000000, written as the 64-bit value the symbol table actually carries.
    let value = 0xffff_ffff_8000_0000u64;
    let (obj, site_offset) = absolute_symbol_reference(R_X86_64_32S, value, 0)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("the most negative value an R_X86_64_32S can hold"),
    )?;
    ctx.expect_output(&linked, "", 0)?;
    let mut c = Check::new("an R_X86_64_32S at the bottom of its range");
    assert_stored32(
        &mut c,
        &linked.elf,
        "output.text[mov].imm32",
        linked.address_of(DEFAULT_ENTRY)? + site_offset,
        value,
    );
    c.note(
        "the signed range is not symmetric with the unsigned one: -0x80000000 fits an \
         R_X86_64_32S and would be a wild overflow for an R_X86_64_32",
    );
    c.finish()
});

link_test!(addend_causes_the_overflow, |ctx| {
    // The symbol alone fits; the addend is what pushes S + A past 0xffffffff.
    let (obj, _) = absolute_symbol_reference(R_X86_64_32, 0xffff_fff0, 0x20)?;
    assert_refused(
        ctx,
        &Link::new()
            .object("a.o", obj)
            .label("a legal symbol with an illegal addend"),
        "relocating an R_X86_64_32 whose addend carries it past 0xffffffff",
        "bigsym",
    )?;
    ctx.note(
        "0xfffffff0 on its own is a legal R_X86_64_32 value, so a linker that range-checks S \
         instead of S + A accepts this link and writes 0x00000010",
    );
    Ok(())
});

link_test!(pc32_displacement_too_far, |ctx| {
    // An absolute symbol at 0x7fff_0000_0000 is far beyond +/-2 GB of any code address.
    let mut code = Code::new();
    code.call_pc32("farsym");
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::absolute("farsym", 0x7fff_0000_0000)),
    )?;
    assert_refused(
        ctx,
        &Link::new().object("a.o", obj).label("a call 140 TB away"),
        "relocating an R_X86_64_PC32 whose displacement needs more than 32 bits",
        "farsym",
    )?;
    ctx.note(
        "PC-relative fields overflow too, and the check is on the displacement rather than \
         on the address: two sections 3 GB apart in a legitimate image break exactly here",
    );
    Ok(())
});

link_test!(overflow_in_a_data_section, |ctx| {
    let mut code = Code::new();
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(
                SectionSpec::data(".data", vec![0u8; 4])
                    .align(4)
                    .reloc(Reloc::sym(0, "bigsym", R_X86_64_32, 0)),
            )
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("slot", ".data", 0).object(4))
            .symbol(SymbolSpec::absolute("bigsym", 0x1_0000_0000)),
    )?;
    assert_refused(
        ctx,
        &Link::new()
            .object("a.o", obj)
            .label("an overflowing pointer word in .data"),
        "relocating an overflowing R_X86_64_32 inside .data",
        "bigsym",
    )?;
    ctx.note(
        "nothing in .text is wrong here; a linker that only range-checks code relocations \
         produces a binary with a silently truncated pointer in it",
    );
    Ok(())
});

/// Worked examples: the value that does not fit, and the one that just does.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::error("An address that does not fit", "ld -o prog big.o", || {
            absolute_symbol_reference(R_X86_64_32, 0x1_0000_0000, 0)
                .map(|(bytes, _)| bytes)
                .map_err(|f| f.messages.join("; "))
        })
        .request(
            "big.o: `mov eax, imm32` with an R_X86_64_32 against `bigsym', which the symbol \
             table defines as SHN_ABS with st_value 0x100000000 — a value no layout decision \
             can change",
        )
        .response(
            "The linker exits non-zero with a diagnostic naming `bigsym' and writes no \
             output file. GNU ld's wording is 'relocation truncated to fit: R_X86_64_32 \
             against symbol `bigsym''; the suite never checks the wording",
        )
        .note(
            "Truncating instead would store 0x00000000 and hand over a program that reads \
             address zero — a crash with nothing in it pointing back at this relocation.",
        ),
        ExampleSpec::object(
            "The largest value that does fit",
            "ld -o prog edge.o",
            || {
                absolute_symbol_reference(R_X86_64_32, 0xffff_ffff, 0)
                    .map(|(bytes, _)| bytes)
                    .map_err(|f| f.messages.join("; "))
            },
        )
        .request(
            "The same object with `bigsym' at 0xffffffff, the largest value an unsigned \
             32-bit field can hold",
        )
        .response(
            "The link succeeds and the imm32 holds 0xffffffff. The same value under an \
             R_X86_64_32S would be an error, because -1 and 4294967295 are not the same \
             number in a signed field",
        )
        .note(
            "Write the two range checks separately. Folding them into one signed test \
             rejects this legal link; folding them into one unsigned test accepts the \
             0x80000000 that stage 27 rejects.",
        )
        .runs("", 0),
    ]
}
