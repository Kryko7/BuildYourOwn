//! Stage 02 — every size, count and index is a LEB128.
//!
//! The spec allows an integer to be written in **any** number of bytes up to the field's
//! width — `ceil(32 / 7) = 5` bytes for a `u32`, ten for a `u64` — so a five-byte encoding of
//! `1` is not a mistake to be tolerated, it is a legal encoding a decoder has to read. That
//! is what the padding tests assert: `wasmtime` accepts every one of them and answers the
//! same number, so a runtime that stops after one byte, or that rejects padding, is wrong.
//!
//! The line is at the *width*, not at the shortest form. A sixth byte in a `u32` field and an
//! eleventh in a `u64` field are refused, and so is a five-byte `u32` whose top byte carries
//! bits past 32 — `80 80 80 80 10` is a well-formed LEB128 of 2^32, and 2^32 is not a `u32`.
//!
//! One thing to know when reading a failure here: `wasmtime` decodes the module eagerly but
//! compiles function bodies later, so a bad LEB128 in a *section* is reported as "failed to
//! parse WebAssembly module" while a bad one inside an `i32.const` comes back as "failed to
//! compile". Both are refusals — exit non-zero, nothing on stdout — and the suite only ever
//! asserts that much, never the wording.

use crate::examples::ExampleSpec;
use crate::stages::{expect, expect_rejected, Stage, Test};
use crate::wasm::{
    section, sleb, sleb_padded, uleb, uleb_padded, vector, wasm_name, Enc, Module, ValType,
};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 2,
        slug: "leb128",
        name: "LEB128 integers and their edges",
        ext: false,
        hints: &[
            "Read a uLEB128 seven bits at a time, lowest group first, and stop at the first byte whose top bit is clear — the count of bytes is not fixed and nothing tells you it in advance",
            "A u32 field may legally take up to five bytes and a u64 up to ten, so accept padding; refuse a sixth (or eleventh) byte and refuse a value whose bits run past the field's width",
            "Signed operands like the one in i32.const are sLEB128: sign-extend the last group from its bit 6, which is why i32.const -1 is the single byte 7f",
            "A LEB128 that reaches the end of the file with its continuation bit still set is a malformed module, not a zero",
        ],
        examples,
        tests: vec![
            Test::new(
                "a size, a count, an index and a constant in their longest legal form decode the same",
                padded_everywhere,
            ),
            Test::new(
                "a padded signed operand keeps its sign",
                padded_negative,
            ),
            Test::new(
                "a uLEB128 that runs past the width of its field is refused",
                overlong_u32,
            ),
            Test::new(
                "an sLEB128 operand that runs past the width of its value is refused",
                overlong_operand,
            ),
            Test::new(
                "a u32 field holding a number that does not fit in 32 bits is refused",
                value_too_wide,
            ),
            Test::new(
                "a LEB128 still asking for another byte at the end of the file is refused",
                unterminated,
            ),
            Test::new("i32.const of the smallest i32 needs all five bytes", i32_min),
            Test::new("i64.const of the smallest i64 needs all ten bytes", i64_min),
        ],
    }
}

/// Every integer field of a one-function module, written exactly as asked rather than in its
/// shortest form.
///
/// The module is always the same one — `f() -> i32 { 7 }` — so anything that changes about
/// what it prints came from an encoding, not from the program.
#[derive(Clone)]
struct Fields {
    /// The function's result type, and so the type of the constant it returns.
    result: ValType,
    /// `0x41` for `i32.const`, `0x42` for `i64.const`.
    const_op: u8,
    /// The constant's operand bytes, as an sLEB128.
    operand: Vec<u8>,
    /// The type section's size field, when it is not to be written honestly.
    type_size: Option<Vec<u8>>,
    /// The function section's vector count.
    func_count: Option<Vec<u8>>,
    /// The export entry's function index.
    export_index: Option<Vec<u8>>,
    /// The single code entry's body size.
    body_size: Option<Vec<u8>>,
    /// Bytes written after the last section.
    trailer: Option<Vec<u8>>,
    /// When true the body's terminating `end` is left off, so the last thing in the code
    /// section is the constant's operand.
    omit_end: bool,
}

impl Fields {
    /// The honest module: `f() -> i32 { 7 }`, every integer in its shortest form.
    fn seven() -> Fields {
        Fields {
            result: ValType::I32,
            const_op: 0x41,
            operand: sleb(7),
            type_size: None,
            func_count: None,
            export_index: None,
            body_size: None,
            trailer: None,
            omit_end: false,
        }
    }

    /// The same, returning an `i64`.
    fn seven_i64() -> Fields {
        Fields {
            result: ValType::I64,
            const_op: 0x42,
            operand: sleb(7),
            ..Fields::seven()
        }
    }

    /// Encode it.
    ///
    /// Written with [`Enc`] rather than `ModuleBuilder` for one reason: the builder always
    /// writes the shortest encoding, and every case in this stage is about what happens when
    /// something else is written instead.
    fn build(&self, label: &str) -> Module {
        let mut e = Enc::with_header();

        // type: one function type, () -> result.
        let functype = vec![0x60, 0x00, 0x01, self.result.code()];
        let tbody = vector(&[functype]);
        match &self.type_size {
            None => {
                e.section(section::TYPE, &tbody);
            }
            Some(s) => {
                e.section_with_size(
                    section::TYPE,
                    s,
                    &tbody,
                    &format!("{} bytes, in {} LEB128 byte(s)", tbody.len(), s.len()),
                );
            }
        }

        // function: one entry, type 0. The vector count is the knob.
        let count = self.func_count.clone().unwrap_or_else(|| uleb(1));
        let mut fbody = count.clone();
        fbody.extend_from_slice(&uleb(0));
        let at = e.at();
        e.section(section::FUNCTION, &fbody);
        e.annotate(
            at + 1 + uleb(fbody.len() as u64).len(),
            count.len(),
            "function.count",
            format!("1 function, in {} LEB128 byte(s)", count.len()),
        );

        // export: 'f' → func, at the given index.
        let index = self.export_index.clone().unwrap_or_else(|| uleb(0));
        let mut entry = wasm_name("f");
        entry.push(0x00);
        entry.extend_from_slice(&index);
        let xbody = vector(&[entry.clone()]);
        let at = e.at();
        e.section(section::EXPORT, &xbody);
        e.annotate(
            at + 1 + uleb(xbody.len() as u64).len() + 1 + (entry.len() - index.len()),
            index.len(),
            "export[0].index",
            format!("function 0, in {} LEB128 byte(s)", index.len()),
        );

        // code: no locals, one constant, end.
        let mut inner = uleb(0);
        inner.push(self.const_op);
        inner.extend_from_slice(&self.operand);
        if !self.omit_end {
            inner.push(0x0b);
        }
        let size = self
            .body_size
            .clone()
            .unwrap_or_else(|| uleb(inner.len() as u64));
        let mut entry = size.clone();
        entry.extend_from_slice(&inner);
        let cbody = vector(&[entry]);
        let at = e.at();
        e.section(section::CODE, &cbody);
        let body_at = at + 1 + uleb(cbody.len() as u64).len() + 1;
        e.annotate(
            body_at,
            size.len(),
            "code[0].size",
            format!("{} bytes, in {} LEB128 byte(s)", inner.len(), size.len()),
        );
        e.annotate(
            body_at + size.len() + 1 + 1,
            self.operand.len(),
            "code[0].const",
            format!(
                "{} operand, {} byte(s)",
                if self.const_op == 0x41 {
                    "i32.const"
                } else {
                    "i64.const"
                },
                self.operand.len()
            ),
        );

        if let Some(t) = &self.trailer {
            e.put(
                t,
                "(after the last section)",
                format!("{} byte(s)", t.len()),
            );
        }
        e.finish(label)
    }
}

/// The module every padding test builds: nothing about it is short-form.
fn padded_module() -> Module {
    Fields {
        type_size: Some(uleb_padded(5, 5)),
        func_count: Some(uleb_padded(1, 5)),
        export_index: Some(uleb_padded(0, 5)),
        // The body is eight bytes: the local-declaration count, i32.const, its five-byte
        // operand, and end.
        body_size: Some(uleb_padded(8, 5)),
        operand: sleb_padded(7, 5),
        ..Fields::seven()
    }
    .build("leb-padded")
}

wasm_test!(padded_everywhere, |ctx| {
    // The type section's body really is five bytes, the function vector really holds one
    // entry, the export really points at function 0 and the body really is four bytes; every
    // one of those numbers is written in the longest form a u32 field allows.
    expect(ctx, &padded_module(), "7")?;
    ctx.note(
        "five-byte encodings of 5, 1, 0 and 8 — the longest legal form for a u32 — are \
         accepted and decode to the same module",
    );
    Ok(())
});

wasm_test!(padded_negative, |ctx| {
    // Padding a signed LEB128 means repeating the sign, not zeroes: -1 padded to five bytes
    // is ff ff ff ff 7f, and the last group's bit 6 is what carries the sign.
    let m = Fields {
        operand: sleb_padded(-1, 5),
        ..Fields::seven()
    }
    .build("leb-padded-negative");
    expect(ctx, &m, "-1")?;
    let m = Fields {
        operand: sleb_padded(-1, 10),
        ..Fields::seven_i64()
    }
    .build("leb-padded-negative-i64");
    expect(ctx, &m, "-1")?;
    Ok(())
});

wasm_test!(overlong_u32, |ctx| {
    // Six bytes carry 42 bits. No u32 field is allowed that many, whatever the value is.
    let m = Fields {
        type_size: Some(uleb_padded(5, 6)),
        ..Fields::seven()
    }
    .build("leb-overlong-section-size");
    expect_rejected(ctx, &m, "a section size may take at most five bytes")?;

    let m = Fields {
        func_count: Some(uleb_padded(1, 6)),
        ..Fields::seven()
    }
    .build("leb-overlong-vector-count");
    expect_rejected(ctx, &m, "a vector count is a u32, so at most five bytes")?;

    let m = Fields {
        export_index: Some(uleb_padded(0, 6)),
        ..Fields::seven()
    }
    .build("leb-overlong-func-index");
    expect_rejected(ctx, &m, "a function index is a u32, so at most five bytes")?;
    Ok(())
});

wasm_test!(overlong_operand, |ctx| {
    let m = Fields {
        operand: sleb_padded(7, 6),
        ..Fields::seven()
    }
    .build("leb-overlong-i32-const");
    expect_rejected(ctx, &m, "an i32.const operand may take at most five bytes")?;

    let m = Fields {
        operand: sleb_padded(7, 11),
        ..Fields::seven_i64()
    }
    .build("leb-overlong-i64-const");
    expect_rejected(ctx, &m, "an i64.const operand may take at most ten bytes")?;
    Ok(())
});

wasm_test!(value_too_wide, |ctx| {
    // 80 80 80 80 10 is five bytes — a legal length — and decodes to 2^32, which is one more
    // than the largest u32. The length limit and the value limit are two different checks and
    // a decoder needs both.
    let too_wide = vec![0x80, 0x80, 0x80, 0x80, 0x10];
    let m = Fields {
        func_count: Some(too_wide.clone()),
        ..Fields::seven()
    }
    .build("leb-value-past-u32");
    expect_rejected(
        ctx,
        &m,
        "the vector count is five bytes long, which is legal, but holds 2^32, which is not a u32",
    )?;

    let m = Fields {
        export_index: Some(too_wide),
        ..Fields::seven()
    }
    .build("leb-index-past-u32");
    expect_rejected(ctx, &m, "the same value in a function index field")?;
    Ok(())
});

wasm_test!(unterminated, |ctx| {
    // A custom section id, then a size that keeps asking for one more byte until the file
    // stops. There is no zero to fall back on: the module is malformed.
    let mut e = Enc::with_header();
    e.put(&[section::CUSTOM], "section[0].id", "custom section");
    e.put(
        &[0x80, 0x80, 0x80],
        "section[0].size",
        "a uLEB128 whose continuation bit is still set at the end of the file",
    );
    let m = e.finish("leb-unterminated-size");
    expect_rejected(ctx, &m, "the size field never ends")?;

    // The same inside a function body, where the operand of an i32.const runs off the end.
    let m = Fields {
        operand: vec![0x80, 0x80, 0x80],
        omit_end: true,
        ..Fields::seven()
    }
    .build("leb-unterminated-operand");
    expect_rejected(ctx, &m, "the i32.const operand never ends")?;
    Ok(())
});

wasm_test!(i32_min, |ctx| {
    // sleb(i32::MIN) is 80 80 80 80 78: five bytes, the last of them 0x78, whose bit 6 is set
    // and sign-extends the lot. A decoder that forgets the sign extension answers 0.
    let operand = sleb(i32::MIN as i64);
    let m = Fields {
        operand: operand.clone(),
        ..Fields::seven()
    }
    .build("leb-i32-min");
    expect(ctx, &m, "-2147483648")?;
    ctx.note(format!(
        "i32.const {} is {} operand bytes: {:02x?}",
        i32::MIN,
        operand.len(),
        operand
    ));
    Ok(())
});

wasm_test!(i64_min, |ctx| {
    let operand = sleb(i64::MIN);
    let m = Fields {
        operand: operand.clone(),
        ..Fields::seven_i64()
    }
    .build("leb-i64-min");
    expect(ctx, &m, "-9223372036854775808")?;
    ctx.note(format!(
        "i64.const {} is {} operand bytes, the longest an sLEB128 in a module ever gets",
        i64::MIN,
        operand.len()
    ));
    Ok(())
});

/// Worked examples: padding that must be accepted, and the byte too far.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("Every integer written the long way", padded_module)
            .summary(
                "`f() -> i32 { 7 }` whose type section size, function vector count, export \
                 index and code body size are each written as a five-byte uLEB128, and whose \
                 i32.const operand is a five-byte sLEB128",
            )
            .command("run --invoke f mod.wasm")
            .output("7")
            .note(
                "Five bytes is the longest a u32 field may legally be, so none of this is \
                 malformed — it is padding a producer is allowed to emit, and a decoder that \
                 reads a fixed number of bytes, or that insists on the shortest form, refuses \
                 a module the spec accepts.",
            ),
        ExampleSpec::module("One byte too many", || {
            Fields {
                func_count: Some(uleb_padded(1, 6)),
                ..Fields::seven()
            }
            .build("leb-overlong-vector-count")
        })
        .summary(
            "the same module with the function section's vector count padded to six bytes \
             instead of five",
        )
        .command("run --invoke f mod.wasm")
        .output("nothing on stdout, a message on stderr, and a non-zero exit status")
        .note(
            "The value is still 1. What is wrong is the width: ceil(32 / 7) is 5, so a sixth \
             byte in a u32 field is a malformed module. Check the byte count as you go rather \
             than after you have shifted 42 bits into a u64 and lost the evidence.",
        ),
    ]
}
