//! Stage 11 — Segment permissions and W^X.
//!
//! `p_flags` is the only place permissions are recorded: `sh_flags` on a section header is
//! advice to the linker, and the kernel never reads it. So the linker has to translate
//! `SHF_EXECINSTR` and `SHF_WRITE` into `PF_X` and `PF_W` itself, and it has to keep code
//! and data in *different* segments, because a page that is both writable and executable is
//! the oldest exploit primitive there is.

use crate::asm::{Code, Reg, STDOUT};
use crate::assert::{Check, Failure};
use crate::elf::read::{Elf, Segment};
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
        number: 11,
        slug: "segment_permissions",
        name: "Segment permissions and W^X",
        ext: false,
        hints: &[
            "Turn `SHF_EXECINSTR` into `PF_X` and `SHF_WRITE` into `PF_W`, always keeping \
             `PF_R`: a segment nothing can read is a segment nothing can use",
            "Group sections by the permission set they end up with and give each group its \
             own `PT_LOAD`, so that `.text` never shares a segment with `.data`",
            "No `PT_LOAD` may carry both `PF_W` and `PF_X` — merging read-only data into the \
             text segment is fine, merging `.data` into it is not",
            "Permissions are enforced by the MMU at page granularity, so two sections that \
             share a page share its permissions whatever the section headers say",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "the segment holding .text is executable",
                text_segment_is_executable,
            ),
            Test::new(
                "the segment holding .data is writable",
                data_segment_is_writable,
            ),
            Test::new(
                "no PT_LOAD is both writable and executable",
                nothing_is_writable_and_executable,
            ),
            Test::new(
                "a program that stores to .data and reads it back works",
                data_is_writable_at_run_time,
            ),
            Test::new(
                "the segment holding .rodata is readable",
                rodata_segment_is_readable,
            ),
            Test::new(
                "a program that writes to .bss and reads it back works",
                bss_is_writable_at_run_time,
            ),
            Test::new(
                "code contributed by a second object is executable too",
                the_whole_text_segment_is_executable,
            ),
            Test::new(
                "the permissions hold for a program with .text, .rodata, .data and .bss",
                all_four_kinds_of_section,
            ),
        ],
    }
}

link_test!(text_segment_is_executable, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", print_and_exit("x\n", 0)?))?;
    let exe = &linked.elf;
    let mut c = Check::new("the permissions of the segment that holds .text");
    match exe.section(".text") {
        Some(text) => match exe.segment_at(text.addr) {
            Some(seg) => {
                c.that(
                    "output.segment(.text).p_flags",
                    "PF_X — the processor refuses to fetch instructions from a page without \
                     it",
                    seg.executable(),
                    flags_string(seg.flags),
                );
                c.that(
                    "output.segment(.text).p_flags",
                    "PF_R as well",
                    seg.readable(),
                    flags_string(seg.flags),
                );
            }
            None => {
                c.that(
                    "output.segment(.text)",
                    "some PT_LOAD covering .text",
                    false,
                    format!(".text is at 0x{:x} and no PT_LOAD maps it", text.addr),
                );
            }
        },
        None => {
            c.note(
                "the output has no section named .text; the entry point's segment is checked \
                 instead",
            );
        }
    }
    c.that(
        "output.segment(e_entry).p_flags",
        "PF_X at the entry point, whatever the section table says",
        exe.segment_at(exe.entry)
            .map(Segment::executable)
            .unwrap_or(false),
        segment_summary(exe, exe.entry),
    );
    if !c.ok() {
        c.block("output program headers", exe.program_header_table());
    }
    c.finish()?;
    ctx.expect_output(&linked, "x\n", 0)?;
    Ok(())
});

link_test!(data_segment_is_writable, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", counter_program(21)?))?;
    let exe = &linked.elf;
    let addr = linked.address_of("counter")?;
    let mut c = Check::new("the permissions of the segment that holds .data");
    match exe.segment_at(addr) {
        Some(seg) => {
            c.that(
                "output.segment(.data).p_flags",
                "PF_W — a store to .data faults without it",
                seg.writable(),
                flags_string(seg.flags),
            );
            c.that(
                "output.segment(.data).p_flags",
                "PF_R as well",
                seg.readable(),
                flags_string(seg.flags),
            );
        }
        None => {
            c.that(
                "output.segment(.data)",
                "some PT_LOAD covering .data",
                false,
                format!("'counter' is at 0x{addr:x} and no PT_LOAD maps it"),
            );
        }
    }
    if !c.ok() {
        c.block("output program headers", exe.program_header_table());
    }
    c.finish()?;
    ctx.expect_output(&linked, "", 42)?;
    Ok(())
});

link_test!(nothing_is_writable_and_executable, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", counter_program(11)?))?;
    assert_no_wx(&linked.elf)?;
    ctx.expect_output(&linked, "", 22)?;
    Ok(())
});

link_test!(data_is_writable_at_run_time, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", counter_program(37)?))?;
    assert_runnable_layout(&linked)?;
    let exe = &linked.elf;
    let addr = linked.address_of("counter")?;
    let mut c = Check::new("what the linker wrote into .data before the program ran");
    match exe.u32_at_vaddr(addr) {
        Ok(got) => c.eq("output[counter]", 37u32, got),
        Err(e) => c.that(
            "output[counter]",
            "four readable bytes",
            false,
            e.to_string(),
        ),
    };
    c.finish()?;
    // The program stores 37*2 back into `counter`, reloads it and exits with it. A
    // read-only data segment would kill it with SIGSEGV instead.
    ctx.expect_output(&linked, "", 74)?;
    Ok(())
});

link_test!(rodata_segment_is_readable, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", print_and_exit("ro\n", 0)?))?;
    let exe = &linked.elf;
    let mut c = Check::new("the permissions of the segment that holds .rodata");
    match exe.section(".rodata").and_then(|s| exe.segment_at(s.addr)) {
        Some(seg) => {
            c.that(
                "output.segment(.rodata).p_flags",
                "PF_R — the string is read by the write(2) the program makes",
                seg.readable(),
                flags_string(seg.flags),
            );
            c.observe("output.segment(.rodata).writable", seg.writable());
            if seg.writable() {
                c.note(
                    "this linker put .rodata in a writable segment; that is legal but wasteful \
                     of protection, and the suite only records it",
                );
            }
            if seg.executable() {
                c.note(
                    "this linker merged .rodata into the text segment, which is what GNU ld \
                     does without --rosegment; read-only data in an executable segment is \
                     allowed",
                );
            }
        }
        None => {
            c.note(
                "the output has no .rodata section header; the string is checked by running \
                 the program instead",
            );
        }
    }
    c.finish()?;
    assert_no_wx(exe)?;
    ctx.expect_output(&linked, "ro\n", 0)?;
    Ok(())
});

link_test!(bss_is_writable_at_run_time, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", bss_writer(23)?))?;
    let exe = &linked.elf;
    let addr = linked.address_of("scratch")?;
    let mut c = Check::new("the permissions of the segment that holds .bss");
    match exe.segment_at(addr) {
        Some(seg) => {
            c.that(
                "output.segment(.bss).p_flags",
                "PF_W — .bss exists to be written",
                seg.writable(),
                flags_string(seg.flags),
            );
            c.that(
                "output.segment(.bss).p_flags",
                "not PF_X",
                !seg.executable(),
                flags_string(seg.flags),
            );
        }
        None => {
            c.that(
                "output.segment(.bss)",
                "some PT_LOAD covering .bss",
                false,
                format!("'scratch' is at 0x{addr:x} and no PT_LOAD maps it"),
            );
        }
    }
    c.finish()?;
    ctx.expect_output(&linked, "", 23)?;
    Ok(())
});

link_test!(the_whole_text_segment_is_executable, |ctx| {
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", caller("call\n", "other")?)
            .object("b.o", callee_returning("other", 13)?),
    )?;
    let exe = &linked.elf;
    let mut c = Check::new("the segments the two .text contributions ended up in");
    for name in [DEFAULT_ENTRY, "other"] {
        let addr = linked.address_of(name)?;
        c.that(
            &format!("output.segment('{name}').p_flags"),
            "PF_X — both objects' code has to be executable, not just the first one's",
            exe.segment_at(addr)
                .map(Segment::executable)
                .unwrap_or(false),
            segment_summary(exe, addr),
        );
    }
    if !c.ok() {
        c.block("output program headers", exe.program_header_table());
    }
    c.finish()?;
    // The call into the second object is the run-time proof: if that page were not
    // executable the program would die with SIGSEGV instead of exiting 13.
    ctx.expect_output(&linked, "call\n", 13)?;
    Ok(())
});

link_test!(all_four_kinds_of_section, |ctx| {
    let linked = ctx.link_ok(&Link::new().object("a.o", every_kind_of_section("all\n", 17)?))?;
    assert_runnable_layout(&linked)?;
    assert_no_wx(&linked.elf)?;
    let exe = &linked.elf;
    let mut c = Check::new("the permission of each of the four kinds of section");
    for (symbol, want_write, want_exec) in [
        (DEFAULT_ENTRY, false, true),
        ("message", false, false),
        ("counter", true, false),
        ("scratch", true, false),
    ] {
        let addr = linked.address_of(symbol)?;
        let Some(seg) = exe.segment_at(addr) else {
            c.that(
                &format!("output.segment('{symbol}')"),
                "some PT_LOAD covering it",
                false,
                format!("0x{addr:x} is in no PT_LOAD"),
            );
            continue;
        };
        c.that(
            &format!("output.segment('{symbol}').PF_R"),
            "readable",
            seg.readable(),
            flags_string(seg.flags),
        );
        if want_write {
            c.that(
                &format!("output.segment('{symbol}').PF_W"),
                "writable, because the program stores to it",
                seg.writable(),
                flags_string(seg.flags),
            );
        }
        if want_exec {
            c.that(
                &format!("output.segment('{symbol}').PF_X"),
                "executable, because the processor fetches from it",
                seg.executable(),
                flags_string(seg.flags),
            );
        }
        if !want_exec && seg.executable() {
            c.note(format!(
                "'{symbol}' landed in an executable segment; merging read-only data into \
                 .text is allowed, merging writable data into it is not, and the W^X check \
                 covers that"
            ));
        }
    }
    if !c.ok() {
        c.block("output program headers", exe.program_header_table());
        c.block("output section headers", exe.section_header_table());
    }
    c.finish()?;
    ctx.expect_output(&linked, "all\n", 17)?;
    Ok(())
});

/// No loadable segment may carry `PF_W` and `PF_X` at once.
fn assert_no_wx(exe: &Elf) -> Result<(), Failure> {
    let mut c = Check::new("W^X: whether any loadable segment is both writable and executable");
    for s in exe.loads() {
        c.that(
            &format!("output.segment[{}].p_flags", s.index),
            "not PF_W and PF_X together — a linker that merged .data into the text segment \
             hands every buffer overflow a place to put its shellcode",
            !(s.writable() && s.executable()),
            s.describe(),
        );
    }
    if !c.ok() {
        c.block("output program headers", exe.program_header_table());
        c.note(
            "the fix is to lay writable sections out on their own pages and give them their \
             own PT_LOAD; permissions are enforced per page, so sharing a page means sharing \
             the flags",
        );
    }
    c.finish()
}

/// An object whose `.data` holds a four-byte counter: the program doubles it in place,
/// reloads it and exits with the result. It cannot work unless `.data` is writable.
fn counter_program(initial: u32) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.mov_r32_rip(Reg::Rax, "counter", 0);
    code.add_r32_r32(Reg::Rax, Reg::Rax);
    code.mov_rip_r32("counter", Reg::Rax, 0);
    code.mov_r32_imm32(Reg::Rax, 0);
    code.mov_r32_rip(Reg::Rax, "counter", 0);
    code.sys_exit_eax();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::data(".data", initial.to_le_bytes().to_vec()).align(4))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("counter", ".data", 0).object(4)),
    )
}

/// An object that stores `value` into `.bss`, reads it back and exits with it.
fn bss_writer(value: u32) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.mov_r32_imm32(Reg::Rax, value);
    code.mov_rip_r32("scratch", Reg::Rax, 0);
    code.mov_r32_imm32(Reg::Rax, 0);
    code.mov_r32_rip(Reg::Rax, "scratch", 0);
    code.sys_exit_eax();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::bss(".bss", 64).align(16))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("scratch", ".bss", 0).object(64)),
    )
}

/// One object with all four kinds of allocated section: executable code, read-only data,
/// writable initialised data and zero-filled `.bss`.
///
/// `_start` prints the `.rodata` string, loads the `.data` word, adds the (zero) `.bss`
/// byte to it and exits with the sum — so every one of the four has to be mapped with the
/// right permission for the program to reach its exit syscall.
fn every_kind_of_section(message: &str, status: u32) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "message", message.len() as u32);
    code.mov_r32_rip(Reg::Rax, "counter", 0);
    code.mov_r32_imm32(Reg::Rdx, 0);
    code.movzx_r32_byte_rip(Reg::Rdx, "scratch", 0);
    code.add_r32_r32(Reg::Rax, Reg::Rdx);
    code.mov_rip_r32("scratch", Reg::Rax, 0);
    code.mov_r32_imm32(Reg::Rax, 0);
    code.movzx_r32_byte_rip(Reg::Rax, "scratch", 0);
    code.sys_exit_eax();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .section(SectionSpec::data(".data", status.to_le_bytes().to_vec()).align(4))
            .section(SectionSpec::bss(".bss", 64).align(16))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::local("message", ".rodata", 0).object(message.len() as u64))
            .symbol(SymbolSpec::global("counter", ".data", 0).object(4))
            .symbol(SymbolSpec::global("scratch", ".bss", 0).object(64)),
    )
}

/// Worked examples: the flags the MMU actually enforces.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "Code, constants, data and bss in one object",
            "ld -o prog all.o",
            || every_kind_of_section("all\n", 17).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "all.o: .text (ALLOC|EXECINSTR), .rodata (ALLOC), .data (ALLOC|WRITE) holding the \
             little-endian word 17, and a 64-byte .bss (ALLOC|WRITE, SHT_NOBITS)",
        )
        .response(
            "PT_LOADs whose p_flags give .text PF_R|PF_X, .data and .bss PF_R|PF_W, and \
             .rodata at least PF_R — and no single PT_LOAD carrying PF_W and PF_X together",
        )
        .note(
            "The program stores into .bss and reads it back, so a read-only data segment is \
             not a style problem: the binary dies with SIGSEGV before it reaches its exit \
             syscall.",
        )
        .runs("all\n", 17),
        ExampleSpec::object(
            "A counter that has to be writable",
            "ld -o prog counter.o",
            || counter_program(37).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "counter.o: .data holds the four bytes `25 00 00 00` (37); .text loads them, \
             doubles the value, stores it back, reloads it and exits with it",
        )
        .response(
            "A writable, non-executable PT_LOAD covering .data, the four bytes of the \
             initialiser present in the file at the right offset, and an exit status of 74",
        )
        .note(
            "This is the cheapest test there is for `p_flags`: the store is one instruction, \
             and the difference between PF_W and no PF_W is the difference between exit 74 \
             and signal 11.",
        )
        .runs("", 74),
    ]
}
