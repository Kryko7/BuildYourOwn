//! Stage 01 — the eight bytes every module starts with.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::stages::{expect, expect_rejected, expect_start_rejected, f_i32, Stage, Test};
use crate::wasm::{Enc, Expr, Module};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 1,
        slug: "magic_and_version",
        name: "The magic number and the version",
        ext: false,
        hints: &[
            "A module begins with the four bytes 00 61 73 6d — a NUL followed by 'asm' — and then a four-byte version",
            "The version is 01 00 00 00: little-endian 1, not the big-endian 00 00 00 01 a network protocol would write",
            "Check both before you decode anything else, and refuse the module without running a single instruction",
            "A file shorter than those eight bytes is not a truncated module, it is not a module at all",
        ],
        examples,
        tests: vec![
            Test::new("the eight-byte header is accepted", good_header),
            Test::new("a wrong magic byte is refused", wrong_magic),
            Test::new("the ASCII text 'asm' without the leading NUL is refused", no_nul),
            Test::new("version 0 is refused", version_zero),
            Test::new("version 2 is refused", version_two),
            Test::new("a big-endian version is refused", big_endian_version),
            Test::new("an empty file is refused", empty_file),
            Test::new("a file holding only the magic is refused", magic_only),
        ],
    }
}

/// The smallest module that proves the header was read: one export, one constant.
fn seven() -> Module {
    f_i32("seven", Expr::new().i32_const(7))
}

/// `seven()` with its first eight bytes replaced.
fn with_header(label: &str, header: &[u8]) -> Module {
    let good = seven();
    let mut bytes = header.to_vec();
    bytes.extend_from_slice(&good.bytes[8.min(good.bytes.len())..]);
    let mut m = Module {
        bytes,
        anns: Vec::new(),
        label: label.to_string(),
    };
    m.anns.push(crate::wasm::Ann {
        offset: 0,
        length: 4.min(header.len()),
        field: "header.magic".to_string(),
        value: format!("{:02x?}", &header[..4.min(header.len())]),
    });
    if header.len() >= 8 {
        m.anns.push(crate::wasm::Ann {
            offset: 4,
            length: 4,
            field: "header.version".to_string(),
            value: format!(
                "{} (little-endian)",
                u32::from_le_bytes([header[4], header[5], header[6], header[7]])
            ),
        });
    }
    // Everything after the header is the same well-formed module, annotated as one span so
    // the listing makes it obvious that only the header differs.
    if m.bytes.len() > header.len() {
        m.anns.push(crate::wasm::Ann {
            offset: header.len(),
            length: m.bytes.len() - header.len(),
            field: "(the rest of a valid module)".to_string(),
            value: "type, function, export and code sections for `f() -> i32 { 7 }`".to_string(),
        });
    }
    m
}

wasm_test!(good_header, |ctx| {
    expect(ctx, &seven(), "7")?;
    Ok(())
});

wasm_test!(wrong_magic, |ctx| {
    let m = with_header("magic-wasm", b"\0wsm\x01\x00\x00\x00");
    expect_rejected(ctx, &m, "the third magic byte is 'w', not 'a'")?;
    Ok(())
});

wasm_test!(no_nul, |ctx| {
    // A tempting mistake: comparing the *string* "asm" and forgetting the leading NUL.
    let m = with_header("magic-asm0", b"asm\0\x01\x00\x00\x00");
    expect_rejected(ctx, &m, "the magic is \\0asm, not asm\\0")?;
    Ok(())
});

wasm_test!(version_zero, |ctx| {
    let m = with_header("version-0", b"\0asm\x00\x00\x00\x00");
    expect_rejected(ctx, &m, "version 0 does not exist")?;
    Ok(())
});

wasm_test!(version_two, |ctx| {
    let m = with_header("version-2", b"\0asm\x02\x00\x00\x00");
    expect_rejected(
        ctx,
        &m,
        "the binary format version is still 1; a 2 in that field is a module from a future \
         nobody has specified",
    )?;
    Ok(())
});

wasm_test!(big_endian_version, |ctx| {
    let m = with_header("version-be", b"\0asm\x00\x00\x00\x01");
    expect_rejected(
        ctx,
        &m,
        "the version is a little-endian u32, so 1 is 01 00 00 00",
    )?;
    Ok(())
});

wasm_test!(empty_file, |ctx| {
    let m = Module {
        bytes: Vec::new(),
        anns: Vec::new(),
        label: "empty-file".to_string(),
    };
    let run = expect_start_rejected(ctx, &m, "a zero-byte file is not a module")?;
    let mut c = Check::new("the empty-file error", &run);
    c.module(&m);
    c.that(
        "stderr",
        "a message, not a panic or a silent success",
        !run.stderr.trim().is_empty(),
        run.stderr.clone(),
    );
    c.finish()
});

wasm_test!(magic_only, |ctx| {
    let mut e = Enc::new();
    e.put(b"\0asm", "header.magic", "\\0asm");
    let m = e.finish("magic-only");
    expect_start_rejected(ctx, &m, "the four magic bytes with no version after them")?;
    Ok(())
});

/// Worked examples: the header that works, and the one that looks right and is not.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("The smallest module that returns a number", seven)
            .summary(
                "the eight-byte header, a type section for `() -> i32`, a function section, \
                 an export named `f`, and a body of `i32.const 7`",
            )
            .command("run --invoke f mod.wasm")
            .output("7")
            .note(
                "Read the first eight bytes before anything else: 00 61 73 6d 01 00 00 00. \
                 Everything after them is a sequence of sections, each one an id byte, a \
                 uLEB128 size, and that many bytes.",
            ),
        ExampleSpec::module("A version written the wrong way round", || {
            with_header("version-be", b"\0asm\x00\x00\x00\x01")
        })
        .summary("the same module with the version bytes reversed to 00 00 00 01")
        .command("run --invoke f mod.wasm")
        .output(
            "nothing on stdout, a message on stderr, and a non-zero exit status — the module \
             never runs",
        )
        .note(
            "The version is a little-endian u32. Reading it big-endian gives 16 777 216, and \
             a module that is refused must be refused before a single instruction executes: \
             stdout stays empty.",
        ),
    ]
}
