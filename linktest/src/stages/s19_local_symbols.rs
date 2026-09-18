//! Stage 19 — Local symbols do not clash.
//!
//! Every `static` variable, every string literal, every compiler-generated label is an
//! `STB_LOCAL` symbol, and half the objects in a real link use the same handful of names for
//! them. A local is scoped to the object it came from: it never collides with anything, it
//! never satisfies anyone else's reference, and it is never a candidate when a global of the
//! same name is on the table. That means the symbol table the linker builds is really two
//! tables — one global, and one per input.

use crate::asm::{Code, Reg, STDOUT};
use crate::assert::{Check, Failure};
use crate::elf::write::{ObjectBuilder, SectionSpec, SymbolSpec};
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 19,
        slug: "local_symbols",
        name: "Local symbols do not clash",
        ext: false,
        hints: &[
            "Resolve a relocation against a local symbol inside the object that owns it: take \
             the symbol index straight out of that object's own .symtab and never consult the \
             global table",
            "The binding half of st_info decides everything; sh_info of .symtab tells you \
             where the locals stop, and locals always come first",
            "A local never enters the global table, so two objects may each have a local \
             `helper` and a third object's reference to `helper` must still find the global \
             one — or fail as undefined if there is none",
            "Section symbols (STT_SECTION, empty name) are locals too: `.rodata` in five \
             objects means five distinct section symbols with five distinct addresses",
        ],
        examples: stage_examples,
        tests: vec![
            Test::new(
                "two objects with a local of the same name each use their own",
                two_locals_same_name,
            ),
            Test::new(
                "changing a local's value changes only its own object's answer",
                local_values_are_independent,
            ),
            Test::new(
                "a local is invisible outside its object, so a global of that name wins",
                local_does_not_shadow_the_global,
            ),
            Test::new(
                "a reference to a name that is only ever local is undefined",
                local_does_not_satisfy_a_reference,
            ),
            Test::new(
                "section symbols of identically named sections do not clash",
                section_symbols_do_not_clash,
            ),
            Test::new(
                "locals link cleanly where the same names as globals would collide",
                locals_where_globals_would_collide,
            ),
            Test::new(
                "the control program still satisfies every layout invariant",
                control_layout,
            ),
        ],
    }
}

link_test!(two_locals_same_name, |ctx| {
    // a.o's local `lv` is 10, b.o's is 20; _start adds its own to whatever `second` returns.
    let a = main_adding_local(10, "second")?;
    let b = function_returning_local("second", 20)?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("two objects with a local named 'lv'"),
    )?;
    assert_runnable_layout(&linked)?;
    // 20 from b.o's local, plus 10 from a.o's: each object read its own four bytes.
    ctx.expect_output(&linked, "locals\n", 30)?;
    Ok(())
});

link_test!(local_values_are_independent, |ctx| {
    for (mine, theirs) in [(10u32, 20u32), (3, 4), (100, 7)] {
        let a = main_adding_local(mine, "second")?;
        let b = function_returning_local("second", theirs)?;
        let linked = ctx.link_ok(
            &Link::new()
                .object("a.o", a)
                .object("b.o", b)
                .out(&format!("prog_{mine}_{theirs}"))
                .label(&format!("locals {mine} and {theirs}")),
        )?;
        ctx.expect_output(&linked, "locals\n", (mine + theirs) as i32)?;
    }
    let mut c = Check::new("that each object's local kept its own value");
    c.note(
        "three links of the same shape with different constants: the exit status tracks both \
         locals, so neither object read the other's",
    );
    c.finish()
});

link_test!(local_does_not_shadow_the_global, |ctx| {
    // a.o has a *local* `shared` worth 7. b.o has a *global* `shared` worth 55. c.o only
    // references `shared`, so it must find b.o's global — a.o's local is not a candidate.
    let a = main_with_local_named("shared", 7, "fetch")?;
    let b = global_word("shared", 55)?;
    let c = function_returning_global("fetch", "shared")?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .object("c.o", c)
            .label("a local and a global both named 'shared'"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "shared\n", 55)?;
    let mut check = Check::new("which definition of 'shared' the third object reached");
    check.note(
        "exit 55 is b.o's global; exit 7 would mean a.o's local had leaked into the global \
         table",
    );
    check.finish()
});

link_test!(local_does_not_satisfy_a_reference, |ctx| {
    // b.o defines `only_local` — but as a local, so as far as a.o is concerned it does not
    // exist at all.
    let a = caller("never printed\n", "only_local")?;
    let b = local_only_function("only_local", 9)?;
    let run = ctx.link_fails(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("a reference to a name that is local elsewhere"),
        "linking a reference that only a local symbol could satisfy",
    )?;
    let mut c = Check::new("the diagnostic for a reference no global satisfies");
    c.mentions("linker.stderr", "only_local", &run.diagnostics());
    if !c.ok() {
        c.note(
            "a local definition is invisible across object boundaries; finding one and using \
             it would link programs that should not link",
        );
        c.block("linker command", run.output.command_line());
    }
    c.finish()
});

link_test!(section_symbols_do_not_clash, |ctx| {
    // Both objects have a section called .rodata and reach into it through the *section*
    // symbol, which is a local with an empty name. Two sections, two symbols, two addresses.
    let a = print_via_section_then_call("A\n", "part_b")?;
    let b = print_via_section_and_return("part_b", "B\n")?;
    let linked = ctx.link_ok(
        &Link::new()
            .object("a.o", a)
            .object("b.o", b)
            .label("two objects whose .rodata section symbols share a name"),
    )?;
    assert_runnable_layout(&linked)?;
    ctx.expect_output(&linked, "A\nB\n", 0)?;
    let mut c = Check::new("what happened to the two .rodata inputs");
    let rodatas: Vec<String> = linked
        .elf
        .sections_named(".rodata")
        .map(|s| format!("0x{:x}+0x{:x}", s.addr, s.size))
        .collect();
    c.observe("output.sections['.rodata']", rodatas.join(", "));
    c.note(
        "whether the linker merges the two inputs into one output .rodata or keeps them \
         apart is its business; what is not is that each object's section symbol resolves to \
         its own bytes",
    );
    c.finish()
});

link_test!(locals_where_globals_would_collide, |ctx| {
    // The contrast: the same two objects, the same name, one bit of st_info different.
    let main = print_and_exit("contrast\n", 0)?;
    let with_locals = ctx.link_ok(
        &Link::new()
            .object("main.o", main.clone())
            .object("a.o", named_word("twin", 1, false)?)
            .object("b.o", named_word("twin", 2, false)?)
            .label("two local definitions of 'twin'"),
    )?;
    ctx.expect_output(&with_locals, "contrast\n", 0)?;

    let run = ctx.link(
        &Link::new()
            .object("main.o", main)
            .object("a.o", named_word("twin", 1, true)?)
            .object("b.o", named_word("twin", 2, true)?)
            .out("global_twin")
            .label("the same two definitions made global"),
    )?;
    let mut c = Check::new("that the binding is what decides the collision");
    c.that(
        "linker.exit_status",
        "a non-zero exit — as globals the two definitions collide",
        !run.output.success(),
        run.output.status_line(),
    );
    if run.output.success() {
        c.block("linker command", run.output.command_line());
    } else {
        c.mentions("linker.stderr", "twin", &run.diagnostics());
    }
    c.finish()
});

link_test!(control_layout, |ctx| {
    let a = main_adding_local(4, "second")?;
    let b = function_returning_local("second", 8)?;
    let linked = ctx.link_ok(&Link::new().object("a.o", a).object("b.o", b))?;
    assert_runnable_layout(&linked)?;
    assert_entry_is(&linked, DEFAULT_ENTRY)?;
    ctx.expect_output(&linked, "locals\n", 12)?;
    Ok(())
});

// ---------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------

/// `_start`: print `locals\n`, call `callee`, add this object's own local `lv`, exit.
fn main_adding_local(local_value: u32, callee: &str) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "message", 7);
    code.call(callee);
    code.add_r32_rip(Reg::Rax, "lv", 0);
    code.sys_exit_eax();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"locals\n".to_vec()))
            .section(SectionSpec::data(".data", local_value.to_le_bytes().to_vec()).align(4))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::local("message", ".rodata", 0).object(7))
            .symbol(SymbolSpec::local("lv", ".data", 0).object(4))
            .symbol(SymbolSpec::undefined(callee)),
    )
}

/// A global function that returns *its own* object's local `lv`.
fn function_returning_local(name: &str, local_value: u32) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.mov_r32_rip(Reg::Rax, "lv", 0);
    code.ret();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::data(".data", local_value.to_le_bytes().to_vec()).align(4))
            .symbol(SymbolSpec::global(name, ".text", 0).func())
            .symbol(SymbolSpec::local("lv", ".data", 0).object(4)),
    )
}

/// `_start`: print `shared\n`, call `callee`, exit with what it returned. This object also
/// carries a *local* symbol called `name`, which must not be visible to anyone else.
fn main_with_local_named(name: &str, value: u32, callee: &str) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "message", 7);
    code.call(callee);
    code.sys_exit_eax();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", b"shared\n".to_vec()))
            .section(SectionSpec::data(".data", value.to_le_bytes().to_vec()).align(4))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::local("message", ".rodata", 0).object(7))
            .symbol(SymbolSpec::local(name, ".data", 0).object(4))
            .symbol(SymbolSpec::undefined(callee)),
    )
}

/// An object whose `.data` holds one global 32-bit word.
fn global_word(name: &str, value: u32) -> Result<Vec<u8>, Failure> {
    build(
        ObjectBuilder::new()
            .section(SectionSpec::data(".data", value.to_le_bytes().to_vec()).align(4))
            .symbol(SymbolSpec::global(name, ".data", 0).object(4)),
    )
}

/// The same four bytes under a name that is local or global depending on `global`.
fn named_word(name: &str, value: u32, global: bool) -> Result<Vec<u8>, Failure> {
    let sym = if global {
        SymbolSpec::global(name, ".data", 0).object(4)
    } else {
        SymbolSpec::local(name, ".data", 0).object(4)
    };
    build(
        ObjectBuilder::new()
            .section(SectionSpec::data(".data", value.to_le_bytes().to_vec()).align(4))
            .symbol(sym),
    )
}

/// A global function that loads a *global* word defined in some other object.
fn function_returning_global(name: &str, global: &str) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.mov_r32_rip(Reg::Rax, global, 0);
    code.ret();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::global(name, ".text", 0).func())
            .symbol(SymbolSpec::undefined(global)),
    )
}

/// An object that defines `name` as a *local* function — a definition nobody else can see.
fn local_only_function(name: &str, value: u32) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.mov_r32_imm32(Reg::Rax, value);
    code.ret();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .symbol(SymbolSpec::local(name, ".text", 0).func()),
    )
}

/// `_start`: write this object's `.rodata` through its *section* symbol, call `callee`, exit 0.
fn print_via_section_then_call(message: &str, callee: &str) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write_section(STDOUT, ".rodata", 0, message.len() as u32);
    code.call(callee);
    code.sys_exit(0);
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(DEFAULT_ENTRY, ".text", 0).func())
            .symbol(SymbolSpec::undefined(callee)),
    )
}

/// A global function that writes *its own* `.rodata` through its own section symbol.
fn print_via_section_and_return(name: &str, message: &str) -> Result<Vec<u8>, Failure> {
    let mut code = Code::new();
    code.sys_write_section(STDOUT, ".rodata", 0, message.len() as u32);
    code.ret();
    build(
        ObjectBuilder::new()
            .section(text_of(&code))
            .section(SectionSpec::rodata(".rodata", message.as_bytes().to_vec()))
            .symbol(SymbolSpec::global(name, ".text", 0).func()),
    )
}

/// Worked examples: the two objects that both call their four bytes `lv`.
fn stage_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::object(
            "An object with a local the whole program shares a name for",
            "ld -o prog a.o b.o",
            || main_adding_local(10, "second").map_err(|f| f.messages.join("; ")),
        )
        .request(
            "a.o: .data holds the four bytes 10, and the symbol `lv` that names them has \
             STB_LOCAL in the top nibble of st_info. Its .rela.text entry for `add eax, \
             [rip+lv]` points at that local symbol's index *in this object's* .symtab",
        )
        .response(
            "The relocation is resolved against a.o's own `lv`: S is the address a.o's .data \
             got, plus st_value 0. b.o's identically named `lv` is a different symbol in a \
             different table and never comes into it",
        )
        .note(
            "Look at .symtab's sh_info: it is the index of the first non-local symbol. \
             Everything before it is scoped to this file and must be resolved from this \
             file's table alone.",
        )
        .runs("locals\n", 30),
        ExampleSpec::object(
            "The other object, with its own 'lv'",
            "ld -o prog a.o b.o",
            || function_returning_local("second", 20).map_err(|f| f.messages.join("; ")),
        )
        .request(
            "b.o: one global, `second`, and one local, `lv`, worth 20. The two objects agree \
             on nothing but the name of the function they share",
        )
        .response(
            "Both .data sections are allocated, at different addresses, and each object's \
             `mov`/`add` reaches its own. The program exits 30 — 20 + 10 — which no single \
             `lv` could produce",
        )
        .note(
            "If your global table is keyed by name alone and you insert locals into it, this \
             link either reports a bogus duplicate or makes both objects read the same four \
             bytes.",
        )
        .runs("locals\n", 30),
    ]
}
