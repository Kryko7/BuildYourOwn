//! Stage 43 — Init and fini arrays: the code that runs before and after `main`.
//!
//! A C program with a `__attribute__((constructor))` function, or a C++ program with a
//! static object that has a constructor, does not call that code from `main` — it cannot,
//! because the code lives in a different translation unit and nothing references it. What
//! happens instead is that the compiler emits a pointer to it in a section called
//! `.init_array`, the linker concatenates every input's `.init_array` into one, and the
//! startup code walks the result calling each entry before it calls `main`.
//!
//! Two jobs for the linker, and both are easy to get slightly wrong. The first is
//! **ordering**: the entries come out in the order the objects appeared on the command
//! line, so a program's constructors run in link order and not in any order the source
//! suggests. The second is the **bracketing symbols**: `__init_array_start` and
//! `__init_array_end` are not defined in any object file. The linker invents them, pointing
//! at the two ends of the section it just built, and if no input has an `.init_array` at
//! all they must still resolve — to an empty range, so the loop that walks them runs zero
//! times rather than crashing.
//!
//! `.fini_array` is the same machinery pointing the other way, walked in reverse at exit.

use crate::asm::{Code, Reg, STDOUT};
use crate::assert::{Check, Failure, FailureKind};
use crate::elf::read::Section;
use crate::elf::write::{ObjectBuilder, Reloc, SectionSpec, SymbolSpec};
use crate::elf::*;
use crate::examples::ExampleSpec;
use crate::link::{Link, Linked};
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 43,
        slug: "init_arrays",
        name: "Init and fini arrays",
        ext: true,
        hints: &[
            "`.init_array` is `SHT_INIT_ARRAY`, allocated and writable, and holds an array of \
             8-byte function pointers — concatenate the inputs' copies in command-line order",
            "`__init_array_start` and `__init_array_end` come from no object file: the linker \
             defines them at the two ends of the section it produced",
            "When nothing has an `.init_array`, those two symbols must still resolve and must \
             be equal, so a walker runs zero times instead of walking nothing",
            "`.fini_array` is the same in every respect, with its own pair of symbols, and is \
             walked backwards at exit",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "an .init_array section survives the link with its type",
                init_array_is_kept,
            )
            .ext(),
            Test::new("it is allocated and writable", init_array_is_allocated).ext(),
            Test::new(
                "the entries of several objects are concatenated",
                entries_are_concatenated,
            )
            .ext(),
            Test::new(
                "in the order the objects were given on the command line",
                order_follows_the_command_line,
            )
            .ext(),
            Test::new(
                "the linker defines __init_array_start and __init_array_end",
                the_bracketing_symbols_exist,
            )
            .ext(),
            Test::new(
                "and they bracket exactly the section it built",
                the_symbols_bracket_the_section,
            )
            .ext(),
            Test::new(
                "with no .init_array anywhere, the symbols are still defined and equal",
                empty_array_still_resolves,
            )
            .ext(),
            Test::new(".fini_array gets the same treatment", fini_array_too).ext(),
            Test::new(
                "a program walks the array and the constructors actually run",
                the_constructors_run,
            )
            .ext(),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------------------

/// An object holding one `.init_array` entry pointing at a function that does nothing,
/// plus a `ret` for it to point at.
fn ctor_object(name: &str, section: &str) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.ret();
    build(
        ObjectBuilder::new()
            .section(text_named(".text", &code))
            .section(
                SectionSpec::new(
                    section,
                    array_type(section),
                    SHF_ALLOC | SHF_WRITE,
                    vec![0; 8],
                )
                .align(8)
                .entsize(8)
                .reloc(Reloc::sym(0, name, R_X86_64_64, 0)),
            )
            .symbol(SymbolSpec::global(name, ".text", 0).func()),
    )
}

fn array_type(section: &str) -> u32 {
    if section == ".fini_array" {
        SHT_FINI_ARRAY
    } else {
        SHT_INIT_ARRAY
    }
}

/// A `_start` that measures `.init_array` and exits with its length in bytes.
///
/// It calls nothing: the count is what makes the length observable, measured from the
/// linker's own symbols. [`calling_walker`] is the one that runs the constructors.
fn array_walker() -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    // eax = __init_array_end - __init_array_start, in bytes: eight per constructor.
    code.lea_rip(Reg::Rax, "__init_array_end", 0);
    code.lea_rip(Reg::Rdi, "__init_array_start", 0);
    code.sub_r32_r32(Reg::Rax, Reg::Rdi);
    code.sys_exit_eax();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::undefined("__init_array_start"))
            .symbol(SymbolSpec::undefined("__init_array_end")),
    )
}

/// An object whose `.init_array` entry points at a constructor that writes `letter` to
/// stdout — so running it is visible, and so is the order constructors run in.
fn printing_ctor_object(name: &str, letter: u8) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "letter", 1);
    code.ret();
    build(
        ObjectBuilder::new()
            .section(text_named(".text", &code))
            .section(SectionSpec::rodata(".rodata", vec![letter]))
            .section(
                SectionSpec::new(
                    ".init_array",
                    SHT_INIT_ARRAY,
                    SHF_ALLOC | SHF_WRITE,
                    vec![0; 8],
                )
                .align(8)
                .entsize(8)
                .reloc(Reloc::sym(0, name, R_X86_64_64, 0)),
            )
            .symbol(SymbolSpec::global(name, ".text", 0).func())
            .symbol(SymbolSpec::local("letter", ".rodata", 0).object(1)),
    )
}

/// A `_start` that really walks `.init_array`: it calls every eight-byte entry from
/// `__init_array_start` up to `__init_array_end`, then exits with the array's length in
/// bytes. `rbx` is the cursor because a constructor (and the syscall in it) may clobber
/// every caller-saved register.
///
/// ```text
///         lea  rbx, [rip+__init_array_start]
/// loop:   lea  rax, [rip+__init_array_end]     ; 7 bytes
///         cmp  rbx, rax                        ; 48 39 c3
///         je   done                            ; 74 08
///         call qword [rbx]                     ; ff 13
///         add  rbx, 8                          ; 48 83 c3 08
///         jmp  loop                            ; eb ec (-20)
/// done:   eax = end - start; exit(eax)
/// ```
fn calling_walker() -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.lea_rip(Reg::Rbx, "__init_array_start", 0);
    code.lea_rip(Reg::Rax, "__init_array_end", 0);
    code.raw(&[0x48, 0x39, 0xc3]); // cmp rbx, rax
    code.je_rel8(8); // over call (2) + add (4) + jmp (2)
    code.raw(&[0xff, 0x13]); // call qword [rbx]
    code.raw(&[0x48, 0x83, 0xc3, 0x08]); // add rbx, 8
    code.raw(&[0xeb, 0xec]); // jmp back 20 bytes, to the second lea
    code.lea_rip(Reg::Rax, "__init_array_end", 0);
    code.lea_rip(Reg::Rdi, "__init_array_start", 0);
    code.sub_r32_r32(Reg::Rax, Reg::Rdi);
    code.sys_exit_eax();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::undefined("__init_array_start"))
            .symbol(SymbolSpec::undefined("__init_array_end")),
    )
}

/// The output's `.init_array`, or a failure that says the linker dropped it — the linker's
/// failure, not the suite's.
fn output_init_array(linked: &Linked) -> Result<&Section, Failure> {
    linked.elf.section(".init_array").ok_or_else(|| {
        linked.attach(Failure::new(
            FailureKind::Assertion,
            "the output has no .init_array: the linker dropped the constructors",
        ))
    })
}

fn stage_examples() -> Vec<ExampleSpec> {
    vec![ExampleSpec::object(
        "Three constructors, counted by the program itself",
        "ld -o prog a.o b.o c.o main.o",
        || array_walker().map_err(|f| f.messages.join("; ")),
    )
    .request(
        "a.o, b.o and c.o each hold one 8-byte .init_array entry relocated to a function of \
         their own; main.o is a _start that references __init_array_start and \
         __init_array_end, neither of which any object defines",
    )
    .response(
        "A 24-byte .init_array of type SHT_INIT_ARRAY, the two symbols defined by the linker \
         at its ends, and a program that exits with 24",
    )
    .note(
        "The exit status is the array's length in bytes, measured from inside the process \
         using nothing but symbols the linker invented. Getting __init_array_end wrong by \
         one entry is the classic bug: the program then either skips the last constructor \
         or calls a pointer that was never written.",
    )]
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

link_test!(init_array_is_kept, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", ctor_object("ctor_a", ".init_array")?)
            .object("main.o", exit_only(0)?),
    )?;
    let mut c = Check::new("the .init_array section in the output");
    match linked.elf.section(".init_array") {
        Some(s) => {
            c.eq(
                "output.section['.init_array'].sh_type",
                SHT_INIT_ARRAY,
                s.sh_type,
            );
            c.eq("output.section['.init_array'].sh_size", 8u64, s.size);
        }
        None => {
            c.that(
                "output.section['.init_array']",
                "a section",
                false,
                "missing — the linker dropped it",
            );
        }
    }
    c.finish()
});

link_test!(init_array_is_allocated, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", ctor_object("ctor_a", ".init_array")?)
            .object("main.o", exit_only(0)?),
    )?;
    let s = output_init_array(&linked)?;
    let mut c = Check::new("the section's flags and placement");
    c.note(
        "The array is walked at run time, so it has to be in memory; the startup code may \
         also blank it once used, so it is writable. A linker that treats an unknown section \
         name as non-allocated will drop it out of every segment and the program will fault \
         reading it.",
    );
    c.that(
        "output.section['.init_array'].sh_flags",
        "SHF_ALLOC | SHF_WRITE",
        s.is_alloc() && s.is_write(),
        format!("0x{:x}", s.flags),
    );
    c.block("where it landed", segment_summary(&linked.elf, s.addr));
    c.finish()
});

link_test!(entries_are_concatenated, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", ctor_object("ctor_a", ".init_array")?)
            .object("b.o", ctor_object("ctor_b", ".init_array")?)
            .object("c.o", ctor_object("ctor_c", ".init_array")?)
            .object("main.o", exit_only(0)?),
    )?;
    let s = output_init_array(&linked)?;
    let mut c = Check::new("three objects, each with one entry");
    c.note(
        "Nothing references these sections and nothing names them in a script: the linker \
         concatenates them because they share a name, exactly as it does for .text.",
    );
    c.eq("output.section['.init_array'].sh_size", 24u64, s.size);
    c.finish()
});

link_test!(order_follows_the_command_line, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", ctor_object("ctor_a", ".init_array")?)
            .object("b.o", ctor_object("ctor_b", ".init_array")?)
            .object("main.o", exit_only(0)?),
    )?;
    let data = output_init_array(&linked)
        .and_then(|_| {
            linked.elf.section_data(".init_array").map_err(|e| {
                linked.attach(Failure::new(
                    FailureKind::MalformedOutput,
                    format!("the output's .init_array cannot be read: {e}"),
                ))
            })
        })?
        .to_vec();
    // An array shorter than the two entries asked for reads as 0, which no ctor is at.
    let entry = |i: usize| -> u64 {
        data.get(i * 8..i * 8 + 8)
            .and_then(|b| b.try_into().ok())
            .map(u64::from_le_bytes)
            .unwrap_or(0)
    };
    let a = linked.elf.symbol_address("ctor_a").unwrap_or(0);
    let b = linked.elf.symbol_address("ctor_b").unwrap_or(0);
    let mut c = Check::new("which constructor comes first");
    c.note(
        "a.o was given before b.o, so ctor_a's pointer comes first and the program's \
         constructors run in that order. This is why the order of object files on a link \
         line is observable behaviour and not a detail of the build system.",
    );
    c.eq("entry 0 points at ctor_a", a, entry(0));
    c.eq("entry 1 points at ctor_b", b, entry(1));
    c.finish()
});

link_test!(the_bracketing_symbols_exist, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", ctor_object("ctor_a", ".init_array")?)
            .object("main.o", array_walker()?),
    )?;
    let mut c = Check::new("symbols no input file defines");
    c.note(
        "Both symbols were undefined in every object and the link succeeded, so the linker \
         supplied them. A linker that only resolves symbols it has seen defined will report \
         an undefined reference here and be unable to link any real C program.",
    );
    for name in ["__init_array_start", "__init_array_end"] {
        c.that(
            &format!("output.symbol['{name}']"),
            "defined by the linker",
            linked.elf.symbol(name).is_some(),
            "missing",
        );
    }
    c.finish()
});

link_test!(the_symbols_bracket_the_section, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", ctor_object("ctor_a", ".init_array")?)
            .object("b.o", ctor_object("ctor_b", ".init_array")?)
            .object("main.o", array_walker()?),
    )?;
    let s = output_init_array(&linked)?;
    let start = linked.elf.symbol_address("__init_array_start").unwrap_or(0);
    let end = linked.elf.symbol_address("__init_array_end").unwrap_or(0);
    let mut c = Check::new("where the two symbols point");
    c.note(
        "start is the section's address and end is one past its last byte, so the difference \
         divided by eight is the number of constructors. Getting `end` wrong by one entry is \
         the classic bug and it either skips the last constructor or calls a pointer that is \
         not there.",
    );
    c.eq("__init_array_start", s.addr, start);
    c.eq("__init_array_end", s.addr.saturating_add(s.size), end);
    c.eq(
        "the array holds two entries",
        Some(2u64),
        end.checked_sub(start).map(|bytes| bytes / 8),
    );
    c.finish()
});

link_test!(empty_array_still_resolves, |ctx| {
    // Nothing in this link has an .init_array at all.
    let linked = ctx.link_ok(&Link::new().object("main.o", array_walker()?))?;
    let start = linked.elf.symbol_address("__init_array_start").unwrap_or(1);
    let end = linked.elf.symbol_address("__init_array_end").unwrap_or(2);
    let mut c = Check::new("a program with no constructors at all");
    c.note(
        "The common case, and the one that must not be special-cased into a failure. The \
         two symbols exist, they are equal, and the walker computes a length of zero and \
         exits with it.",
    );
    c.eq("the symbols are equal", start, end);
    ctx.expect_output(&linked, "", 0)?;
    c.finish()
});

link_test!(fini_array_too, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", ctor_object("dtor_a", ".fini_array")?)
            .object("b.o", ctor_object("dtor_b", ".fini_array")?)
            .object("main.o", exit_only(0)?),
    )?;
    let mut c = Check::new("the destructor array");
    match linked.elf.section(".fini_array") {
        Some(s) => {
            c.eq(
                "output.section['.fini_array'].sh_type",
                SHT_FINI_ARRAY,
                s.sh_type,
            );
            c.eq("output.section['.fini_array'].sh_size", 16u64, s.size);
            c.that(
                "it is allocated and writable",
                "SHF_ALLOC | SHF_WRITE",
                s.is_alloc() && s.is_write(),
                format!("0x{:x}", s.flags),
            );
        }
        None => {
            c.that(
                "output.section['.fini_array']",
                "a section",
                false,
                "missing — the linker dropped it",
            );
        }
    }
    c.note(
        "Identical machinery, opposite direction: the startup code walks this one backwards \
         after main returns, so the last constructor to run is the first destructor.",
    );
    c.finish()
});

link_test!(the_constructors_run, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", printing_ctor_object("ctor_a", b'a')?)
            .object("b.o", printing_ctor_object("ctor_b", b'b')?)
            .object("c.o", printing_ctor_object("ctor_c", b'c')?)
            .object("main.o", calling_walker()?),
    )?;
    assert_runnable_layout(&linked)?;
    // Each constructor prints its own letter, in the order the array holds them; the exit
    // status is the array's length — three entries of eight bytes.
    ctx.expect_output(&linked, "abc", 24)?;
    ctx.note(
        "Three objects contributed a constructor each, and a _start that knows nothing but \
         the linker's two symbols called all three, in command-line order. An entry the \
         linker left unrelocated is a call to address 0, and that is a SIGSEGV here.",
    );
    Ok(())
});
