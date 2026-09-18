//! Stage 25 — R_X86_64_64.
//!
//! The pointer relocation. Wherever a program stores the address of something — a jump
//! table, a `static char *p = "hi";`, a vtable, a list of function pointers — the compiler
//! leaves eight zero bytes and an `R_X86_64_64` saying "put `S + A` here". There is no
//! program counter in the arithmetic and no range to check: the field is as wide as the
//! address space, so the only way to get it wrong is to compute `S` wrongly or to write the
//! bytes in the wrong order.

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
        number: 25,
        slug: "absolute_64",
        name: "R_X86_64_64",
        ext: false,
        hints: &[
            "R_X86_64_64 is the easy one: store the 64-bit value S + A, little-endian, into \
             the eight bytes at the relocation's site — no program counter, no truncation, \
             no range check",
            "The site is in a data section, not in .text, so the patch loop has to run over \
             every relocated section of every input and not just over the code",
            "Patch the bytes of the output image after the section has been copied into it: \
             patching the input object's buffer and copying afterwards works only until two \
             inputs share a section name",
            "A relocation inside .rodata is still applied — read-only is a property of the \
             final mapping, not a reason to leave the word zero",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "a .data pointer to .rodata is loaded and used at run time",
                pointer_in_data,
            ),
            Test::new(
                "the eight bytes hold S + A and nothing around them moves",
                exactly_eight_bytes,
            ),
            Test::new(
                "an array of three pointers is patched entry by entry",
                array_of_pointers,
            ),
            Test::new(
                "a pointer with a non-zero addend points into the middle of the object",
                pointer_with_addend,
            ),
            Test::new(
                "a pointer to a symbol defined in another object",
                pointer_across_objects,
            ),
            Test::new(
                "an R_X86_64_64 inside .rodata is relocated too",
                pointer_in_rodata,
            ),
            Test::new(
                "a pointer to a function holds that function's address",
                function_pointer,
            ),
            Test::new(
                "an R_X86_64_64 against a symbol nothing defines names the symbol",
                undefined_pointer_target,
            ),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------

/// `_start` loads the eight bytes at `ptr` into rsi, writes `len` bytes through it and
/// exits with `status`.
fn load_and_write(ptr: &str, len: u32, status: u32) -> Code {
    let mut code = Code::new();
    code.mov_r64_rip(Reg::Rsi, ptr, R_X86_64_PC32, -4);
    code.sys_write_rsi(STDOUT, len);
    code.sys_exit(status);
    code
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

link_test!(pointer_in_data, |ctx| {
    let obj = pointer_program("through a pointer\n", 0)?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).label("a .data pointer"))?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "through a pointer\n", 0)?;
    let site = linked.address_of("ptr")?;
    let target = linked.address_of("message")?;
    let mut c = Check::new("the eight bytes of the .data pointer");
    assert_abs64(&mut c, &linked.elf, "output.data[ptr]", site, target, 0);
    if !c.ok() {
        c.note(
            "the program printed nothing or died because rsi held whatever was left in the \
             word — a zero here means the relocation was never applied",
        );
        c.block("linker command", linked.run.output.command_line());
    }
    c.finish()
});

link_test!(exactly_eight_bytes, |ctx| {
    let message = "eight\n";
    let mut data = vec![0x11u8; 8];
    data.extend_from_slice(&[0u8; 8]);
    data.extend_from_slice(&[0x22u8; 8]);
    let code = load_and_write("ptr", message.len() as u32, 0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .section(SectionSpec::data(".data", data).align(8).reloc(Reloc::sym(
                8,
                "message",
                R_X86_64_64,
                0,
            )))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("message", ".rodata", 0).object(message.len() as u64))
            .symbol(SymbolSpec::global("before", ".data", 0).object(8))
            .symbol(SymbolSpec::global("ptr", ".data", 8).object(8))
            .symbol(SymbolSpec::global("after", ".data", 16).object(8)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("a pointer between two guard words"),
    )?;
    ctx.expect_output(&linked, message, 0)?;
    let mut c = Check::new("the patched word and its neighbours");
    assert_abs64(
        &mut c,
        &linked.elf,
        "output.data[ptr]",
        linked.address_of("ptr")?,
        linked.address_of("message")?,
        0,
    );
    let before = linked.address_of("before")?;
    let after = linked.address_of("after")?;
    match (
        linked.elf.read_at_vaddr(before, 8),
        linked.elf.read_at_vaddr(after, 8),
    ) {
        (Ok(b), Ok(a)) => {
            c.bytes_eq("output.data[before]", &[0x11u8; 8], b);
            c.bytes_eq("output.data[after]", &[0x22u8; 8], a);
        }
        _ => {
            c.that(
                "output.data",
                "readable guard words on both sides of the pointer",
                false,
                format!("before 0x{before:x}, after 0x{after:x}"),
            );
        }
    }
    c.note(
        "an R_X86_64_64 owns exactly eight bytes; a linker that writes a wider store, or \
         that writes the value big-endian, corrupts whatever the compiler put next to it",
    );
    c.finish()
});

link_test!(array_of_pointers, |ctx| {
    // Three pointers into one pool, printed in order: "ab", " c", "d ".
    let pool = b"ab cd ef".to_vec();
    let mut code = Code::new();
    for i in 0..3i64 {
        code.mov_r64_rip(Reg::Rsi, "arr", R_X86_64_PC32, -4 + i * 8);
        code.sys_write_rsi(STDOUT, 2);
    }
    code.sys_exit(0);
    let relocs = (0..3u64).map(|i| Reloc::sym(i * 8, "pool", R_X86_64_64, (i * 2) as i64));
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", pool))
            .section(
                SectionSpec::data(".data", vec![0u8; 24])
                    .align(8)
                    .relocs(relocs),
            )
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("pool", ".rodata", 0).object(8))
            .symbol(SymbolSpec::global("arr", ".data", 0).object(24)),
    )?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).label("an array of pointers"))?;
    ctx.expect_output(&linked, "ab cd ", 0)?;
    let arr = linked.address_of("arr")?;
    let pool_addr = linked.address_of("pool")?;
    let mut c = Check::new("each of the three pointers");
    for i in 0..3u64 {
        assert_abs64(
            &mut c,
            &linked.elf,
            &format!("output.data[arr][{i}]"),
            arr + i * 8,
            pool_addr,
            (i * 2) as i64,
        );
    }
    c.finish()
});

link_test!(pointer_with_addend, |ctx| {
    let code = load_and_write("ptr", 3, 0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"abcXYZ".to_vec()))
            .section(
                SectionSpec::data(".data", vec![0u8; 8])
                    .align(8)
                    .reloc(Reloc::sym(0, "blob", R_X86_64_64, 3)),
            )
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("blob", ".rodata", 0).object(6))
            .symbol(SymbolSpec::global("ptr", ".data", 0).object(8)),
    )?;
    let linked = ctx.link_ok(&Link::new().object("a.o", obj).label("a pointer with A = 3"))?;
    ctx.expect_output(&linked, "XYZ", 0)?;
    let mut c = Check::new("a pointer whose addend moves it three bytes on");
    assert_abs64(
        &mut c,
        &linked.elf,
        "output.data[ptr]",
        linked.address_of("ptr")?,
        linked.address_of("blob")?,
        3,
    );
    c.finish()
});

link_test!(pointer_across_objects, |ctx| {
    let code = load_and_write("ptr", 3, 0);
    let a = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(
                SectionSpec::data(".data", vec![0u8; 8])
                    .align(8)
                    .reloc(Reloc::sym(0, "faraway", R_X86_64_64, 0)),
            )
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("ptr", ".data", 0).object(8))
            .symbol(SymbolSpec::undefined("faraway")),
    )?;
    let b = build(
        ObjectBuilder::new()
            .section(SectionSpec::rodata(".rodata", b"far\n".to_vec()))
            .symbol(SymbolSpec::global("faraway", ".rodata", 0).object(4)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("a pointer resolved from the second object"),
    )?;
    ctx.expect_output(&linked, "far", 0)?;
    let mut c = Check::new("a pointer whose target lives in another input");
    assert_abs64(
        &mut c,
        &linked.elf,
        "output.data[ptr]",
        linked.address_of("ptr")?,
        linked.address_of("faraway")?,
        0,
    );
    c.finish()
});

link_test!(pointer_in_rodata, |ctx| {
    // The relocated word is inside .rodata itself: read-only at run time, still patched.
    let mut rodata = b"ro!\n".to_vec();
    rodata.resize(8, 0);
    rodata.extend_from_slice(&[0u8; 8]);
    let code = load_and_write("roptr", 4, 0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(
                SectionSpec::rodata(".rodata", rodata)
                    .align(8)
                    .reloc(Reloc::sym(8, "text_of_it", R_X86_64_64, 0)),
            )
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("text_of_it", ".rodata", 0).object(4))
            .symbol(SymbolSpec::global("roptr", ".rodata", 8).object(8)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("a relocated word inside .rodata"),
    )?;
    ctx.expect_output(&linked, "ro!\n", 0)?;
    let mut c = Check::new("a pointer that lives in a read-only section");
    assert_abs64(
        &mut c,
        &linked.elf,
        "output.rodata[roptr]",
        linked.address_of("roptr")?,
        linked.address_of("text_of_it")?,
        0,
    );
    c.note(
        "a static link resolves this at link time, so the word being in a read-only segment \
         costs nothing; only a dynamic link would need a run-time relative relocation here",
    );
    c.finish()
});

link_test!(function_pointer, |ctx| {
    let mut fun = Code::new();
    fun.mov_r32_imm32(Reg::Rax, 19);
    fun.ret();
    let fun_len = fun.len();
    let mut code = Code::new();
    code.mov_r64_rip(Reg::Rax, "fp", R_X86_64_PC32, -4);
    code.raw(&[0xff, 0xd0]); // call rax
    code.sys_exit_eax();
    let start_len = code.len();
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
                SymbolSpec::global("fun", ".text", start_len)
                    .func()
                    .size(fun_len),
            )
            .symbol(SymbolSpec::global("fp", ".data", 0).object(8)),
    )?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", obj)
            .label("a function pointer in .data"),
    )?;
    ctx.expect_output(&linked, "", 19)?;
    let mut c = Check::new("a .data word holding the address of a function");
    let fp = linked.address_of("fp")?;
    let fun_addr = linked.address_of("fun")?;
    assert_abs64(&mut c, &linked.elf, "output.data[fp]", fp, fun_addr, 0);
    c.that(
        "output.data[fp]",
        "an address inside an executable PT_LOAD — the program calls through it",
        linked
            .elf
            .segment_at(fun_addr)
            .map(|s| s.executable())
            .unwrap_or(false),
        segment_summary(&linked.elf, fun_addr),
    );
    c.finish()
});

link_test!(undefined_pointer_target, |ctx| {
    let code = load_and_write("ptr", 3, 0);
    let obj = build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(
                SectionSpec::data(".data", vec![0u8; 8])
                    .align(8)
                    .reloc(Reloc::sym(0, "missing_target", R_X86_64_64, 0)),
            )
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::global("ptr", ".data", 0).object(8))
            .symbol(SymbolSpec::undefined("missing_target")),
    )?;
    let run = ctx.link_fails(
        &Link::new().object("a.o", obj).label("a pointer to nothing"),
        "linking an R_X86_64_64 whose symbol nothing defines",
    )?;
    let mut c = Check::new("the diagnostic for an unresolvable pointer");
    c.mentions("linker.stderr", "missing_target", &run.output.stderr);
    c.note(
        "the relocation is in .data rather than .text, which is exactly why it is easy to \
         miss: a linker that only checks the symbols its code references accepts this one",
    );
    c.finish()
});

/// Worked examples: the pointer in `.data` and the array of them.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object("A pointer in .data", "ld -o prog pointer.o", || {
            pointer_program("through a pointer\n", 0).map_err(|f| f.messages.join("; "))
        })
        .request(
            "pointer.o: .rodata holds the string, .data holds eight zero bytes with an \
             R_X86_64_64 naming `message' and addend 0, and .text loads those eight bytes \
             into rsi and calls write(2) with them",
        )
        .response(
            "The eight bytes of .data hold the final address of `message', little-endian, \
             and the program prints the string. No other byte of .data changes",
        )
        .note(
            "This is the relocation that catches a linker which patches the input buffer \
             rather than the output image: the value is right in the .o that was read and \
             zero in the file that was written.",
        )
        .runs("through a pointer\n", 0),
        ExampleSpec::text("Why there is no range check", "ld -o prog table.o")
            .request(
                "A jump table of eight-byte entries, each an R_X86_64_64 against a label with a \
             different addend",
            )
            .response(
                "Each entry holds S + A. The field is 64 bits wide and the address space is 64 \
             bits wide, so unlike R_X86_64_32 and R_X86_64_PC32 there is no value that can \
             fail to fit and nothing to diagnose",
            )
            .note(
                "Addends on pointers are how a compiler writes `&array[3]` or a pointer to a \
             struct field, so they are common rather than exotic.",
            ),
    ]
}
