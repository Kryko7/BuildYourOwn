//! Stage 14 — Concatenating input sections.
//!
//! The central loop of a linker: every input's `.text` becomes one output `.text`, every
//! input's `.rodata` one output `.rodata`, and the contributions are laid down one after
//! another in the order the command line named the files. Each contribution keeps its own
//! `sh_addralign`, so there may be padding between them, and every symbol's new address is
//! "where this contribution landed, plus the symbol's old `st_value`".

use crate::asm::{Code, Reg, STDOUT};
use crate::assert::{Check, Failure};
use crate::elf::read::Elf;
use crate::elf::write::{ObjectBuilder, SectionSpec, SymbolSpec};
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 14,
        slug: "section_concatenation",
        name: "Concatenating input sections",
        ext: false,
        hints: &[
            "Collect the input sections by output name, then place them one after another, \
             remembering for each contribution the offset it was given",
            "Keep the command-line order: every linker lays contributions down in the order \
             the inputs were named, and programs — and debuggers — rely on it",
            "Round the running address up to each contribution's own `sh_addralign` before \
             placing it, not once for the whole output section",
            "A symbol's output address is the address its contribution got plus its input \
             `st_value`; get this wrong by one contribution and every relocation is wrong",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                ".text from three objects is concatenated and all of it runs",
                three_texts_run,
            ),
            Test::new(
                ".rodata from three objects is all present and each string is intact",
                three_rodatas_survive,
            ),
            Test::new(
                "input sections come out in command-line order",
                command_line_order,
            ),
            Test::new(
                "reversing the command line reverses the addresses",
                reversed_command_line,
            ),
            Test::new(
                "each contribution keeps its own sh_addralign",
                per_contribution_alignment,
            ),
            Test::new(
                "a symbol inside a contribution keeps its offset within it",
                offsets_inside_a_contribution,
            ),
            Test::new(
                ".data from three objects is concatenated and the program reads all of it",
                three_datas_are_summed,
            ),
            Test::new(
                "each contribution's bytes survive concatenation unchanged",
                contributed_bytes_are_unchanged,
            ),
        ],
    }
}

link_test!(three_texts_run, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", chain_head("A", "fb")?)
            .object("b.o", chain_middle("B", "fb", "fc")?)
            .object("c.o", chain_tail("C", "fc", 7)?)
            .label("three objects whose .text sections call one another in turn"),
    )?;
    assert_runnable_layout(&linked)?;
    let mut c = Check::new("where the three .text contributions landed");
    for name in [DEFAULT_ENTRY, "fb", "fc"] {
        let addr = linked.address_of(name)?;
        c.that(
            &format!("output.segment('{name}')"),
            "inside an executable PT_LOAD",
            linked
                .elf
                .segment_at(addr)
                .map(|s| s.executable())
                .unwrap_or(false),
            segment_summary(&linked.elf, addr),
        );
    }
    c.finish()?;
    // Each link of the chain prints its own letter before calling the next, so the stdout
    // is the proof that all three contributions are present, mapped and reachable.
    ctx.expect_output(&linked, "ABC", 7)?;
    Ok(())
});

link_test!(three_rodatas_survive, |ctx| {
    let linked = ctx.link_ok(&three_rodata_link()?)?;
    let exe = &linked.elf;
    let mut c = Check::new("the three .rodata contributions, read back out of the output");
    for (symbol, want) in [
        ("s_a", &b"AAA"[..]),
        ("s_b", &b"B1B"[..]),
        ("s_c", &b"CCC"[..]),
    ] {
        let addr = linked.address_of(symbol)?;
        match exe.read_at_vaddr(addr, want.len() as u64) {
            Ok(got) => c.bytes_eq(&format!("output[{symbol}]"), want, got),
            Err(e) => c.that(
                &format!("output[{symbol}]"),
                "readable through some PT_LOAD",
                false,
                e.to_string(),
            ),
        };
    }
    if !c.ok() {
        c.block("output section headers", exe.section_header_table());
    }
    c.finish()?;
    ctx.expect_output(&linked, "AAAB1BCCC", 0)?;
    Ok(())
});

link_test!(command_line_order, |ctx| {
    let linked = ctx.link_ok(&three_rodata_link()?)?;
    let a = linked.address_of("s_a")?;
    let b = linked.address_of("s_b")?;
    let c_addr = linked.address_of("s_c")?;
    let mut c = Check::new("the order the three .rodata contributions were placed in");
    c.that(
        "output.symbol['s_a'] < output.symbol['s_b'] < output.symbol['s_c']",
        "the order a.o b.o c.o were named on the command line",
        a < b && b < c_addr,
        format!("s_a 0x{a:x}, s_b 0x{b:x}, s_c 0x{c_addr:x}"),
    );
    c.finish()?;
    ctx.note(
        "the ELF spec does not require any particular order; every linker in use — GNU ld, \
         gold, lld, mold — lays contributions down in command-line order, and this suite \
         holds a hand-written linker to the same convention",
    );
    ctx.expect_output(&linked, "AAAB1BCCC", 0)?;
    Ok(())
});

link_test!(reversed_command_line, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("c.o", rodata_only("s_c", b"CCC")?)
            .object("b.o", rodata_only("s_b", b"B1B")?)
            .object("a.o", rodata_head()?)
            .label("the same three objects in the opposite order"),
    )?;
    let a = linked.address_of("s_a")?;
    let b = linked.address_of("s_b")?;
    let c_addr = linked.address_of("s_c")?;
    let mut c = Check::new("what reversing the command line did to the layout");
    c.that(
        "output.symbol['s_c'] < output.symbol['s_b'] < output.symbol['s_a']",
        "the reversed order — the layout follows the argv, not the file names",
        c_addr < b && b < a,
        format!("s_a 0x{a:x}, s_b 0x{b:x}, s_c 0x{c_addr:x}"),
    );
    c.finish()?;
    // The program writes the three strings in its own fixed order, so the stdout does not
    // change: only the addresses do.
    ctx.expect_output(&linked, "AAAB1BCCC", 0)?;
    Ok(())
});

link_test!(per_contribution_alignment, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", aligned_head(16)?)
            .object("b.o", aligned_only("s_b", b"B1B", 64)?)
            .object("c.o", aligned_only("s_c", b"CCC", 256)?)
            .label("three .rodata contributions wanting 16-, 64- and 256-byte alignment"),
    )?;
    let mut c = Check::new("each contribution against the alignment it asked for");
    for (symbol, align) in [("s_a", 16u64), ("s_b", 64), ("s_c", 256)] {
        let addr = linked.address_of(symbol)?;
        c.that(
            &format!("output.symbol['{symbol}']"),
            &format!("an address that is a multiple of {align}"),
            addr % align == 0,
            format!("0x{addr:x} (0x{:x} past the boundary)", addr % align),
        );
    }
    c.note(
        "alignment is per contribution, not per output section: the padding between two \
         contributions is whatever the second one's sh_addralign demands",
    );
    c.finish()?;
    ctx.expect_output(&linked, "AAAB1BCCC", 0)?;
    Ok(())
});

link_test!(offsets_inside_a_contribution, |ctx| {
    let linked = ctx.link_ok(&three_rodata_link()?)?;
    let base = linked.address_of("s_b")?;
    let mid = linked.address_of("s_b_mid")?;
    let mut c = Check::new("a symbol one byte into the middle contribution");
    c.addr_eq("output.symbol['s_b_mid']", base + 1, mid);
    match linked.elf.read_at_vaddr(mid, 1) {
        Ok(got) => c.bytes_eq("output[s_b_mid]", b"1", got),
        Err(e) => c.that("output[s_b_mid]", "readable", false, e.to_string()),
    };
    c.note(
        "st_value is an offset inside the input section; the output address is the address \
         the contribution was given plus that offset, and nothing else",
    );
    c.finish()?;
    ctx.expect_output(&linked, "AAAB1BCCC", 0)?;
    Ok(())
});

link_test!(three_datas_are_summed, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", data_head(1)?)
            .object("b.o", data_only("vb", 2)?)
            .object("c.o", data_only("vc", 4)?)
            .label("three objects each contributing one .data word"),
    )?;
    let exe = &linked.elf;
    let mut c = Check::new("the three .data contributions in the output's file image");
    for (symbol, want) in [("va", 1u32), ("vb", 2), ("vc", 4)] {
        let addr = linked.address_of(symbol)?;
        match exe.u32_at_vaddr(addr) {
            Ok(got) => c.eq(&format!("output[{symbol}]"), want, got),
            Err(e) => c.that(
                &format!("output[{symbol}]"),
                "four readable bytes",
                false,
                e.to_string(),
            ),
        };
    }
    let va = linked.address_of("va")?;
    let vb = linked.address_of("vb")?;
    let vc = linked.address_of("vc")?;
    c.that(
        "output.symbol['va'] < output.symbol['vb'] < output.symbol['vc']",
        "command-line order in .data as well as in .rodata",
        va < vb && vb < vc,
        format!("va 0x{va:x}, vb 0x{vb:x}, vc 0x{vc:x}"),
    );
    c.finish()?;
    // The program adds the three words together: 1 + 2 + 4.
    ctx.expect_output(&linked, "", 7)?;
    Ok(())
});

link_test!(contributed_bytes_are_unchanged, |ctx| {
    let b_obj = callee_returning("fb", 9)?;
    let c_obj = callee_returning("fc", 5)?;
    let want_b = section_bytes(&b_obj, ".text")?;
    let want_c = section_bytes(&c_obj, ".text")?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", caller("go\n", "fb")?)
            .object("b.o", b_obj)
            .object("c.o", c_obj)
            .label("a caller and two relocation-free callees"),
    )?;
    let exe = &linked.elf;
    let mut c = Check::new("the bytes of two relocation-free contributions");
    for (symbol, want) in [("fb", &want_b), ("fc", &want_c)] {
        let addr = linked.address_of(symbol)?;
        match exe.read_at_vaddr(addr, want.len() as u64) {
            Ok(got) => c.bytes_eq(&format!("output[{symbol}]"), want, got),
            Err(e) => c.that(
                &format!("output[{symbol}]"),
                "readable through some PT_LOAD",
                false,
                e.to_string(),
            ),
        };
    }
    c.note(
        "neither callee has a relocation, so concatenation must leave their bytes exactly as \
         the input had them; only the addresses around them changed",
    );
    if !c.ok() {
        c.block("output section headers", exe.section_header_table());
    }
    c.finish()?;
    ctx.expect_output(&linked, "go\n", 9)?;
    Ok(())
});

// ---------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------

/// The standard three-object `.rodata` link this stage keeps reusing.
fn three_rodata_link() -> Result<Link, Failure> {
    Ok(Link::new()
        .object("a.o", rodata_head()?)
        .object("b.o", rodata_only("s_b", b"B1B")?)
        .object("c.o", rodata_only("s_c", b"CCC")?)
        .label("three objects each contributing a .rodata"))
}

/// `_start`: write all three strings, then exit 0. Owns the first `.rodata`.
fn rodata_head() -> Result<Vec<u8>, Failure> {
    head_object(b"AAA", 1)
}

/// The same head, but its `.rodata` demands `align` bytes of alignment.
fn aligned_head(align: u64) -> Result<Vec<u8>, Failure> {
    head_object(b"AAA", align)
}

/// The shared body of [`rodata_head`] and [`aligned_head`].
fn head_object(text: &[u8], align: u64) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "s_a", 3);
    code.sys_write(STDOUT, "s_b", 3);
    code.sys_write(STDOUT, "s_c", 3);
    code.sys_exit(0);
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", text.to_vec()).align(align))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("s_a", ".rodata", 0).object(3))
            .symbol(SymbolSpec::undefined("s_b"))
            .symbol(SymbolSpec::undefined("s_c")),
    )
}

/// An object that is nothing but a `.rodata` contribution and the global naming it.
///
/// The middle one also carries `s_b_mid`, one byte in, so a test can prove that offsets
/// inside a contribution survive.
fn rodata_only(symbol: &str, text: &[u8]) -> Result<Vec<u8>, Failure> {
    aligned_only(symbol, text, 1)
}

/// The same, with an explicit `sh_addralign`.
fn aligned_only(symbol: &str, text: &[u8], align: u64) -> Result<Vec<u8>, Failure> {
    let mut b = ObjectBuilder::new()
        .section(SectionSpec::rodata(".rodata", text.to_vec()).align(align))
        .symbol(SymbolSpec::global(symbol, ".rodata", 0).object(text.len() as u64));
    if symbol == "s_b" {
        b = b.symbol(SymbolSpec::global("s_b_mid", ".rodata", 1).object(1));
    }
    build(b)
}

/// `_start`: load three `.data` words, add them up and exit with the sum. Owns the first.
fn data_head(value: u32) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.mov_r32_rip(Reg::Rax, "va", 0);
    code.add_r32_rip(Reg::Rax, "vb", 0);
    code.add_r32_rip(Reg::Rax, "vc", 0);
    code.sys_exit_eax();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::data(".data", value.to_le_bytes().to_vec()).align(4))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("va", ".data", 0).object(4))
            .symbol(SymbolSpec::undefined("vb"))
            .symbol(SymbolSpec::undefined("vc")),
    )
}

/// An object that is nothing but one initialised `.data` word.
fn data_only(symbol: &str, value: u32) -> Result<Vec<u8>, Failure> {
    build(
        ObjectBuilder::new()
            .section(SectionSpec::data(".data", value.to_le_bytes().to_vec()).align(4))
            .symbol(SymbolSpec::global(symbol, ".data", 0).object(4)),
    )
}

/// The head of the calling chain: print `letter`, call `next`, exit with what it returned.
fn chain_head(letter: &str, next: &str) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "letter_a", 1);
    code.call(next);
    code.sys_exit_eax();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", letter.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::local("letter_a", ".rodata", 0).object(1))
            .symbol(SymbolSpec::undefined(next)),
    )
}

/// A middle link: print `letter`, call `next`, return whatever it returned.
fn chain_middle(letter: &str, name: &str, next: &str) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "letter_b", 1);
    code.call(next);
    code.ret();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", letter.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(name, ".text", 0).func())
            .symbol(SymbolSpec::local("letter_b", ".rodata", 0).object(1))
            .symbol(SymbolSpec::undefined(next)),
    )
}

/// The last link: print `letter`, return `value`.
fn chain_tail(letter: &str, name: &str, value: u32) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "letter_c", 1);
    code.mov_r32_imm32(Reg::Rax, value);
    code.ret();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", letter.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(name, ".text", 0).func())
            .symbol(SymbolSpec::local("letter_c", ".rodata", 0).object(1)),
    )
}

/// The bytes of one section of an object the suite just built.
fn section_bytes(object: &[u8], name: &str) -> Result<Vec<u8>, Failure> {
    let elf = Elf::parse(object)
        .map_err(|e| Failure::harness(format!("the suite's own object does not parse: {e}")))?;
    let data = elf
        .section_data(name)
        .map_err(|e| Failure::harness(format!("the suite's own object has no '{name}': {e}")))?;
    Ok(data.to_vec())
}

/// Worked examples: three inputs, one output section.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "The first of three .rodata contributions",
            "ld -o prog a.o b.o c.o",
            || rodata_head().map_err(|f| f.messages.join("; ")),
        )
        .request(
            "a.o: .text writes `AAA`, `B1B` and `CCC` in turn and exits 0; its own .rodata \
             holds `AAA` and defines s_a, while s_b and s_c are undefined and come from b.o \
             and c.o, which are nothing but .rodata contributions",
        )
        .response(
            "One output .rodata holding `AAAB1BCCC` — the three contributions in \
             command-line order, each at the address the linker gave it — with s_a < s_b < \
             s_c, s_b_mid one byte past s_b, and every lea patched to reach its own string",
        )
        .note(
            "Sorting the contributions by name, by size or by symbol order all produce a \
             program that still prints the same nine characters. The order only becomes \
             visible when you compare addresses, which is why the suite does.",
        )
        .runs("AAAB1BCCC", 0),
        ExampleSpec::object(
            "The head of a three-object call chain",
            "ld -o prog a.o b.o c.o",
            || chain_head("A", "fb").map_err(|f| f.messages.join("; ")),
        )
        .request(
            "a.o: _start writes `A`, calls the undefined `fb` with an R_X86_64_PLT32, then \
             exits with whatever is in eax; b.o's `fb` writes `B` and calls c.o's `fc`, \
             which writes `C` and returns 7",
        )
        .response(
            "One output .text holding all three contributions inside the same executable \
             PT_LOAD, with both calls' displacements patched across contribution boundaries; \
             the program prints `ABC` and exits 7",
        )
        .note(
            "Each contribution's own offset has to be remembered: a call's displacement is \
             computed from the address the *target's* contribution got, not from the address \
             of the output section.",
        )
        .runs("ABC", 7),
    ]
}
