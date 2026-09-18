//! Stage 23 — br_table, its default and a runtime index.
//!
//! `br_table` is the only instruction whose target is decided while the program runs, and
//! the decision is deliberately total: an index that is past the end of the label vector
//! takes the default label. That is **not** a trap, and it is not a validation error
//! either — the index is an ordinary i32 nobody can know in advance.
//!
//! The index is read as **unsigned**, so `-1` is 4 294 967 295 and lands on the default of
//! every table anyone will ever write. A runtime that compares it as a signed integer finds
//! a negative index "less than the label count" only if it also gets the comparison wrong,
//! but it will happily index a Rust `Vec` with a huge number and panic; the test with
//! `0xffffffff` in it is there to catch exactly that.

use crate::examples::ExampleSpec;
use crate::stages::{case_i32, run_cases, single_with_locals, Case, Stage, Test};
use crate::wasm::{ftype, op, BlockType, Expr, Module, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 23,
        slug: "br_table",
        name: "br_table, its default and a runtime index",
        ext: false,
        hints: &[
            "`br_table l0 l1 … ln ld` pops one i32: if it is less than the number of labels it branches to that label, and otherwise to the default `ld`",
            "The index is **unsigned**, so -1 is 4294967295 and takes the default. An out-of-range index is a runtime decision, never a trap and never a validation error",
            "Every label in the table, the default included, must have the same arity, and `br_table` carries that many values off the stack exactly as `br` does",
            "The entries are relative depths like any other branch target, so the same label may appear more than once and the table need not be in any order",
        ],
        examples,
        tests: vec![
            Test::new("each index selects the label at that position", each_index_its_label),
            Test::new("an index past the end of the table takes the default", past_the_end),
            Test::new("a br_table with one label is a br", one_label),
            Test::new("repeated entries send several indices to the same label", repeated_entries),
            Test::new("a br_table carries a result value out of a typed block", carrying_a_value),
            Test::new("a table three blocks deep reaches each of the three", three_deep),
            Test::new("a br_table as the last instruction of a function returns from it", at_the_end),
            Test::new("a br_table drives a state machine that terminates", state_machine),
        ],
    }
}

/// One `i32` local, the accumulator every jump-table case reads at the end.
const ACC: &[(u32, ValType)] = &[(1, ValType::I32)];

/// Four nested blocks with a `br_table` at the bottom of the innermost one.
///
/// Landing after the `end` of the innermost block stores 100 in local `acc`, the next one
/// 200, the next 300; the outermost block is the default's target and leaves the local at
/// the 999 it was seeded with. The labels the table may name are therefore 0, 1, 2 and 3.
///
/// `acc` is a parameter because the two shapes of case below number their locals
/// differently: a `() -> i32` case has the accumulator at 0, a `(i32) -> i32` case has the
/// index parameter at 0 and the accumulator at 1.
fn jump_table(index: Expr, acc: u32, entries: &[u32], default: u32) -> Expr {
    let inner = index.br_table(entries, default);
    Expr::new()
        .i32_const(999)
        .local_set(acc)
        .block(
            BlockType::Empty,
            Expr::new()
                .block(
                    BlockType::Empty,
                    Expr::new()
                        .block(
                            BlockType::Empty,
                            Expr::new()
                                .block(BlockType::Empty, inner)
                                .i32_const(100)
                                .local_set(acc)
                                .br(2),
                        )
                        .i32_const(200)
                        .local_set(acc)
                        .br(1),
                )
                .i32_const(300)
                .local_set(acc)
                .br(0),
        )
        .local_get(acc)
}

/// A `() -> i32` jump-table case whose index is a constant.
fn table_case(
    name: impl Into<String>,
    index: i32,
    entries: &[u32],
    default: u32,
    want: i32,
) -> Case {
    case_i32(
        name,
        jump_table(Expr::new().i32_const(index), 0, entries, default),
        want,
    )
    .locals(ACC)
}

/// A `(i32) -> i32` jump-table case whose index comes in as an argument at run time.
fn arg_table_case(
    name: impl Into<String>,
    arg: &str,
    entries: &[u32],
    default: u32,
    want: i32,
) -> Case {
    Case::new(
        name,
        ftype(&[ValType::I32], &[ValType::I32]),
        jump_table(Expr::new().local_get(0), 1, entries, default),
        &[&want.to_string()],
    )
    .locals(ACC)
    .args(&[arg])
}

wasm_test!(each_index_its_label, |ctx| {
    run_cases(
        ctx,
        "each-index-its-label",
        vec![
            table_case("index 0 takes the first label", 0, &[0, 1, 2], 3, 100),
            table_case("index 1 takes the second label", 1, &[0, 1, 2], 3, 200),
            table_case("index 2 takes the third label", 2, &[0, 1, 2], 3, 300),
            // The same table with the index arriving at run time rather than folded into
            // the module: a runtime that decides the target while decoding fails here.
            arg_table_case("a run-time index of 0", "0", &[0, 1, 2], 3, 100),
            arg_table_case("a run-time index of 1", "1", &[0, 1, 2], 3, 200),
            arg_table_case("a run-time index of 2", "2", &[0, 1, 2], 3, 300),
        ],
    )
});

wasm_test!(past_the_end, |ctx| {
    run_cases(
        ctx,
        "past-the-end",
        vec![
            table_case("index 3 is one past the end", 3, &[0, 1, 2], 3, 999),
            table_case("index 99 is far past the end", 99, &[0, 1, 2], 3, 999),
            // -1 as an i32 is 0xffffffff as an index: the largest one there is, and still
            // only the default, not a trap and not a panic.
            table_case("index -1 is 4294967295 unsigned", -1, &[0, 1, 2], 3, 999),
            table_case(
                "index INT_MIN is 2147483648 unsigned",
                i32::MIN,
                &[0, 1, 2],
                3,
                999,
            ),
            arg_table_case("a run-time index of 3", "3", &[0, 1, 2], 3, 999),
            arg_table_case("a run-time index of 4294967295", "-1", &[0, 1, 2], 3, 999),
        ],
    )
});

wasm_test!(one_label, |ctx| {
    // `br_table 1 1` — one entry and a default, both naming the same label — is a br 1 with
    // an index that is read, popped and then ignored.
    run_cases(
        ctx,
        "one-label",
        vec![
            table_case("index 0 hits the only label", 0, &[1], 1, 200),
            table_case(
                "index 1 falls to the default, which is the same label",
                1,
                &[1],
                1,
                200,
            ),
            table_case("index 99 goes to the same place", 99, &[1], 1, 200),
            table_case("index -1 goes to the same place", -1, &[1], 1, 200),
            // An empty label vector is legal: every index takes the default.
            table_case(
                "a table with no entries at all is the default",
                0,
                &[],
                2,
                300,
            ),
        ],
    )
});

wasm_test!(repeated_entries, |ctx| {
    // 0 → the first label, 1, 2 and 3 → the second, 4 → the third, anything else → default.
    let entries = &[0u32, 1, 1, 1, 2];
    run_cases(
        ctx,
        "repeated-entries",
        vec![
            table_case("index 0 is on its own", 0, entries, 3, 100),
            table_case("index 1 shares a label", 1, entries, 3, 200),
            table_case("index 2 shares the same label", 2, entries, 3, 200),
            table_case("index 3 shares the same label again", 3, entries, 3, 200),
            table_case("index 4 is on its own", 4, entries, 3, 300),
            table_case("index 5 is past the end", 5, entries, 3, 999),
        ],
    )
});

/// Two nested `block (result i32)`s: the value 42 is carried out of one or the other.
///
/// Branching to the inner label lands before the `i32.const 1  i32.add`, so the answer says
/// which label the table chose.
fn carry(index: Expr) -> Expr {
    Expr::new().block(
        BlockType::Value(ValType::I32),
        Expr::new()
            .block(
                BlockType::Value(ValType::I32),
                Expr::new().i32_const(42).then(index).br_table(&[0, 1], 1),
            )
            .i32_const(1)
            .op(op::I32_ADD),
    )
}

wasm_test!(carrying_a_value, |ctx| {
    run_cases(
        ctx,
        "carrying-a-value",
        vec![
            case_i32(
                "index 0 carries the value to the inner label",
                carry(Expr::new().i32_const(0)),
                43,
            ),
            case_i32(
                "index 1 carries the value straight out of both blocks",
                carry(Expr::new().i32_const(1)),
                42,
            ),
            case_i32(
                "an index past the end carries it out on the default",
                carry(Expr::new().i32_const(7)),
                42,
            ),
            // The index sits above the carried value, so br_table pops the index first and
            // then takes the label's single result from what is left.
            case_i32(
                "the index is popped before the value the label asks for",
                Expr::new().block(
                    BlockType::Value(ValType::I32),
                    Expr::new()
                        .i32_const(11)
                        .i32_const(22)
                        .i32_const(0)
                        .br_table(&[0], 0),
                ),
                22,
            ),
        ],
    )
});

wasm_test!(three_deep, |ctx| {
    // The table is a mapping, not a sequence: index 0 goes to the *third* label and index 2
    // to the first. A runtime that branches `index` levels out passes the other tests here
    // and fails this one.
    let entries = &[2u32, 0, 1];
    run_cases(
        ctx,
        "three-deep",
        vec![
            table_case("index 0 names the label two levels out", 0, entries, 3, 300),
            table_case("index 1 names the innermost label", 1, entries, 3, 100),
            table_case("index 2 names the label one level out", 2, entries, 3, 200),
            table_case("index 3 still takes the default", 3, entries, 3, 999),
            arg_table_case(
                "the same mapping with a run-time index",
                "1",
                entries,
                3,
                100,
            ),
        ],
    )
});

wasm_test!(at_the_end, |ctx| {
    run_cases(
        ctx,
        "at-the-end",
        vec![
            // At the top level of a function the only label is the function body itself,
            // whose arity is the function's result count, so this is a return.
            Case::new(
                "a br_table at the top level of a function returns its result",
                ftype(&[ValType::I32], &[ValType::I32]),
                Expr::new().i32_const(5).local_get(0).br_table(&[0], 0),
                &["5"],
            )
            .args(&["0"]),
            Case::new(
                "the same br_table with an index past the end still returns",
                ftype(&[ValType::I32], &[ValType::I32]),
                Expr::new().i32_const(5).local_get(0).br_table(&[0], 0),
                &["5"],
            )
            .args(&["9"]),
            // Two values under the index: the label's arity is one, so the 5 is discarded
            // along with the rest of the frame.
            case_i32(
                "a br_table at the end takes the label's arity and drops the rest",
                Expr::new()
                    .i32_const(5)
                    .i32_const(6)
                    .i32_const(0)
                    .br_table(&[0], 0),
                6,
            ),
            Case::new(
                "a br_table as the last instruction of a function that returns nothing",
                ftype(&[], &[]),
                Expr::new().i32_const(0).br_table(&[0], 0),
                &[],
            ),
        ],
    )
});

/// A three-state machine driven by a `br_table`, wrapped in a loop.
///
/// The state local selects one of three arms; each arm sets the next state and branches
/// back to the loop. State 3 is past the end of the table, so the default takes the machine
/// out of the enclosing block. The answer is how many times round it went.
fn machine(start: i32) -> Expr {
    Expr::new()
        .i32_const(start)
        .local_set(0)
        .block(
            BlockType::Empty,
            Expr::new().loop_(
                BlockType::Empty,
                Expr::new()
                    .local_get(1)
                    .i32_const(1)
                    .op(op::I32_ADD)
                    .local_set(1)
                    .block(
                        BlockType::Empty,
                        Expr::new()
                            .block(
                                BlockType::Empty,
                                Expr::new()
                                    .block(
                                        BlockType::Empty,
                                        // Inside here: 0..2 are the three state blocks, 3 is
                                        // the loop and 4 is the block around it.
                                        Expr::new().local_get(0).br_table(&[0, 1, 2], 4),
                                    )
                                    .i32_const(1)
                                    .local_set(0)
                                    .br(2),
                            )
                            .i32_const(2)
                            .local_set(0)
                            .br(1),
                    )
                    .i32_const(3)
                    .local_set(0)
                    .br(0),
            ),
        )
        .local_get(1)
}

wasm_test!(state_machine, |ctx| {
    let two_locals = &[(2u32, ValType::I32)];
    run_cases(
        ctx,
        "state-machine",
        vec![
            Case::new(
                "starting at state 0 takes four steps to reach the default",
                ftype(&[], &[ValType::I32]),
                machine(0),
                &["4"],
            )
            .locals(two_locals),
            Case::new(
                "starting at state 1 takes three",
                ftype(&[], &[ValType::I32]),
                machine(1),
                &["3"],
            )
            .locals(two_locals),
            Case::new(
                "starting at state 2 takes two",
                ftype(&[], &[ValType::I32]),
                machine(2),
                &["2"],
            )
            .locals(two_locals),
            Case::new(
                "starting past the end of the table stops after one step",
                ftype(&[], &[ValType::I32]),
                machine(7),
                &["1"],
            )
            .locals(two_locals),
        ],
    )
});

/// The four-block jump table, with the index arriving as an argument.
fn dispatch() -> Module {
    single_with_locals(
        "dispatch",
        ftype(&[ValType::I32], &[ValType::I32]),
        ACC,
        jump_table(Expr::new().local_get(0), 1, &[0, 1, 2], 3),
    )
}

/// The same table with an index that never matches, to show the default.
fn dispatch_default() -> Module {
    single_with_locals(
        "dispatch-default",
        ftype(&[], &[ValType::I32]),
        ACC,
        jump_table(Expr::new().i32_const(-1), 0, &[0, 1, 2], 3),
    )
}

/// Worked examples: a jump table and the same table falling to its default.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("A three-way jump table", dispatch)
            .summary(
                "four nested blocks; the innermost ends in `br_table 0 1 2 3` on the \
                 function's argument, and each landing site stores 100, 200 or 300",
            )
            .command("run --invoke f mod.wasm 1")
            .output("200")
            .note(
                "Pass 0, 1 or 2 and the answer is 100, 200 or 300; pass anything else and \
                 the default label 3 takes control out of the outermost block, leaving the \
                 local at the 999 it was seeded with.",
            ),
        ExampleSpec::module("The same table, with -1", dispatch_default)
            .summary("the identical table with `i32.const -1` as the index")
            .command("run --invoke f mod.wasm")
            .output("999")
            .note(
                "The index is unsigned: -1 is 4294967295, which is past the end of a \
                 three-entry table, so the default is taken. It is not a trap — there is no \
                 such thing as an out-of-range br_table index at run time.",
            ),
    ]
}
