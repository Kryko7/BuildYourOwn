//! Stage 09 — Program headers and `PT_LOAD`.
//!
//! Section headers are for linkers; program headers are for the kernel. `execve` reads the
//! ELF header, walks the program header table and `mmap`s one region per `PT_LOAD` — it
//! never looks at a section header. So the whole of stage 09 is one idea: every byte the
//! program will touch has to be inside some `PT_LOAD`, and the segments have to be a sane
//! description of memory rather than a copy of the section table.

use crate::assert::Check;
use crate::elf::read::Segment;
use crate::elf::*;
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 9,
        slug: "program_headers",
        name: "Program headers and PT_LOAD",
        ext: false,
        hints: &[
            "The kernel maps `PT_LOAD` segments and ignores section headers entirely, so \
             every allocated section has to end up inside one of them",
            "Group the output sections by permission and emit one `PT_LOAD` per group; one \
             segment per section also works, it is just wasteful of pages",
            "`p_filesz` is what is read from the file and `p_memsz` what is mapped, so \
             `p_filesz <= p_memsz` always, and the difference is `.bss`",
            "This is a static link: no `PT_INTERP` and no `PT_DYNAMIC`, because there is no \
             loader to name and nothing for it to do",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "the output has at least one PT_LOAD and satisfies every layout invariant",
                at_least_one_load,
            ),
            Test::new("p_filesz never exceeds p_memsz", filesz_within_memsz),
            Test::new(
                "no two PT_LOAD segments cover the same address",
                loads_do_not_overlap,
            ),
            Test::new(
                "every allocated section of the output lies inside some PT_LOAD",
                sections_are_covered,
            ),
            Test::new(
                "a static link has no PT_INTERP and no PT_DYNAMIC",
                no_interpreter_no_dynamic,
            ),
            Test::new(
                "the program header table itself lies inside the file",
                table_in_file,
            ),
            Test::new(
                "the entry point lies inside an executable PT_LOAD",
                entry_in_an_executable_load,
            ),
            Test::new("no PT_LOAD is mapped at address zero", no_load_at_zero),
        ],
    }
}

link_test!(at_least_one_load, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", print_and_exit("loads\n", 0)?))?;
    assert_runnable_layout(&linked)?;
    let loads: Vec<Segment> = linked.elf.loads().copied().collect();
    let mut c = Check::new("the loadable segments of the output");
    c.at_least("output.PT_LOAD count", 1usize, loads.len());
    for s in &loads {
        c.observe(&format!("output.segment[{}]", s.index), s.describe());
    }
    c.finish()?;
    match ctx.reference_link(&Link::new().object("a.o", print_and_exit("loads\n", 0)?))? {
        Some(reference) => {
            let n = reference.elf.loads().count();
            ctx.note(format!(
                "GNU ld chose {n} PT_LOAD segments for the same input; the count is the \
                 linker's own business and is never asserted"
            ));
        }
        None => ctx.note(
            "no separate reference linker to compare the segment count against — the count \
             is not an invariant anyway, only the coverage is",
        ),
    }
    ctx.expect_output(&linked, "loads\n", 0)?;
    Ok(())
});

link_test!(filesz_within_memsz, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", bss_prober("fm\n", 4096, 5)?))?;
    let exe = &linked.elf;
    let mut c = Check::new("p_filesz against p_memsz, for every program header");
    for s in &exe.segments {
        c.that(
            &format!("output.segment[{}].p_filesz", s.index),
            "at most p_memsz — the file image cannot be larger than the memory image",
            s.filesz <= s.memsz,
            s.describe(),
        );
        c.that(
            &format!("output.segment[{}].p_offset", s.index),
            "a file range that lies inside the file",
            s.offset.saturating_add(s.filesz) <= linked.bytes.len() as u64,
            format!(
                "offset 0x{:x} + filesz 0x{:x} vs a file of 0x{:x} bytes",
                s.offset,
                s.filesz,
                linked.bytes.len()
            ),
        );
    }
    c.note(
        "the gap between p_filesz and p_memsz is exactly .bss: the kernel zero-fills it, \
         which is why a megabyte of .bss costs no file bytes",
    );
    if !c.ok() {
        c.block("output program headers", exe.program_header_table());
    }
    c.finish()?;
    ctx.expect_output(&linked, "fm\n", 5)?;
    Ok(())
});

link_test!(loads_do_not_overlap, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", bss_prober("ov\n", 8192, 6)?))?;
    let loads: Vec<Segment> = linked.elf.loads().copied().collect();
    let mut c = Check::new("whether any two loadable segments claim the same address");
    for (i, a) in loads.iter().enumerate() {
        for b in loads.iter().skip(i + 1) {
            let overlap = a.memsz > 0
                && b.memsz > 0
                && a.vaddr < b.vaddr.saturating_add(b.memsz)
                && b.vaddr < a.vaddr.saturating_add(a.memsz);
            c.that(
                &format!("output.segment[{}] vs output.segment[{}]", a.index, b.index),
                "two disjoint memory ranges — the kernel maps them one after another and the \
                 later mapping would silently replace the earlier one",
                !overlap,
                format!("{} overlaps {}", a.describe(), b.describe()),
            );
        }
    }
    if !c.ok() {
        c.block("output program headers", linked.elf.program_header_table());
    }
    c.finish()?;
    ctx.expect_output(&linked, "ov\n", 6)?;
    Ok(())
});

link_test!(sections_are_covered, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", bss_prober("cov\n", 1024, 8)?))?;
    let exe = &linked.elf;
    let mut c = Check::new("whether every allocated section is inside a PT_LOAD");
    for s in allocated_sections(exe) {
        let end = s.addr.saturating_add(s.size);
        let covered = exe
            .loads()
            .any(|l| s.addr >= l.vaddr && end <= l.vaddr.saturating_add(l.memsz));
        c.that(
            &format!("output.section['{}']", s.name),
            "entirely inside one PT_LOAD — the kernel maps segments, not sections",
            covered,
            format!(
                "0x{:x}..0x{end:x}, and {}",
                s.addr,
                segment_summary(exe, s.addr)
            ),
        );
    }
    if !c.ok() {
        c.block("output program headers", exe.program_header_table());
        c.block("output section headers", exe.section_header_table());
    }
    c.finish()?;
    ctx.expect_output(&linked, "cov\n", 8)?;
    Ok(())
});

link_test!(no_interpreter_no_dynamic, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", print_and_exit("static\n", 0)?))?;
    let exe = &linked.elf;
    let mut c = Check::new("the segment types a static executable is allowed to carry");
    for s in &exe.segments {
        c.that(
            &format!("output.segment[{}].p_type", s.index),
            "anything but PT_INTERP or PT_DYNAMIC — there is no loader in a static link",
            s.p_type != PT_INTERP && s.p_type != PT_DYNAMIC,
            segment_type_name(s.p_type),
        );
    }
    c.that(
        "output.sections",
        "no .interp and no .dynamic section either",
        exe.section(".interp").is_none() && exe.section(".dynamic").is_none(),
        exe.sections
            .iter()
            .map(|s| s.name.clone())
            .collect::<Vec<_>>()
            .join(" "),
    );
    c.note(
        "PT_GNU_STACK, PT_NOTE and PT_PHDR are all allowed and all optional; only the two \
         that summon a dynamic loader are forbidden",
    );
    c.finish()?;
    ctx.expect_output(&linked, "static\n", 0)?;
    Ok(())
});

link_test!(table_in_file, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", print_and_exit("tbl\n", 0)?))?;
    let exe = &linked.elf;
    let span = u64::from(exe.phnum) * u64::from(exe.phentsize);
    let end = exe.phoff.saturating_add(span);
    let mut c = Check::new("where the program header table lives");
    c.at_least("output.e_phoff", u64::from(EHDR_SIZE), exe.phoff);
    c.that(
        "output.e_phoff + e_phnum * e_phentsize",
        "a range inside the file — the kernel reads the table straight out of it",
        end <= linked.bytes.len() as u64,
        format!(
            "0x{:x}..0x{end:x} of 0x{:x} bytes",
            exe.phoff,
            linked.bytes.len()
        ),
    );
    let covered = exe
        .loads()
        .any(|l| exe.phoff >= l.offset && end <= l.offset.saturating_add(l.filesz));
    c.observe("output.program_header_table.mapped", covered);
    c.note(
        "GNU ld also maps the table by giving the first PT_LOAD p_offset 0, which is what \
         lets a program read its own headers; that is convention, not a requirement",
    );
    c.finish()?;
    ctx.expect_output(&linked, "tbl\n", 0)?;
    Ok(())
});

link_test!(entry_in_an_executable_load, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", caller("entry\n", "other")?)
            .object("b.o", callee_returning("other", 12)?),
    )?;
    let exe = &linked.elf;
    let seg = exe.segment_at(exe.entry);
    let mut c = Check::new("the segment the entry point falls in");
    c.that(
        "output.e_entry",
        "inside a PT_LOAD whose p_flags has PF_X",
        seg.map(Segment::executable).unwrap_or(false),
        match seg {
            Some(s) => format!("entry 0x{:x} is in {}", exe.entry, s.describe()),
            None => format!("entry 0x{:x} is in no PT_LOAD at all", exe.entry),
        },
    );
    c.that(
        "output.e_entry",
        "inside a readable segment too — the processor has to fetch from it",
        seg.map(Segment::readable).unwrap_or(false),
        seg.map(Segment::describe).unwrap_or_default(),
    );
    if !c.ok() {
        c.block("output program headers", exe.program_header_table());
    }
    c.finish()?;
    ctx.expect_output(&linked, "entry\n", 12)?;
    Ok(())
});

link_test!(no_load_at_zero, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", print_and_exit("nz\n", 0)?))?;
    let exe = &linked.elf;
    let mut c = Check::new("the addresses the loadable segments were given");
    for s in exe.loads() {
        c.that(
            &format!("output.segment[{}].p_vaddr", s.index),
            "a non-zero address — a non-PIE executable is never loaded at 0, and the first \
             page has to stay unmapped so that a null dereference faults",
            s.vaddr != 0,
            s.describe(),
        );
    }
    c.note(
        "the usual base is 0x400000; anything above the kernel's mmap_min_addr works, and \
         the suite does not care which the linker picked",
    );
    c.finish()?;
    ctx.expect_output(&linked, "nz\n", 0)?;
    Ok(())
});

/// Worked examples: what the kernel reads, and what it ignores.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "One object, three kinds of bytes",
            "ld -o prog probe.o",
            || bss_prober("cov\n", 1024, 8).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "probe.o: a .text (SHF_ALLOC|SHF_EXECINSTR), a .data (SHF_ALLOC|SHF_WRITE) \
             holding the message, and a 1024-byte SHT_NOBITS .bss — three sections with \
             three different permission sets",
        )
        .response(
            "A program header table with at least one PT_LOAD; every one of those three \
             sections inside some PT_LOAD; p_filesz <= p_memsz everywhere, with the .bss \
             showing up as p_memsz - p_filesz; no PT_INTERP, no PT_DYNAMIC, and no two \
             PT_LOADs covering the same address",
        )
        .note(
            "The tempting bug is emitting one PT_LOAD per section with p_offset copied from \
             sh_offset. That breaks the moment two sections share a page, because p_offset \
             and p_vaddr then disagree modulo the page size — stage 12's congruence.",
        )
        .runs("cov\n", 8),
        ExampleSpec::object(
            "Two objects, one text segment",
            "ld -o prog main.o other.o",
            || caller("entry\n", "other").map_err(|f| f.messages.join("; ")),
        )
        .request(
            "main.o: .text with write(1, message, 6), `call other` (R_X86_64_PLT32) and \
             exit(eax); other.o defines `other` as `mov eax, 12; ret`",
        )
        .response(
            "Both .text contributions inside the same executable PT_LOAD, e_entry pointing \
             at _start inside it, and the call's displacement patched so that control \
             reaches the second object's code",
        )
        .note(
            "The entry point has to land in a segment with PF_X. A linker that forgets the \
             flag produces a file that links, parses and segfaults on the first instruction.",
        )
        .runs("entry\n", 12),
    ]
}
