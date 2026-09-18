//! Stage 03 — a module is a sequence of framed sections, and the frame is load-bearing.
//!
//! Every section is `id, uLEB128 size, size bytes of body`. The size is what lets a decoder
//! skip a section it does not understand, which means it is also the thing that must be
//! believed exactly: a size one byte short does not shift the reader by one, it makes the
//! next byte in the body be read as the next section's id, and everything after that is
//! nonsense. All three lying-size cases here are refused for that reason.
//!
//! Two rules about order, and they are not the same rule. Every non-custom section may appear
//! **at most once**, and they must come in **ascending id order** — except the data count
//! section, whose id is 12 but which sits before the code section (id 10), because the code
//! section needs to know how many data segments there are before it validates a `memory.init`.
//!
//! One expectation had to be written down rather than guessed. The spec says a section id
//! that no version of the format defines makes the module malformed, and `wasmtime` agrees:
//! id 42 and id 13 are both refused, wherever they appear. A decoder must *not* treat an
//! unknown id like an unknown custom section and skip it — that leniency is what lets a
//! module smuggle a section a runtime will never validate.
//!
//! The last test is looser than it looks and that is the spec's doing: a data count section is
//! only **required** when the code uses `memory.init` or `data.drop`, so a module with a
//! passive data segment and no data count section is perfectly legal — `wasmtime` runs it.
//! What is never legal is a data count that disagrees with the data section, and that is what
//! the test asserts, in both directions.

use crate::examples::ExampleSpec;
use crate::stages::{expect, expect_rejected, Stage, Test};
use crate::wasm::{section, sleb, uleb, vector, wasm_name, Enc, Module};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 3,
        slug: "section_framing",
        name: "Section order, duplicates and unknown ids",
        ext: false,
        hints: &[
            "Read sections in a loop: one id byte, a uLEB128 size, then exactly that many bytes of body — and start the next section at the byte after them, never where the body happened to stop parsing",
            "Keep the id of the last non-custom section you saw and refuse anything whose id is not strictly larger; that one check catches both duplicates and out-of-order sections",
            "The data count section has id 12 but belongs before the code section, so treat it as the exception to ascending order rather than sorting by id",
            "An id above 12 is a malformed module, not something to skip: only id 0, the custom section, may carry bytes you do not understand",
        ],
        examples,
        tests: vec![
            Test::new("a module whose sections are in ascending id order runs", in_order),
            Test::new("a second type section is refused", duplicate_section),
            Test::new("the code section may not come before the function section", out_of_order),
            Test::new("a section id no version of the format defines is refused", unknown_id),
            Test::new("a section body of zero bytes is refused", empty_body),
            Test::new(
                "a section whose size stops short makes the next id be read from its body",
                size_too_small,
            ),
            Test::new("a section whose size runs past the end of the file is refused", size_past_end),
            Test::new(
                "the data count must agree with the number of data segments",
                data_count_disagrees,
            ),
        ],
    }
}

/// The type section's body: one type, `() -> i32`.
fn type_body() -> Vec<u8> {
    vector(&[vec![0x60, 0x00, 0x01, 0x7f]])
}

/// The function section's body: one function of type 0.
fn function_body() -> Vec<u8> {
    vector(&[uleb(0)])
}

/// The memory section's body: one memory of one page, no maximum.
fn memory_body() -> Vec<u8> {
    vector(&[vec![0x00, 0x01]])
}

/// The export section's body: `f` → function 0.
fn export_body() -> Vec<u8> {
    let mut entry = wasm_name("f");
    entry.push(0x00);
    entry.extend_from_slice(&uleb(0));
    vector(&[entry])
}

/// The code section's body: one function returning 7.
fn code_body() -> Vec<u8> {
    let mut inner = uleb(0);
    inner.push(0x41);
    inner.extend_from_slice(&sleb(7));
    inner.push(0x0b);
    let mut entry = uleb(inner.len() as u64);
    entry.extend_from_slice(&inner);
    vector(&[entry])
}

/// A data section holding `n` passive segments of three bytes each.
fn data_body(n: usize) -> Vec<u8> {
    let mut seg = vec![0x01];
    seg.extend_from_slice(&uleb(3));
    seg.extend_from_slice(b"abc");
    vector(&vec![seg; n])
}

/// The well-formed module the whole stage is a variation on: it returns 7 and owns a memory.
fn good() -> Module {
    let mut e = Enc::with_header();
    e.section(section::TYPE, &type_body());
    e.section(section::FUNCTION, &function_body());
    e.section(section::MEMORY, &memory_body());
    e.section(section::EXPORT, &export_body());
    e.section(section::CODE, &code_body());
    e.finish("sections-in-order")
}

wasm_test!(in_order, |ctx| {
    expect(ctx, &good(), "7")?;

    // The same module with the one section whose id is out of step with its position: the
    // data count section, id 12, written before the code section, id 10.
    let mut e = Enc::with_header();
    e.section(section::TYPE, &type_body());
    e.section(section::FUNCTION, &function_body());
    e.section(section::MEMORY, &memory_body());
    e.section(section::EXPORT, &export_body());
    e.section(section::DATA_COUNT, &uleb(1));
    e.section(section::CODE, &code_body());
    e.section(section::DATA, &data_body(1));
    let m = e.finish("sections-with-data-count");
    expect(ctx, &m, "7")?;
    ctx.note("the data count section is id 12 and still belongs before the code section, id 10");
    Ok(())
});

wasm_test!(duplicate_section, |ctx| {
    // Both copies are well formed on their own; what is wrong is that there are two.
    let mut e = Enc::with_header();
    e.section(section::TYPE, &type_body());
    e.section(section::TYPE, &type_body());
    e.section(section::FUNCTION, &function_body());
    e.section(section::EXPORT, &export_body());
    e.section(section::CODE, &code_body());
    let m = e.finish("two-type-sections");
    expect_rejected(ctx, &m, "a non-custom section may appear at most once")?;
    Ok(())
});

wasm_test!(out_of_order, |ctx| {
    // Every section is present and every one of them is well formed; only the order is wrong.
    // A decoder that dispatches on the id without remembering the last one it saw accepts
    // this happily and then has to guess what a code section before a function section means.
    let mut e = Enc::with_header();
    e.section(section::TYPE, &type_body());
    e.section(section::CODE, &code_body());
    e.section(section::FUNCTION, &function_body());
    e.section(section::EXPORT, &export_body());
    let m = e.finish("code-before-function");
    expect_rejected(
        ctx,
        &m,
        "the code section (id 10) comes after the function section (id 3)",
    )?;

    // And the shorter version of the same rule, between two sections that are only one apart.
    let mut e = Enc::with_header();
    e.section(section::TYPE, &type_body());
    e.section(section::EXPORT, &export_body());
    e.section(section::FUNCTION, &function_body());
    e.section(section::CODE, &code_body());
    let m = e.finish("export-before-function");
    expect_rejected(
        ctx,
        &m,
        "the export section (id 7) comes after the function section (id 3)",
    )?;
    Ok(())
});

wasm_test!(unknown_id, |ctx| {
    // 42 is not a section. The tempting reading of "skip what you do not understand" is that
    // the size field makes this harmless; the spec's reading is that only id 0 carries bytes
    // a runtime may ignore, and everything else is a module from a format nobody has written.
    let mut e = Enc::with_header();
    e.section(section::TYPE, &type_body());
    e.section(section::FUNCTION, &function_body());
    e.section(section::EXPORT, &export_body());
    e.section(section::CODE, &code_body());
    e.section(42, &[0x01, 0x02, 0x03]);
    let m = e.finish("unknown-section-42");
    let run = expect_rejected(
        ctx,
        &m,
        "id 42 names no section, and an unknown id is not a custom section",
    )?;
    ctx.note(format!(
        "wasmtime refuses a section with id 42 even at the very end of the module: {}",
        crate::stages::first_meaningful_line(&run.stderr)
    ));

    // 13 is the first id after the last one the format defines, which is the id a runtime is
    // most likely to let through by accident.
    let mut e = Enc::with_header();
    e.section(section::TYPE, &type_body());
    e.section(13, &[]);
    e.section(section::FUNCTION, &function_body());
    e.section(section::EXPORT, &export_body());
    e.section(section::CODE, &code_body());
    let m = e.finish("unknown-section-13");
    expect_rejected(ctx, &m, "13 is one past the last id the format defines")?;
    Ok(())
});

wasm_test!(empty_body, |ctx| {
    // A size of 0 is a well-formed frame around nothing. It is still malformed, because every
    // non-custom section's body begins with a vector count, and a count needs at least one
    // byte: there is no such thing as a section that holds no bytes at all.
    let mut e = Enc::with_header();
    e.section(section::TYPE, &[]);
    e.section(section::FUNCTION, &function_body());
    e.section(section::EXPORT, &export_body());
    e.section(section::CODE, &code_body());
    let m = e.finish("type-section-size-zero");
    expect_rejected(
        ctx,
        &m,
        "a zero-length body cannot even hold the vector count",
    )?;

    // The empty *vector* is a different thing and is perfectly legal: one byte, the count 0.
    let mut e = Enc::with_header();
    e.section(section::TYPE, &type_body());
    e.section(section::FUNCTION, &function_body());
    e.section(section::EXPORT, &export_body());
    e.section(section::CODE, &code_body());
    e.section(section::DATA, &uleb(0));
    let m = e.finish("empty-data-vector");
    expect(ctx, &m, "7")?;
    ctx.note("a section whose body is the single byte 00 — an empty vector — is legal");
    Ok(())
});

wasm_test!(size_too_small, |ctx| {
    // The type section's body is five bytes. Saying three means the decoder stops after
    // `01 60 00` and reads the next byte, `01`, as a section id: the type section again, with
    // a size of 0x7f. Nothing about that is recoverable, which is why the size must be
    // believed rather than inferred from where the body stopped making sense.
    let body = type_body();
    let mut e = Enc::with_header();
    e.section_with_size(
        section::TYPE,
        &uleb(3),
        &body,
        &format!("3, but the body is {} bytes", body.len()),
    );
    e.section(section::FUNCTION, &function_body());
    e.section(section::EXPORT, &export_body());
    e.section(section::CODE, &code_body());
    let m = e.finish("type-size-too-small");
    expect_rejected(
        ctx,
        &m,
        "the next section id is read from inside the type section's body",
    )?;
    Ok(())
});

wasm_test!(size_past_end, |ctx| {
    // The other direction: a size that promises more bytes than the file has. A decoder that
    // slices without checking is the one that panics here.
    let body = type_body();
    let mut e = Enc::with_header();
    e.section_with_size(
        section::TYPE,
        &uleb(99),
        &body,
        &format!(
            "99, but only {} bytes follow, to the end of the file",
            body.len()
        ),
    );
    let m = e.finish("type-size-past-end");
    expect_rejected(
        ctx,
        &m,
        "the section claims 99 bytes and the file ends after 5",
    )?;

    // The same lie in the last section of an otherwise complete module.
    let mut e = Enc::with_header();
    e.section(section::TYPE, &type_body());
    e.section(section::FUNCTION, &function_body());
    e.section(section::EXPORT, &export_body());
    let code = code_body();
    e.section_with_size(
        section::CODE,
        &uleb(99),
        &code,
        &format!("99, but only {} bytes follow", code.len()),
    );
    let m = e.finish("code-size-past-end");
    expect_rejected(ctx, &m, "the code section runs past the end of the file")?;
    Ok(())
});

wasm_test!(data_count_disagrees, |ctx| {
    // One passive segment, and a data count section that says something else. The count is
    // there so the code section can validate a memory.init before the data section has been
    // read, so a count that does not match makes that validation a lie.
    for (count, segments) in [(2usize, 1usize), (0, 1), (1, 2)] {
        let mut e = Enc::with_header();
        e.section(section::TYPE, &type_body());
        e.section(section::FUNCTION, &function_body());
        e.section(section::MEMORY, &memory_body());
        e.section(section::EXPORT, &export_body());
        e.section(section::DATA_COUNT, &uleb(count as u64));
        e.section(section::CODE, &code_body());
        e.section(section::DATA, &data_body(segments));
        let m = e.finish(format!("data-count-{count}-segments-{segments}"));
        expect_rejected(
            ctx,
            &m,
            &format!("the data count says {count} and the data section holds {segments}"),
        )?;
    }

    // And the case that is legal however wrong it looks: no data count section at all. It is
    // only required when the code uses memory.init or data.drop, and this code does not.
    let mut e = Enc::with_header();
    e.section(section::TYPE, &type_body());
    e.section(section::FUNCTION, &function_body());
    e.section(section::MEMORY, &memory_body());
    e.section(section::EXPORT, &export_body());
    e.section(section::CODE, &code_body());
    e.section(section::DATA, &data_body(1));
    let m = e.finish("passive-data-no-count");
    expect(ctx, &m, "7")?;
    ctx.note(
        "a passive data segment without a data count section is legal while no memory.init \
         or data.drop names it",
    );
    Ok(())
});

/// Worked examples: the frame that works, and the frame that shifts everything after it.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("Five sections, ascending", good)
            .summary(
                "the type, function, memory, export and code sections of `f() -> i32 { 7 }`, \
                 each one an id byte, a uLEB128 size and that many bytes of body",
            )
            .command("run --invoke f mod.wasm")
            .output("7")
            .note(
                "Ids 1, 3, 5, 7, 10 — strictly ascending. Remember the last id you decoded and \
                 refuse anything that is not larger; that single rule catches both a repeated \
                 section and a section in the wrong place.",
            ),
        ExampleSpec::module("A size three bytes short", || {
            let body = type_body();
            let mut e = Enc::with_header();
            e.section_with_size(section::TYPE, &uleb(3), &body, "3, but the body is 5 bytes");
            e.section(section::FUNCTION, &function_body());
            e.section(section::EXPORT, &export_body());
            e.section(section::CODE, &code_body());
            e.finish("type-size-too-small")
        })
        .summary("the same module whose type section declares three bytes and is followed by five")
        .command("run --invoke f mod.wasm")
        .output("nothing on stdout, a message on stderr, and a non-zero exit status")
        .note(
            "After three bytes the decoder is standing on `01`, the last two bytes of the \
             function type, and reads it as another type section with a size of 0x7f. The \
             size field is the only thing that says where a section ends, so a decoder that \
             carries on from wherever its body parser stopped will follow a lie all the way \
             to the end of the file.",
        ),
    ]
}
