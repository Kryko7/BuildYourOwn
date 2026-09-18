//! Stage 28 — loads and stores: every width and signedness.
//!
//! The whole stage runs against one page of memory declared `min 1, no max`, and every case
//! is its own export invoked in its own process, so a case that wants to see a byte has to
//! write it first: a fresh instance always starts from a page of zeroes.
//!
//! Two things are worth saying out loud. The signed and unsigned narrow loads are the same
//! instruction with a different sign extension — `i32.load8_s` of `0xff` is `-1` and
//! `i32.load8_u` of the same byte is `255` — and that is the only difference between them;
//! neither of them touches the byte. And a narrow *store* is not a wide store with a mask:
//! `i32.store8` writes exactly one byte and must leave its neighbours alone, which is what
//! the "only its own bytes" test is for.
//!
//! The float tests compare through `i32.reinterpret_f32` / `i64.reinterpret_f64` rather than
//! through the printed decimal, so nothing here depends on how a runtime formats a float —
//! a stage-19 concern, not a stage-28 one.

use crate::examples::ExampleSpec;
use crate::stages::{case_f32, case_f64, case_i32, case_i64, run_cases_with, Stage, Test};
use crate::wasm::{ftype, op, Expr, Func, Limits, Module, ModuleBuilder, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 28,
        slug: "loads_and_stores",
        name: "Loads and stores: every width and signedness",
        ext: false,
        hints: &[
            "A load and a store both take a memory immediate — an alignment hint and a static offset — before they touch the stack; decode both even when you ignore the alignment",
            "The narrow loads come in pairs: `load8_s` sign-extends the byte into the result type, `load8_u` zero-extends it, and 0xff is -1 or 255 depending on which one you wrote",
            "A narrow store truncates: `i32.store8` writes the low byte of the operand and nothing else, so the three bytes after it must still hold whatever they held before",
            "Memory starts as a page of zeroes, and a store followed by a load of the same address and width must give back exactly what went in — floats included, bit for bit",
        ],
        examples,
        tests: vec![
            Test::new("a fresh memory is a page of zeroes", zeroed),
            Test::new("an 8-bit load extends according to its signedness", eight_bit),
            Test::new("a 16-bit load extends according to its signedness", sixteen_bit),
            Test::new("a 32-bit load into an i64 extends according to its signedness", thirty_two_bit),
            Test::new("a narrow store writes only its own bytes", narrow_store),
            Test::new("every integer store and load round-trips", integer_round_trip),
            Test::new("a float store and load round-trips bit for bit", float_round_trip),
            Test::new("a narrow load takes the low bytes of a wide store", narrow_load),
            Test::new("a load and a store move bytes, not types", bytes_not_types),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Building blocks
// ---------------------------------------------------------------------------------------

/// A builder with one page of memory and nothing else in it.
fn one_page(label: &str) -> ModuleBuilder {
    ModuleBuilder::new(label).memory(Limits::min(1))
}

/// A module with one page of memory exporting `f: () -> i32`.
fn mem_i32(label: &str, body: Expr) -> Module {
    let mut b = one_page(label);
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(ty, Func::new(body));
    b.export_func("f", idx).build()
}

/// A store of `value` at the literal address `addr`, at the given width.
fn store(addr: i32, op: crate::wasm::Op, align: u32, value: Expr) -> Expr {
    Expr::new().i32_const(addr).then(value).mem(op, align, 0)
}

/// A load at the literal address `addr`, at the given width.
fn load(addr: i32, op: crate::wasm::Op, align: u32) -> Expr {
    Expr::new().i32_const(addr).mem(op, align, 0)
}

/// `i32.store8` of `value` at `addr`.
fn put8(addr: i32, value: i32) -> Expr {
    store(addr, op::I32_STORE8, 0, Expr::new().i32_const(value))
}

/// `i32.load8_u` at `addr`.
fn get8(addr: i32) -> Expr {
    load(addr, op::I32_LOAD8_U, 0)
}

/// Write `value` as an `i32` at `addr`, then read it back at the given width.
fn round_trip_i32(addr: i32, value: i32, back: crate::wasm::Op, align: u32) -> Expr {
    store(addr, op::I32_STORE, 2, Expr::new().i32_const(value)).then(load(addr, back, align))
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

wasm_test!(zeroed, |ctx| {
    // Nothing is written anywhere in this sweep: every case only reads.
    run_cases_with(
        ctx,
        one_page("zeroed"),
        vec![
            case_i32("i32.load8_u at 0", get8(0), 0),
            case_i32("i32.load8_s at 0", load(0, op::I32_LOAD8_S, 0), 0),
            case_i32("i32.load16_u at 2", load(2, op::I32_LOAD16_U, 1), 0),
            case_i32("i32.load at 1024", load(1024, op::I32_LOAD, 2), 0),
            case_i64("i64.load at 32768", load(32768, op::I64_LOAD, 3), 0),
            case_i32(
                "i32.load at 65532, the last four bytes of the page",
                load(65532, op::I32_LOAD, 2),
                0,
            ),
            case_i32("i32.load8_u at 65535, the last byte", get8(65535), 0),
            case_f32(
                "f32.load at 0 is a positive zero",
                load(0, op::F32_LOAD, 2),
                0.0,
            ),
            case_f64(
                "f64.load at 8 is a positive zero",
                load(8, op::F64_LOAD, 3),
                0.0,
            ),
        ],
    )
});

wasm_test!(eight_bit, |ctx| {
    // One byte, read six ways. The byte is written first so the case is self-contained.
    run_cases_with(
        ctx,
        one_page("load8"),
        vec![
            case_i32(
                "i32.load8_s of 0xff is -1",
                put8(0, 0xff).then(load(0, op::I32_LOAD8_S, 0)),
                -1,
            ),
            case_i32(
                "i32.load8_u of 0xff is 255",
                put8(0, 0xff).then(get8(0)),
                255,
            ),
            case_i64(
                "i64.load8_s of 0xff is -1",
                put8(0, 0xff).then(load(0, op::I64_LOAD8_S, 0)),
                -1,
            ),
            case_i64(
                "i64.load8_u of 0xff is 255",
                put8(0, 0xff).then(load(0, op::I64_LOAD8_U, 0)),
                255,
            ),
            case_i32(
                "i32.load8_s of 0x80 is -128, the most negative byte",
                put8(0, 0x80).then(load(0, op::I32_LOAD8_S, 0)),
                -128,
            ),
            case_i32(
                "i32.load8_u of 0x80 is 128",
                put8(0, 0x80).then(get8(0)),
                128,
            ),
            case_i32(
                "i32.load8_s of 0x7f is 127, where the two agree",
                put8(0, 0x7f).then(load(0, op::I32_LOAD8_S, 0)),
                127,
            ),
        ],
    )
});

wasm_test!(sixteen_bit, |ctx| {
    let put16 = |v: i32| store(0, op::I32_STORE16, 1, Expr::new().i32_const(v));
    run_cases_with(
        ctx,
        one_page("load16"),
        vec![
            case_i32(
                "i32.load16_s of 0xffff is -1",
                put16(0xffff).then(load(0, op::I32_LOAD16_S, 1)),
                -1,
            ),
            case_i32(
                "i32.load16_u of 0xffff is 65535",
                put16(0xffff).then(load(0, op::I32_LOAD16_U, 1)),
                65535,
            ),
            case_i32(
                "i32.load16_s of 0x8000 is -32768",
                put16(0x8000).then(load(0, op::I32_LOAD16_S, 1)),
                -32768,
            ),
            case_i32(
                "i32.load16_u of 0x8000 is 32768",
                put16(0x8000).then(load(0, op::I32_LOAD16_U, 1)),
                32768,
            ),
            case_i32(
                "i32.load16_s of 0x7fff is 32767, where the two agree",
                put16(0x7fff).then(load(0, op::I32_LOAD16_S, 1)),
                32767,
            ),
            case_i64(
                "i64.load16_s of 0xffff is -1",
                put16(0xffff).then(load(0, op::I64_LOAD16_S, 1)),
                -1,
            ),
            case_i64(
                "i64.load16_u of 0xffff is 65535",
                put16(0xffff).then(load(0, op::I64_LOAD16_U, 1)),
                65535,
            ),
            case_i32(
                "i32.store16 of 0x12345678 keeps only 0x5678",
                put16(0x1234_5678).then(load(0, op::I32_LOAD16_U, 1)),
                0x5678,
            ),
        ],
    )
});

wasm_test!(thirty_two_bit, |ctx| {
    run_cases_with(
        ctx,
        one_page("load32"),
        vec![
            case_i64(
                "i64.load32_s of 0xffffffff is -1",
                round_trip_i32(0, -1, op::I64_LOAD32_S, 2),
                -1,
            ),
            case_i64(
                "i64.load32_u of 0xffffffff is 4294967295",
                round_trip_i32(0, -1, op::I64_LOAD32_U, 2),
                4_294_967_295,
            ),
            case_i64(
                "i64.load32_s of 0x80000000 is -2147483648",
                round_trip_i32(0, i32::MIN, op::I64_LOAD32_S, 2),
                -2_147_483_648,
            ),
            case_i64(
                "i64.load32_u of 0x80000000 is 2147483648",
                round_trip_i32(0, i32::MIN, op::I64_LOAD32_U, 2),
                2_147_483_648,
            ),
            case_i64(
                "i64.load32_s of 0x7fffffff is 2147483647, where the two agree",
                round_trip_i32(0, i32::MAX, op::I64_LOAD32_S, 2),
                2_147_483_647,
            ),
            case_i64(
                "a full i64.load sees the four written bytes and four zeroes",
                round_trip_i32(0, -1, op::I64_LOAD, 3),
                4_294_967_295,
            ),
            case_i64(
                "i64.store32 writes the low half of its operand",
                store(
                    0,
                    op::I64_STORE32,
                    2,
                    Expr::new().i64_const(0x1234_5678_9abc_def0u64 as i64),
                )
                .then(load(0, op::I64_LOAD32_U, 2)),
                0x9abc_def0,
            ),
        ],
    )
});

wasm_test!(narrow_store, |ctx| {
    // Every case writes into an otherwise untouched page, so a neighbour that is not zero
    // is a neighbour the store clobbered.
    run_cases_with(
        ctx,
        one_page("narrow-store"),
        vec![
            case_i32(
                "i32.store8 of 0xffffffff at 4 writes 0xff there",
                put8(4, -1).then(get8(4)),
                255,
            ),
            case_i32(
                "the byte before an i32.store8 is untouched",
                put8(4, -1).then(get8(3)),
                0,
            ),
            case_i32(
                "the byte after an i32.store8 is untouched",
                put8(4, -1).then(get8(5)),
                0,
            ),
            case_i32(
                "the third byte after an i32.store8 is untouched",
                put8(4, -1).then(get8(7)),
                0,
            ),
            case_i32(
                "an i32.store8 does not reach four bytes, as i32.load at 4 shows",
                put8(4, -1).then(load(4, op::I32_LOAD, 2)),
                255,
            ),
            case_i32(
                "i32.store16 of 0xffffffff writes two bytes",
                store(8, op::I32_STORE16, 1, Expr::new().i32_const(-1)).then(load(
                    8,
                    op::I32_LOAD,
                    2,
                )),
                65535,
            ),
            case_i32(
                "i64.store32 of -1 writes four bytes and no fifth",
                store(16, op::I64_STORE32, 2, Expr::new().i64_const(-1)).then(get8(20)),
                0,
            ),
            case_i64(
                "i64.store8 of -1 writes one byte and no second",
                store(24, op::I64_STORE8, 0, Expr::new().i64_const(-1)).then(load(
                    24,
                    op::I64_LOAD,
                    3,
                )),
                255,
            ),
        ],
    )
});

wasm_test!(integer_round_trip, |ctx| {
    run_cases_with(
        ctx,
        one_page("int-round-trip"),
        vec![
            case_i32(
                "i32.store then i32.load of 0x12345678",
                round_trip_i32(0, 0x1234_5678, op::I32_LOAD, 2),
                0x1234_5678,
            ),
            case_i32(
                "i32.store then i32.load of -1",
                round_trip_i32(4, -1, op::I32_LOAD, 2),
                -1,
            ),
            case_i32(
                "i32.store then i32.load of INT_MIN",
                round_trip_i32(8, i32::MIN, op::I32_LOAD, 2),
                i32::MIN,
            ),
            case_i64(
                "i64.store then i64.load of 0x0123456789abcdef",
                store(
                    16,
                    op::I64_STORE,
                    3,
                    Expr::new().i64_const(0x0123_4567_89ab_cdef),
                )
                .then(load(16, op::I64_LOAD, 3)),
                0x0123_4567_89ab_cdef,
            ),
            case_i64(
                "i64.store then i64.load of INT64_MIN",
                store(24, op::I64_STORE, 3, Expr::new().i64_const(i64::MIN)).then(load(
                    24,
                    op::I64_LOAD,
                    3,
                )),
                i64::MIN,
            ),
            case_i32(
                "i32.store8 then i32.load8_u of 0xab",
                put8(32, 0xab).then(get8(32)),
                0xab,
            ),
            case_i32(
                "i32.store16 then i32.load16_u of 0xabcd",
                store(34, op::I32_STORE16, 1, Expr::new().i32_const(0xabcd)).then(load(
                    34,
                    op::I32_LOAD16_U,
                    1,
                )),
                0xabcd,
            ),
            case_i64(
                "i64.store16 then i64.load16_u of 0xbeef",
                store(36, op::I64_STORE16, 1, Expr::new().i64_const(0xbeef)).then(load(
                    36,
                    op::I64_LOAD16_U,
                    1,
                )),
                0xbeef,
            ),
            case_i64(
                "i64.store32 then i64.load32_u of 0xdeadbeef",
                store(40, op::I64_STORE32, 2, Expr::new().i64_const(0xdead_beef)).then(load(
                    40,
                    op::I64_LOAD32_U,
                    2,
                )),
                0xdead_beef,
            ),
        ],
    )
});

wasm_test!(float_round_trip, |ctx| {
    // Compared as bits, so the answer does not depend on how the runtime prints a float.
    let f32_trip = |addr: i32, bits: u32| {
        store(addr, op::F32_STORE, 2, Expr::new().f32_bits(bits))
            .then(load(addr, op::F32_LOAD, 2))
            .op(op::I32_REINTERPRET_F32)
    };
    let f64_trip = |addr: i32, bits: u64| {
        store(addr, op::F64_STORE, 3, Expr::new().f64_bits(bits))
            .then(load(addr, op::F64_LOAD, 3))
            .op(op::I64_REINTERPRET_F64)
    };
    run_cases_with(
        ctx,
        one_page("float-round-trip"),
        vec![
            case_i32(
                "f32.store then f32.load of 1.5",
                f32_trip(0, 1.5f32.to_bits()),
                1.5f32.to_bits() as i32,
            ),
            case_i32(
                "f32.store then f32.load of a negative zero",
                f32_trip(4, (-0.0f32).to_bits()),
                (-0.0f32).to_bits() as i32,
            ),
            case_i32(
                "f32.store then f32.load of an infinity",
                f32_trip(8, f32::INFINITY.to_bits()),
                f32::INFINITY.to_bits() as i32,
            ),
            case_i32(
                "f32.store then f32.load keeps a NaN payload",
                f32_trip(12, 0x7fc0_0001),
                0x7fc0_0001u32 as i32,
            ),
            case_i64(
                "f64.store then f64.load of 1.5",
                f64_trip(16, 1.5f64.to_bits()),
                1.5f64.to_bits() as i64,
            ),
            case_i64(
                "f64.store then f64.load of a negative zero",
                f64_trip(24, (-0.0f64).to_bits()),
                (-0.0f64).to_bits() as i64,
            ),
            case_i64(
                "f64.store then f64.load keeps a NaN payload",
                f64_trip(32, 0x7ff8_0000_0000_0001),
                0x7ff8_0000_0000_0001u64 as i64,
            ),
            case_f64(
                "and the value itself comes back, not only its bits",
                store(40, op::F64_STORE, 3, Expr::new().f64_const(-2.25)).then(load(
                    40,
                    op::F64_LOAD,
                    3,
                )),
                -2.25,
            ),
        ],
    )
});

wasm_test!(narrow_load, |ctx| {
    // 0x12345678 is stored little-endian as 78 56 34 12, so the low byte is at the address
    // the store named.
    let wide = |addr: i32| store(addr, op::I32_STORE, 2, Expr::new().i32_const(0x1234_5678));
    run_cases_with(
        ctx,
        one_page("narrow-load"),
        vec![
            case_i32(
                "i32.load8_u at 0 of 0x12345678 is 0x78",
                wide(0).then(get8(0)),
                0x78,
            ),
            case_i32(
                "i32.load8_u at 1 of 0x12345678 is 0x56",
                wide(0).then(get8(1)),
                0x56,
            ),
            case_i32(
                "i32.load8_u at 3 of 0x12345678 is 0x12",
                wide(0).then(get8(3)),
                0x12,
            ),
            case_i32(
                "i32.load16_u at 0 of 0x12345678 is 0x5678",
                wide(0).then(load(0, op::I32_LOAD16_U, 1)),
                0x5678,
            ),
            case_i32(
                "i32.load16_u at 2 of 0x12345678 is 0x1234",
                wide(0).then(load(2, op::I32_LOAD16_U, 1)),
                0x1234,
            ),
            case_i64(
                "i64.load32_u at 0 of a stored i64 is its low half",
                store(
                    8,
                    op::I64_STORE,
                    3,
                    Expr::new().i64_const(0x0123_4567_89ab_cdef),
                )
                .then(load(8, op::I64_LOAD32_U, 2)),
                0x89ab_cdef,
            ),
            case_i32(
                "i32.load8_u at the top byte of a stored i64 is 0x01",
                store(
                    8,
                    op::I64_STORE,
                    3,
                    Expr::new().i64_const(0x0123_4567_89ab_cdef),
                )
                .then(get8(15)),
                0x01,
            ),
        ],
    )
});

wasm_test!(bytes_not_types, |ctx| {
    // Nothing in memory remembers what wrote it: an i32.store followed by an f32.load is a
    // reinterpret that went through the page.
    run_cases_with(
        ctx,
        one_page("bytes-not-types"),
        vec![
            case_f32(
                "an i32.store of 1.5's bits reads back as 1.5 through f32.load",
                store(
                    0,
                    op::I32_STORE,
                    2,
                    Expr::new().i32_const(1.5f32.to_bits() as i32),
                )
                .then(load(0, op::F32_LOAD, 2)),
                1.5,
            ),
            case_f64(
                "an i64.store of 1.5's bits reads back as 1.5 through f64.load",
                store(
                    8,
                    op::I64_STORE,
                    3,
                    Expr::new().i64_const(1.5f64.to_bits() as i64),
                )
                .then(load(8, op::F64_LOAD, 3)),
                1.5,
            ),
            case_i32(
                "an f32.store reads back as its bits through i32.load",
                store(16, op::F32_STORE, 2, Expr::new().f32_const(1.5)).then(load(
                    16,
                    op::I32_LOAD,
                    2,
                )),
                1.5f32.to_bits() as i32,
            ),
            case_i64(
                "an f64.store of -0.0 reads back as INT64_MIN through i64.load",
                store(24, op::F64_STORE, 3, Expr::new().f64_const(-0.0)).then(load(
                    24,
                    op::I64_LOAD,
                    3,
                )),
                i64::MIN,
            ),
            case_i32(
                "four i32.store8 build a value one i32.load reads",
                put8(32, 0x78)
                    .then(put8(33, 0x56))
                    .then(put8(34, 0x34))
                    .then(put8(35, 0x12))
                    .then(load(32, op::I32_LOAD, 2)),
                0x1234_5678,
            ),
            case_f32(
                "four i32.store8 build a float one f32.load reads",
                put8(40, 0x00)
                    .then(put8(41, 0x00))
                    .then(put8(42, 0xc0))
                    .then(put8(43, 0x3f))
                    .then(load(40, op::F32_LOAD, 2)),
                1.5,
            ),
        ],
    )
});

// ---------------------------------------------------------------------------------------
// Examples
// ---------------------------------------------------------------------------------------

/// `f() -> i32`: write `0xff` at address 0, read it back signed.
fn signed_byte() -> Module {
    mem_i32(
        "load8-signed",
        put8(0, 0xff).then(load(0, op::I32_LOAD8_S, 0)),
    )
}

/// `f() -> i32`: `i32.store8` of `0xffffffff`, then an `i32.load` of all four bytes.
fn one_byte_only() -> Module {
    mem_i32(
        "store8-neighbours",
        put8(0, -1).then(load(0, op::I32_LOAD, 2)),
    )
}

/// Worked examples: the signedness pair, and the narrow store that stays narrow.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("The same byte, read signed", signed_byte)
            .summary(
                "one page of memory, `i32.store8` of 0xff at address 0, then `i32.load8_s` \
                 of the same address",
            )
            .command("run --invoke f mod.wasm")
            .output("-1")
            .note(
                "Change the last instruction to `i32.load8_u` and the same byte prints 255. \
                 The two instructions read identical memory; only the extension differs.",
            ),
        ExampleSpec::module("A store that stays one byte wide", one_byte_only)
            .summary(
                "`i32.store8` of 0xffffffff at address 0, then an `i32.load` of the four \
                 bytes starting there",
            )
            .command("run --invoke f mod.wasm")
            .output("255")
            .note(
                "The store wrote 0xff into byte 0 and nothing into bytes 1, 2 and 3, which \
                 are still the zeroes a fresh memory starts with — so the four bytes read \
                 back as 0x000000ff. A runtime that masks a full 32-bit store instead of \
                 writing one byte prints -1 here.",
            ),
    ]
}
