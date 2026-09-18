//! Stage 37 — A six-object freestanding program.
//!
//! The first stage that is a *program* rather than a probe. Six relocatable objects, each
//! one a translation unit with its own `.text`, its own `.rodata` and (for two of them) its
//! own `.data` and `.bss`, are linked into one static executable. Every earlier stage shows
//! up at once: section concatenation, a global symbol table, `PLT32` calls across objects,
//! `PC32` references to local strings, addends, a zeroed `.bss`. If this stage is green the
//! linker can build something a person would actually run.

use crate::asm::{Code, Reg, STDOUT};
use crate::assert::{Check, Failure};
use crate::elf::read::Elf;
use crate::elf::write::{ObjectBuilder, SectionSpec, SymbolSpec};
use crate::elf::*;
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 37,
        slug: "multi_object_program",
        name: "A six-object freestanding program",
        ext: false,
        hints: &[
            "Read every input before resolving anything: object six defines what object one \
             referenced, and object one is read first",
            "Concatenate the same-named sections in command-line order, then give each symbol \
             its final address as output section base + input contribution offset + st_value",
            "Apply relocations only once every address is fixed — a PLT32 call between two \
             objects is just S + A - P as soon as both .text contributions have landed",
            "The binary inherits argv and stdin from whoever runs it; a correct link touches \
             neither, so the same program must behave identically however it is invoked",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new("a six-object program links", six_objects_link),
            Test::new("the program prints all six of its lines", prints_six_lines),
            Test::new(
                "the exit status is the sum every object contributed",
                exit_status_is_the_sum,
            ),
            Test::new("every layout invariant holds for six objects", layout),
            Test::new(
                "every symbol the six objects referenced is resolved",
                every_symbol_resolved,
            ),
            Test::new(
                "running the program twice gives the same result",
                runs_twice,
            ),
            Test::new(
                "arguments and unread stdin change nothing",
                argv_and_stdin_are_untouched,
            ),
            Test::new(
                "the six objects may be given in any order",
                order_does_not_change_behaviour,
            ),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// The program
// ---------------------------------------------------------------------------------------

/// What `main.o` prints before it calls anything.
const BANNER: &str = "linktest: six objects\n";

/// Everything the six-object program writes to stdout, in order.
const EXPECTED_STDOUT: &str = "linktest: six objects\nalpha\nbeta\ngamma\ndelta\nepsilon\n";

/// `7 + 11 + 13 + 17 + 23` — one term per translation unit, and the program's exit status.
const EXPECTED_STATUS: i32 = 71;

/// The five functions `main.o` calls, in call order.
const PARTS: [&str; 5] = ["part_a", "part_b", "part_c", "part_d", "part_e"];

/// `main.o`: print the banner, call the five contributors, exit with the running total.
///
/// `ebx` holds the accumulator across the calls. None of the callees touches it — they use
/// only the syscall registers and `eax` — so the sum survives five `call`s without a frame.
fn main_object() -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "banner", BANNER.len() as u32);
    code.xor_r32_r32(Reg::Rbx, Reg::Rbx);
    for part in PARTS {
        code.call(part);
        code.add_r32_r32(Reg::Rbx, Reg::Rax);
    }
    code.mov_r32_r32(Reg::Rax, Reg::Rbx);
    code.sys_exit_eax();

    let mut b = ObjectBuilder::new()
        .section(text_of(&code))
        .section(SectionSpec::rodata(".rodata", BANNER.as_bytes().to_vec()))
        .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
        .symbol(SymbolSpec::local("banner", ".rodata", 0).object(BANNER.len() as u64));
    for part in PARTS {
        b = b.symbol(SymbolSpec::undefined(part));
    }
    build(b)
}

/// One plain contributor: print `message` from its own `.rodata`, return `value` in `eax`.
fn part_object(name: &str, local: &str, message: &str, value: u32) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, local, message.len() as u32);
    code.mov_r32_imm32(Reg::Rax, value);
    code.ret();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(name, ".text", 0).func())
            .symbol(SymbolSpec::local(local, ".rodata", 0).object(message.len() as u64)),
    )
}

/// `delta.o`: the contributor whose term lives in `.data` and is loaded at run time.
///
/// `mov eax, [rip+delta_value]` is a four-byte load through a `PC32` relocation, so the term
/// only comes out right if `.data` was copied into the image *and* the displacement was
/// patched with the address `.data` actually got.
fn delta_object(value: u32) -> Result<Vec<u8>, Failure> {
    let message = "delta\n";
    let mut code = Code::new();
    code.sys_write(STDOUT, "delta_msg", message.len() as u32);
    code.mov_r32_rip(Reg::Rax, "delta_value", 0);
    code.ret();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .section(SectionSpec::data(".data", value.to_le_bytes().to_vec()).align(4))
            .symbol(SymbolSpec::global("part_d", ".text", 0).func())
            .symbol(SymbolSpec::local("delta_msg", ".rodata", 0).object(message.len() as u64))
            .symbol(SymbolSpec::local("delta_value", ".data", 0).object(4)),
    )
}

/// `epsilon.o`: the contributor that reads, writes and re-reads its own `.bss`.
///
/// It first reads byte zero of `.bss` — a correct linker leaves it zero — adds the term,
/// stores the result eight bytes further in through an addend, and loads it straight back.
/// A `.bss` that is not mapped writable makes this object die with SIGSEGV; a `.bss` that is
/// not zeroed makes the program exit with the wrong status.
fn epsilon_object(value: u32) -> Result<Vec<u8>, Failure> {
    let message = "epsilon\n";
    let mut code = Code::new();
    code.sys_write(STDOUT, "epsilon_msg", message.len() as u32);
    code.movzx_r32_byte_rip(Reg::Rax, "scratch", 0);
    code.add_r32_imm32(Reg::Rax, value);
    code.mov_rip_r32("scratch", Reg::Rax, 8);
    code.mov_r32_rip(Reg::Rax, "scratch", 8);
    code.ret();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .section(SectionSpec::bss(".bss", 4096).align(16))
            .symbol(SymbolSpec::global("part_e", ".text", 0).func())
            .symbol(SymbolSpec::local("epsilon_msg", ".rodata", 0).object(message.len() as u64))
            .symbol(SymbolSpec::global("scratch", ".bss", 0).object(4096)),
    )
}

/// The six objects, in the order the command line lists them.
fn six_objects() -> Result<Vec<(&'static str, Vec<u8>)>, Failure> {
    Ok(vec![
        ("main.o", main_object()?),
        ("alpha.o", part_object("part_a", "alpha_msg", "alpha\n", 7)?),
        ("beta.o", part_object("part_b", "beta_msg", "beta\n", 11)?),
        (
            "gamma.o",
            part_object("part_c", "gamma_msg", "gamma\n", 13)?,
        ),
        ("delta.o", delta_object(17)?),
        ("epsilon.o", epsilon_object(23)?),
    ])
}

/// A link of the given objects, in the given order.
fn link_of(objects: &[(&'static str, Vec<u8>)], label: &str) -> Link {
    let mut link = Link::new().out("six").label(label);
    for (name, bytes) in objects {
        link = link.object(name, bytes.clone());
    }
    link
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

link_test!(six_objects_link, |ctx| {
    let objects = six_objects()?;
    let linked = ctx.link_ok(&link_of(&objects, "six objects in command-line order"))?;
    let mut c = Check::new("that six objects became one executable");
    c.that(
        "output.size",
        "a non-empty file",
        !linked.bytes.is_empty(),
        linked.bytes.len(),
    );
    c.eq("output.e_type", ET_EXEC, linked.elf.e_type);
    c.finish()
});

link_test!(prints_six_lines, |ctx| {
    let objects = six_objects()?;
    let linked = ctx.link_ok(&link_of(&objects, "six objects"))?;
    ctx.expect_output(&linked, EXPECTED_STDOUT, EXPECTED_STATUS)?;
    Ok(())
});

link_test!(exit_status_is_the_sum, |ctx| {
    let objects = six_objects()?;
    let linked = ctx.link_ok(&link_of(&objects, "six objects"))?;
    let out = ctx.run(&linked)?;
    let mut c = Check::new("the status the five terms add up to");
    c.note("7 (alpha) + 11 (beta) + 13 (gamma) + 17 (delta, from .data) + 23 (epsilon, via .bss)");
    c.eq("program.exit_status", Some(EXPECTED_STATUS), out.code);
    if !c.ok() {
        c.block("program stdout", out.stdout.clone());
        c.block("linker command", linked.run.output.command_line());
        c.block("output program headers", linked.elf.program_header_table());
    }
    c.finish()
});

link_test!(layout, |ctx| {
    let objects = six_objects()?;
    let linked = ctx.link_ok(&link_of(&objects, "six objects"))?;
    assert_runnable_layout(&linked)?;
    assert_entry_is(&linked, DEFAULT_ENTRY)?;

    let mut c = Check::new("where the six objects' contributions landed");
    let text = linked
        .elf
        .section(".text")
        .ok_or_else(|| linked.attach(Failure::harness("the output has no section named .text")))?;
    c.that(
        "output..text.sh_flags",
        "SHF_ALLOC | SHF_EXECINSTR",
        text.is_alloc() && text.is_exec(),
        flags_string(text.flags as u32),
    );
    for part in PARTS {
        let addr = linked.address_of(part)?;
        c.that(
            &format!("output.{part}"),
            "an address inside the concatenated .text",
            addr >= text.addr && addr < text.addr + text.size,
            format!(
                "0x{addr:x}, while .text is 0x{:x}..0x{:x}",
                text.addr,
                text.addr + text.size
            ),
        );
    }
    // .bss costs no file space wherever the linker decided to put it.
    if let Some(bss) = linked.elf.section(".bss") {
        c.eq("output..bss.sh_type", SHT_NOBITS, bss.sh_type);
        c.that(
            "output..bss",
            "mapped by a writable PT_LOAD",
            linked
                .elf
                .segment_at(bss.addr)
                .map(|s| s.writable())
                .unwrap_or(false),
            segment_summary(&linked.elf, bss.addr),
        );
    }
    if !c.ok() {
        c.block("output section headers", linked.elf.section_header_table());
    }
    c.finish()
});

link_test!(every_symbol_resolved, |ctx| {
    let objects = six_objects()?;
    let linked = ctx.link_ok(&link_of(&objects, "six objects"))?;
    let mut c = Check::new("that nothing was left dangling");

    for name in PARTS.iter().chain([DEFAULT_ENTRY, "scratch"].iter()) {
        match linked.elf.symbol(name) {
            Some(sym) => {
                c.that(
                    &format!("output.symtab[{name}].st_shndx"),
                    "a defined symbol, not SHN_UNDEF",
                    !sym.is_undefined(),
                    sym.shndx,
                );
            }
            None => {
                c.that(
                    &format!("output.symtab[{name}]"),
                    "present in the output symbol table",
                    false,
                    "missing",
                );
            }
        }
    }
    for sym in &linked.elf.symbols {
        if sym.name.is_empty() || sym.stype == STT_SECTION || sym.stype == STT_FILE {
            continue;
        }
        c.that(
            &format!("output.symtab[{}].st_shndx", sym.name),
            "no undefined symbol left in a finished static link",
            !sym.is_undefined(),
            format!("'{}' is SHN_UNDEF", sym.name),
        );
    }
    c.that(
        "output.relocations",
        "no relocation sections left: a static link applies them all",
        linked.elf.relocations.is_empty(),
        linked.elf.relocations.len(),
    );
    if !c.ok() {
        c.block("output section headers", linked.elf.section_header_table());
    }
    c.finish()
});

link_test!(runs_twice, |ctx| {
    let objects = six_objects()?;
    let linked = ctx.link_ok(&link_of(&objects, "six objects"))?;
    let first = ctx.expect_output(&linked, EXPECTED_STDOUT, EXPECTED_STATUS)?;
    let second = ctx.expect_output(&linked, EXPECTED_STDOUT, EXPECTED_STATUS)?;
    let mut c = Check::new("that the program is not stateful between runs");
    c.bytes_eq("program.stdout", &first.stdout_bytes, &second.stdout_bytes);
    c.eq("program.exit_status", first.code, second.code);
    c.note("a .bss the linker forgot to zero would make the second run differ from the first");
    c.finish()
});

link_test!(argv_and_stdin_are_untouched, |ctx| {
    let objects = six_objects()?;
    let linked = ctx.link_ok(&link_of(&objects, "six objects"))?;
    let plain = ctx.run(&linked)?;
    let with_args = ctx.run_with(&linked, &["arg1", "arg2"], b"some stdin\n")?;
    let mut c = Check::new("that argv and stdin do not reach a program that never reads them");
    c.bytes_eq(
        "program.stdout",
        &plain.stdout_bytes,
        &with_args.stdout_bytes,
    );
    c.eq("program.exit_status", plain.code, with_args.code);
    c.eq("program.exit_status", Some(EXPECTED_STATUS), with_args.code);
    c.note(
        "the kernel puts argc, argv and envp on the stack at _start; a program that never \
         touches them must not care, and unread stdin must not turn into a broken pipe",
    );
    if !c.ok() {
        c.block("program stderr (with arguments)", with_args.stderr.clone());
    }
    c.finish()
});

link_test!(order_does_not_change_behaviour, |ctx| {
    let objects = six_objects()?;
    let forward = ctx.link_ok(&link_of(&objects, "main.o first"))?;
    let forward_out = ctx.expect_output(&forward, EXPECTED_STDOUT, EXPECTED_STATUS)?;

    let mut reversed = objects.clone();
    reversed.reverse();
    let mut link = link_of(&reversed, "main.o last");
    link = link.out("six_reversed");
    let backward = ctx.link_ok(&link)?;
    let backward_out = ctx.expect_output(&backward, EXPECTED_STDOUT, EXPECTED_STATUS)?;

    let mut c = Check::new("that the order of six objects does not change what the program does");
    c.bytes_eq(
        "program.stdout",
        &forward_out.stdout_bytes,
        &backward_out.stdout_bytes,
    );
    c.eq("program.exit_status", forward_out.code, backward_out.code);
    c.note(
        "the addresses are free to move — only an archive's position on the command line is \
         semantics, and there is no archive here",
    );
    if forward.elf.entry != backward.elf.entry {
        c.note(format!(
            "the two links put _start at 0x{:x} and 0x{:x}; that is allowed, only the \
             behaviour has to match",
            forward.elf.entry, backward.elf.entry
        ));
    }
    c.finish()
});

/// Worked examples: the caller half and the `.bss` half of the six-object program.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "main.o: five undefined references and a running total",
            "ld -o six main.o alpha.o beta.o gamma.o delta.o epsilon.o",
            || main_object().map_err(|f| f.messages.join("; ")),
        )
        .request(
            "main.o: a .rodata holding the banner, a .text that writes it, zeroes ebx, calls \
             part_a..part_e accumulating eax into ebx, and exits with the total; five \
             SHN_UNDEF symbols and six relocations — one PC32 for the banner, five PLT32 for \
             the calls",
        )
        .response(
            "An ET_EXEC file in which each of the five call displacements holds S + A - P for \
             the address that object's .text contribution ended up at, and e_entry is the \
             address of _start",
        )
        .note(
            "The five callees are read after main.o, so a linker that resolves a symbol the \
             moment it sees the reference fails here. Resolution happens once, after every \
             input has been read.",
        )
        .runs(EXPECTED_STDOUT, EXPECTED_STATUS),
        ExampleSpec::object(
            "epsilon.o: read .bss, write it, read it back",
            "ld -o six main.o alpha.o beta.o gamma.o delta.o epsilon.o",
            || epsilon_object(23).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "epsilon.o: a .rodata string, a 4096-byte SHT_NOBITS .bss with a global 'scratch', \
             and a .text that reads scratch[0], adds 23, stores the result at scratch+8 and \
             loads it back — three PC32 relocations against the same symbol with addends 0, 8 \
             and 8",
        )
        .response(
            "A writable PT_LOAD whose p_memsz covers the 4096 bytes and whose p_filesz does \
             not, with those bytes zero when the kernel maps them; the three displacements \
             differ by the addends",
        )
        .note(
            "This is the object that catches a linker that gave .bss file bytes, or that put \
             it in a read-only segment: the first fails the exit status, the second kills the \
             program with SIGSEGV.",
        ),
    ]
}

/// Re-exported so the module doc can point at the parser the tests read the output with.
#[allow(dead_code)]
type ParsedOutput = Elf;
