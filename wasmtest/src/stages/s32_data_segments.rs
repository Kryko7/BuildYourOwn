//! Stage 32 — active and passive data segments.
//!
//! An **active** segment names a memory and an offset, and its bytes are copied in at
//! instantiation, before the start function and before any export can be called. A
//! **passive** segment is copied nowhere; it sits in the instance waiting for a
//! `memory.init` (stage 33), and until then the memory it belongs to is still zeroes.
//! Segments are applied in the order they appear, so where two of them overlap the later
//! one wins.
//!
//! The interesting failure is a segment that does not fit. It is **not** a validation
//! error: the module decodes, it validates, and then instantiation copies the bytes,
//! discovers the destination is outside the memory, and traps with the ordinary
//! `out of bounds memory access`. A runtime that refuses such a module while validating
//! gets the right exit status for the wrong reason, and a runtime that copies the part that
//! fits before noticing gets the memory wrong. The distinction matters because the segment
//! offset may be a `global.get`, whose value is not known until instantiation.
//!
//! The `global.get` test is looser than it first looked, and the reason is worth writing
//! down. WebAssembly 2.0 validates constant expressions in a context holding the
//! **imported** globals alone, which makes a segment offset that reads a global the module
//! defines itself invalid. `wasmtime` 48 accepts it — it follows the later relaxation that
//! lets a constant expression read any preceding immutable global — so this stage asserts
//! what the reference actually does: a defined immutable global works as an offset, and the
//! segment really is copied to the address it names. What is still refused everywhere, and
//! is tested here, is a **mutable** global: that is not a constant expression under any
//! version of the rule. The imported form is legal too, but the `wasmtime` CLI has no way
//! to supply a global import, so the only thing observable about it is that the module is
//! refused for the missing import rather than for its offset.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::stages::{case_i32, expect_rejected, expect_trap, run_cases_with, trap, Stage, Test};
use crate::wasm::{
    const_global_get, const_i32, ftype, global_i32, op, Data, DataMode, Expr, Func, ImportKind,
    Limits, Module, ModuleBuilder, ValType,
};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 32,
        slug: "data_segments",
        name: "Active and passive data segments",
        ext: false,
        hints: &[
            "Copy every active segment into its memory at instantiation, in the order the section lists them, before the start function runs and before any export is callable",
            "A passive segment is copied nowhere: keep its bytes in the instance for `memory.init` and leave the memory as it was",
            "A segment that does not fit is an instantiation trap, not a validation error — the module validates, then instantiation fails with `out of bounds memory access` and nothing runs",
            "The offset is a constant expression, so it may be a `global.get` of an immutable global rather than a literal, and its value is only known once the instance is being built",
        ],
        examples,
        tests: vec![
            Test::new("an active segment is in memory before any code runs", active_is_there),
            Test::new("several active segments each land at their own offset", several_segments),
            Test::new("a later segment overwrites an earlier one where they overlap", overlap),
            Test::new("a segment offset may be a global.get of an immutable global", global_offset),
            Test::new("an empty segment and one that ends exactly at the end of memory", edges),
            Test::new("a segment that runs past the end of memory is an instantiation trap", past_the_end),
            Test::new("a passive segment is not copied into memory", passive_is_not_copied),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Building blocks
// ---------------------------------------------------------------------------------------

/// One page, in bytes.
const PAGE: i32 = 65_536;

/// A builder with one page of memory and nothing else in it.
fn one_page(label: &str) -> ModuleBuilder {
    ModuleBuilder::new(label).memory(Limits::min(1))
}

/// A module with one page of memory exporting `f: () -> i32`.
fn mem_i32(b: ModuleBuilder, body: Expr) -> Module {
    let mut b = b;
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(ty, Func::new(body));
    b.export_func("f", idx).build()
}

/// `i32.load8_u` at `addr`.
fn get8(addr: i32) -> Expr {
    Expr::new().i32_const(addr).mem(op::I32_LOAD8_U, 0, 0)
}

/// `i32.load` at `addr`.
fn get32(addr: i32) -> Expr {
    Expr::new().i32_const(addr).mem(op::I32_LOAD, 2, 0)
}

/// An active data segment for memory 0 with a `global.get` offset.
fn data_at_global(global: u32, bytes: &[u8]) -> Data {
    Data {
        mode: DataMode::Active {
            memory: 0,
            offset: const_global_get(global),
        },
        bytes: bytes.to_vec(),
    }
}

/// An active data segment for memory 0 at a literal offset, written out so the offset may
/// be a value `data_active` would not take.
fn data_at(offset: i32, bytes: &[u8]) -> Data {
    Data {
        mode: DataMode::Active {
            memory: 0,
            offset: const_i32(offset),
        },
        bytes: bytes.to_vec(),
    }
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

wasm_test!(active_is_there, |ctx| {
    // Not one of these cases writes anything: the bytes are in memory before `f0` starts.
    run_cases_with(
        ctx,
        one_page("active-data").data_active(0, b"hello"),
        vec![
            case_i32("the first byte of the segment is 'h'", get8(0), 0x68),
            case_i32("the last byte of the segment is 'o'", get8(4), 0x6f),
            case_i32("the byte after the segment is still zero", get8(5), 0),
            case_i32(
                "the first four bytes read as one i32 are 'h' 'e' 'l' 'l' little-endian",
                get32(0),
                0x6c6c_6568,
            ),
            case_i32("and the rest of the page is untouched", get32(1024), 0),
        ],
    )
});

wasm_test!(several_segments, |ctx| {
    run_cases_with(
        ctx,
        one_page("several-data")
            .data_active(0, b"AB")
            .data_active(16, b"CD")
            .data_active(1000, b"EF"),
        vec![
            case_i32("segment 0 put 'A' at 0", get8(0), 0x41),
            case_i32("segment 0 put 'B' at 1", get8(1), 0x42),
            case_i32(
                "the gap between segment 0 and segment 1 is zero",
                get8(2),
                0,
            ),
            case_i32("segment 1 put 'C' at 16", get8(16), 0x43),
            case_i32("segment 1 put 'D' at 17", get8(17), 0x44),
            case_i32("segment 2 put 'E' at 1000", get8(1000), 0x45),
            case_i32("segment 2 put 'F' at 1001", get8(1001), 0x46),
            case_i32("and nothing landed at 1002", get8(1002), 0),
        ],
    )
});

wasm_test!(overlap, |ctx| {
    ctx.note("segments are applied in section order, so the last writer of a byte wins");
    run_cases_with(
        ctx,
        one_page("overlapping-data")
            .data_active(0, b"aaaaaaaa")
            .data_active(4, b"ZZ"),
        vec![
            case_i32("byte 3 is outside the overlap and still 'a'", get8(3), 0x61),
            case_i32("byte 4 is in the overlap and is 'Z'", get8(4), 0x5a),
            case_i32("byte 5 is in the overlap and is 'Z'", get8(5), 0x5a),
            case_i32("byte 6 is past the overlap and still 'a'", get8(6), 0x61),
            case_i32("byte 7 is the last of the first segment", get8(7), 0x61),
            case_i32("byte 8 was never written", get8(8), 0),
        ],
    )
});

wasm_test!(global_offset, |ctx| {
    // Two immutable globals, 16 and 32, each naming where a segment goes.
    run_cases_with(
        ctx,
        one_page("data-defined-global")
            .global(global_i32(16, false))
            .global(global_i32(32, false))
            .data(data_at_global(0, b"GG"))
            .data(data_at_global(1, b"HH")),
        vec![
            case_i32(
                "the first segment landed where global 0 says",
                get8(16),
                0x47,
            ),
            case_i32("and covered the byte after it", get8(17), 0x47),
            case_i32("the byte before it was never written", get8(15), 0),
            case_i32(
                "the second segment landed where global 1 says",
                get8(32),
                0x48,
            ),
            case_i32("and nothing landed at address 0", get8(0), 0),
        ],
    )?;
    ctx.note(
        "wasmtime accepts a defined immutable global here; WebAssembly 2.0 allows only an \
         imported one, and the later relaxation allows any preceding immutable global",
    );

    // A mutable global is not a constant expression under any version of the rule.
    let mutable = mem_i32(
        one_page("data-mutable-global")
            .global(global_i32(16, true))
            .data(data_at_global(0, b"G")),
        get8(16),
    );
    expect_rejected(
        ctx,
        &mutable,
        "a mutable global is not a constant expression, so it cannot be a segment offset",
    )?;

    // The imported form the spec has always allowed. `wasmtime run` cannot supply a global
    // import, so what is observable is a missing import, not a bad offset.
    let imported = mem_i32(
        one_page("data-imported-global")
            .import(
                "env",
                "base",
                ImportKind::Global {
                    ty: ValType::I32,
                    mutable: false,
                },
            )
            .data(data_at_global(0, b"G")),
        get8(16),
    );
    let run = ctx.invoke(&imported, "f", &[])?;
    let mut c = Check::new("a data offset reading an imported immutable global", &run);
    c.module(&imported);
    c.that(
        "exit",
        "a non-zero status: the CLI cannot supply `env.base`",
        run.exit.failed(),
        run.exit.label(),
    );
    c.that(
        "stdout",
        "nothing — the instance was never built",
        run.stdout.is_empty(),
        run.stdout.clone(),
    );
    c.that(
        "stderr",
        "a complaint about the missing import, not about the segment",
        run.stderr_has("import"),
        crate::stages::first_meaningful_line(&run.stderr),
    );
    c.note(
        "the offset itself is legal here; the module cannot be instantiated because nothing \
         provides the global it reads",
    );
    c.finish()?;
    Ok(())
});

wasm_test!(edges, |ctx| {
    run_cases_with(
        ctx,
        one_page("edge-data")
            .data(data_at(0, b""))
            .data(data_at(PAGE - 4, b"ABCD"))
            .data(data_at(PAGE, b"")),
        vec![
            case_i32("the empty segment at 0 wrote nothing", get8(0), 0),
            case_i32(
                "the segment ending at the last byte put 'A' at 65532",
                get8(PAGE - 4),
                0x41,
            ),
            case_i32("and 'D' in the very last byte", get8(PAGE - 1), 0x44),
            case_i32(
                "its four bytes read as one i32 at the last legal address",
                get32(PAGE - 4),
                0x4443_4241,
            ),
            case_i32("the byte before it was never written", get8(PAGE - 5), 0),
            case_i32(
                "and the module instantiated at all, so its empty segment at offset 65536 is legal",
                Expr::new().memory_size(),
                1,
            ),
        ],
    )
});

wasm_test!(past_the_end, |ctx| {
    // Each of these modules validates. Instantiation is where it falls over, which is why
    // the reason is a trap and not a decode error.
    ctx.note("this is an instantiation trap, after validation: the module itself is well formed");
    let cases: &[(&str, i32, &[u8])] = &[
        ("data-past-by-one", PAGE - 3, b"ABCD"),
        ("data-at-the-end", PAGE, b"A"),
        ("data-empty-past-the-end", PAGE + 1, b""),
        ("data-far-away", 1_000_000, b"A"),
    ];
    for (label, offset, bytes) in cases {
        let m = mem_i32(one_page(label).data(data_at(*offset, bytes)), get8(0));
        let run = expect_trap(ctx, &m, "f", &[], trap::MEMORY_OUT_OF_BOUNDS)?;
        let mut c = Check::new(
            format!("that '{label}' never runs a single instruction"),
            &run,
        );
        c.module(&m);
        c.that(
            "stdout",
            "nothing at all: instantiation failed before `f` could be called",
            run.stdout.is_empty(),
            run.stdout.clone(),
        );
        c.finish()?;
    }
    Ok(())
});

wasm_test!(passive_is_not_copied, |ctx| {
    run_cases_with(
        ctx,
        one_page("passive-data")
            .data_passive(b"hello")
            .data_active(8, b"X"),
        vec![
            case_i32("the passive segment did not land at 0", get8(0), 0),
            case_i32("nor anywhere else at the start of memory", get32(0), 0),
            case_i32("nor one byte in", get8(1), 0),
            case_i32(
                "the active segment in the same module did land",
                get8(8),
                0x58,
            ),
            case_i32("and the byte after it is zero", get8(9), 0),
            case_i32(
                "so a passive segment costs the memory nothing until memory.init runs",
                get32(5),
                0x5800_0000,
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// Examples
// ---------------------------------------------------------------------------------------

/// `f() -> i32`: an active segment, read back without writing anything.
fn active_example() -> Module {
    mem_i32(one_page("active-hello").data_active(0, b"hello"), get8(0))
}

/// `f() -> i32`: a segment four bytes long whose last byte falls outside the memory.
fn overrunning_example() -> Module {
    mem_i32(
        one_page("data-overruns").data(data_at(PAGE - 3, b"ABCD")),
        get8(0),
    )
}

/// Worked examples: the segment that is simply there, and the one that never gets there.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("Bytes that are there before the code is", active_example)
            .summary(
                "one page of memory, an active data segment putting \"hello\" at offset 0, \
                 and a body that only reads byte 0",
            )
            .command("run --invoke f mod.wasm")
            .output("104")
            .note(
                "104 is 'h'. Nothing in the function wrote it: an active segment is copied \
                 in while the instance is being built, which is why the export can read it \
                 on its first instruction.",
            ),
        ExampleSpec::module("A segment that does not fit", overrunning_example)
            .summary(
                "the same module with a four-byte segment at offset 65 533, three bytes \
                 short of the end of the page",
            )
            .command("run --invoke f mod.wasm")
            .output(
                "nothing on stdout, `wasm trap: out of bounds memory access` on stderr, and \
                 a non-zero exit status",
            )
            .note(
                "This module validates perfectly well; it is instantiation that fails, and \
                 it fails before `f` is entered. Refusing it during validation is the wrong \
                 place — an offset may be a `global.get` whose value only the imports know.",
            ),
    ]
}
