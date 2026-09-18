//! Stage 16 — A global defined in another object.
//!
//! The first stage where the linker has to have a *symbol table of its own*. Up to here an
//! input's relocations only ever pointed at things that input already defined; from here on
//! `_start` calls a function that lives in a different file, and the only way to patch the
//! `call` is to have gathered every global definition from every input first, then looked
//! the name up.

use crate::asm::{Code, Reg, STDOUT};
use crate::assert::Check;
use crate::elf::write::{ObjectBuilder, SectionSpec, SymbolSpec};
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 16,
        slug: "global_across_objects",
        name: "A global defined in another object",
        ext: false,
        hints: &[
            "Make one pass over every input first, recording each STB_GLOBAL definition \
             (name, its object, its section and st_value) before you try to patch anything",
            "A relocation's S is the *output* address of the symbol it names: the address the \
             defining input section was given, plus the symbol's st_value inside it",
            "Undefined entries (st_shndx == SHN_UNDEF) are references, not definitions — an \
             object mentioning a name never means it provides it",
            "Command-line order must not change the answer here: with only objects on the \
             line, every one of them is loaded, so `a.o b.o` and `b.o a.o` link the same \
             program",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "a call to a global defined in another object links and runs",
                call_across_objects,
            ),
            Test::new(
                "the call displacement is S + A - P for the addresses the linker chose",
                call_displacement,
            ),
            Test::new("a chain of three objects resolves", chain_of_three),
            Test::new(
                "a data symbol defined in another object is read at run time",
                data_across_objects,
            ),
            Test::new(
                "the resolved callee lies inside an executable segment",
                callee_is_executable,
            ),
            Test::new(
                "the resolved symbol appears in the output's .symtab as a definition",
                symbol_is_defined_in_output,
            ),
            Test::new(
                "the same two objects in the other order link the same program",
                other_order,
            ),
            Test::new(
                "two objects referring to the same global get the same address",
                one_definition_one_address,
            ),
        ],
    }
}

link_test!(call_across_objects, |ctx| {
    let a = caller("two objects\n", "other")?;
    let b = callee_returning("other", 7)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("a caller and its callee"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "two objects\n", 7)?;
    Ok(())
});

link_test!(call_displacement, |ctx| {
    let a = caller("disp\n", "other")?;
    let b = callee_returning("other", 3)?;
    let linked = ctx.link_ok(&Link::new().object("a.o", a).object("b.o", b))?;
    ctx.expect_output(&linked, "disp\n", 3)?;

    let site = linked.address_of(DEFAULT_ENTRY)? + CALLER_CALL_DISP_OFFSET;
    let target = linked.address_of("other")?;
    let mut c = Check::new("the displacement the linker wrote into the call");
    // `call` is R_X86_64_PLT32 with A = -4; in a static link with no PLT that is S + A - P.
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
            "_start is at 0x{:x} and 'other' at 0x{target:x}",
            linked.address_of(DEFAULT_ENTRY)?
        ));
        c.block("linker command", linked.run.output.command_line());
        c.block("output section headers", linked.elf.section_header_table());
    }
    c.finish()
});

link_test!(chain_of_three, |ctx| {
    // _start -> middle -> deep, with each link of the chain in its own object.
    let a = caller("chain\n", "middle")?;
    let b = tail_call("middle", "deep")?;
    let c = callee_returning("deep", 21)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .object("c.o", c)
            .label("a three-object chain"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "chain\n", 21)?;
    let mut check = Check::new("that every link of the chain got an address");
    for name in [DEFAULT_ENTRY, "middle", "deep"] {
        match linked.elf.symbol_address(name) {
            Ok(a) => {
                check.observe(
                    &format!("output.symtab['{name}'].st_value"),
                    format!("0x{a:x}"),
                );
            }
            Err(e) => {
                check.that(
                    &format!("output.symtab['{name}']"),
                    "a defined symbol in the output",
                    false,
                    e.to_string(),
                );
            }
        }
    }
    check.finish()
});

link_test!(data_across_objects, |ctx| {
    // b.o owns the four bytes; a.o only knows the name.
    let a = read_global_and_exit("value")?;
    let b = data_word("value", 93)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("a data symbol defined elsewhere"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "", 93)?;
    Ok(())
});

link_test!(callee_is_executable, |ctx| {
    let a = caller("exec\n", "other")?;
    let b = callee_returning("other", 1)?;
    let linked = ctx.link_ok(&Link::new().object("a.o", a).object("b.o", b))?;
    let target = linked.address_of("other")?;
    let mut c = Check::new("where the callee ended up");
    let seg = linked.elf.segment_at(target);
    c.that(
        "output.symtab['other'].st_value",
        "an address inside an executable PT_LOAD — it is code and it is about to be called",
        seg.map(|s| s.executable()).unwrap_or(false),
        segment_summary(&linked.elf, target),
    );
    c.that(
        "output.symtab['other'].st_value",
        "an address different from _start's — the two objects' .text cannot overlap",
        target != linked.address_of(DEFAULT_ENTRY)?,
        format!("0x{target:x}"),
    );
    if !c.ok() {
        c.block("output program headers", linked.elf.program_header_table());
        c.block("output section headers", linked.elf.section_header_table());
    }
    c.finish()?;
    ctx.expect_output(&linked, "exec\n", 1)?;
    Ok(())
});

link_test!(symbol_is_defined_in_output, |ctx| {
    let a = caller("symtab\n", "other")?;
    let b = callee_returning("other", 5)?;
    let linked = ctx.link_ok(&Link::new().object("a.o", a).object("b.o", b))?;
    let mut c = Check::new("the output's own symbol table");
    match linked.elf.symbol("other") {
        Some(s) => {
            c.that(
                "output.symtab['other'].st_shndx",
                "not SHN_UNDEF — the reference was resolved, so the output defines it",
                !s.is_undefined(),
                format!("st_shndx {}", s.shndx),
            );
            c.ne("output.symtab['other'].st_value", 0u64, s.value);
        }
        None => {
            c.that(
                "output.symtab['other']",
                "a symbol table entry for the resolved global",
                false,
                "no symbol of that name in the output",
            );
            c.note(
                "keeping a .symtab in the output is not strictly required by the ABI, but it \
                 is what every linker does and what every debugger needs",
            );
        }
    }
    if !c.ok() {
        c.block("linker command", linked.run.output.command_line());
        c.block("output sections", linked.elf.section_header_table());
    }
    c.finish()?;
    ctx.expect_output(&linked, "symtab\n", 5)?;
    Ok(())
});

link_test!(other_order, |ctx| {
    let a = caller("order\n", "other")?;
    let b = callee_returning("other", 11)?;
    let forward = ctx.link_ok(
        &Link::new()
            .object("a.o", a.clone())
            .object("b.o", b.clone())
            .label("caller first"),
    )?;
    ctx.expect_output(&forward, "order\n", 11)?;
    let reversed = ctx.link_ok(
        &Link::new()
            .object("b.o", b)
            .object("a.o", a)
            .out("reversed")
            .label("callee first"),
    )?;
    assert_runnable_layout(&reversed)?;
    ctx.expect_output(&reversed, "order\n", 11)?;
    let mut c = Check::new("that object order does not change the program");
    c.that(
        "output.e_entry",
        "an entry point inside an executable segment in both links",
        reversed
            .elf
            .segment_at(reversed.elf.entry)
            .map(|s| s.executable())
            .unwrap_or(false),
        segment_summary(&reversed.elf, reversed.elf.entry),
    );
    c.note(
        "the two links may lay the sections out differently — only the behaviour has to \
         match, not the addresses",
    );
    c.finish()
});

link_test!(one_definition_one_address, |ctx| {
    // Two separate objects call the same function; both calls must land on one address.
    let a = caller("shared\n", "other")?;
    let b = tail_call("middle", "other")?;
    let c = callee_returning("other", 13)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .object("c.o", c)
            .label("two callers, one callee"),
    )?;
    ctx.expect_output(&linked, "shared\n", 13)?;

    let target = linked.address_of("other")?;
    let from_start = linked.address_of(DEFAULT_ENTRY)? + CALLER_CALL_DISP_OFFSET;
    let from_middle = linked.address_of("middle")? + 1;
    let mut check = Check::new("that both calls point at the one definition");
    assert_pc32(
        &mut check,
        &linked.elf,
        "output.text[_start + call].disp32",
        from_start,
        target,
        -4,
    );
    assert_pc32(
        &mut check,
        &linked.elf,
        "output.text[middle + call].disp32",
        from_middle,
        target,
        -4,
    );
    if !check.ok() {
        check.block("output section headers", linked.elf.section_header_table());
    }
    check.finish()
});

/// An object defining `name` as a function that immediately calls `next` and returns its
/// result — one link of a call chain, referring to `next` as an undefined global.
fn tail_call(name: &str, next: &str) -> Result<Vec<u8>, crate::assert::Failure> {
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

/// An object whose `_start` loads the four bytes at `name` and exits with them; `name` is
/// undefined here and has to come from somewhere else.
fn read_global_and_exit(name: &str) -> Result<Vec<u8>, crate::assert::Failure> {
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

/// An object whose `.data` holds one 32-bit `value` under the global name `name`.
fn data_word(name: &str, value: u32) -> Result<Vec<u8>, crate::assert::Failure> {
    build(
        ObjectBuilder::new()
            .section(SectionSpec::data(".data", value.to_le_bytes().to_vec()).align(4))
            .symbol(SymbolSpec::global(name, ".data", 0).object(4)),
    )
}

/// An object that prints `message` and exits 0 — used only to give an example a second file.
fn print_only(message: &str) -> Result<Vec<u8>, crate::assert::Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "message", message.len() as u32);
    code.mov_r32_imm32(Reg::Rax, 0);
    code.ret();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .symbol(SymbolSpec::global("banner", ".text", 0).func())
            .symbol(SymbolSpec::local("message", ".rodata", 0).object(message.len() as u64)),
    )
}

/// Worked examples: the two halves of the smallest cross-object program.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "The caller: a call to a name it does not define",
            "ld -o prog a.o b.o",
            || caller("two objects\n", "other").map_err(|f| f.messages.join("; ")),
        )
        .request(
            "a.o: .text writes the banner, then `e8 00 00 00 00` — a call whose displacement \
             is still zero — followed by exit(eax). Its .symtab holds _start (defined) and \
             `other` with st_shndx = SHN_UNDEF: a reference, not a definition. The .rela.text \
             entry is an R_X86_64_PLT32 against `other` with addend -4",
        )
        .response(
            "The linker resolves `other` to the definition b.o carries, and writes \
             S + A - P into the four bytes at the call site: S is the output address of \
             `other`, A is -4, P is the address of the displacement field itself",
        )
        .note(
            "The -4 is not a fudge factor. P is the address of the four-byte field, but the \
             processor adds the displacement to the address of the *next* instruction, four \
             bytes further on — the addend carries that difference.",
        )
        .runs("two objects\n", 7),
        ExampleSpec::object(
            "The callee: one global definition and nothing else",
            "ld -o prog a.o b.o",
            || callee_returning("other", 7).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "b.o: a five-byte .text (`mov eax, 7` then `ret`), no .rodata, no relocations, and \
             one STB_GLOBAL STT_FUNC symbol `other` at st_value 0 of that .text",
        )
        .response(
            "b.o's .text is concatenated after a.o's, so `other`'s output address is the \
             address that second .text got plus st_value 0. The program prints the banner and \
             exits 7 — the value b.o put in eax",
        )
        .note(
            "Symbol resolution is one pass over the inputs to collect definitions, then one \
             pass over the relocations to spend them. Trying to do both at once breaks the \
             moment a definition comes after the reference.",
        )
        .runs("two objects\n", 7),
        ExampleSpec::object(
            "A function the program may or may not call",
            "ld -o prog a.o b.o c.o",
            || print_only("banner\n").map_err(|f| f.messages.join("; ")),
        )
        .request(
            "c.o: a global `banner` that writes its own .rodata and returns; the string is \
             reached through a *local* symbol, so nothing in this object is visible to the \
             others except the name `banner`",
        )
        .response(
            "Whether or not anything calls it, an object named on the command line is always \
             loaded in full: its .text and .rodata are allocated and its global is recorded",
        )
        .note(
            "This is the difference between an object and an archive member. Objects on the \
             command line are unconditional; archive members (stage 32) are pulled in only \
             when they resolve something.",
        ),
    ]
}
