//! Shared building blocks for stage tests: canonical trap reasons, module shorthands, and
//! the "build a module, invoke it, compare what it printed" checks every stage leans on.

use crate::assert::{Check, Failure, FailureKind};
use crate::runtime::Run;
use crate::stages::Ctx;
use crate::wasm::{ftype, Expr, Func, FuncType, Module, ModuleBuilder, ValType};

/// The spec's own trap reasons, matched as substrings of stderr, case-insensitively.
///
/// A runtime is free to wrap them in whatever sentence it likes — `wasmtime` writes
/// ``wasm trap: wasm `unreachable` instruction executed`` for [`UNREACHABLE`] — so the
/// suite only ever asks that the canonical words are in there somewhere.
pub mod trap {
    /// The `unreachable` instruction was executed.
    pub const UNREACHABLE: &str = "unreachable";
    /// `i32.div_s` and friends with a zero divisor.
    pub const DIVIDE_BY_ZERO: &str = "integer divide by zero";
    /// `INT_MIN / -1`, and `i32.trunc_f*` of a finite value that does not fit.
    pub const INTEGER_OVERFLOW: &str = "integer overflow";
    /// `i32.trunc_f*` of a NaN or an infinity.
    pub const INVALID_CONVERSION: &str = "invalid conversion to integer";
    /// A load, a store or a bulk operation that left the memory.
    pub const MEMORY_OUT_OF_BOUNDS: &str = "out of bounds memory access";
    /// A table access past the end of the table.
    pub const TABLE_OUT_OF_BOUNDS: &str = "out of bounds table access";
    /// `call_indirect` past the end of the table; some runtimes word it this way instead.
    pub const UNDEFINED_ELEMENT: &str = "undefined element";
    /// `call_indirect` through a null table slot.
    pub const UNINITIALIZED_ELEMENT: &str = "uninitialized element";
    /// `call_indirect` where the slot's function has another type.
    pub const INDIRECT_TYPE_MISMATCH: &str = "indirect call type mismatch";
    /// Recursion deeper than the runtime's stack allows.
    pub const STACK_EXHAUSTED: &str = "call stack exhausted";

    /// The two wordings a runtime may use for an out-of-range `call_indirect` index.
    ///
    /// The spec calls it "undefined element"; `wasmtime` writes both — "undefined element:
    /// out of bounds table access" — and a from-scratch runtime may pick either.
    pub const TABLE_INDEX_OUT_OF_RANGE: &[&str] = &[UNDEFINED_ELEMENT, TABLE_OUT_OF_BOUNDS];
}

/// The word every runtime puts in front of a trap reason. Matched loosely: a runtime that
/// writes "trap:" instead of "wasm trap:" still passes, because the reason is what matters.
pub const TRAP_MARKER: &str = "trap";

// ---------------------------------------------------------------------------------------
// Module shorthands
// ---------------------------------------------------------------------------------------

/// A module whose only export is one function, named `f`.
pub fn single(label: &str, sig: FuncType, body: Expr) -> Module {
    let mut b = ModuleBuilder::new(label);
    let ty = b.add_type(sig);
    let idx = b.add_func(ty, Func::new(body));
    b.export_func("f", idx).build()
}

/// A module whose only export is one function with locals, named `f`.
pub fn single_with_locals(
    label: &str,
    sig: FuncType,
    locals: &[(u32, ValType)],
    body: Expr,
) -> Module {
    let mut b = ModuleBuilder::new(label);
    let ty = b.add_type(sig);
    let idx = b.add_func(ty, Func::with_locals(locals, body));
    b.export_func("f", idx).build()
}

/// A module exporting one `() -> i32` function named `f`.
pub fn f_i32(label: &str, body: Expr) -> Module {
    single(label, ftype(&[], &[ValType::I32]), body)
}

/// A module exporting one `() -> i64` function named `f`.
pub fn f_i64(label: &str, body: Expr) -> Module {
    single(label, ftype(&[], &[ValType::I64]), body)
}

/// A module exporting one `() -> f32` function named `f`.
pub fn f_f32(label: &str, body: Expr) -> Module {
    single(label, ftype(&[], &[ValType::F32]), body)
}

/// A module exporting one `() -> f64` function named `f`.
pub fn f_f64(label: &str, body: Expr) -> Module {
    single(label, ftype(&[], &[ValType::F64]), body)
}

/// A module exporting one `() -> ()` function named `f`.
pub fn f_void(label: &str, body: Expr) -> Module {
    single(label, ftype(&[], &[]), body)
}

// ---------------------------------------------------------------------------------------
// How a runtime prints a value
// ---------------------------------------------------------------------------------------

/// How a correct runtime writes an `f32` result.
///
/// `wasmtime` formats floats the way Rust's `Display` does — `1.5`, `-0`, `inf`, `-inf`,
/// `NaN` — and a `NaN` of either sign or any payload prints as the bare word, which is why
/// the NaN-payload tests go through `reinterpret` and compare integers instead.
pub fn show_f32(v: f32) -> String {
    v.to_string()
}

/// How a correct runtime writes an `f64` result.
pub fn show_f64(v: f64) -> String {
    v.to_string()
}

// ---------------------------------------------------------------------------------------
// The checks
// ---------------------------------------------------------------------------------------

/// Invoke `export` and assert stdout is exactly these lines.
pub fn expect_lines(
    ctx: &mut Ctx,
    m: &Module,
    export: &str,
    args: &[&str],
    want: &[&str],
) -> Result<Run, Failure> {
    let run = ctx.invoke(m, export, args)?;
    check_output(m, &run, &format!("--invoke {export}"), want)?;
    Ok(run)
}

/// Invoke `export` and assert stdout is exactly this one line.
pub fn expect_line(
    ctx: &mut Ctx,
    m: &Module,
    export: &str,
    args: &[&str],
    want: &str,
) -> Result<Run, Failure> {
    expect_lines(ctx, m, export, args, &[want])
}

/// Invoke `export` with no arguments and assert it prints `want`.
pub fn expect(ctx: &mut Ctx, m: &Module, want: &str) -> Result<Run, Failure> {
    expect_lines(ctx, m, "f", &[], &[want])
}

/// Run the module as a WASI command and assert stdout is exactly `want`.
pub fn expect_stdout(ctx: &mut Ctx, m: &Module, args: &[&str], want: &str) -> Result<Run, Failure> {
    let run = ctx.start(m, args)?;
    let mut c = Check::new("the WASI command's output", &run);
    c.module(m);
    require_success(&mut c, &run);
    c.eq("stdout", want, run.stdout.as_str());
    c.finish()?;
    Ok(run)
}

fn require_success(c: &mut Check, run: &Run) {
    if !run.exit.success() {
        c.that(
            "exit",
            "exit status 0",
            false,
            format!(
                "{}{}",
                run.exit.label(),
                if run.stderr.trim().is_empty() {
                    String::new()
                } else {
                    format!(" — stderr says: {}", first_meaningful_line(&run.stderr))
                }
            ),
        );
    }
}

/// The first line of stderr that is not one of the runtime's own warnings.
///
/// `wasmtime` prints `warning: using --invoke with a function that returns values is
/// experimental` on every call this suite makes; quoting that back in a failure message
/// would bury the actual error.
pub fn first_meaningful_line(stderr: &str) -> String {
    stderr
        .lines()
        .find(|l| !l.trim().is_empty() && !l.trim_start().to_lowercase().starts_with("warning"))
        .unwrap_or("")
        .trim()
        .to_string()
}

fn check_output(m: &Module, run: &Run, what: &str, want: &[&str]) -> Result<(), Failure> {
    let mut c = Check::new(format!("{what} of module '{}'", m.label), run);
    c.module(m);
    require_success(&mut c, run);
    let got = run.lines();
    let want_owned: Vec<String> = want.iter().map(|s| s.to_string()).collect();
    if got != want_owned {
        c.eq("stdout.lines", want_owned.clone(), got.clone());
        if got.len() != want_owned.len() {
            c.note(format!(
                "a runtime prints one line per returned value: {} expected, {} seen",
                want_owned.len(),
                got.len()
            ));
        }
    }
    c.finish()
}

/// Invoke `export` and assert the runtime trapped with the given canonical reason.
pub fn expect_trap(
    ctx: &mut Ctx,
    m: &Module,
    export: &str,
    args: &[&str],
    reason: &str,
) -> Result<Run, Failure> {
    expect_trap_one_of(ctx, m, export, args, &[reason])
}

/// Invoke `export` and assert the runtime trapped with one of several acceptable reasons.
///
/// Where the spec allows more than one wording — an out-of-range `call_indirect` index is
/// "undefined element" to the spec and "out of bounds table access" to some runtimes — the
/// test names every one it accepts, and says so in the report.
pub fn expect_trap_one_of(
    ctx: &mut Ctx,
    m: &Module,
    export: &str,
    args: &[&str],
    reasons: &[&str],
) -> Result<Run, Failure> {
    let run = ctx.invoke(m, export, args)?;
    check_trap(m, &run, reasons)?;
    Ok(run)
}

/// Run the module as a WASI command and assert it trapped with the given reason.
pub fn expect_start_trap(
    ctx: &mut Ctx,
    m: &Module,
    args: &[&str],
    reason: &str,
) -> Result<Run, Failure> {
    let run = ctx.start(m, args)?;
    check_trap(m, &run, &[reason])?;
    Ok(run)
}

fn check_trap(m: &Module, run: &Run, reasons: &[&str]) -> Result<(), Failure> {
    let wanted = reasons
        .iter()
        .map(|r| format!("'{r}'"))
        .collect::<Vec<_>>()
        .join(" or ");
    let mut c = Check::new(
        format!("that module '{}' traps with {wanted}", m.label),
        run,
    );
    c.module(m);
    c.that(
        "exit",
        "a non-zero exit status, because a trap aborts the program",
        run.exit.failed(),
        run.exit.label(),
    );
    let matched = reasons.iter().any(|r| run.stderr_has(r));
    c.that(
        "stderr",
        &format!("the canonical trap reason {wanted} somewhere on stderr"),
        matched,
        first_meaningful_line(&run.stderr),
    );
    if !matched && !run.stderr.trim().is_empty() {
        c.note("the reason is matched as a case-insensitive substring, anywhere on stderr");
    }
    c.finish()
}

/// Assert a module is refused before anything runs: a decode or a validation failure.
///
/// The three things that are actually promised, and the only three asserted here:
///
/// * a non-zero exit status;
/// * **nothing on stdout** — a module that does not validate never executes, so a start
///   function or an exported body cannot have produced a byte;
/// * something on stderr saying so. The wording is the runtime's own, so it is only
///   recorded as a `note`, never compared.
pub fn expect_rejected(ctx: &mut Ctx, m: &Module, why: &str) -> Result<Run, Failure> {
    let run = ctx.invoke(m, "f", &[])?;
    check_rejected(m, &run, why)?;
    Ok(run)
}

/// The same, for a module that has no callable export to name (a WASI command, or a module
/// so broken it has no export section at all).
pub fn expect_start_rejected(ctx: &mut Ctx, m: &Module, why: &str) -> Result<Run, Failure> {
    let run = ctx.start(m, &[])?;
    check_rejected(m, &run, why)?;
    Ok(run)
}

fn check_rejected(m: &Module, run: &Run, why: &str) -> Result<(), Failure> {
    let mut c = Check::new(format!("that '{}' is refused: {why}", m.label), run);
    c.module(m);
    c.that(
        "exit",
        "a non-zero exit status: the module must not be accepted",
        run.exit.failed(),
        run.exit.label(),
    );
    c.that(
        "stdout",
        "nothing at all — a module that does not decode or validate never executes",
        run.stdout.is_empty(),
        run.stdout.clone(),
    );
    c.that(
        "stderr",
        "a message saying what is wrong (the wording is yours)",
        !run.stderr.trim().is_empty(),
        "(empty)",
    );
    c.note(format!(
        "the runtime said: {}",
        first_meaningful_line(&run.stderr)
    ));
    c.finish()
}

/// Assert a module is accepted: it decodes, validates and runs to completion.
pub fn expect_accepted(ctx: &mut Ctx, m: &Module, export: &str) -> Result<Run, Failure> {
    let run = ctx.invoke(m, export, &[])?;
    let mut c = Check::new(format!("that '{}' is accepted", m.label), &run);
    c.module(m);
    require_success(&mut c, &run);
    c.finish()?;
    Ok(run)
}

// ---------------------------------------------------------------------------------------
// Case sweeps
// ---------------------------------------------------------------------------------------

/// What a case's exported function must produce.
#[derive(Debug, Clone)]
pub enum Want {
    /// These exact lines on stdout, and a zero exit status.
    Lines(Vec<String>),
    /// A trap whose reason is one of these.
    Trap(Vec<String>),
}

/// One case of a sweep: a named function, its signature, and what it must produce.
///
/// A sweep builds **one** module holding every case as its own export, then invokes them one
/// at a time. That keeps a stage to a single encoder pass and a single file on disk while
/// still giving every case its own process, its own result and its own line in the report.
pub struct Case {
    /// What the case is called in a failure message.
    pub name: String,
    /// The function's signature.
    pub sig: FuncType,
    /// Its local declarations.
    pub locals: Vec<(u32, ValType)>,
    /// Its body.
    pub body: Expr,
    /// What it must produce.
    pub want: Want,
    /// Arguments passed to `--invoke`.
    pub args: Vec<String>,
}

impl Case {
    /// A case with an explicit signature and expected output lines.
    pub fn new(name: impl Into<String>, sig: FuncType, body: Expr, want: &[&str]) -> Case {
        Case {
            name: name.into(),
            sig,
            locals: Vec::new(),
            body,
            want: Want::Lines(want.iter().map(|s| s.to_string()).collect()),
            args: Vec::new(),
        }
    }

    /// Give the case locals.
    pub fn locals(mut self, locals: &[(u32, ValType)]) -> Case {
        self.locals = locals.to_vec();
        self
    }

    /// Pass arguments to `--invoke`.
    pub fn args(mut self, args: &[&str]) -> Case {
        self.args = args.iter().map(|s| s.to_string()).collect();
        self
    }

    /// Expect a trap rather than a result.
    pub fn traps(mut self, reasons: &[&str]) -> Case {
        self.want = Want::Trap(reasons.iter().map(|s| s.to_string()).collect());
        self
    }
}

/// A `() -> i32` case that must print `want`.
pub fn case_i32(name: impl Into<String>, body: Expr, want: i32) -> Case {
    Case::new(
        name,
        ftype(&[], &[ValType::I32]),
        body,
        &[&want.to_string()],
    )
}

/// A `() -> i64` case that must print `want`.
pub fn case_i64(name: impl Into<String>, body: Expr, want: i64) -> Case {
    Case::new(
        name,
        ftype(&[], &[ValType::I64]),
        body,
        &[&want.to_string()],
    )
}

/// A `() -> f32` case that must print `want`, formatted the way a runtime prints floats.
pub fn case_f32(name: impl Into<String>, body: Expr, want: f32) -> Case {
    Case::new(name, ftype(&[], &[ValType::F32]), body, &[&show_f32(want)])
}

/// A `() -> f64` case that must print `want`.
pub fn case_f64(name: impl Into<String>, body: Expr, want: f64) -> Case {
    Case::new(name, ftype(&[], &[ValType::F64]), body, &[&show_f64(want)])
}

/// A `() -> i32` case that must trap for the given canonical reason.
pub fn case_trap(name: impl Into<String>, body: Expr, reasons: &[&str]) -> Case {
    Case::new(name, ftype(&[], &[ValType::I32]), body, &[]).traps(reasons)
}

/// A `() -> ()` case that must trap.
pub fn case_void_trap(name: impl Into<String>, body: Expr, reasons: &[&str]) -> Case {
    Case::new(name, ftype(&[], &[]), body, &[]).traps(reasons)
}

/// Build a module from `base` plus one export per case, then run every case.
///
/// The export names are `f0`, `f1`, … so a case's name can be a sentence; failures name the
/// case, the export and the module.
pub fn run_cases_with(
    ctx: &mut Ctx,
    mut base: ModuleBuilder,
    cases: Vec<Case>,
) -> Result<(), Failure> {
    if cases.is_empty() {
        return Err(Failure::harness("a sweep with no cases proves nothing"));
    }
    let mut plan: Vec<(String, String, Want, Vec<String>)> = Vec::new();
    for (i, c) in cases.into_iter().enumerate() {
        let ty = base.add_type(c.sig);
        let idx = base.add_func(ty, Func::with_locals(&c.locals, c.body));
        let export = format!("f{i}");
        base = base.export_func(&export, idx);
        plan.push((c.name, export, c.want, c.args));
    }
    let m = base.build();
    for (name, export, want, args) in plan {
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        match want {
            Want::Lines(lines) => {
                let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
                let run = ctx.invoke(&m, &export, &argv)?;
                check_output(
                    &m,
                    &run,
                    &format!("case '{name}' (--invoke {export})"),
                    &refs,
                )
                .map_err(|f| f.note(format!("the case is '{name}'")))?;
            }
            Want::Trap(reasons) => {
                let refs: Vec<&str> = reasons.iter().map(String::as_str).collect();
                let run = ctx.invoke(&m, &export, &argv)?;
                check_trap(&m, &run, &refs).map_err(|f| f.note(format!("the case is '{name}'")))?;
            }
        }
    }
    Ok(())
}

/// Build a plain module holding every case and run them.
pub fn run_cases(ctx: &mut Ctx, label: &str, cases: Vec<Case>) -> Result<(), Failure> {
    run_cases_with(ctx, ModuleBuilder::new(label), cases)
}

/// Turn any error into a harness failure that names what the harness was doing.
pub fn harness<T, E: std::fmt::Display>(what: &str, r: Result<T, E>) -> Result<T, Failure> {
    r.map_err(|e| Failure::new(FailureKind::Harness, format!("{what}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wasm::op;

    #[test]
    fn a_single_export_module_is_named_f() {
        let m = f_i32("seven", Expr::new().i32_const(7));
        let listing = m.listing(4096);
        assert!(listing.contains("'f' → func"), "{listing}");
        assert!(listing.contains("i32.const 7"), "{listing}");
    }

    #[test]
    fn floats_print_the_way_a_runtime_prints_them() {
        assert_eq!(show_f64(1.5), "1.5");
        assert_eq!(show_f64(-0.0), "-0");
        assert_eq!(show_f64(f64::INFINITY), "inf");
        assert_eq!(show_f64(f64::NEG_INFINITY), "-inf");
        assert_eq!(show_f64(f64::NAN), "NaN");
        assert_eq!(show_f32(3.25), "3.25");
    }

    #[test]
    fn warnings_are_not_mistaken_for_errors() {
        let stderr = "warning: using `--invoke` is experimental\nError: it went wrong\n";
        assert_eq!(first_meaningful_line(stderr), "Error: it went wrong");
        assert_eq!(first_meaningful_line("warning: only this\n"), "");
    }

    #[test]
    fn a_case_carries_its_signature_and_expectation() {
        let c = case_i32("one plus one", Expr::new().i32_const(2), 2);
        assert_eq!(c.sig, ftype(&[], &[ValType::I32]));
        match c.want {
            Want::Lines(l) => assert_eq!(l, vec!["2"]),
            Want::Trap(_) => panic!("expected lines"),
        }
        let t = case_trap(
            "divide by zero",
            Expr::new().op(op::I32_ADD),
            &[trap::DIVIDE_BY_ZERO],
        );
        assert!(matches!(t.want, Want::Trap(_)));
    }
}
