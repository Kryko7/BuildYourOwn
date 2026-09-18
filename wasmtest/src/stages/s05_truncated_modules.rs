//! Stage 05 — files that stop early, and sizes that promise bytes nobody wrote.
//!
//! The rule is easy to say and easy to get wrong in a hundred places: **every** read of a
//! module has to be able to fail. A decoder written against a slice that is always long
//! enough passes every test in stage 01 to 04 and then panics, hangs or runs half a program
//! the first time it meets a file that was cut short — which is the normal state of a file
//! that came down a network or off a disk that filled up.
//!
//! The sweep truncates a good module at every single byte offset and requires that the
//! runtime refuses each one, cleanly: a non-zero exit, nothing on stdout, and no signal. The
//! harness turns a signal into a `runtime crash` failure of its own, so a segfault here is
//! reported as exactly that rather than as a wrong answer.
//!
//! One expectation had to be corrected against the reference, and it is the interesting part
//! of the stage. **Not every prefix is malformed.** A module is a sequence of sections, so a
//! prefix that happens to stop on a section boundary can be a perfectly good, smaller module:
//! the first eight bytes are the empty module, the header plus the type section is a module
//! with one unused type, and everything up to the data section is the whole program minus its
//! data. `wasmtime` accepts all three. The sweep therefore asserts something sharper than
//! "every truncation fails": it asserts that the offsets the runtime accepts are **exactly**
//! those three, named in advance, and that every other offset in the file is refused. A
//! runtime that accepts one more prefix than that is reading past the end of something.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::stages::{expect, expect_rejected, expect_start_rejected, Stage, Test};
use crate::wasm::{section, sleb, uleb, vector, wasm_name, Enc, Module};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 5,
        slug: "truncated_modules",
        name: "Truncated modules and sizes that lie",
        ext: false,
        hints: &[
            "Make every read of the module bounds-checked and make the failure a value you return, not a panic: a short file is ordinary input, not a bug in your program",
            "A section's size and a function body's size are promises about bytes that may not be there — compare them against what is left of the file before you slice",
            "A body's size is also its end: parse the instructions inside that window and refuse a body whose `end` falls outside it, rather than reading on into the next one",
            "The code section's vector count and the function section's must agree, and both must agree with how many bodies the file actually holds",
        ],
        examples,
        tests: vec![
            Test::new("a file cut off inside the magic or the version is refused", cut_header),
            Test::new(
                "every truncation after the header is refused, except the prefixes that are whole modules",
                truncation_sweep,
            )
            .min_timeout_ms(30_000),
            Test::new("a module one byte short of its last end is refused", one_byte_short),
            Test::new("a section size promising more bytes than the file holds is refused", section_size_lies),
            Test::new("a function body whose size stops before its end is refused", body_size_too_small),
            Test::new("a function body whose size runs past the code section is refused", body_size_too_big),
            Test::new("a code section that declares more bodies than follow it is refused", body_count_lies),
            Test::new("bytes after the last section are not ignored", trailing_bytes),
        ],
    }
}

/// The type section's body: one type, `() -> i32`.
fn type_body() -> Vec<u8> {
    vector(&[vec![0x60, 0x00, 0x01, 0x7f]])
}

/// The function section's body: `n` functions, all of type 0.
fn function_body(n: usize) -> Vec<u8> {
    vector(&vec![uleb(0); n])
}

/// The export section's body: `f` → function 0.
fn export_body() -> Vec<u8> {
    let mut entry = wasm_name("f");
    entry.push(0x00);
    entry.extend_from_slice(&uleb(0));
    vector(&[entry])
}

/// One code entry: its size, no locals, `i32.const v`, `end`.
///
/// `size_delta` is added to the size field without changing the bytes, which is how the two
/// lying-body tests are built.
fn code_entry(v: i32, size_delta: i64) -> Vec<u8> {
    let mut inner = uleb(0);
    inner.push(0x41);
    inner.extend_from_slice(&sleb(v as i64));
    inner.push(0x0b);
    let declared = (inner.len() as i64 + size_delta).max(0) as u64;
    let mut out = uleb(declared);
    out.extend_from_slice(&inner);
    out
}

/// The module the sweep cuts up: six sections, the last of them a data section that the
/// program does not need, so the prefix ending just before it is itself a whole module.
fn good() -> Module {
    let mut e = Enc::with_header();
    e.section(section::TYPE, &type_body());
    e.section(section::FUNCTION, &function_body(1));
    e.section(section::MEMORY, &vector(&[vec![0x00, 0x01]]));
    e.section(section::EXPORT, &export_body());
    e.section(section::CODE, &vector(&[code_entry(7, 0)]));
    let mut seg = vec![0x00, 0x41, 0x00, 0x0b];
    seg.extend_from_slice(&uleb(5));
    seg.extend_from_slice(b"hello");
    e.section(section::DATA, &vector(&[seg]));
    e.finish("truncation-base")
}

/// Where the section with this id starts in `m`.
fn section_at(m: &Module, id: u8) -> usize {
    m.anns
        .iter()
        .find(|a| a.field == format!("section[{id}].id"))
        .map(|a| a.offset)
        .unwrap_or(m.bytes.len())
}

/// The prefix lengths of `good()` that are themselves complete, valid modules.
///
/// Each one stops exactly where a section begins, and what came before it needs nothing that
/// follows: the bare header, the header plus the type section, and everything but the data.
fn whole_module_prefixes(m: &Module) -> Vec<usize> {
    vec![
        section_at(m, section::TYPE),
        section_at(m, section::FUNCTION),
        section_at(m, section::DATA),
    ]
}

wasm_test!(cut_header, |ctx| {
    // One to seven bytes: inside `\0asm`, or inside the four version bytes. None of them is a
    // module, and none of them may be padded out with zeroes and read anyway.
    let m = good();
    for n in 1..8usize {
        let cut = m.truncated(n);
        expect_start_rejected(
            ctx,
            &cut,
            &format!("{n} bytes is short of the eight-byte header"),
        )?;
    }
    ctx.note("seven truncations inside the header, all refused");
    Ok(())
});

wasm_test!(truncation_sweep, |ctx| {
    // Every offset from the end of the header to one byte short of the whole file. The
    // invocation is the WASI command path rather than `--invoke f`, because a prefix that
    // stops before the export section has no `f` to name and would fail for that reason
    // instead of for the bytes.
    let m = good();
    let whole = whole_module_prefixes(&m);
    let mut accepted: Vec<usize> = Vec::new();
    let mut printed: Vec<usize> = Vec::new();
    let mut tried = 0usize;
    for n in 8..m.len() {
        let cut = m.truncated(n);
        let run = ctx.start(&cut, &[])?;
        tried += 1;
        if run.exit.success() {
            accepted.push(n);
        }
        if !run.stdout.is_empty() {
            printed.push(n);
        }
    }

    let run = ctx.start(&m, &[])?;
    let mut c = Check::new("what a truncated module is allowed to do", &run);
    c.module(&m);
    c.eq("accepted prefixes", whole.clone(), accepted.clone());
    c.that(
        "stdout",
        "nothing, at every truncation: a module that does not decode never runs",
        printed.is_empty(),
        format!("output at offsets {printed:?}"),
    );
    c.note(
        "the three accepted prefixes are the ones that are complete modules in their own \
         right: the bare header, the header plus the type section, and everything before the \
         data section",
    );
    c.finish()?;
    ctx.note(format!(
        "{tried} truncations tried, {} refused, accepted only at offsets {accepted:?}",
        tried - accepted.len()
    ));
    Ok(())
});

wasm_test!(one_byte_short, |ctx| {
    // The last byte of this module is the last byte of the data segment. Losing it leaves a
    // data section whose vector promised five bytes and delivered four.
    let m = good();
    let cut = m.truncated(m.len() - 1);
    expect_start_rejected(
        ctx,
        &cut,
        "the data segment is one byte shorter than it says",
    )?;

    // And the same for a module that ends in a function body, where the missing byte is the
    // `end` opcode itself.
    let mut e = Enc::with_header();
    e.section(section::TYPE, &type_body());
    e.section(section::FUNCTION, &function_body(1));
    e.section(section::EXPORT, &export_body());
    e.section(section::CODE, &vector(&[code_entry(7, 0)]));
    let m = e.finish("ends-in-a-body");
    let cut = m.truncated(m.len() - 1);
    expect_rejected(ctx, &cut, "the body's terminating end is gone")?;
    Ok(())
});

wasm_test!(section_size_lies, |ctx| {
    // A size field is a promise about bytes that follow. Here the file ends first.
    let body = vector(&[code_entry(7, 0)]);
    let mut e = Enc::with_header();
    e.section(section::TYPE, &type_body());
    e.section(section::FUNCTION, &function_body(1));
    e.section(section::EXPORT, &export_body());
    e.section_with_size(
        section::CODE,
        &uleb(4096),
        &body,
        &format!(
            "4096, but only {} bytes follow, to the end of the file",
            body.len()
        ),
    );
    let m = e.finish("code-size-4096");
    expect_rejected(
        ctx,
        &m,
        "the code section promises 4096 bytes and the file ends",
    )?;

    // The same promise in the middle of the module, where the sections after it are real and
    // would decode perfectly well if the size had been honest.
    let types = type_body();
    let mut e = Enc::with_header();
    e.section_with_size(
        section::TYPE,
        &uleb(4096),
        &types,
        "4096, but the rest of the module is shorter than that",
    );
    e.section(section::FUNCTION, &function_body(1));
    e.section(section::EXPORT, &export_body());
    e.section(section::CODE, &vector(&[code_entry(7, 0)]));
    let m = e.finish("type-size-4096");
    expect_rejected(
        ctx,
        &m,
        "the type section swallows the rest of the file and more",
    )?;
    Ok(())
});

wasm_test!(body_size_too_small, |ctx| {
    // Two bodies. The first declares one byte fewer than it holds, so its `end` opcode falls
    // outside its own window — and straight into where the second body's size field should
    // be. A decoder that parses instructions until it meets an `end`, rather than until the
    // body's size runs out, never notices.
    let mut e = Enc::with_header();
    e.section(section::TYPE, &type_body());
    e.section(section::FUNCTION, &function_body(2));
    let mut exports = wasm_name("f");
    exports.push(0x00);
    exports.extend_from_slice(&uleb(0));
    let mut g = wasm_name("g");
    g.push(0x00);
    g.extend_from_slice(&uleb(1));
    e.section(section::EXPORT, &vector(&[exports, g]));
    e.section(
        section::CODE,
        &vector(&[code_entry(7, -1), code_entry(9, 0)]),
    );
    let m = e.finish("body-size-one-short");
    expect_rejected(ctx, &m, "the first body's end lands inside the second body")?;
    Ok(())
});

wasm_test!(body_size_too_big, |ctx| {
    // The other direction, and the one that reads past the end of the section: a single body
    // whose size field claims three more bytes than the code section has left.
    let mut e = Enc::with_header();
    e.section(section::TYPE, &type_body());
    e.section(section::FUNCTION, &function_body(1));
    e.section(section::EXPORT, &export_body());
    e.section(section::CODE, &vector(&[code_entry(7, 3)]));
    let m = e.finish("body-size-three-too-many");
    expect_rejected(
        ctx,
        &m,
        "the body claims three bytes more than the code section holds",
    )?;
    Ok(())
});

wasm_test!(body_count_lies, |ctx| {
    // The code section's vector says two and one body follows. The function section agrees
    // with the vector, so the only thing that can catch this is running out of bytes.
    let mut body = uleb(2);
    body.extend_from_slice(&code_entry(7, 0));
    let mut e = Enc::with_header();
    e.section(section::TYPE, &type_body());
    e.section(section::FUNCTION, &function_body(2));
    e.section(section::EXPORT, &export_body());
    e.section(section::CODE, &body);
    let m = e.finish("code-count-two-bodies-one");
    expect_rejected(
        ctx,
        &m,
        "the code vector says two and the section holds one body",
    )?;

    // And with the function section saying one, so the two counts disagree as well.
    let mut e = Enc::with_header();
    e.section(section::TYPE, &type_body());
    e.section(section::FUNCTION, &function_body(1));
    e.section(section::EXPORT, &export_body());
    e.section(section::CODE, &body);
    let m = e.finish("code-count-two-functions-one");
    expect_rejected(
        ctx,
        &m,
        "the function section declares one function and the code section two bodies",
    )?;
    Ok(())
});

wasm_test!(trailing_bytes, |ctx| {
    // A module ends where the file ends, so a byte after the last section is the start of
    // another one — and there is nothing behind it. A single 00 is the worst case: it is the
    // custom section id, the one a lenient decoder is most willing to skip.
    let good = good();
    for (tail, why) in [
        (
            vec![0x00u8],
            "a lone 00 is a custom section id with no size after it",
        ),
        (
            vec![0x00, 0x00],
            "a custom section of size 0 has no room for its name",
        ),
        (vec![0xff], "ff names no section at all"),
    ] {
        let mut bytes = good.bytes.clone();
        bytes.extend_from_slice(&tail);
        let mut m = good.clone();
        m.label = format!("trailing-{:02x?}", tail);
        m.bytes = bytes;
        expect_start_rejected(ctx, &m, why)?;
    }

    // What is legal after the last section is another whole section — here an empty custom
    // section, three bytes, which is the shortest well-formed thing that can follow.
    let mut bytes = good.bytes.clone();
    bytes.extend_from_slice(&[section::CUSTOM, 0x01, 0x00]);
    let mut m = good.clone();
    m.label = "trailing-empty-custom".to_string();
    m.bytes = bytes;
    expect(ctx, &m, "7")?;
    Ok(())
});

/// Worked examples: a truncated module, and a body size that swallows its neighbour.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("A file that stops in the middle of a body", || {
            let m = good();
            m.truncated(m.len() - 8)
        })
        .summary(
            "`f() -> i32 { 7 }` with a memory and a data segment, cut eight bytes short so \
             the data segment's bytes are missing",
        )
        .command("run mod.wasm")
        .output("nothing on stdout, a message on stderr, and a non-zero exit status")
        .note(
            "The section frame is intact and the vector count is intact; what is missing is \
             the bytes they promised. Check what is left of the file before every slice, and \
             return the failure rather than panicking — a short file is ordinary input.",
        ),
        ExampleSpec::module("A body one byte shorter than it is", || {
            let mut e = Enc::with_header();
            e.section(section::TYPE, &type_body());
            e.section(section::FUNCTION, &function_body(2));
            let mut f = wasm_name("f");
            f.push(0x00);
            f.extend_from_slice(&uleb(0));
            let mut g = wasm_name("g");
            g.push(0x00);
            g.extend_from_slice(&uleb(1));
            e.section(section::EXPORT, &vector(&[f, g]));
            e.section(
                section::CODE,
                &vector(&[code_entry(7, -1), code_entry(9, 0)]),
            );
            e.finish("body-size-one-short")
        })
        .summary(
            "two function bodies, the first of which declares one byte fewer than it holds, \
             so its `end` opcode lands where the second body's size field should be",
        )
        .command("run --invoke f mod.wasm")
        .output("nothing on stdout, a message on stderr, and a non-zero exit status")
        .note(
            "A body's size is a window, not a hint. Parse instructions inside it and require \
             the `end` to be the last byte of that window; stopping at the first `end` you \
             meet accepts this module and then reads the next body from the wrong offset.",
        ),
    ]
}
