//! Stage 33 — `memory.init` and `data.drop`. **[ext]**
//!
//! `memory.init seg` takes three `i32`s off the stack — destination, source offset within
//! the segment, and length, in that order — and copies that many bytes out of passive data
//! segment `seg` into memory. It is the instruction an active segment is sugar for, except
//! that the module decides when it happens.
//!
//! Both ranges are checked, and both are checked **before** anything is copied: the
//! destination must satisfy `d + n <= memory size` and the source `s + n <= segment length`,
//! or the instruction traps with the usual `out of bounds memory access`. Length 0 is legal
//! wherever both ends are still inside their range, which includes the very end of memory
//! and the very end of the segment, and it copies nothing.
//!
//! `data.drop seg` releases the segment's bytes. The spec models it as replacing the segment
//! with an empty one rather than as a flag, and `wasmtime` behaves exactly that way, which
//! has two visible consequences this stage pins down: dropping an already-dropped segment is
//! not an error, and a `memory.init` of **length 0** on a dropped segment is not an error
//! either, because `0 + 0 <= 0` still holds. Only a copy of one byte or more from a dropped
//! segment traps. A runtime that keeps a "dropped" flag and traps on any `memory.init` that
//! names the segment fails that last pair of cases.
//!
//! The segment index is a validation-time thing: it is checked against the data count
//! section, so `memory.init 7` in a module with two segments never runs at all.

use crate::examples::ExampleSpec;
use crate::stages::{case_i32, case_void_trap, expect_rejected, run_cases_with, trap, Stage, Test};
use crate::wasm::{ftype, op, Expr, Func, Limits, Module, ModuleBuilder, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 33,
        slug: "memory_init",
        name: "memory.init and data.drop",
        ext: true,
        hints: &[
            "`memory.init seg` pops length, then source offset, then destination — the operands are pushed in the order destination, source, length, so pop them the other way round",
            "Check both ranges before copying anything: `d + n` against the memory size and `s + n` against the segment length, in arithmetic wide enough that a huge length cannot wrap",
            "Length 0 is legal wherever both ends are in range, which includes the end of memory and the end of the segment; it copies nothing and must not trap",
            "`data.drop` replaces the segment with an empty one rather than setting a flag: dropping twice is fine, and a later `memory.init` of length 0 from it is fine too",
        ],
        examples,
        tests: vec![
            Test::new("memory.init copies a whole passive segment", whole_segment).ext(),
            Test::new("memory.init copies a slice from the middle of a segment", middle).ext(),
            Test::new("a memory.init of length zero is legal at either end", zero_length).ext(),
            Test::new("a destination past the end of memory traps", dest_out_of_range).ext(),
            Test::new("a source range past the end of the segment traps", source_out_of_range).ext(),
            Test::new("data.drop empties a segment, and dropping twice is not an error", drop_it).ext(),
            Test::new("memory.init of a segment that does not exist is a validation error", unknown_segment)
                .ext(),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Building blocks
// ---------------------------------------------------------------------------------------

/// One page, in bytes.
const PAGE: i32 = 65_536;

/// The first passive segment every module in this stage carries.
const SEG0: &[u8] = b"abcdefgh";

/// The second one, so a `data.drop` of the first can be shown not to touch it.
const SEG1: &[u8] = b"XYZ";

/// A builder with one page of memory and the stage's two passive segments.
fn two_segments(label: &str) -> ModuleBuilder {
    ModuleBuilder::new(label)
        .memory(Limits::min(1))
        .data_passive(SEG0)
        .data_passive(SEG1)
}

/// A module with one page of memory and the two segments, exporting `f: () -> i32`.
fn mem_i32(b: ModuleBuilder, body: Expr) -> Module {
    let mut b = b;
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(ty, Func::new(body));
    b.export_func("f", idx).build()
}

/// `memory.init seg` with literal destination, source and length.
fn init(seg: u32, dest: i32, src: i32, len: i32) -> Expr {
    Expr::new()
        .i32_const(dest)
        .i32_const(src)
        .i32_const(len)
        .memory_init(seg)
}

/// `i32.load8_u` at `addr`.
fn get8(addr: i32) -> Expr {
    Expr::new().i32_const(addr).mem(op::I32_LOAD8_U, 0, 0)
}

/// `i32.load` at `addr`.
fn get32(addr: i32) -> Expr {
    Expr::new().i32_const(addr).mem(op::I32_LOAD, 2, 0)
}

/// `i32.store8` of `value` at `addr`.
fn put8(addr: i32, value: i32) -> Expr {
    Expr::new()
        .i32_const(addr)
        .i32_const(value)
        .mem(op::I32_STORE8, 0, 0)
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

wasm_test!(whole_segment, |ctx| {
    run_cases_with(
        ctx,
        two_segments("init-whole"),
        vec![
            case_i32(
                "the first byte of the segment lands at the destination",
                init(0, 0, 0, 8).then(get8(0)),
                0x61,
            ),
            case_i32(
                "the last byte lands seven bytes further on",
                init(0, 0, 0, 8).then(get8(7)),
                0x68,
            ),
            case_i32(
                "and nothing lands past the end of the copy",
                init(0, 0, 0, 8).then(get8(8)),
                0,
            ),
            case_i32(
                "the four bytes 'abcd' read as one i32",
                init(0, 0, 0, 8).then(get32(0)),
                0x6463_6261,
            ),
            case_i32(
                "the destination need not be 0",
                init(0, 1000, 0, 8).then(get8(1000)),
                0x61,
            ),
            case_i32(
                "and the bytes before it are untouched",
                init(0, 1000, 0, 8).then(get8(999)),
                0,
            ),
            case_i32(
                "the second segment is a different segment",
                init(1, 0, 0, 3).then(get8(0)),
                0x58,
            ),
            case_i32(
                "two inits from two segments can share a memory",
                init(0, 0, 0, 8).then(init(1, 16, 0, 3)).then(get8(16)),
                0x58,
            ),
        ],
    )
});

wasm_test!(middle, |ctx| {
    run_cases_with(
        ctx,
        two_segments("init-middle"),
        vec![
            case_i32(
                "source offset 2, length 3 starts at 'c'",
                init(0, 0, 2, 3).then(get8(0)),
                0x63,
            ),
            case_i32(
                "and the second byte is 'd'",
                init(0, 0, 2, 3).then(get8(1)),
                0x64,
            ),
            case_i32("and the third is 'e'", init(0, 0, 2, 3).then(get8(2)), 0x65),
            case_i32(
                "and the fourth byte of memory was never written",
                init(0, 0, 2, 3).then(get8(3)),
                0,
            ),
            case_i32(
                "source offset 7, length 1 is the last byte of the segment",
                init(0, 0, 7, 1).then(get8(0)),
                0x68,
            ),
            case_i32(
                "a later init can overwrite an earlier one",
                init(0, 0, 0, 8).then(init(0, 0, 4, 1)).then(get8(0)),
                0x65,
            ),
            case_i32(
                "and leaves the bytes it did not cover alone",
                init(0, 0, 0, 8).then(init(0, 0, 4, 1)).then(get8(1)),
                0x62,
            ),
        ],
    )
});

wasm_test!(zero_length, |ctx| {
    run_cases_with(
        ctx,
        two_segments("init-zero-length"),
        vec![
            case_i32(
                "a zero-length init at 0 copies nothing",
                put8(0, 7).then(init(0, 0, 0, 0)).then(get8(0)),
                7,
            ),
            case_i32(
                "a zero-length init at the very end of memory is legal",
                init(0, PAGE, 0, 0).i32_const(1),
                1,
            ),
            case_i32(
                "so is one whose source offset is the end of the segment",
                init(0, 0, 8, 0).i32_const(1),
                1,
            ),
            case_i32(
                "both at once is legal too",
                init(0, PAGE, 8, 0).i32_const(1),
                1,
            ),
            case_void_trap(
                "but a destination one byte past the end traps even with length 0",
                init(0, PAGE + 1, 0, 0),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "and so does a source offset one byte past the end of the segment",
                init(0, 0, 9, 0),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
        ],
    )
});

wasm_test!(dest_out_of_range, |ctx| {
    run_cases_with(
        ctx,
        two_segments("init-dest-range"),
        vec![
            case_i32(
                "a copy ending exactly at the end of memory is the last legal one",
                init(0, PAGE - 8, 0, 8).then(get8(PAGE - 1)),
                0x68,
            ),
            case_void_trap(
                "one byte further on and it traps",
                init(0, PAGE - 7, 0, 8),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "a destination at the end of memory with a real length traps",
                init(0, PAGE, 0, 1),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "a destination of -1 is 4294967295 and traps",
                init(0, -1, 0, 1),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "a destination far outside the memory traps",
                init(0, 1_000_000, 0, 1),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_i32(
                "the byte before the last legal copy is untouched by it",
                init(0, PAGE - 8, 0, 8).then(get8(PAGE - 9)),
                0,
            ),
        ],
    )
});

wasm_test!(source_out_of_range, |ctx| {
    ctx.note("segment 0 is eight bytes long, so s + n must be no more than 8");
    run_cases_with(
        ctx,
        two_segments("init-source-range"),
        vec![
            case_i32(
                "s=0 n=8 is the whole segment and is legal",
                init(0, 0, 0, 8).then(get8(7)),
                0x68,
            ),
            case_void_trap(
                "s=1 n=8 reads one byte past the end of the segment",
                init(0, 0, 1, 8),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "s=8 n=1 does the same",
                init(0, 0, 8, 1),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "s=0 n=9 does the same",
                init(0, 0, 0, 9),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "a source offset of -1 is 4294967295 and traps",
                init(0, 0, -1, 1),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "a length of -1 is 4294967295 and traps rather than wrapping",
                init(0, 0, 0, -1),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "the three-byte segment 1 will not give up a fourth byte",
                init(1, 0, 0, 4),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_i32(
                "though it will give up all three",
                init(1, 0, 0, 3).then(get8(2)),
                0x5a,
            ),
        ],
    )
});

wasm_test!(drop_it, |ctx| {
    ctx.note(
        "data.drop replaces the segment with an empty one, so a zero-length init on a \
         dropped segment is still in range and still legal",
    );
    let drop0 = Expr::new().data_drop(0);
    run_cases_with(
        ctx,
        two_segments("data-drop"),
        vec![
            case_void_trap(
                "an init of one byte after the drop traps",
                drop0.clone().then(init(0, 0, 0, 1)),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_void_trap(
                "and so does an init of the whole segment",
                drop0.clone().then(init(0, 0, 0, 8)),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
            case_i32(
                "dropping the same segment twice is not an error",
                drop0.clone().then(drop0.clone()).i32_const(1),
                1,
            ),
            case_i32(
                "a zero-length init on a dropped segment is still legal",
                drop0.clone().then(init(0, 0, 0, 0)).i32_const(1),
                1,
            ),
            case_i32(
                "what an earlier init copied stays copied",
                init(0, 0, 0, 8).then(drop0.clone()).then(get8(0)),
                0x61,
            ),
            case_i32(
                "dropping segment 0 does not drop segment 1",
                drop0.clone().then(init(1, 0, 0, 3)).then(get8(0)),
                0x58,
            ),
            case_void_trap(
                "and dropping segment 1 as well leaves nothing to copy",
                drop0.clone().data_drop(1).then(init(1, 0, 0, 1)),
                &[trap::MEMORY_OUT_OF_BOUNDS],
            ),
        ],
    )
});

wasm_test!(unknown_segment, |ctx| {
    // The data count section says how many segments there are; an index past it is a
    // validation error, so nothing runs.
    let past_the_end = mem_i32(two_segments("init-seg-2"), init(2, 0, 0, 1).i32_const(0));
    expect_rejected(
        ctx,
        &past_the_end,
        "the module declares two data segments, so segment 2 does not exist",
    )?;

    let far = mem_i32(two_segments("init-seg-7"), init(7, 0, 0, 1).i32_const(0));
    expect_rejected(ctx, &far, "segment 7 does not exist either")?;

    let dropped = mem_i32(
        two_segments("drop-seg-2"),
        Expr::new().data_drop(2).i32_const(0),
    );
    expect_rejected(ctx, &dropped, "data.drop names the same index space")?;

    // A module with a memory but no data section at all: there is no segment 0 to name, and
    // no data count section to name it in.
    let no_data = mem_i32(
        ModuleBuilder::new("init-no-data").memory(Limits::min(1)),
        init(0, 0, 0, 1).i32_const(0),
    );
    expect_rejected(
        ctx,
        &no_data,
        "memory.init in a module with no data count section has no segment to name",
    )?;
    Ok(())
});

// ---------------------------------------------------------------------------------------
// Examples
// ---------------------------------------------------------------------------------------

/// `f() -> i32`: copy the whole passive segment to address 0, then read its first byte.
fn init_example() -> Module {
    mem_i32(two_segments("init-hello"), init(0, 0, 0, 8).then(get8(0)))
}

/// `f() -> i32`: drop the segment, then try to copy a byte out of it.
fn drop_example() -> Module {
    mem_i32(
        two_segments("init-after-drop"),
        Expr::new()
            .data_drop(0)
            .then(init(0, 0, 0, 1))
            .then(get8(0)),
    )
}

/// Worked examples: the copy, and the copy that comes too late.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("Copying a passive segment into memory", init_example)
            .summary(
                "one page of memory, a passive segment holding \"abcdefgh\", and a body of \
                 `i32.const 0; i32.const 0; i32.const 8; memory.init 0` followed by a read \
                 of byte 0",
            )
            .command("run --invoke f mod.wasm")
            .output("97")
            .note(
                "97 is 'a'. Before the `memory.init` that byte was 0: a passive segment is \
                 not copied at instantiation, which is the whole difference between it and \
                 an active one.",
            ),
        ExampleSpec::module("An init after the segment was dropped", drop_example)
            .summary("the same module with a `data.drop 0` in front of the `memory.init`")
            .command("run --invoke f mod.wasm")
            .output(
                "nothing on stdout, `wasm trap: out of bounds memory access` on stderr, and \
                 a non-zero exit status",
            )
            .note(
                "A dropped segment is an empty segment, so asking for one byte of it is a \
                 source range of 0..1 against a length of 0 — out of bounds. Ask for zero \
                 bytes instead and the same module runs to the end.",
            ),
    ]
}
