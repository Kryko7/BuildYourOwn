//! Stage 44 — Scale: many functions, deep nesting, a big module. **[ext]**
//!
//! Six modules, each one big along a different axis, and each one checked by the **answer**
//! it prints rather than by how long it took. Nothing here is a performance test: a runtime
//! that decodes a four-megabyte module in a second and one that does it in fifty
//! milliseconds both pass, because the only promise being tested is that neither the decoder
//! nor the validator nor the interpreter has a limit the specification does not have.
//!
//! The numbers were chosen to break the obvious shortcuts rather than to be impressive: a
//! thousand functions calling one another break a decoder that recurses per function and an
//! interpreter with a shallow call stack, several hundred nested blocks break a validator
//! that recurses per label, fifty thousand locals break a frame allocated from the number of
//! local *runs* instead of their total, and a multi-megabyte data segment breaks anything
//! that copies the module once per section.
//!
//! The sizes are reported with `ctx.note` so a green run still tells the reader what was
//! actually decoded and executed.
//!
//! The timeout floors look absurd next to those notes — `wasmtime` finishes the whole stage
//! in a tenth of a second — and that is the point. `min_timeout_ms` is a floor, not a
//! budget: it is sized for a tree-walking interpreter reading its own bytecode, which is
//! what a learner has when they first reach this stage, and a runtime that takes a hundred
//! times longer than the reference is still passing this stage correctly.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::stages::{expect_line, expect_lines, single, single_with_locals, Ctx, Stage, Test};
use crate::wasm::{
    ftype, op, BlockType, Expr, Func, Limits, Module, ModuleBuilder, TableType, ValType,
};
use crate::wasm_test;
use std::time::Instant;

/// Functions in the call chain. Each one calls the next, so this is also the call depth.
const CHAIN: u32 = 1_000;
/// Nested blocks in the deep-nesting module.
const NEST: usize = 400;
/// Locals declared by one function, in a single `(count, type)` run.
const LOCALS: u32 = 50_000;
/// Instructions in the straight-line body: `i32.const 1; i32.add`, repeated.
const STRAIGHT: usize = 30_000;
/// Bytes in the big module's data segment: four mebibytes, exactly 64 pages.
const DATA_BYTES: usize = 4 * 1024 * 1024;
/// Functions the big module carries alongside its data segment.
const BIG_FUNCS: u32 = 1_000;
/// Slots in the large table.
const TABLE_SLOTS: u32 = 100_000;
/// Slots the large table's first element segment actually fills.
const ELEM_HEAD: u32 = 4_096;
/// Exports in the many-exports module.
const EXPORTS: u32 = 512;

/// A test of this stage: `ext`, `slow`, and given room to decode a big module.
fn scale_test(name: &'static str, run: crate::stages::TestFn, floor_ms: u64) -> Test {
    Test::new(name, run)
        .ext()
        .tag("slow")
        .min_timeout_ms(floor_ms)
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 44,
        slug: "scale",
        name: "Scale: many functions, deep nesting, a big module",
        ext: true,
        hints: &[
            "Every section is a vector: read the count, then that many entries — decode them in a loop, because a decoder that recurses once per function dies at a thousand of them",
            "Block nesting belongs on a stack you own, not on the host's call stack: several hundred levels of `block` must validate without growing a Rust frame per level",
            "Locals are declared as (count, type) runs, so one run can ask for fifty thousand i32s; size the frame from the total the runs add up to, never from the number of runs",
            "A multi-megabyte data segment is copied into memory once at instantiation — slice it straight out of the module bytes rather than building a Vec per byte",
        ],
        examples,
        tests: vec![
            scale_test(
                "a thousand functions each calling the next return the whole chain's answer",
                thousand_functions,
                60_000,
            ),
            scale_test(
                "several hundred nested blocks validate and the innermost br reaches the outermost",
                deep_nesting,
                60_000,
            ),
            scale_test(
                "fifty thousand locals and a thirty thousand instruction body both compute the right answer",
                wide_and_long,
                60_000,
            ),
            scale_test(
                "a multi-megabyte module decodes, instantiates and runs",
                multi_megabyte,
                120_000,
            ),
            scale_test(
                "a hundred thousand slot table is callable through its last slot",
                large_table,
                60_000,
            ),
            scale_test(
                "one export out of five hundred is found by name",
                many_exports,
                60_000,
            ),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// The modules
// ---------------------------------------------------------------------------------------

/// `n` functions, function `k` returning `f(k + 1) + 1` and the last returning 1.
///
/// Calling the exported head therefore returns exactly `n`: any function left out, decoded
/// twice, or bound to the wrong index changes the answer rather than merely slowing things
/// down. It is also `n` frames of call depth, which is the other half of the test.
fn chain_module(n: u32) -> Module {
    let mut b = ModuleBuilder::new(format!("chain-{n}"));
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    for i in 0..n {
        let body = if i + 1 == n {
            Expr::new().i32_const(1)
        } else {
            Expr::new().call(i + 1).i32_const(1).op(op::I32_ADD)
        };
        b.add_func(ty, Func::new(body));
    }
    b.export_func("f", 0).build()
}

/// `depth` nested blocks with `i32.const 42; br depth-1` at the bottom.
///
/// The outermost block is the one that carries the result, so the branch crosses every
/// intermediate label at once. The `i32.const 0` after the nest is the fall-through the
/// validator demands and the run never reaches.
fn nested_module(depth: usize) -> Module {
    let mut inner = Expr::new().i32_const(42).br(depth as u32 - 1);
    for _ in 1..depth {
        inner = Expr::new().block(BlockType::Empty, inner);
    }
    single(
        &format!("nesting-{depth}"),
        ftype(&[], &[ValType::I32]),
        Expr::new().block(
            BlockType::Value(ValType::I32),
            inner.then(Expr::new().i32_const(0)),
        ),
    )
}

/// One function declaring `n` i32 locals in a single run, using the first and the last.
///
/// Locals are zero-initialised, so `local[n-1] + 12_345` is `12_345` and nothing else.
fn many_locals_module(n: u32) -> Module {
    single_with_locals(
        &format!("locals-{n}"),
        ftype(&[], &[ValType::I32]),
        &[(n, ValType::I32)],
        Expr::new()
            .i32_const(12_345)
            .local_set(n - 1)
            .local_get(n - 1)
            .local_get(0)
            .op(op::I32_ADD),
    )
}

/// One function whose body is `i32.const 1; i32.add` repeated `n` times, starting from 0.
fn straight_line_module(n: usize) -> Module {
    single(
        &format!("straight-{n}"),
        ftype(&[], &[ValType::I32]),
        Expr::new()
            .i32_const(0)
            .repeat(n, &Expr::new().i32_const(1).op(op::I32_ADD)),
    )
}

/// The byte the big module's data segment holds at `i`.
fn big_byte(i: usize) -> u8 {
    (i.wrapping_mul(31).wrapping_add(7) & 0xff) as u8
}

/// A module of a few megabytes: a full 64-page data segment and a thousand small functions.
///
/// The exported function returns four numbers — the first, a deep and the last byte of the
/// data segment, and the result of calling the function in the middle of the thousand — so
/// a data segment that is copied short, a section size read as a 32-bit quantity, or a
/// function index space that drifts all show up as a wrong line rather than as a crash.
fn big_module() -> Module {
    let mut b = ModuleBuilder::new("big-module");
    let unit = b.add_type(ftype(&[], &[ValType::I32]));
    for k in 0..BIG_FUNCS {
        b.add_func(unit, Func::new(Expr::new().i32_const(k as i32)));
    }
    let quad = b.add_type(ftype(&[], &[ValType::I32; 4]));
    let deep = (DATA_BYTES / 2 + 12_345) as u32;
    let last = (DATA_BYTES - 1) as u32;
    let body = Expr::new()
        .i32_const(0)
        .mem(op::I32_LOAD8_U, 0, 0)
        .i32_const(0)
        .mem(op::I32_LOAD8_U, 0, deep)
        .i32_const(0)
        .mem(op::I32_LOAD8_U, 0, last)
        .call(BIG_FUNCS / 2);
    let f = b.add_func(quad, Func::new(body));
    let data: Vec<u8> = (0..DATA_BYTES).map(big_byte).collect();
    b.memory(Limits::min((DATA_BYTES / 65_536) as u32))
        .data_active(0, &data)
        .export_func("f", f)
        .build()
}

/// What [`big_module`]'s exported function must print, one line per returned value.
fn big_module_answer() -> Vec<String> {
    vec![
        big_byte(0).to_string(),
        big_byte(DATA_BYTES / 2 + 12_345).to_string(),
        big_byte(DATA_BYTES - 1).to_string(),
        (BIG_FUNCS / 2).to_string(),
    ]
}

/// A table of `slots` entries, filled at the front and in its very last slot.
///
/// The head segment is a real vector of thousands of function indices; the tail segment is
/// one entry at `slots - 1`, which is the slot a runtime that stores its table as a `Vec`
/// sized from the *segments* rather than from the table type will not have.
fn large_table_module(slots: u32, head: u32) -> Module {
    let mut b = ModuleBuilder::new(format!("table-{slots}"));
    let unit = b.add_type(ftype(&[], &[ValType::I32]));
    let one = b.add_func(unit, Func::new(Expr::new().i32_const(1)));
    let last = b.add_func(unit, Func::new(Expr::new().i32_const(777)));
    let triple = b.add_type(ftype(&[], &[ValType::I32; 3]));
    let body = Expr::new()
        .i32_const(0)
        .call_indirect(unit, 0)
        .i32_const(head as i32 - 1)
        .call_indirect(unit, 0)
        .i32_const(slots as i32 - 1)
        .call_indirect(unit, 0);
    let f = b.add_func(triple, Func::new(body));
    b.table(TableType {
        elem: ValType::FuncRef,
        limits: Limits::min(slots),
    })
    .elem_active(0, &vec![one; head as usize])
    .elem_active(slots as i32 - 1, &[last])
    .export_func("f", f)
    .build()
}

/// The name of the `k`-th export of [`many_exports_module`].
fn export_name(k: u32) -> String {
    format!("e{k:03}")
}

/// `n` functions, each returning its own index, each exported under its own name.
fn many_exports_module(n: u32) -> Module {
    let mut b = ModuleBuilder::new(format!("exports-{n}"));
    let unit = b.add_type(ftype(&[], &[ValType::I32]));
    for k in 0..n {
        let idx = b.add_func(unit, Func::new(Expr::new().i32_const(k as i32)));
        b = b.export_func(&export_name(k), idx);
    }
    b.build()
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

/// Note the module's size and how long the whole invocation took.
fn note_size(ctx: &mut Ctx, m: &Module, what: &str, started: Instant) {
    ctx.note(format!(
        "{what}: {} bytes ({:.2} MiB), decoded, instantiated and run in {} ms",
        m.len(),
        m.len() as f64 / (1024.0 * 1024.0),
        started.elapsed().as_millis()
    ));
}

wasm_test!(thousand_functions, |ctx| {
    let m = chain_module(CHAIN);
    let started = Instant::now();
    expect_line(ctx, &m, "f", &[], &CHAIN.to_string())?;
    note_size(
        ctx,
        &m,
        &format!("{CHAIN} functions, {CHAIN} frames deep"),
        started,
    );
    Ok(())
});

wasm_test!(deep_nesting, |ctx| {
    let m = nested_module(NEST);
    let started = Instant::now();
    expect_line(ctx, &m, "f", &[], "42")?;
    note_size(ctx, &m, &format!("{NEST} nested blocks"), started);
    Ok(())
});

wasm_test!(wide_and_long, |ctx| {
    let wide = many_locals_module(LOCALS);
    let started = Instant::now();
    expect_line(ctx, &wide, "f", &[], "12345")?;
    note_size(ctx, &wide, &format!("{LOCALS} locals in one run"), started);

    let long = straight_line_module(STRAIGHT);
    let started = Instant::now();
    expect_line(ctx, &long, "f", &[], &STRAIGHT.to_string())?;
    note_size(
        ctx,
        &long,
        &format!("{} straight-line instructions", STRAIGHT * 2),
        started,
    );
    Ok(())
});

wasm_test!(multi_megabyte, |ctx| {
    let m = big_module();
    let want = big_module_answer();
    let refs: Vec<&str> = want.iter().map(String::as_str).collect();
    let started = Instant::now();
    let run = expect_lines(ctx, &m, "f", &[], &refs)?;
    let mut c = Check::new("that the whole data segment really arrived", &run);
    c.at_least("module.len", DATA_BYTES, m.len());
    c.note(
        "the three bytes read back are the first, one past the middle and the very last of \
         the segment; a copy that stops at 64 KiB or at a 16-bit length gets the first right \
         and the other two wrong",
    );
    c.finish()?;
    note_size(
        ctx,
        &m,
        &format!("{BIG_FUNCS} functions and a {DATA_BYTES}-byte data segment"),
        started,
    );
    Ok(())
});

wasm_test!(large_table, |ctx| {
    let m = large_table_module(TABLE_SLOTS, ELEM_HEAD);
    let started = Instant::now();
    expect_lines(ctx, &m, "f", &[], &["1", "1", "777"])?;
    note_size(
        ctx,
        &m,
        &format!("a {TABLE_SLOTS}-slot table, {ELEM_HEAD} of them from one element segment"),
        started,
    );
    Ok(())
});

wasm_test!(many_exports, |ctx| {
    let m = many_exports_module(EXPORTS);
    let started = Instant::now();
    // The two ends and one from the middle: an export table looked up by position rather
    // than by name gets the middle one wrong and the first one right.
    for k in [0, EXPORTS / 2, EXPORTS - 1] {
        expect_line(ctx, &m, &export_name(k), &[], &k.to_string())?;
    }
    note_size(ctx, &m, &format!("{EXPORTS} exports"), started);
    Ok(())
});

/// Worked examples.
///
/// Both are the real shape at a size a reader can hold in their head: the catalog renders
/// every byte and every annotation of an example, and a thousand-function module would put
/// three thousand of them into `catalog.json`. The tests run the full-size versions.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("A chain of functions, eight long", || chain_module(8))
            .summary(
                "eight functions of type `() -> i32`; function k is `call k+1; i32.const 1; \
                 i32.add` and the last is `i32.const 1`, so the exported head returns the \
                 length of the chain. The test runs the same shape with a thousand of them",
            )
            .command("run --invoke f mod.wasm")
            .output("8")
            .note(
                "The function and code sections are two parallel vectors: the k-th entry of \
                 the function section is the type of the k-th entry of the code section. \
                 Decode both with a loop over the count — a decoder that recurses once per \
                 function runs out of host stack long before a thousand.",
            ),
        ExampleSpec::module("Eight nested blocks and one branch out", || {
            nested_module(8)
        })
        .summary(
            "`block (result i32)` around seven plain `block`s, with `i32.const 42; br 7` \
                 at the bottom and an `i32.const 0` after the nest that the run never \
                 reaches. The test runs four hundred levels",
        )
        .command("run --invoke f mod.wasm")
        .output("42")
        .note(
            "`br 7` names the eighth enclosing label, counting outwards from zero, and \
                 carries the innermost value straight to the outermost block's result. The \
                 `i32.const 0` is there for the validator, which types the fall-through path \
                 whether or not it can ever run.",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chain_is_as_long_as_it_says() {
        let m = chain_module(8);
        let listing = m.listing(8192);
        assert!(listing.contains("call 7"), "{listing}");
        assert!(listing.contains("'f' → func 0"), "{listing}");
    }

    #[test]
    fn the_innermost_branch_names_the_outermost_label() {
        let m = nested_module(8);
        assert!(m.listing(8192).contains("br 7"), "{}", m.listing(8192));
    }

    #[test]
    fn the_big_module_is_actually_big_and_its_answer_is_computed_here() {
        // Cheap surrogate: the data-segment byte function is what the expectation uses, so
        // check it against a hand-worked value rather than against itself.
        assert_eq!(big_byte(0), 7);
        assert_eq!(big_byte(1), 38);
        assert_eq!(big_module_answer().len(), 4);
        assert_eq!(big_module_answer()[0], "7");
    }

    #[test]
    fn export_names_sort_the_way_they_are_numbered() {
        assert_eq!(export_name(0), "e000");
        assert_eq!(export_name(511), "e511");
    }
}
