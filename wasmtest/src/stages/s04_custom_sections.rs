//! Stage 04 — custom sections, and the discipline of ignoring them. **[ext]**
//!
//! A custom section is id 0: a name, then a payload nobody but its author understands. It is
//! the one section that may appear **anywhere** — before the type section, in any gap between
//! two sections, after the data section — and any number of times, under the same name as
//! often as you like. A runtime's whole job is to walk over it and carry on as if it were not
//! there, and the first four tests are the same module answering `7` with more and more
//! rubbish wrapped around it.
//!
//! The `name` section is the one people are tempted to give meaning to. It is debug
//! information: function names for a stack trace, nothing more. A `name` section full of
//! bytes that decode to nothing at all must not change a single thing about how the module
//! runs, and `wasmtime` does not even look at it until something asks for a symbol.
//!
//! "Ignored" is not the same as "unread", though, and that is the point of the last two
//! tests. A custom section is framed like every other section, so its size must be right and
//! its name must be a valid UTF-8 `name`; a decoder still has to read both to know where the
//! section ends. `wasmtime` refuses a custom section whose size does not match and one whose
//! name is not UTF-8, and so must a runtime that means to skip the payload safely.
//!
//! The whole stage is `ext`: nothing here changes what a module computes, so a runtime can
//! get a long way without it. It is on the list because the first real `.wasm` file a learner
//! picks up off disk — anything a compiler produced — is full of custom sections, and a
//! runtime that trips over them cannot run any of them.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::stages::{expect, expect_rejected, Stage, Test};
use crate::wasm::{section, uleb, Ann, Enc, Expr, Func, Limits, Module, ModuleBuilder, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 4,
        slug: "custom_sections",
        name: "Custom sections and the name section",
        ext: true,
        hints: &[
            "Id 0 is a custom section: read its size, skip exactly that many bytes, and go back to the loop without touching the last-section-id check that orders the others",
            "It is legal before the first section, after the last one, in every gap between two, and repeated under a name you have already seen — none of that is an error to report",
            "The `name` section is debug information for stack traces; whatever is in it, the module computes the same thing, so never let it reach validation",
            "Skipping is not the same as not reading: the section's size and its UTF-8 name are still part of the frame, so a wrong size or a name that is not UTF-8 is a malformed module",
        ],
        examples,
        tests: vec![
            Test::new("a custom section in every gap changes nothing", in_every_gap).ext(),
            Test::new("the same custom section name may be used twice", repeated_name).ext(),
            Test::new("a name section full of rubbish is still ignored", garbage_name_section).ext(),
            Test::new("a megabyte of payload changes nothing", huge_payload).ext(),
            Test::new("a custom section may have an empty name", empty_name).ext(),
            Test::new("a custom section whose declared size is wrong is refused", wrong_size).ext(),
            Test::new("a custom section name that is not valid UTF-8 is refused", bad_utf8_name).ext(),
        ],
    }
}

/// The module every test in this stage wraps: one memory, one data segment and
/// `f() -> i32 { 7 }`, so there are six section boundaries to put a custom section into.
fn base() -> Module {
    let mut b = ModuleBuilder::new("customs-base");
    let ty = b.add_type(crate::wasm::ftype(&[], &[ValType::I32]));
    let idx = b.add_func(ty, Func::new(Expr::new().i32_const(7)));
    b.memory(Limits::min(1))
        .export_func("f", idx)
        .data_active(0, b"hello")
        .build()
}

/// The bytes of one custom section: id 0, a size, a name and a payload.
fn custom_section(name: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut body = uleb(name.len() as u64);
    body.extend_from_slice(name);
    body.extend_from_slice(payload);
    let mut out = vec![section::CUSTOM];
    out.extend_from_slice(&uleb(body.len() as u64));
    out.extend_from_slice(&body);
    out
}

/// The byte offset at which each top-level section of `m` starts, plus the end of the module.
///
/// Read out of the annotations the encoder already wrote, so it stays right whatever the
/// builder emits.
fn section_starts(m: &Module) -> Vec<usize> {
    let mut v: Vec<usize> = m
        .anns
        .iter()
        .filter(|a| a.field.ends_with(".id") && a.field.starts_with("section["))
        .map(|a| a.offset)
        .collect();
    v.push(m.bytes.len());
    v.sort_unstable();
    v.dedup();
    v
}

/// Insert a custom section into every gap of `m`: before the first section, between each
/// pair, and after the last one.
///
/// The original annotations are shifted along so the failure listing still names the real
/// sections, and each inserted section gets one of its own.
fn customs_in_every_gap(m: &Module, label: &str) -> Module {
    let starts = section_starts(m);
    let mut bytes = Vec::with_capacity(m.bytes.len() + starts.len() * 16);
    let mut anns: Vec<Ann> = Vec::new();
    // (offset in the original module, bytes inserted there, its length in the new module).
    let mut inserted: Vec<(usize, usize, usize)> = Vec::new();
    let mut prev = 0usize;
    for (i, start) in starts.iter().enumerate() {
        bytes.extend_from_slice(&m.bytes[prev..*start]);
        let name = format!("gap{i}");
        let sec = custom_section(name.as_bytes(), &[0xde, 0xad, 0xbe, 0xef]);
        anns.push(Ann {
            offset: bytes.len(),
            length: sec.len(),
            field: format!("custom['{name}']"),
            value: "id 0, four bytes of payload a runtime must walk over".to_string(),
        });
        inserted.push((*start, bytes.len(), sec.len()));
        bytes.extend_from_slice(&sec);
        prev = *start;
    }
    bytes.extend_from_slice(&m.bytes[prev..]);

    // An original byte moves right by the length of every custom section inserted at or
    // before its offset.
    for a in &m.anns {
        let shift: usize = inserted
            .iter()
            .filter(|(at, _, _)| *at <= a.offset)
            .map(|(_, _, len)| *len)
            .sum();
        anns.push(Ann {
            offset: a.offset + shift,
            length: a.length,
            field: a.field.clone(),
            value: a.value.clone(),
        });
    }
    anns.sort_by_key(|a| (a.offset, a.length));
    Module {
        bytes,
        anns,
        label: label.to_string(),
    }
}

/// `base()` with `sections` appended after its last section.
fn with_trailing(sections: &[Vec<u8>], label: &str) -> Module {
    let m = base();
    let mut e = Enc::new();
    e.raw(&m.bytes);
    for a in &m.anns {
        e.annotate(a.offset, a.length, a.field.clone(), a.value.clone());
    }
    for s in sections {
        e.put(s, "custom", format!("id 0, {} bytes in all", s.len()));
    }
    e.finish(label)
}

wasm_test!(in_every_gap, |ctx| {
    // Before the type section, between every pair of sections, and after the data section.
    let m = customs_in_every_gap(&base(), "customs-in-every-gap");
    let gaps = section_starts(&base()).len();
    expect(ctx, &m, "7")?;
    ctx.note(format!(
        "{gaps} custom sections inserted into a {}-byte module; it still answers 7",
        base().len()
    ));
    Ok(())
});

wasm_test!(repeated_name, |ctx| {
    // Nothing says a custom section name is unique. Two sections called `producers` with
    // different payloads is what a two-pass toolchain produces, and both must be skipped.
    let m = with_trailing(
        &[
            custom_section(b"producers", b"\x01first"),
            custom_section(b"producers", b"\x02second"),
            custom_section(b"producers", b""),
        ],
        "customs-same-name-three-times",
    );
    expect(ctx, &m, "7")?;
    Ok(())
});

wasm_test!(garbage_name_section, |ctx| {
    // A `name` section whose payload is not a name map at all. It is debug information, so
    // there is nothing here to be wrong about: a runtime that validates it has invented a
    // rule the spec does not have, and one that lets it reach the code has a worse problem.
    let rubbish = vec![
        0xff, 0xfe, 0x99, 0x01, 0x02, 0x00, 0x80, 0x80, 0x80, 0x80, 0x80,
    ];
    let m = with_trailing(&[custom_section(b"name", &rubbish)], "name-section-rubbish");
    expect(ctx, &m, "7")?;

    // And the same payload before the type section, where a decoder meets it first.
    let mut bytes = base().bytes[..8].to_vec();
    bytes.extend_from_slice(&custom_section(b"name", &rubbish));
    bytes.extend_from_slice(&base().bytes[8..]);
    let m = Module {
        bytes,
        anns: Vec::new(),
        label: "name-section-rubbish-first".to_string(),
    };
    expect(ctx, &m, "7")?;
    ctx.note(
        "the name section is a function-name map for stack traces; its contents never reach \
         validation, so rubbish in it is not a malformed module",
    );
    Ok(())
});

wasm_test!(huge_payload, |ctx| {
    // A megabyte of payload, which is far more than the rest of the module. A decoder that
    // copies a custom section's bytes somewhere instead of skipping them pays for this; one
    // that skips does not care how big it is.
    let payload = vec![0xa5u8; 1 << 20];
    let m = with_trailing(&[custom_section(b"big", &payload)], "custom-one-megabyte");
    let run = expect(ctx, &m, "7")?;
    let mut c = Check::new("a module that is mostly custom section", &run);
    c.module(&base());
    c.at_least("module.len", 1 << 20, m.len());
    c.finish()?;
    ctx.note(format!(
        "{} bytes of module, of which {} are a payload nobody reads",
        m.len(),
        payload.len()
    ));
    Ok(())
});

wasm_test!(empty_name, |ctx| {
    // A name of zero length is a valid `name`: the size field says 0 and no bytes follow.
    let m = with_trailing(&[custom_section(b"", b"\x01\x02\x03")], "custom-empty-name");
    expect(ctx, &m, "7")?;

    // And a custom section that is nothing but an empty name: three bytes, id, size, count.
    let m = with_trailing(&[custom_section(b"", b"")], "custom-empty-entirely");
    expect(ctx, &m, "7")?;
    Ok(())
});

wasm_test!(wrong_size, |ctx| {
    // A custom section is framed exactly like every other one, so a size that does not match
    // its contents is a malformed module — the "ignore what you do not understand" licence is
    // about the *payload*, never about the frame around it.
    let mut too_big = vec![section::CUSTOM];
    too_big.extend_from_slice(&uleb(99));
    too_big.extend_from_slice(&uleb(1));
    too_big.extend_from_slice(b"c");
    too_big.push(0x01);
    let m = with_trailing(&[too_big], "custom-size-past-end");
    expect_rejected(
        ctx,
        &m,
        "the custom section claims 99 bytes and four follow",
    )?;

    // Short in the other direction: the size stops inside the payload, so the next byte the
    // decoder reads as a section id is really part of this section.
    let mut too_small = vec![section::CUSTOM];
    too_small.extend_from_slice(&uleb(3));
    too_small.extend_from_slice(&uleb(1));
    too_small.extend_from_slice(b"c");
    too_small.extend_from_slice(&[0x01, 0x02, 0x03]);
    let m = with_trailing(&[too_small], "custom-size-too-small");
    expect_rejected(ctx, &m, "three bytes are declared and five follow")?;

    // And the name length running past the section's own end.
    let mut body = uleb(40);
    body.extend_from_slice(b"short");
    let mut sec = vec![section::CUSTOM];
    sec.extend_from_slice(&uleb(body.len() as u64));
    sec.extend_from_slice(&body);
    let m = with_trailing(&[sec], "custom-name-past-section");
    expect_rejected(ctx, &m, "the name says 40 bytes and the section holds five")?;
    Ok(())
});

wasm_test!(bad_utf8_name, |ctx| {
    // `name` is UTF-8 in the spec, not an opaque byte string, and ff fe is not UTF-8.
    let m = with_trailing(
        &[custom_section(&[0xff, 0xfe], b"\x01")],
        "custom-name-not-utf8",
    );
    expect_rejected(ctx, &m, "ff fe is not a UTF-8 sequence")?;

    // A lone continuation byte: the shortest possible way to get this wrong.
    let m = with_trailing(
        &[custom_section(&[0x80], b"")],
        "custom-name-lone-continuation",
    );
    expect_rejected(
        ctx,
        &m,
        "80 is a continuation byte with nothing to continue",
    )?;
    Ok(())
});

/// Worked examples: a module wrapped in custom sections, and one whose frame is broken.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("The same module, buried in custom sections", || {
            customs_in_every_gap(&base(), "customs-in-every-gap")
        })
        .summary(
            "`f() -> i32 { 7 }` with a memory and a data segment, and a four-byte custom \
             section inserted before the first section, in every gap between two, and after \
             the last one",
        )
        .command("run --invoke f mod.wasm")
        .output("7")
        .note(
            "Id 0 is the only section that may appear more than once and in any position, so \
             the ascending-id check that orders the other sections has to skip it entirely. \
             Read the size, jump that many bytes, and go back to the top of the loop.",
        ),
        ExampleSpec::module("A custom section that lies about its length", || {
            let mut sec = vec![section::CUSTOM];
            sec.extend_from_slice(&uleb(99));
            sec.extend_from_slice(&uleb(1));
            sec.extend_from_slice(b"c");
            sec.push(0x01);
            with_trailing(&[sec], "custom-size-past-end")
        })
        .summary("the same module with a trailing custom section declaring 99 bytes of body")
        .command("run --invoke f mod.wasm")
        .output("nothing on stdout, a message on stderr, and a non-zero exit status")
        .note(
            "Ignoring a custom section's payload does not mean ignoring its frame. The size \
             is what tells a decoder where the section ends, so a size that runs past the end \
             of the file is a malformed module however uninteresting the bytes inside it are.",
        ),
    ]
}
