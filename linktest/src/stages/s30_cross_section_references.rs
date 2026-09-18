//! Stage 30 — References across sections and objects.
//!
//! By now every relocation kind works in isolation. This stage is about the bookkeeping
//! underneath them: once `.text`, `.rodata` and `.data` have been concatenated from several
//! inputs and given addresses, a relocation has to be able to start in any of them and land
//! in any other. There is nothing new to compute — only a symbol table and a section map
//! that stay right no matter which direction the arrow points.

use crate::asm::{Code, Reg, STDOUT};
use crate::assert::Check;
use crate::elf::write::{ObjectBuilder, Reloc, SectionSpec, SymbolSpec};
use crate::elf::{R_X86_64_64, R_X86_64_PC32};
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 30,
        slug: "cross_section_references",
        name: "References across sections and objects",
        ext: false,
        hints: &[
            "Give every input section its final address before applying a single \
             relocation: the value of a symbol is its section's address plus st_value, and \
             that is only knowable once layout is finished",
            "Keep one map from (input object, section index) to the address that section \
             landed at — it answers both 'where is this symbol' and 'where is this \
             relocation site'",
            "A relocation's site is in the section that owns the .rela, and its target can \
             be in any section of any object; nothing about the arithmetic changes with the \
             direction",
            "Data can point at code and code at data: a function pointer in .data is an \
             R_X86_64_64 whose symbol happens to be STT_FUNC, and nothing special is needed \
             for it",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(".text reaches a string in .rodata", text_to_rodata),
            Test::new(".text reaches a string in .data", text_to_data),
            Test::new(".data holds a pointer into .text", data_to_text),
            Test::new(".rodata holds a pointer into .rodata", rodata_to_rodata),
            Test::new(
                "two objects reference each other in both directions",
                both_directions_across_objects,
            ),
            Test::new(
                "a chain of three objects each calling the next",
                a_chain_of_three,
            ),
            Test::new("a section holding a pointer into itself", self_reference),
            Test::new(
                "one program whose sections all point at each other",
                every_direction_at_once,
            ),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

link_test!(text_to_rodata, |ctx| {
    let mut code = Code::new();
    let site_offset = code.len() + WRITE_LEA_DISP_OFFSET;
    code.sys_write(STDOUT, "romsg", 8);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"in .ro\n\0".to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("romsg", ".rodata", 0).object(7)),
    )?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).label(".text -> .rodata"))?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "in .ro\n\0", 0)?;
    let mut c = Check::new("a reference from code into read-only data");
    let target = linked.address_of("romsg")?;
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[lea].disp32",
        linked.address_of(DEFAULT_ENTRY)? + site_offset,
        target,
        -4,
    );
    c.that(
        "output.segment_at(romsg)",
        "a readable, non-executable mapping — or at worst one that is merely readable",
        linked
            .elf
            .segment_at(target)
            .map(|s| s.readable())
            .unwrap_or(false),
        segment_summary(&linked.elf, target),
    );
    c.finish()
});

link_test!(text_to_data, |ctx| {
    let mut code = Code::new();
    let site_offset = code.len() + WRITE_LEA_DISP_OFFSET;
    code.sys_write(STDOUT, "damsg", 8);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::data(".data", b"in .data".to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("damsg", ".data", 0).object(8)),
    )?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).label(".text -> .data"))?;
    ctx.expect_output(&linked, "in .data", 0)?;
    let mut c = Check::new("a reference from code into writable data");
    let target = linked.address_of("damsg")?;
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[lea].disp32",
        linked.address_of(DEFAULT_ENTRY)? + site_offset,
        target,
        -4,
    );
    c.that(
        "output.segment_at(damsg)",
        "a writable mapping — .data is not read-only",
        linked
            .elf
            .segment_at(target)
            .map(|s| s.writable())
            .unwrap_or(false),
        segment_summary(&linked.elf, target),
    );
    c.note(
        ".text and .data almost always end up in different segments with different \
         permissions, so this displacement crosses a page boundary the linker chose",
    );
    c.finish()
});

link_test!(data_to_text, |ctx| {
    let mut fun = Code::new();
    fun.mov_r32_imm32(Reg::Rax, 13);
    fun.ret();
    let fun_len = fun.len();
    let mut code = Code::new();
    code.mov_r64_rip(Reg::Rax, "fp", R_X86_64_PC32, -4);
    code.raw(&[0xff, 0xd0]); // call rax
    code.sys_exit_eax();
    let fun_offset = code.len();
    code.append(&fun);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(
                SectionSpec::data(".data", vec![0u8; 8])
                    .align(8)
                    .reloc(Reloc::sym(0, "fun", R_X86_64_64, 0)),
            )
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(
                SymbolSpec::global("fun", ".text", fun_offset)
                    .func()
                    .size(fun_len),
            )
            .symbol(SymbolSpec::global("fp", ".data", 0).object(8)),
    )?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).label(".data -> .text"))?;
    ctx.expect_output(&linked, "", 13)?;
    let mut c = Check::new("a function pointer stored in .data");
    let fun_addr = linked.address_of("fun")?;
    assert_abs64(
        &mut c,
        &linked.elf,
        "output.data[fp]",
        linked.address_of("fp")?,
        fun_addr,
        0,
    );
    c.that(
        "output.segment_at(fun)",
        "an executable mapping — the program calls through this pointer",
        linked
            .elf
            .segment_at(fun_addr)
            .map(|s| s.executable())
            .unwrap_or(false),
        segment_summary(&linked.elf, fun_addr),
    );
    c.finish()
});

link_test!(rodata_to_rodata, |ctx| {
    let mut rodata = b"self\n".to_vec();
    rodata.resize(8, 0);
    rodata.extend_from_slice(&[0u8; 8]);
    let mut code = Code::new();
    code.mov_r64_rip(Reg::Rsi, "roptr", R_X86_64_PC32, -4);
    code.sys_write_rsi(STDOUT, 5);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(
                SectionSpec::rodata(".rodata", rodata)
                    .align(8)
                    .reloc(Reloc::sym(8, "romsg", R_X86_64_64, 0)),
            )
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("romsg", ".rodata", 0).object(5))
            .symbol(SymbolSpec::global("roptr", ".rodata", 8).object(8)),
    )?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).label(".rodata -> .rodata"))?;
    ctx.expect_output(&linked, "self\n", 0)?;
    let mut c = Check::new("a pointer inside .rodata to another part of .rodata");
    assert_abs64(
        &mut c,
        &linked.elf,
        "output.rodata[roptr]",
        linked.address_of("roptr")?,
        linked.address_of("romsg")?,
        0,
    );
    c.finish()
});

link_test!(both_directions_across_objects, |ctx| {
    // a.o calls into b.o; b.o's code reads a string defined in a.o.
    let mut first = Code::new();
    let call_site = first.len() + 1;
    first.call("show");
    first.sys_exit(0);
    let a = build(
        ObjectBuilder::new()
            .section(text_of(&first))
            .section(SectionSpec::rodata(".rodata", b"from a.o\n".to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("amsg", ".rodata", 0).object(9))
            .symbol(SymbolSpec::undefined("show")),
    )?;
    let mut second = Code::new();
    let lea_site = second.len() + WRITE_LEA_DISP_OFFSET;
    second.sys_write(STDOUT, "amsg", 9);
    second.ret();
    let b = build(
        ObjectBuilder::new()
            .section(text_of(&second))
            .symbol(SymbolSpec::global("show", ".text", 0).func())
            .symbol(SymbolSpec::undefined("amsg")),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("two objects pointing at each other"),
    )?;
    ctx.expect_output(&linked, "from a.o\n", 0)?;
    let mut c = Check::new("references in both directions between two inputs");
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[a.o call].disp32",
        linked.address_of(DEFAULT_ENTRY)? + call_site,
        linked.address_of("show")?,
        -4,
    );
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[b.o lea].disp32",
        linked.address_of("show")? + lea_site,
        linked.address_of("amsg")?,
        -4,
    );
    c.note(
        "neither object can be linked alone; each supplies exactly the symbol the other is \
         missing",
    );
    c.finish()
});

link_test!(a_chain_of_three, |ctx| {
    // _start -> bfn -> cfn, each adding to what the next returned.
    let mut first = Code::new();
    let a_site = first.len() + 1;
    first.call("bfn");
    first.sys_exit_eax();
    let a = build(
        ObjectBuilder::new()
            .section(text_of(&first))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::undefined("bfn")),
    )?;
    let mut second = Code::new();
    let b_site = second.len() + 1;
    second.call("cfn");
    second.add_r32_imm32(Reg::Rax, 1);
    second.ret();
    let b = build(
        ObjectBuilder::new()
            .section(text_of(&second))
            .symbol(SymbolSpec::global("bfn", ".text", 0).func())
            .symbol(SymbolSpec::undefined("cfn")),
    )?;
    let mut third = Code::new();
    third.mov_r32_imm32(Reg::Rax, 10);
    third.ret();
    let c_obj = build(
        ObjectBuilder::new()
            .section(text_of(&third))
            .symbol(SymbolSpec::global("cfn", ".text", 0).func()),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .object("c.o", c_obj)
            .label("a three-link chain"),
    )?;
    ctx.expect_output(&linked, "", 11)?;
    let mut c = Check::new("each link of the chain");
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[a.o -> bfn].disp32",
        linked.address_of(DEFAULT_ENTRY)? + a_site,
        linked.address_of("bfn")?,
        -4,
    );
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text[b.o -> cfn].disp32",
        linked.address_of("bfn")? + b_site,
        linked.address_of("cfn")?,
        -4,
    );
    c.note(
        "the exit status is 10 + 1, so both calls returned; a wrong displacement anywhere in \
         the chain shows up as a signal rather than a status",
    );
    c.finish()
});

link_test!(self_reference, |ctx| {
    // .data holds a string and, eight bytes on, a pointer back to it.
    let mut data = b"itself\n".to_vec();
    data.resize(8, 0);
    data.extend_from_slice(&[0u8; 8]);
    let mut code = Code::new();
    code.mov_r64_rip(Reg::Rsi, "dptr", R_X86_64_PC32, -4);
    code.sys_write_rsi(STDOUT, 7);
    code.sys_exit(0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::data(".data", data).align(8).reloc(Reloc::sym(
                8,
                "dmsg",
                R_X86_64_64,
                0,
            )))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("dmsg", ".data", 0).object(7))
            .symbol(SymbolSpec::global("dptr", ".data", 8).object(8)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("a section pointing into itself"),
    )?;
    ctx.expect_output(&linked, "itself\n", 0)?;
    let mut c = Check::new("a relocation whose site and target share a section");
    let ptr = linked.address_of("dptr")?;
    let msg = linked.address_of("dmsg")?;
    assert_abs64(&mut c, &linked.elf, "output.data[dptr]", ptr, msg, 0);
    c.addr_eq("output.data[dptr] - output.data[dmsg]", 8, ptr - msg);
    c.finish()
});

link_test!(every_direction_at_once, |ctx| {
    // .rodata: "ro\n" "r2\n" pad, then a pointer to .data; .data: "da\n" pad, then a
    // pointer to .rodata + 3.
    let mut rodata = b"ro\nr2\n".to_vec();
    rodata.resize(8, 0);
    rodata.extend_from_slice(&[0u8; 8]);
    let mut data = b"da\n".to_vec();
    data.resize(8, 0);
    data.extend_from_slice(&[0u8; 8]);

    let mut code = Code::new();
    let to_rodata = code.len() + WRITE_LEA_DISP_OFFSET;
    code.sys_write(STDOUT, "romsg", 3);
    let to_data = code.len() + WRITE_LEA_DISP_OFFSET;
    code.sys_write(STDOUT, "damsg", 3);
    code.mov_r64_rip(Reg::Rsi, "d2r", R_X86_64_PC32, -4);
    code.sys_write_rsi(STDOUT, 3);
    code.mov_r64_rip(Reg::Rsi, "r2d", R_X86_64_PC32, -4);
    code.sys_write_rsi(STDOUT, 3);
    code.sys_exit(0);

    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(
                SectionSpec::rodata(".rodata", rodata)
                    .align(8)
                    .reloc(Reloc::sym(8, "damsg", R_X86_64_64, 0)),
            )
            .section(SectionSpec::data(".data", data).align(8).reloc(Reloc::sym(
                8,
                "romsg",
                R_X86_64_64,
                3,
            )))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("romsg", ".rodata", 0).object(3))
            .symbol(SymbolSpec::global("r2d", ".rodata", 8).object(8))
            .symbol(SymbolSpec::global("damsg", ".data", 0).object(3))
            .symbol(SymbolSpec::global("d2r", ".data", 8).object(8)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("four arrows between three sections"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "ro\nda\nr2\nda\n", 0)?;
    let base = linked.address_of(DEFAULT_ENTRY)?;
    let mut c = Check::new("four references, one in each direction");
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text -> .rodata",
        base + to_rodata,
        linked.address_of("romsg")?,
        -4,
    );
    assert_pc32(
        &mut c,
        &linked.elf,
        "output.text -> .data",
        base + to_data,
        linked.address_of("damsg")?,
        -4,
    );
    assert_abs64(
        &mut c,
        &linked.elf,
        "output.data -> .rodata",
        linked.address_of("d2r")?,
        linked.address_of("romsg")?,
        3,
    );
    assert_abs64(
        &mut c,
        &linked.elf,
        "output.rodata -> .data",
        linked.address_of("r2d")?,
        linked.address_of("damsg")?,
        0,
    );
    c.finish()
});

/// Worked examples: the two directions that are easy to get wrong.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "Code pointing at data, data pointing at code",
            "ld -o prog both.o",
            || {
                let mut fun = Code::new();
                fun.mov_r32_imm32(Reg::Rax, 13);
                fun.ret();
                let fun_len = fun.len();
                let mut code = Code::new();
                code.mov_r64_rip(Reg::Rax, "fp", R_X86_64_PC32, -4);
                code.raw(&[0xff, 0xd0]);
                code.sys_exit_eax();
                let fun_offset = code.len();
                code.append(&fun);
                build(
                    ObjectBuilder::new()
                        .section(text_of(&code))
                        .section(
                            SectionSpec::data(".data", vec![0u8; 8])
                                .align(8)
                                .reloc(Reloc::sym(0, "fun", R_X86_64_64, 0)),
                        )
                        .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
                        .symbol(
                            SymbolSpec::global("fun", ".text", fun_offset)
                                .func()
                                .size(fun_len),
                        )
                        .symbol(SymbolSpec::global("fp", ".data", 0).object(8)),
                )
                .map_err(|f| f.messages.join("; "))
            },
        )
        .request(
            "both.o: .text loads eight bytes from .data into rax and calls through them; \
             .data is eight zero bytes with an R_X86_64_64 naming the STT_FUNC symbol `fun', \
             which lives later in the same .text",
        )
        .response(
            "The .data word holds the final address of `fun', which is inside an executable \
             PT_LOAD, and the program exits 13. The RIP-relative load that reads the word is \
             an ordinary PC32 in the other direction",
        )
        .note(
            "Nothing about the function pointer is special. If the linker needs a case for \
             'the symbol is code', the section map is doing work the symbol table should be.",
        )
        .runs("", 13),
        ExampleSpec::text("A chain of three objects", "ld -o prog a.o b.o c.o")
            .request("a.o calls `bfn', which b.o defines and which calls `cfn', which c.o defines")
            .response(
                "Both calls are patched to S + A - P against the addresses the linker chose, and \
             the program runs through all three objects. No input can be linked alone",
            )
            .note(
                "Resolve every symbol before applying any relocation. Doing both in one pass \
             over the inputs works until an object references something defined later.",
            ),
    ]
}
