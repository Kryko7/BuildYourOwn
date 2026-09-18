//! Stage 11 — Unknown indices and immutable globals.
//!
//! Almost every immediate in a function body is an index into something the module declares:
//! functions, types, locals, globals, tables, memories, and the block labels the body itself
//! builds up. All of them are dense and zero-based, so the whole check is `index < count` —
//! and all of them are checked before anything runs, which is what lets a runtime resolve
//! an index to a pointer once and never look at it again.
//!
//! Two of them are less obvious than the rest. A label is not declared anywhere: its depth
//! is counted against the frames the validator is already holding, and the function body is
//! itself the outermost frame, so inside no blocks `br 0` is a return and `br 1` is out of
//! range. And a load or a store names memory 0 implicitly, so in a module with no memory
//! section there is no memory 0 to name — `wasmtime` says `unknown memory 0`.
//!
//! The last test names the highest legal index of every space at once and must be accepted.
//! Off-by-one errors come in two directions, and a suite that only ever tests indices that
//! are too large would pass a runtime that rejects the last valid one as well.

use crate::examples::ExampleSpec;
use crate::stages::{expect, expect_rejected, single, single_with_locals, Stage, Test};
use crate::wasm::{
    ftype, global_i32, op, BlockType, Expr, Func, Limits, Module, ModuleBuilder, TableType, ValType,
};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 11,
        slug: "unknown_indices",
        name: "Unknown indices and immutable globals",
        ext: false,
        hints: &[
            "Every index in a body is checked against the size of its space before the body runs: functions, types, locals, globals, tables and memories are all dense and zero-based",
            "A label index is a depth, not a declaration: count it against the frames you are holding, remembering that the function body is itself the outermost frame",
            "A load or a store names memory 0 implicitly, so a module with no memory section cannot hold one at all",
            "A global carries a mutability flag next to its type, and `global.set` on a global whose flag is 0 is a validation error, not a trap",
        ],
        examples,
        tests: vec![
            Test::new("a call to a function index that does not exist is refused", unknown_function),
            Test::new("local.get past the last local is refused", unknown_local),
            Test::new("global.get past the last global is refused", unknown_global),
            Test::new("call_indirect naming a type index that does not exist is refused", unknown_type),
            Test::new("a br one past the outermost label is refused", unknown_label),
            Test::new("a table index and a memory index that do not exist are refused", unknown_table_and_memory),
            Test::new("global.set on an immutable global is refused", immutable_global),
            Test::new("the last valid index of every index space is accepted", last_valid_indices),
        ],
    }
}

wasm_test!(unknown_function, |ctx| {
    let m = single(
        "call-999",
        ftype(&[], &[ValType::I32]),
        Expr::new().call(999),
    );
    expect_rejected(
        ctx,
        &m,
        "call 999 in a module whose function index space holds exactly one entry",
    )?;
    Ok(())
});

wasm_test!(unknown_local, |ctx| {
    // Two parameters and one declared local make indices 0, 1 and 2; 3 is one too far.
    let m = single_with_locals(
        "local-get-3",
        ftype(&[ValType::I32, ValType::I32], &[ValType::I32]),
        &[(1, ValType::I32)],
        Expr::new().local_get(3),
    );
    expect_rejected(
        ctx,
        &m,
        "two parameters and one local give local indices 0 to 2, and the body asks for 3",
    )?;
    Ok(())
});

wasm_test!(unknown_global, |ctx| {
    let m = single(
        "global-get-0",
        ftype(&[], &[ValType::I32]),
        Expr::new().global_get(0),
    );
    expect_rejected(
        ctx,
        &m,
        "global.get 0 in a module with no global section: an empty index space has no \
         zeroth entry",
    )?;
    Ok(())
});

wasm_test!(unknown_type, |ctx| {
    let m = call_indirect_bad_type();
    expect_rejected(
        ctx,
        &m,
        "call_indirect names type 9 for the signature it will check the table slot against, \
         and the type section holds one type",
    )?;
    Ok(())
});

wasm_test!(unknown_label, |ctx| {
    // `br 0` from the top level of a body targets the function itself and is a return, so
    // `br 1` is the first depth that names nothing.
    let too_deep = single(
        "br-1-at-top-level",
        ftype(&[], &[ValType::I32]),
        Expr::new().i32_const(7).br(1),
    );
    expect_rejected(
        ctx,
        &too_deep,
        "br 1 outside any block: the function body is the only frame there is, and it is \
         depth 0",
    )?;

    // The same body one depth shallower is a plain return and must be accepted, so that a
    // runtime which is one out in the other direction is caught too.
    let outermost = single(
        "br-0-at-top-level",
        ftype(&[], &[ValType::I32]),
        Expr::new().i32_const(7).br(0),
    );
    expect(ctx, &outermost, "7")?;
    Ok(())
});

wasm_test!(unknown_table_and_memory, |ctx| {
    let table = table_get_one();
    expect_rejected(
        ctx,
        &table,
        "table.get 1 in a module with a single table, which is table 0",
    )?;

    let memory = single(
        "load-without-memory",
        ftype(&[], &[ValType::I32]),
        Expr::new().i32_const(0).i32_load(0),
    );
    expect_rejected(
        ctx,
        &memory,
        "i32.load in a module with no memory section: the instruction names memory 0 \
         implicitly and there is no memory 0",
    )?;
    Ok(())
});

wasm_test!(immutable_global, |ctx| {
    let m = set_immutable();
    expect_rejected(
        ctx,
        &m,
        "global 0 is declared with its mutability flag at 0, so `global.set` on it cannot \
         validate — this is a type error, not a trap the module could catch",
    )?;
    Ok(())
});

wasm_test!(last_valid_indices, |ctx| {
    // One module that names the highest legal index of every space it has: type 1, function
    // 1, local 0, global 1, table 0, memory 0, and label 1 from inside one block.
    let m = last_indices();
    expect(ctx, &m, "37")?;
    ctx.note(
        "named the last valid index of the type, function, local, global, table, memory and \
         label spaces in one body",
    );
    Ok(())
});

// ---------------------------------------------------------------------------------------
// Modules
// ---------------------------------------------------------------------------------------

/// `call_indirect (type 9)` against a module holding one type and one table.
fn call_indirect_bad_type() -> Module {
    let mut b = ModuleBuilder::new("call-indirect-type-9");
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(ty, Func::new(Expr::new().i32_const(0).call_indirect(9, 0)));
    b.table(TableType {
        elem: ValType::FuncRef,
        limits: Limits::min(1),
    })
    .export_func("f", idx)
    .build()
}

/// `table.get 1` against a module with one table.
fn table_get_one() -> Module {
    let mut b = ModuleBuilder::new("table-get-1");
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(
        ty,
        Func::new(Expr::new().i32_const(0).table_get(1).ref_is_null()),
    );
    b.table(TableType {
        elem: ValType::FuncRef,
        limits: Limits::min(1),
    })
    .export_func("f", idx)
    .build()
}

/// `global.set 0` where global 0 is immutable.
fn set_immutable() -> Module {
    let mut b = ModuleBuilder::new("set-immutable-global");
    let ty = b.add_type(ftype(&[], &[]));
    let idx = b.add_func(ty, Func::new(Expr::new().i32_const(1).global_set(0)));
    b.global(global_i32(7, false)).export_func("f", idx).build()
}

/// A module that names the highest legal index of every space it declares, and answers 37.
///
/// Types 0 and 1, functions 0 (`f`) and 1 (`inc`), one local, globals 0 and 1, one table,
/// one memory. The body stores 30 through memory 0, reads it back, adds global 1 (which is
/// 5), calls function 1, calls it again through table 0 with type 1, and returns the result
/// with `br 1` — the function's own label, seen from inside one block.
fn last_indices() -> Module {
    let mut b = ModuleBuilder::new("last-valid-indices");
    let t_f = b.add_type(ftype(&[], &[ValType::I32]));
    let t_inc = b.add_type(ftype(&[ValType::I32], &[ValType::I32]));
    let f = b.add_func(
        t_f,
        Func::with_locals(
            &[(1, ValType::I32)],
            Expr::new()
                .i32_const(0)
                .i32_const(30)
                .i32_store(0)
                .i32_const(0)
                .i32_load(0)
                .local_set(0)
                .block(
                    BlockType::Value(ValType::I32),
                    Expr::new()
                        .local_get(0)
                        .global_get(1)
                        .op(op::I32_ADD)
                        .call(1)
                        .i32_const(0)
                        .call_indirect(1, 0)
                        .br(1),
                ),
        ),
    );
    let inc = b.add_func(
        t_inc,
        Func::new(Expr::new().local_get(0).i32_const(1).op(op::I32_ADD)),
    );
    b.memory(Limits::min(1))
        .global(global_i32(1, false))
        .global(global_i32(5, true))
        .table(TableType {
            elem: ValType::FuncRef,
            limits: Limits::min(1),
        })
        .elem_active(0, &[inc])
        .export_func("f", f)
        .build()
}

/// Worked examples: the index that is one too far, and the one that is exactly far enough.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("global.set on a global that cannot be set", set_immutable)
            .summary(
                "a module with one immutable `i32` global initialised to 7, and a `() -> ()` \
                 function whose body is `i32.const 1`, `global.set 0`",
            )
            .command("run --invoke f mod.wasm")
            .output(
                "nothing on stdout, a message on stderr, and a non-zero exit status — the \
                 module never runs",
            )
            .note(
                "The global section writes a type byte and then a mutability byte; 0 means \
                 immutable. Refusing the write at validation time is what lets a runtime put \
                 an immutable global in read-only memory or fold it into the code.",
            ),
        ExampleSpec::module("The last legal index of every space", last_indices)
            .summary(
                "two types, two functions, one local, two globals, one table and one memory, \
                 with a body that names the highest index of each — type 1, function 1, \
                 local 0, global 1, table 0, memory 0 — and returns with `br 1`",
            )
            .command("run --invoke f mod.wasm")
            .output("37")
            .note(
                "The bound is `index < count`, so the last valid index is `count - 1` and a \
                 runtime that writes `<=` refuses this module. `br 1` here is the function's \
                 own label seen from inside one block: the body is the outermost frame.",
            ),
    ]
}
