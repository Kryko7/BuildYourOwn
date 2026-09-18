//! Stage 08 — The ELF header of the executable.
//!
//! Section A was about reading; from here on the linker writes. The first sixty-four bytes
//! it writes are the ones the kernel looks at before anything else, and almost every field
//! in them is fixed by the ABI rather than chosen by the linker: `ET_EXEC` because this is a
//! non-PIE static link, `EM_X86_64` because that is the machine, 64/56/64 because those are
//! the ELF64 structure sizes. The only two fields that are genuinely the linker's own
//! decisions are `e_entry` and where it put the program header table.

use crate::assert::Check;
use crate::elf::read::Symbol;
use crate::elf::*;
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 8,
        slug: "executable_header",
        name: "The ELF header of the executable",
        ext: false,
        hints: &[
            "Write `e_type = ET_EXEC` (2), not `ET_DYN`: this track links non-PIE static \
             executables, so the addresses in the file are the addresses the program runs at",
            "`e_ehsize` is 64, `e_phentsize` 56 and `e_shentsize` 64 — these are structure \
             sizes, not choices, and a kernel that disagrees with them refuses the file",
            "`e_ident[7..16]` is padding and must be zero, `e_version` and `e_ident[6]` are \
             both 1, and `e_flags` is 0 on x86-64 — there are no processor flags to set",
            "Emit a `.symtab` (and its `.strtab`) naming the globals you defined: the suite \
             looks `_start` up there, and so does every debugger and every `nm`",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "the output is an ET_EXEC ELFCLASS64 little-endian x86-64 file",
                identity_fields,
            ),
            Test::new(
                "e_version is 1 and the e_ident padding is zero",
                version_and_padding,
            ),
            Test::new(
                "e_ehsize, e_phentsize and e_shentsize are the ELF64 structure sizes",
                structure_sizes,
            ),
            Test::new(
                "e_phoff points past the header at a table of at least one entry",
                program_header_table_located,
            ),
            Test::new(
                "e_entry is not zero and the program starts there",
                entry_is_set,
            ),
            Test::new(
                "e_flags is zero — x86-64 defines no processor flags",
                flags_are_zero,
            ),
            Test::new(
                "the output carries a .symtab naming the globals it defined",
                symtab_names_globals,
            ),
            Test::new(
                "the same header invariants hold for a program with .data and .bss",
                header_of_a_bigger_program,
            ),
        ],
    }
}

link_test!(identity_fields, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", print_and_exit("header\n", 0)?))?;
    let exe = &linked.elf;
    let mut c = Check::new("the identity half of the executable's ELF header");
    c.bytes_eq("output.e_ident[0..4]", &ELF_MAGIC, &exe.ident[..4]);
    c.eq("output.e_ident[EI_CLASS]", ELFCLASS64, exe.ident[4]);
    c.eq("output.e_ident[EI_DATA]", ELFDATA2LSB, exe.ident[5]);
    c.eq("output.e_type", ET_EXEC, exe.e_type);
    c.eq("output.e_machine", EM_X86_64, exe.e_machine);
    c.note(
        "ET_DYN would be a position-independent executable, which needs a dynamic loader and \
         relocations at start-up; this track links non-PIE static binaries",
    );
    if !c.ok() {
        c.hex(
            "the first 64 bytes of the output",
            &linked.bytes,
            &[16..18, 18..20],
        );
    }
    c.finish()?;
    ctx.expect_output(&linked, "header\n", 0)?;
    Ok(())
});

link_test!(version_and_padding, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", exit_only(0)?))?;
    let exe = &linked.elf;
    let mut c = Check::new("the version byte, the version word and the e_ident padding");
    c.eq("output.e_ident[EI_VERSION]", EV_CURRENT, exe.ident[6]);
    c.eq("output.e_version", EV_CURRENT as u32, exe.e_version);
    let padding = &exe.ident[9..16];
    c.that(
        "output.e_ident[9..16]",
        "seven zero bytes — EI_PAD is reserved and must be written as zero",
        padding.iter().all(|b| *b == 0),
        format!("{padding:02x?}"),
    );
    c.observe("output.e_ident[EI_OSABI]", exe.ident[7]);
    c.note(
        "EI_OSABI (e_ident[7]) may be 0 (System V) or 3 (Linux); Linux itself does not look \
         at it for a static executable, so the suite only records what the linker wrote",
    );
    if !c.ok() {
        c.hex("e_ident", &linked.bytes, &[7..9, 9..16]);
    }
    c.finish()?;
    ctx.expect_output(&linked, "", 0)?;
    Ok(())
});

link_test!(structure_sizes, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", print_and_exit("sizes\n", 3)?))?;
    let exe = &linked.elf;
    let mut c = Check::new("the three structure-size fields of the ELF header");
    c.eq("output.e_ehsize", EHDR_SIZE, exe.ehsize);
    c.eq("output.e_phentsize", PHDR_SIZE, exe.phentsize);
    if exe.shnum > 0 {
        c.eq("output.e_shentsize", SHDR_SIZE, exe.shentsize);
        c.that(
            "output.e_shstrndx",
            "a section index inside the section header table",
            (exe.shstrndx as usize) < exe.sections.len(),
            format!("e_shstrndx {} of {} sections", exe.shstrndx, exe.shnum),
        );
    } else {
        c.note(
            "the output has no section header table at all, which the kernel does not mind; \
             e_shentsize is then unconstrained",
        );
    }
    c.finish()?;
    ctx.expect_output(&linked, "sizes\n", 3)?;
    Ok(())
});

link_test!(program_header_table_located, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", print_and_exit("phdr\n", 0)?))?;
    let exe = &linked.elf;
    let mut c = Check::new("where the program header table is and how big it is");
    c.at_least("output.e_phoff", u64::from(EHDR_SIZE), exe.phoff);
    c.at_least("output.e_phnum", 1u16, exe.phnum);
    let end = exe
        .phoff
        .saturating_add(u64::from(exe.phnum) * u64::from(exe.phentsize));
    c.that(
        "output.e_phoff + e_phnum * e_phentsize",
        "a range that lies inside the file",
        end <= linked.bytes.len() as u64,
        format!(
            "ends at 0x{end:x}, the file is 0x{:x} bytes",
            linked.bytes.len()
        ),
    );
    c.note(
        "the table almost always starts at 0x40, immediately after the header, because the \
         first PT_LOAD has to cover it — but only 'inside the file' is required",
    );
    if !c.ok() {
        c.block("output program headers", exe.program_header_table());
    }
    c.finish()?;
    ctx.expect_output(&linked, "phdr\n", 0)?;
    Ok(())
});

link_test!(entry_is_set, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", print_and_exit("go\n", 21)?))?;
    let mut c = Check::new("e_entry");
    c.ne("output.e_entry", 0u64, linked.elf.entry);
    c.observe("output.e_entry", format!("0x{:x}", linked.elf.entry));
    c.finish()?;
    assert_entry_is(&linked, DEFAULT_ENTRY)?;
    ctx.expect_output(&linked, "go\n", 21)?;
    Ok(())
});

link_test!(flags_are_zero, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", exit_only(2)?))?;
    let mut c = Check::new("e_flags");
    c.eq("output.e_flags", 0u32, linked.elf.e_flags);
    c.note(
        "e_flags carries processor-specific flags; the x86-64 psABI defines none, so the \
         only correct value is 0 — architectures like MIPS and ARM are where it matters",
    );
    c.finish()?;
    ctx.expect_output(&linked, "", 2)?;
    Ok(())
});

link_test!(symtab_names_globals, |ctx| {
    let obj = two_globals("sym\n", 0)?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj))?;
    let exe = &linked.elf;
    let mut c = Check::new("the symbol table of the output");
    c.that(
        "output.sections",
        "a SHT_SYMTAB section — the suite, nm and every debugger read the output's symbols",
        exe.sections.iter().any(|s| s.sh_type == SHT_SYMTAB),
        exe.sections
            .iter()
            .map(|s| s.name.clone())
            .collect::<Vec<_>>()
            .join(" "),
    );
    for name in [DEFAULT_ENTRY, "alpha", "beta"] {
        match exe.symbol(name) {
            Some(sym) => {
                c.that(
                    &format!("output.symbol['{name}'].st_shndx"),
                    "a defined symbol, not SHN_UNDEF",
                    !sym.is_undefined(),
                    describe_symbol(sym),
                );
                c.that(
                    &format!("output.symbol['{name}'].st_value"),
                    "an address inside a PT_LOAD segment",
                    exe.segment_at(sym.value).is_some(),
                    segment_summary(exe, sym.value),
                );
            }
            None => {
                c.that(
                    &format!("output.symbol['{name}']"),
                    "present in the output's .symtab",
                    false,
                    "missing",
                );
            }
        }
    }
    if !c.ok() {
        c.block("output section headers", exe.section_header_table());
        c.note(
            "the linker may drop local and section symbols, but the globals it resolved have \
             to come out with the addresses it gave them",
        );
    }
    c.finish()?;
    ctx.expect_output(&linked, "sym\n", 0)?;
    Ok(())
});

link_test!(header_of_a_bigger_program, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", bss_prober("bigger\n", 256, 11)?))?;
    let exe = &linked.elf;
    let mut c = Check::new("the ELF header of a program with .text, .data and .bss");
    c.eq("output.e_type", ET_EXEC, exe.e_type);
    c.eq("output.e_machine", EM_X86_64, exe.e_machine);
    c.eq("output.e_ehsize", EHDR_SIZE, exe.ehsize);
    c.eq("output.e_phentsize", PHDR_SIZE, exe.phentsize);
    c.eq("output.e_flags", 0u32, exe.e_flags);
    c.ne("output.e_entry", 0u64, exe.entry);
    c.finish()?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "bigger\n", 11)?;
    Ok(())
});

/// One object with `_start` plus two more globals, so the output's `.symtab` has something
/// to name beyond the entry point.
fn two_globals(message: &str, status: u32) -> Result<Vec<u8>, crate::assert::Failure> {
    use crate::asm::{Code, STDOUT};
    use crate::elf::write::{ObjectBuilder, SectionSpec, SymbolSpec};
    let mut code = Code::new();
    code.sys_write(STDOUT, "alpha", message.len() as u32);
    code.sys_exit(status);
    let beta_at = code.len();
    code.ret();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("beta", ".text", beta_at).func())
            .symbol(SymbolSpec::global("alpha", ".rodata", 0).object(message.len() as u64)),
    )
}

/// A symbol, as a failure row reads it.
fn describe_symbol(sym: &Symbol) -> String {
    format!(
        "st_shndx {} st_value 0x{:x} st_size {}",
        sym.shndx, sym.value, sym.size
    )
}

/// Worked examples: the sixty-four bytes the kernel reads first.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "The smallest executable header there is",
            "ld -o prog exit0.o",
            || exit_only(0).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "exit0.o: one .text with `mov eax, 60; mov edi, 0; syscall`, one global _start, \
             no .rodata, no relocations — an ET_REL object whose e_entry is 0 and whose \
             e_phnum is 0, because an object has no program headers",
        )
        .response(
            "An ET_EXEC ELFCLASS64 little-endian EM_X86_64 file: e_version 1, e_ehsize 64, \
             e_phentsize 56, e_shentsize 64, e_flags 0, e_ident[9..16] all zero, e_phoff 0x40 \
             with e_phnum >= 1, and a non-zero e_entry equal to the address of _start",
        )
        .note(
            "Copying the input object's header and patching e_type is the classic first \
             mistake: an object has e_phnum 0 and e_entry 0, and a file with no program \
             headers is not something the kernel will exec.",
        )
        .runs("", 0),
        ExampleSpec::object(
            "A header for a program with data and bss",
            "ld -o prog probe.o",
            || bss_prober("bigger\n", 256, 11).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "probe.o: .text, a .data holding the message, a 256-byte SHT_NOBITS .bss, and \
             globals _start and scratch",
        )
        .response(
            "The same header fields as the minimal case — nothing about e_type, e_machine or \
             the structure sizes changes when the program grows; only e_entry, e_phoff, \
             e_phnum and e_shoff differ",
        )
        .note(
            "The header is boilerplate. What is not boilerplate is that e_shoff, e_phoff and \
             every section's sh_offset have to agree with where the bytes actually landed.",
        )
        .runs("bigger\n", 11),
    ]
}
