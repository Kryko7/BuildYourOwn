//! Stage 42 — readelf, nm and objdump agree.
//!
//! The optional fourth leg. Everything up to here has been checked twice: the program was
//! run, and the file was re-parsed by the suite's own reader. Both of those could be wrong
//! in the same way — the suite could be reading the file exactly as the linker wrote it and
//! both could be wrong about what an ELF64 executable is.
//!
//! So this stage hands the output to three programs nobody in this repository wrote:
//! `readelf`, `nm` and `objdump`. They are binutils' own readers, they have been reading ELF
//! since 1995, and they say so loudly when a file is malformed. If they agree with the
//! suite's parse about the entry point, the segments, the symbols and where the code starts,
//! the output is an ELF file in the sense the rest of the world means.
//!
//! Every test here skips — with a written reason — when its tool is not on `PATH`, because a
//! machine without binutils is a machine where this leg cannot run, not a machine where the
//! linker is wrong.

use crate::assert::{Check, Failure};
use crate::elf::read::{Elf, Segment};
use crate::elf::write::{ObjectBuilder, SectionSpec, SymbolSpec};
use crate::examples::ExampleSpec;
use crate::link::{Link, Linked};
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Ctx, Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 42,
        slug: "toolchain_interop",
        name: "readelf, nm and objdump agree",
        ext: true,
        hints: &[
            "If readelf prints a warning about the file, fix the field it is complaining about \
             — it is almost always sh_link, sh_entsize or an sh_offset that runs past the end",
            "objdump and nm read the section header table, not the program headers: an \
             executable that runs perfectly can still be unreadable to every tool if its \
             section headers are wrong or missing",
            "A .symtab is optional for running and essential for debugging; emit one with \
             st_value set to each symbol's final address, sh_link pointing at the .strtab and \
             sh_info equal to the index of the first non-local",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "readelf -h reports the same type, machine and entry point",
                readelf_header,
            )
            .ext(),
            Test::new(
                "readelf -lW reports the same PT_LOADs with the same permissions",
                readelf_segments,
            )
            .ext(),
            Test::new(
                "readelf -SW agrees about the allocated sections",
                readelf_sections,
            )
            .ext(),
            Test::new(
                "readelf -a reports no errors and no warnings",
                readelf_is_quiet,
            )
            .ext(),
            Test::new(
                "nm lists _start at the address the suite computed",
                nm_start,
            )
            .ext(),
            Test::new("nm sees every global the program defined", nm_globals).ext(),
            Test::new(
                "objdump -d disassembles at the entry point",
                objdump_disassembles,
            )
            .ext(),
            Test::new(
                "objdump -f agrees about the format and the start address",
                objdump_file_header,
            )
            .ext(),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// The program the tools are pointed at
// ---------------------------------------------------------------------------------------

/// What the interop program prints.
const MESSAGE: &str = "interop\n";

/// What it exits with.
const STATUS: i32 = 19;

/// A three-object program with a `.text`, a `.rodata`, a `.data`, a `.bss` and two globals,
/// so every tool has something of each kind to report.
fn program(ctx: &mut Ctx) -> Result<Linked, Failure> {
    let main = caller(MESSAGE, "other")?;
    let other = callee_returning("other", STATUS as u32)?;
    let data = build(
        ObjectBuilder::new()
            .section(SectionSpec::data(".data", vec![0x5au8; 32]).align(8))
            .section(SectionSpec::bss(".bss", 256).align(16))
            .symbol(SymbolSpec::global("table", ".data", 0).object(32))
            .symbol(SymbolSpec::global("slot", ".bss", 0).object(256)),
    )?;
    ctx.link_ok(
        &Link::new()
            .object("main.o", main)
            .object("other.o", other)
            .object("data.o", data)
            .out("interop")
            .label("a three-object program for the toolchain to read"),
    )
}

/// The output file's path as a string, for a tool's argument list.
fn path_of(linked: &Linked) -> String {
    linked.path.display().to_string()
}

/// The flags of one of our segments in readelf's `Flg` spelling: `R`, `RE`, `RW`, `RWE`.
fn flag_string(seg: &Segment) -> String {
    let mut s = String::new();
    if seg.readable() {
        s.push('R');
    }
    if seg.writable() {
        s.push('W');
    }
    if seg.executable() {
        s.push('E');
    }
    s
}

/// Pull the value that follows `label:` out of a `readelf -h` listing.
fn field(text: &str, label: &str) -> Option<String> {
    text.lines()
        .find(|l| l.trim_start().starts_with(label))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim().to_string())
}

/// Every `LOAD` row of a `readelf -lW` listing, as `(flags, vaddr, filesz, memsz)`.
///
/// `-W` keeps every row on one line, so the columns are positional: type, offset, vaddr,
/// paddr, filesz, memsz, then one or two flag letters, then the alignment.
fn load_rows(text: &str) -> Vec<(String, u64, u64, u64)> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.len() < 8 || t[0] != "LOAD" {
            continue;
        }
        let hex = |s: &str| u64::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or(0);
        let flags: String = t[6..t.len() - 1].concat();
        rows.push((flags, hex(t[2]), hex(t[4]), hex(t[5])));
    }
    rows
}

/// A tool's stdout, or a skip when the tool is not installed.
///
/// Returns `Ok(None)` after marking the test skipped, so a test body reads
/// `let Some(out) = tool_output(..)? else { return Ok(()) };`.
fn tool_output(ctx: &mut Ctx, tool: &str, args: &[&str]) -> Result<Option<String>, Failure> {
    if ctx.tool(tool).is_none() {
        ctx.skip(format!(
            "{tool} is not on PATH; the toolchain-interop leg needs binutils installed, and \
             its absence says nothing about the linker"
        ))?;
        return Ok(None);
    }
    let out = ctx.run_tool(tool, args)?;
    if !out.success() {
        return Err(Failure::harness(format!(
            "{tool} {} exited {}: {}",
            args.join(" "),
            out.status_line(),
            out.stderr.trim()
        )));
    }
    Ok(Some(out.stdout))
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

link_test!(readelf_header, |ctx| {
    if ctx.tool("readelf").is_none() {
        return ctx.skip(
            "readelf is not on PATH; without binutils the fourth leg of the verification \
             cannot run, which is a fact about this machine and not about the linker",
        );
    }
    let linked = program(ctx)?;
    let path = path_of(&linked);
    let Some(text) = tool_output(ctx, "readelf", &["-h", &path])? else {
        return Ok(());
    };

    let mut c = Check::new("what readelf -h says about the output");
    let entry = field(&text, "Entry point address")
        .and_then(|v| u64::from_str_radix(v.trim_start_matches("0x"), 16).ok());
    c.that(
        "readelf -h: Entry point address",
        &format!(
            "0x{:x}, the entry the suite read out of the header",
            linked.elf.entry
        ),
        entry == Some(linked.elf.entry),
        format!("{entry:x?}"),
    );
    c.that(
        "readelf -h: Type",
        "EXEC — a statically linked executable, not ET_DYN and not ET_REL",
        field(&text, "Type").is_some_and(|v| v.starts_with("EXEC")),
        field(&text, "Type").unwrap_or_default(),
    );
    c.that(
        "readelf -h: Machine",
        "an x86-64 machine",
        field(&text, "Machine").is_some_and(|v| v.contains("X86-64")),
        field(&text, "Machine").unwrap_or_default(),
    );
    c.that(
        "readelf -h: Class",
        "ELF64",
        field(&text, "Class").is_some_and(|v| v == "ELF64"),
        field(&text, "Class").unwrap_or_default(),
    );
    c.that(
        "readelf -h: Data",
        "two's complement, little endian",
        field(&text, "Data").is_some_and(|v| v.contains("little endian")),
        field(&text, "Data").unwrap_or_default(),
    );
    if !c.ok() {
        c.block("readelf -h", text);
        c.block("the suite's own parse", linked.elf.describe());
    }
    c.finish()
});

link_test!(readelf_segments, |ctx| {
    if ctx.tool("readelf").is_none() {
        return ctx.skip(
            "readelf is not on PATH, so the program header table cannot be cross-read with \
             binutils on this machine",
        );
    }
    let linked = program(ctx)?;
    let path = path_of(&linked);
    let Some(text) = tool_output(ctx, "readelf", &["-lW", &path])? else {
        return Ok(());
    };

    let rows = load_rows(&text);
    let ours: Vec<(String, u64, u64, u64)> = linked
        .elf
        .loads()
        .map(|s| (flag_string(s), s.vaddr, s.filesz, s.memsz))
        .collect();

    let mut c = Check::new("what readelf -lW says about the loadable segments");
    c.eq("readelf -lW: number of LOAD rows", ours.len(), rows.len());
    for (i, (want, got)) in ours.iter().zip(rows.iter()).enumerate() {
        c.that(
            &format!("readelf -lW: LOAD[{i}]"),
            &format!(
                "flags {}, vaddr 0x{:x}, filesz 0x{:x}, memsz 0x{:x}",
                want.0, want.1, want.2, want.3
            ),
            want == got,
            format!(
                "flags {}, vaddr 0x{:x}, filesz 0x{:x}, memsz 0x{:x}",
                got.0, got.1, got.2, got.3
            ),
        );
    }
    c.that(
        "readelf -lW: PT_INTERP",
        "no interpreter — this is a static link",
        !text.contains("INTERP"),
        "readelf found a PT_INTERP",
    );
    if !c.ok() {
        c.block("readelf -lW", text);
        c.block("the suite's own parse", linked.elf.program_header_table());
    }
    c.finish()
});

link_test!(readelf_sections, |ctx| {
    if ctx.tool("readelf").is_none() {
        return ctx.skip(
            "readelf is not on PATH; the section header table cannot be cross-read with \
             binutils on this machine",
        );
    }
    let linked = program(ctx)?;
    let path = path_of(&linked);
    let Some(text) = tool_output(ctx, "readelf", &["-SW", &path])? else {
        return Ok(());
    };

    let mut c = Check::new("what readelf -SW says about the allocated sections");
    for s in allocated_sections(&linked.elf) {
        let needle = format!("{:x}", s.addr);
        let row = text
            .lines()
            .find(|l| l.contains(&format!(" {} ", s.name)) || l.contains(&format!("] {}", s.name)));
        match row {
            Some(line) => {
                c.that(
                    &format!("readelf -SW: {}", s.name),
                    &format!("a row naming address {needle}"),
                    line.to_lowercase().contains(&needle),
                    line.trim().to_string(),
                );
            }
            None => {
                c.that(
                    &format!("readelf -SW: {}", s.name),
                    "a row for every allocated section the suite found",
                    false,
                    "no such row",
                );
            }
        }
    }
    if !c.ok() {
        c.block("readelf -SW", text);
        c.block("the suite's own parse", linked.elf.section_header_table());
    }
    c.finish()
});

link_test!(readelf_is_quiet, |ctx| {
    if ctx.tool("readelf").is_none() {
        return ctx.skip(
            "readelf is not on PATH, so there is no second opinion on whether the file is \
             well formed",
        );
    }
    let linked = program(ctx)?;
    let path = path_of(&linked);
    let out = ctx.run_tool("readelf", &["-a", &path])?;

    let mut c = Check::new("whether readelf -a complains about the output at all");
    c.eq("readelf -a: exit status", Some(0), out.code);
    c.that(
        "readelf -a: stderr",
        "nothing on stderr",
        out.stderr.trim().is_empty(),
        out.stderr.trim().to_string(),
    );
    let complaints: Vec<&str> = out
        .stdout
        .lines()
        .filter(|l| {
            let l = l.to_lowercase();
            l.contains("warning")
                || l.contains("error")
                || l.contains("unable to")
                || l.contains("corrupt")
        })
        .collect();
    c.that(
        "readelf -a: stdout",
        "no warning, error or 'unable to' line anywhere in the full dump",
        complaints.is_empty(),
        format!("{complaints:?}"),
    );
    if !c.ok() {
        c.block("readelf -a", out.stdout.clone());
        c.block("linker command", linked.run.output.command_line());
    }
    c.finish()?;
    ctx.note(
        "readelf -a walks the header, the segments, the sections, the symbol table and the \
         relocations; a silent run is a strong statement about all five",
    );
    Ok(())
});

link_test!(nm_start, |ctx| {
    if ctx.tool("nm").is_none() {
        return ctx.skip(
            "nm is not on PATH; the symbol table cannot be cross-read with binutils on this \
             machine",
        );
    }
    let linked = program(ctx)?;
    let path = path_of(&linked);
    let want = linked.address_of(DEFAULT_ENTRY)?;
    let Some(text) = tool_output(ctx, "nm", &[&path])? else {
        return Ok(());
    };

    let row = text
        .lines()
        .find(|l| l.split_whitespace().last() == Some(DEFAULT_ENTRY));
    let mut c = Check::new("what nm says about _start");
    match row {
        Some(line) => {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let addr = u64::from_str_radix(fields[0], 16).ok();
            c.that(
                "nm: _start address",
                &format!("0x{want:x}, the address in the output's own symbol table"),
                addr == Some(want),
                line.trim().to_string(),
            );
            c.that(
                "nm: _start type letter",
                "T — a global symbol in the text section",
                fields.get(1) == Some(&"T"),
                format!("{:?}", fields.get(1)),
            );
            c.addr_eq("output.e_entry", want, linked.elf.entry);
        }
        None => {
            c.that(
                "nm: _start",
                "a line for _start",
                false,
                "nm listed no _start",
            );
        }
    }
    if !c.ok() {
        c.block("nm", text);
    }
    c.finish()
});

link_test!(nm_globals, |ctx| {
    if ctx.tool("nm").is_none() {
        return ctx.skip("nm is not on PATH, so the global symbols cannot be cross-checked");
    }
    let linked = program(ctx)?;
    let path = path_of(&linked);
    let Some(text) = tool_output(ctx, "nm", &[&path])? else {
        return Ok(());
    };

    let mut c = Check::new("whether nm sees the globals the three objects defined");
    for name in [DEFAULT_ENTRY, "other", "table", "slot"] {
        let want = linked.address_of(name)?;
        let row = text
            .lines()
            .find(|l| l.split_whitespace().last() == Some(name));
        match row {
            Some(line) => {
                let addr = line
                    .split_whitespace()
                    .next()
                    .and_then(|f| u64::from_str_radix(f, 16).ok());
                c.that(
                    &format!("nm: {name}"),
                    &format!("0x{want:x}"),
                    addr == Some(want),
                    line.trim().to_string(),
                );
            }
            None => {
                c.that(
                    &format!("nm: {name}"),
                    "listed by nm",
                    false,
                    "not in nm's output",
                );
            }
        }
    }
    c.that(
        "nm: undefined symbols",
        "no 'U' line — a static link leaves nothing undefined",
        !text
            .lines()
            .any(|l| l.split_whitespace().next() == Some("U")),
        text.lines()
            .filter(|l| l.split_whitespace().next() == Some("U"))
            .collect::<Vec<&str>>()
            .join(" / "),
    );
    if !c.ok() {
        c.block("nm", text);
    }
    c.finish()
});

link_test!(objdump_disassembles, |ctx| {
    if ctx.tool("objdump").is_none() {
        return ctx.skip(
            "objdump is not on PATH; nothing else on this machine can be asked to decode the \
             .text the linker produced",
        );
    }
    let linked = program(ctx)?;
    let path = path_of(&linked);
    let entry = linked.elf.entry;
    let Some(text) = tool_output(ctx, "objdump", &["-d", &path])? else {
        return Ok(());
    };

    let mut c = Check::new("what objdump -d makes of the entry point");
    let label = format!("{entry:x} <{DEFAULT_ENTRY}>");
    c.that(
        "objdump -d",
        &format!("a disassembly starting at 0x{entry:x} labelled {DEFAULT_ENTRY}"),
        text.contains(&label),
        text.lines()
            .filter(|l| l.contains('<') && l.contains('>'))
            .take(4)
            .collect::<Vec<&str>>()
            .join(" / "),
    );
    c.that(
        "objdump -d: .text",
        "a disassembly of section .text",
        text.contains("Disassembly of section .text"),
        text.lines()
            .filter(|l| l.starts_with("Disassembly"))
            .collect::<Vec<&str>>()
            .join(" / "),
    );
    // The first instruction of caller() is `mov eax, 1`, and somewhere after it is the call
    // to `other` — which objdump resolves back to the symbol only if the displacement is right.
    let other = linked.address_of("other")?;
    c.that(
        "objdump -d: the call to other",
        &format!("a call whose target objdump resolves to 0x{other:x} <other>"),
        text.contains("call") && text.contains(&format!("{other:x} <other>")),
        text.lines()
            .filter(|l| l.contains("call"))
            .collect::<Vec<&str>>()
            .join(" / "),
    );
    c.that(
        "objdump -d: bad opcodes",
        "no (bad) in the disassembly of .text",
        !text.contains("(bad)"),
        text.lines()
            .filter(|l| l.contains("(bad)"))
            .take(4)
            .collect::<Vec<&str>>()
            .join(" / "),
    );
    if !c.ok() {
        c.block("objdump -d", text);
    }
    c.finish()
});

link_test!(objdump_file_header, |ctx| {
    if ctx.tool("objdump").is_none() {
        return ctx.skip(
            "objdump is not on PATH, so BFD's opinion of the file format cannot be asked for",
        );
    }
    let linked = program(ctx)?;
    let path = path_of(&linked);
    let Some(text) = tool_output(ctx, "objdump", &["-f", &path])? else {
        return Ok(());
    };

    let mut c = Check::new("what objdump -f says the file is");
    c.that(
        "objdump -f: file format",
        "elf64-x86-64",
        text.contains("elf64-x86-64"),
        text.lines().take(3).collect::<Vec<&str>>().join(" / "),
    );
    c.that(
        "objdump -f: EXEC_P",
        "EXEC_P — BFD agrees this is an executable, not a relocatable or shared object",
        text.contains("EXEC_P"),
        text.lines()
            .filter(|l| l.contains("flags"))
            .collect::<Vec<&str>>()
            .join(" / "),
    );
    let start = text
        .lines()
        .find(|l| l.trim_start().starts_with("start address"))
        .and_then(|l| l.split_whitespace().last())
        .and_then(|v| u64::from_str_radix(v.trim_start_matches("0x"), 16).ok());
    c.that(
        "objdump -f: start address",
        &format!("0x{:x}", linked.elf.entry),
        start == Some(linked.elf.entry),
        format!("{start:x?}"),
    );
    c.that(
        "objdump -f: HAS_SYMS",
        "HAS_SYMS — a symbol table the debugger and the profiler can read",
        text.contains("HAS_SYMS"),
        text.lines()
            .filter(|l| l.contains("flags"))
            .collect::<Vec<&str>>()
            .join(" / "),
    );
    if !c.ok() {
        c.block("objdump -f", text);
    }
    c.finish()?;
    ctx.expect_output(&linked, MESSAGE, STATUS)?;
    Ok(())
});

/// Worked examples: what the three tools are asked, and what the answers have to match.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "The program the toolchain is pointed at",
            "ld -o interop main.o other.o data.o && readelf -a interop",
            || caller(MESSAGE, "other").map_err(|f| f.messages.join("; ")),
        )
        .request(
            "main.o, the first of three: a .rodata string, a .text that writes it and calls \
             the undefined 'other', joined by other.o (which defines it) and data.o (a .data \
             and a .bss with two globals and no code at all)",
        )
        .response(
            "A file readelf -h calls EXEC on X86-64 with the same entry point the suite \
             computed, readelf -lW shows with the same PT_LOADs and the same R/RW/RE flags, \
             nm lists _start, other, table and slot at the suite's addresses in, and \
             objdump -d disassembles from the entry point with no (bad) opcodes",
        )
        .note(
            "This is the leg that catches a linker whose output runs but whose section header \
             table is a fiction: the kernel never reads section headers, and every debugger, \
             profiler and disassembler reads nothing else.",
        )
        .runs(MESSAGE, STATUS),
        ExampleSpec::text(
            "readelf -a, and what a clean run means",
            "readelf -a interop 2>&1 | grep -i -e warning -e error",
        )
        .request(
            "The finished executable, handed to readelf's everything-mode: the ELF header, \
             the section headers, the program headers, the section-to-segment mapping, the \
             symbol table and any relocations left in the file",
        )
        .response(
            "Exit status 0, nothing at all on stderr, and not one line of the dump containing \
             'warning', 'error', 'corrupt' or 'unable to'",
        )
        .note(
            "sh_link on .symtab, sh_info on .symtab, sh_entsize on anything with fixed-size \
             entries: get one of those wrong and the program still runs, readelf still prints \
             a table, and one small warning line is the only thing that ever tells you.",
        ),
    ]
}

/// Re-exported so the module doc can point at the parse the three tools are checked against.
#[allow(dead_code)]
type ParsedOutput = Elf;
