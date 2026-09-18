//! Stage 38 — Two hundred objects.
//!
//! Nothing here is new in kind; everything is new in quantity. Two hundred generated
//! translation units, each with its own `.text`, its own `.rodata` string and one global
//! function, plus a `main.o` that calls all two hundred of them and adds up what they
//! return. A linker whose symbol table is a linear scan, or that re-reads an input once per
//! reference, still passes stage 37 and falls over here: the work is quadratic and the
//! deadline is not.
//!
//! The stage is deliberately not a benchmark. It asserts correctness at scale and *reports*
//! the wall-clock time with [`crate::stages::Ctx::note`], so a slow linker is visible in the
//! report without a number nobody can reproduce becoming a pass/fail condition.

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
use rand::seq::SliceRandom;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 38,
        slug: "many_objects",
        name: "Two hundred objects",
        ext: true,
        hints: &[
            "Put every global in one hash map keyed by name as the inputs are read, so \
             resolution is one lookup per reference rather than a scan over every object",
            "Concatenate each output section once, in input order, and remember every input \
             contribution's base address — recomputing it per relocation is the quadratic \
             trap",
            "Two hundred inputs means two hundred open files; read each one once into memory \
             and close it, or the link dies on the file-descriptor limit long before it dies \
             on time",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new("two hundred objects link into one program", links_at_scale)
                .ext()
                .tag("slow")
                .min_timeout_ms(60_000),
            Test::new(
                "the program prints every line and exits with the sum",
                prints_every_line,
            )
            .ext()
            .tag("slow")
            .min_timeout_ms(60_000),
            Test::new(
                "all two hundred contributions have their own address",
                every_contribution_present,
            )
            .ext()
            .tag("slow")
            .min_timeout_ms(60_000),
            Test::new(
                "the two hundred .text sections come out as one",
                one_concatenated_text,
            )
            .ext()
            .tag("slow")
            .min_timeout_ms(60_000),
            Test::new(
                "two hundred objects in reverse order still link",
                reverse_order,
            )
            .ext()
            .tag("slow")
            .min_timeout_ms(60_000),
            Test::new(
                "a seeded shuffle of the command line changes nothing",
                shuffled_order,
            )
            .ext()
            .tag("slow")
            .min_timeout_ms(60_000),
            Test::new(
                "a two-hundred-object link finishes inside the deadline",
                finishes_in_time,
            )
            .ext()
            .tag("slow")
            .min_timeout_ms(60_000),
            Test::new("every layout invariant survives two hundred inputs", layout)
                .ext()
                .tag("slow")
                .min_timeout_ms(60_000),
            Test::new(
                "no symbol is left undefined after two hundred cross-references",
                nothing_undefined,
            )
            .ext()
            .tag("slow")
            .min_timeout_ms(60_000),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// The generated program
// ---------------------------------------------------------------------------------------

/// How many contributing objects the stage generates.
const N: usize = 200;

/// Every contributor returns 1, so the total — and the exit status — is [`N`].
const EXPECTED_STATUS: i32 = N as i32;

/// The line contributor `i` writes: its own index, zero padded, and a newline.
fn line(i: usize) -> String {
    format!("{i:03}\n")
}

/// Everything the two-hundred-object program writes, in call order.
fn expected_stdout() -> String {
    (0..N).map(line).collect()
}

/// Contributor `i`: write its own line from its own `.rodata`, return 1 in `eax`.
fn term_object(i: usize) -> Result<Vec<u8>, Failure> {
    let text = line(i);
    let msg = format!("msg_{i:03}");
    let mut code = Code::new();
    code.sys_write(STDOUT, &msg, text.len() as u32);
    code.mov_r32_imm32(Reg::Rax, 1);
    code.ret();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", text.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(&term_name(i), ".text", 0).func())
            .symbol(SymbolSpec::local(&msg, ".rodata", 0).object(text.len() as u64)),
    )
}

/// The global every contributor defines and `main.o` references.
fn term_name(i: usize) -> String {
    format!("term_{i:03}")
}

/// `main.o`: call all [`N`] contributors in order, accumulate `eax` into `ebx`, exit with it.
fn main_object() -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.xor_r32_r32(Reg::Rbx, Reg::Rbx);
    for i in 0..N {
        code.call(&term_name(i));
        code.add_r32_r32(Reg::Rbx, Reg::Rax);
    }
    code.mov_r32_r32(Reg::Rax, Reg::Rbx);
    code.sys_exit_eax();
    let mut b = ObjectBuilder::new()
        .section(text_of(&code))
        .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func());
    for i in 0..N {
        b = b.symbol(SymbolSpec::undefined(&term_name(i)));
    }
    build(b)
}

/// Every input of the two-hundred-object program, `main.o` first.
fn many_objects() -> Result<Vec<(String, Vec<u8>)>, Failure> {
    let mut v = Vec::with_capacity(N + 1);
    v.push(("main.o".to_string(), main_object()?));
    for i in 0..N {
        v.push((format!("t{i:03}.o"), term_object(i)?));
    }
    Ok(v)
}

/// A link of the given inputs, in the given order.
fn link_of(objects: &[(String, Vec<u8>)], out: &str, label: &str) -> Link {
    let mut link = Link::new().out(out).label(label);
    for (name, bytes) in objects {
        link = link.object(name, bytes.clone());
    }
    link
}

/// The total size of the `.text` sections the inputs carry, summed by re-reading them with
/// the suite's own parser — the floor under the concatenated output `.text`.
fn total_input_text(objects: &[(String, Vec<u8>)]) -> Result<u64, Failure> {
    let mut total = 0;
    for (name, bytes) in objects {
        let elf = Elf::parse(bytes).map_err(|e| {
            Failure::harness(format!(
                "the suite built a {name} it cannot parse back: {e}"
            ))
        })?;
        total += elf.section(".text").map(|s| s.size).unwrap_or(0);
    }
    Ok(total)
}

/// One line for the report: how long the link took, and how much it had to chew through.
fn timing_note(objects: &[(String, Vec<u8>)], linked: &crate::link::Linked) -> String {
    let input_bytes: usize = objects.iter().map(|(_, b)| b.len()).sum();
    format!(
        "{} inputs, {} KiB of relocatable object, linked in {} ms into a {} KiB executable",
        objects.len(),
        input_bytes / 1024,
        linked.run.output.duration.as_millis(),
        linked.bytes.len() / 1024
    )
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

link_test!(links_at_scale, |ctx| {
    let objects = many_objects()?;
    let linked = ctx.link_ok(&link_of(&objects, "many", "two hundred objects"))?;
    let note = timing_note(&objects, &linked);
    ctx.note(note);
    let mut c = Check::new("that two hundred objects became one executable");
    c.eq("output.e_type", ET_EXEC, linked.elf.e_type);
    c.that(
        "output.size",
        "a non-empty file",
        !linked.bytes.is_empty(),
        linked.bytes.len(),
    );
    c.finish()
});

link_test!(prints_every_line, |ctx| {
    let objects = many_objects()?;
    let linked = ctx.link_ok(&link_of(&objects, "many", "two hundred objects"))?;
    ctx.expect_output(&linked, &expected_stdout(), EXPECTED_STATUS)?;
    ctx.note(format!(
        "{N} contributors, each returning 1, so the exit status is {EXPECTED_STATUS}"
    ));
    Ok(())
});

link_test!(every_contribution_present, |ctx| {
    let objects = many_objects()?;
    let linked = ctx.link_ok(&link_of(&objects, "many", "two hundred objects"))?;
    let mut c = Check::new("that every one of the two hundred contributions is in the output");

    let mut addresses = Vec::with_capacity(N);
    let mut missing = Vec::new();
    for i in 0..N {
        let name = term_name(i);
        match linked.elf.symbol(&name) {
            Some(sym) if !sym.is_undefined() => addresses.push(sym.value),
            _ => missing.push(name),
        }
    }
    c.that(
        "output.symtab",
        "all two hundred term_NNN symbols defined",
        missing.is_empty(),
        format!("{} missing, first few: {:?}", missing.len(), {
            let mut head = missing.clone();
            head.truncate(5);
            head
        }),
    );
    let mut sorted = addresses.clone();
    sorted.sort_unstable();
    sorted.dedup();
    c.eq(
        "output.symtab.distinct_addresses",
        addresses.len(),
        sorted.len(),
    );

    let out = ctx.run(&linked)?;
    let expected = expected_stdout();
    c.bytes_eq("program.stdout", expected.as_bytes(), &out.stdout_bytes);
    c.finish()
});

link_test!(one_concatenated_text, |ctx| {
    let objects = many_objects()?;
    let want = total_input_text(&objects)?;
    let linked = ctx.link_ok(&link_of(&objects, "many", "two hundred objects"))?;
    let mut c = Check::new("the one output .text the two hundred input .texts became");

    let count = linked.elf.sections_named(".text").count();
    c.eq("output.sections_named(.text)", 1usize, count);
    match linked.elf.section(".text") {
        Some(text) => {
            c.at_least("output..text.sh_size", want, text.size);
            c.that(
                "output..text.sh_flags",
                "SHF_ALLOC | SHF_EXECINSTR",
                text.is_alloc() && text.is_exec(),
                flags_string(text.flags as u32),
            );
            let mut outside = 0;
            for i in 0..N {
                if let Some(sym) = linked.elf.symbol(&term_name(i)) {
                    if sym.value < text.addr || sym.value >= text.addr + text.size {
                        outside += 1;
                    }
                }
            }
            c.eq("output.term_NNN outside .text", 0, outside);
            c.note(format!(
                "the {} input .text sections total {want} bytes; the output .text is {} bytes",
                objects.len(),
                text.size
            ));
        }
        None => {
            c.that(
                "output..text",
                "a section named .text in the output",
                false,
                "no such section",
            );
        }
    }
    if !c.ok() {
        c.block("output section headers", linked.elf.section_header_table());
    }
    c.finish()
});

link_test!(reverse_order, |ctx| {
    let objects = many_objects()?;
    let forward = ctx.link_ok(&link_of(&objects, "many", "main.o first"))?;
    let forward_out = ctx.expect_output(&forward, &expected_stdout(), EXPECTED_STATUS)?;

    let mut reversed = objects.clone();
    reversed.reverse();
    let backward = ctx.link_ok(&link_of(&reversed, "many_rev", "main.o last"))?;
    let backward_out = ctx.expect_output(&backward, &expected_stdout(), EXPECTED_STATUS)?;

    let mut c = Check::new("that reversing two hundred inputs changes nothing observable");
    c.bytes_eq(
        "program.stdout",
        &forward_out.stdout_bytes,
        &backward_out.stdout_bytes,
    );
    c.eq("program.exit_status", forward_out.code, backward_out.code);
    c.note(
        "the addresses are free to move: with no archive on the command line, input order \
         decides layout and nothing else",
    );
    c.finish()
});

link_test!(shuffled_order, |ctx| {
    let objects = many_objects()?;
    let mut shuffled = objects.clone();
    shuffled.shuffle(&mut ctx.rng);
    let position = shuffled
        .iter()
        .position(|(name, _)| name == "main.o")
        .unwrap_or(0);
    ctx.note(format!(
        "seed 0x{:x} put main.o at position {position} of {}",
        ctx.seed,
        shuffled.len()
    ));
    let linked = ctx.link_ok(&link_of(&shuffled, "many_shuffled", "a seeded shuffle"))?;
    ctx.expect_output(&linked, &expected_stdout(), EXPECTED_STATUS)?;
    assert_runnable_layout(&linked)?;
    assert_entry_is(&linked, DEFAULT_ENTRY)
});

link_test!(finishes_in_time, |ctx| {
    let objects = many_objects()?;
    let linked = ctx.link_ok(&link_of(&objects, "many", "two hundred objects, timed"))?;
    let elapsed = linked.run.output.duration;
    ctx.note(format!(
        "the {}-input link took {} ms",
        objects.len(),
        elapsed.as_millis()
    ));
    let mut c = Check::new("that a two-hundred-object link is not quadratic");
    c.that(
        "linker.duration_ms",
        "a link of two hundred small objects finishing in under 30 seconds",
        elapsed.as_millis() < 30_000,
        format!("{} ms", elapsed.as_millis()),
    );
    c.note(
        "this is not a benchmark: the number is reported, and only an order-of-magnitude \
         blow-up — a per-reference scan over every input — can fail it",
    );
    c.finish()
});

link_test!(layout, |ctx| {
    let objects = many_objects()?;
    let linked = ctx.link_ok(&link_of(&objects, "many", "two hundred objects"))?;
    assert_runnable_layout(&linked)?;
    assert_entry_is(&linked, DEFAULT_ENTRY)?;

    let mut c = Check::new("the allocated sections of a two-hundred-object output");
    let allocated = allocated_sections(&linked.elf);
    c.that(
        "output.allocated_sections",
        "at least a .text and a .rodata",
        allocated.len() >= 2,
        allocated.len(),
    );
    let mut previous_end = 0u64;
    for s in &allocated {
        c.that(
            &format!("output.section[{}].sh_addr", s.name),
            "an address at or after the end of the previous allocated section",
            s.addr >= previous_end,
            format!("0x{:x} after an end of 0x{previous_end:x}", s.addr),
        );
        if !s.is_nobits() {
            c.that(
                &format!("output.section[{}].sh_offset", s.name),
                "congruent to sh_addr modulo the page size",
                s.offset % PAGE_SIZE == s.addr % PAGE_SIZE,
                format!("offset 0x{:x}, addr 0x{:x}", s.offset, s.addr),
            );
        }
        previous_end = s.addr + s.size;
    }
    if !c.ok() {
        c.block("output section headers", linked.elf.section_header_table());
    }
    c.finish()
});

link_test!(nothing_undefined, |ctx| {
    let objects = many_objects()?;
    let linked = ctx.link_ok(&link_of(&objects, "many", "two hundred objects"))?;
    let mut c = Check::new("that two hundred cross-references all resolved");
    let dangling: Vec<&str> = linked
        .elf
        .symbols
        .iter()
        .filter(|s| {
            !s.name.is_empty() && s.stype != STT_SECTION && s.stype != STT_FILE && s.is_undefined()
        })
        .map(|s| s.name.as_str())
        .collect();
    c.that(
        "output.symtab",
        "no undefined symbol left in a finished static link",
        dangling.is_empty(),
        format!("{} left: {:?}", dangling.len(), {
            let mut head = dangling.clone();
            head.truncate(5);
            head
        }),
    );
    c.that(
        "output.relocations",
        "every relocation applied and none written out",
        linked.elf.relocations.is_empty(),
        linked.elf.relocations.len(),
    );
    c.finish()
});

/// Worked examples: one generated contributor, and the caller that names all two hundred.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "One of two hundred generated contributors",
            "ld -o many main.o t000.o t001.o ... t199.o",
            || term_object(7).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "t007.o: a four-byte .rodata holding \"007\\n\", a .text that writes it and returns \
             1 in eax, one global term_007 and one PC32 relocation — the same shape two \
             hundred times over, with only the index changing",
        )
        .response(
            "term_007's address in the output is inside the single concatenated .text, and \
             the lea's displacement is S + A - P for wherever this object's four bytes of \
             .rodata ended up",
        )
        .note(
            "Nothing about one of these is hard. What is hard is that a linker which walks \
             every input once per reference does 200 x 200 work here and blows the deadline.",
        ),
        ExampleSpec::text(
            "main.o: two hundred undefined symbols, two hundred PLT32 calls",
            "ld -o many main.o t000.o t001.o ... t199.o",
        )
        .request(
            "main.o: a .text of 200 call/add pairs, a symbol table with 200 SHN_UNDEF entries \
             named term_000..term_199, and a .rela.text with 200 R_X86_64_PLT32 entries",
        )
        .response(
            "One executable whose e_entry is _start, whose .text holds all 201 contributions \
             concatenated, and which prints 200 lines and exits 200",
        )
        .note(
            "Resolution has to be a lookup, not a search: build the global symbol table while \
             reading, then walk the relocations once.",
        ),
    ]
}
