//! Stage 41 — WASI preview1: `args_sizes_get`, `args_get`, the environment and `proc_exit`.
//! **[ext]**
//!
//! `argv[0]` is not a fixed string, so nothing here compares it to one. A WASI command is
//! started as `wasmtime run <module.wasm> [args...]`, and the host hands the module the
//! **basename** of the path it was given as the program name — which for this suite is the
//! `NNN-<label>.wasm` file the harness wrote into a fresh temporary directory, so both the
//! number and the directory move between runs. Every test here therefore asserts on
//! `argv[1..]`, on `argc`, and on the *relationship* between `argv[0]` and the buffer size
//! (`args_sizes_get` must report exactly the bytes `args_get` will write, `argv[0]`
//! included), and records what `argv[0]` actually was with `ctx.note`.
//!
//! The environment is the same kind of thing: whether a host passes its own environment
//! through to a guest is the host's choice, and `wasmtime` chooses not to — it answers
//! count 0, size 0 unless it is asked for `--env`. So the environment test asserts
//! consistency (the count, the buffer size, the number of NUL terminators in the buffer and
//! the number of pointers landing inside it all agree) rather than a fixed count, and notes
//! the numbers. A host that does pass its environment through passes the same test.
//!
//! `proc_exit` has type `(i32) -> ()` because it never returns. That is a promise to the
//! *host*, not to the validator: the instructions after the call are still type-checked and
//! still have to leave the stack in the shape the function's result type asks for. Every
//! function here returns nothing, so the dead `fd_write` that each `proc_exit` test puts
//! after the call — the one whose output must never appear — validates on its own.
//!
//! The memory map, on top of the one in [`crate::stages::wasi`]:
//!
//! | address | what |
//! |---|---|
//! | `0x0400` | `argc`, `argv_buf_size`, `environ_count`, `environ_buf_size` out-parameters |
//! | `0x0480` | the decimal-printing scratch buffer |
//! | `0x0500` | the `argv` pointer array |
//! | `0x0600` | the `argv` string buffer |
//! | `0x2000` | the `environ` pointer array |
//! | `0x3000` | the `environ` string buffer |

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::stages::wasi::{self, W};
use crate::stages::{Stage, Test};
use crate::wasm::{ftype, op, BlockType, Expr, Func, Module, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 41,
        slug: "wasi_args_and_exit",
        name: "WASI: args, environ and proc_exit",
        ext: true,
        hints: &[
            "args_sizes_get(argc_out, buf_size_out) writes two i32s: how many arguments there are, and how many bytes args_get will need for all of them including one NUL each",
            "argv[0] is the program name — the basename of the module path the runtime was given — so argc is one more than the number of arguments after the module on the command line",
            "args_get(argv, argv_buf) writes the strings back to back into argv_buf, each terminated by a NUL, and one pointer into that buffer per argument into argv; pass the bytes through untouched, with no re-splitting on spaces and no re-encoding",
            "proc_exit(code) ends the whole program there and then with that exit status: nothing after the call runs, no caller up the stack resumes, and its imported type is (i32) -> () because it never returns",
        ],
        examples,
        tests: vec![
            Test::new(
                "args_sizes_get counts the program name and the bytes it needs",
                sizes_bare,
            )
            .ext()
            .tag("wasi"),
            Test::new(
                "three arguments on the command line make argc four",
                sizes_three,
            )
            .ext()
            .tag("wasi"),
            Test::new(
                "args_get fills the pointer array and the NUL-separated buffer",
                args_get_buffer,
            )
            .ext()
            .tag("wasi"),
            Test::new(
                "an argument is passed through byte for byte, spaces and UTF-8 included",
                args_are_not_reinterpreted,
            )
            .ext()
            .tag("wasi"),
            Test::new(
                "environ_sizes_get and environ_get agree with each other",
                environ_is_consistent,
            )
            .ext()
            .tag("wasi"),
            Test::new("proc_exit(0) ends the program with status 0", exit_zero)
                .ext()
                .tag("wasi"),
            Test::new(
                "proc_exit(7) ends the program with status 7 and runs nothing after it",
                exit_seven,
            )
            .ext()
            .tag("wasi"),
            Test::new(
                "proc_exit inside a nested call does not return to its callers",
                exit_from_a_nested_call,
            )
            .ext()
            .tag("wasi"),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// The module's memory map and its imports
// ---------------------------------------------------------------------------------------

/// Where `args_sizes_get` leaves `argc`.
const ARGC_OUT: i32 = wasi::SCRATCH;
/// Where `args_sizes_get` leaves the size of the `argv` buffer.
const ARGV_BUF_SIZE_OUT: i32 = wasi::SCRATCH + 4;
/// Where `environ_sizes_get` leaves the number of variables.
const ENV_COUNT_OUT: i32 = wasi::SCRATCH + 8;
/// Where `environ_sizes_get` leaves the size of the `environ` buffer.
const ENV_BUF_SIZE_OUT: i32 = wasi::SCRATCH + 12;
/// The scratch buffer the decimal printer fills.
const NUM: i32 = 0x0480;
/// The `argv` pointer array.
const ARGV_PTRS: i32 = 0x0500;
/// The `argv` string buffer.
const ARGV_BUF: i32 = 0x0600;
/// The `environ` pointer array.
const ENV_PTRS: i32 = 0x2000;
/// The `environ` string buffer.
const ENV_BUF: i32 = 0x3000;

/// The one-byte separator every argument listing writes after each string.
const BAR: i32 = 0;
/// The one-byte newline every listing ends a line with.
const NL: i32 = 1;
/// The punctuation every module in this stage keeps at address 0, so a listing can be
/// written with nothing but `fd_write`.
const PUNCTUATION: &[u8] = b"|\n";

/// The imports every module in this stage declares, in index order.
const IMPORTS: &[W] = &[
    W::FdWrite,
    W::ArgsGet,
    W::ArgsSizesGet,
    W::EnvironGet,
    W::EnvironSizesGet,
    W::ProcExit,
];

/// `fd_write`'s function index — imported functions come first, in the order of [`IMPORTS`].
const FD_WRITE: u32 = 0;
/// `args_get`'s function index.
const ARGS_GET: u32 = 1;
/// `args_sizes_get`'s function index.
const ARGS_SIZES_GET: u32 = 2;
/// `environ_get`'s function index.
const ENVIRON_GET: u32 = 3;
/// `environ_sizes_get`'s function index.
const ENVIRON_SIZES_GET: u32 = 4;
/// `proc_exit`'s function index.
const PROC_EXIT: u32 = 5;
/// Locals 0 and 1 belong to the decimal printer; 2 to 5 are this stage's loop variables.
const LOCALS: &[(u32, ValType)] = &[(6, ValType::I32)];

/// Local holding a loop counter.
const I: u32 = 2;
/// Local holding the string being walked.
const PTR: u32 = 3;
/// Local holding the cursor walking it.
const CUR: u32 = 4;
/// Local holding a running total.
const N: u32 = 5;

// ---------------------------------------------------------------------------------------
// Building the modules
// ---------------------------------------------------------------------------------------

/// A WASI command whose `_start` is `body`, with [`PUNCTUATION`] at address 0 and whatever
/// else `data` asks for.
fn command(label: &str, data: &[(i32, &[u8])], body: Expr) -> Module {
    let (mut b, _) = wasi::wasi_command(label, 1, IMPORTS);
    let ty = b.add_type(ftype(&[], &[]));
    let start = b.add_func(ty, Func::with_locals(LOCALS, body));
    b = b.export_func("_start", start).data_active(0, PUNCTUATION);
    for (at, bytes) in data {
        b = b.data_active(*at, bytes);
    }
    b.build()
}

/// Print one unsigned decimal number and a newline on stdout.
fn say(value: Expr) -> Expr {
    wasi::print_i32(wasi::STDOUT, value, NUM, FD_WRITE)
}

/// The `i32` at a fixed address.
fn load(at: i32) -> Expr {
    Expr::new().i32_const(at).i32_load(0)
}

/// `for i in 0..*count_at { body }`, counting in local [`I`].
fn for_each(count_at: i32, body: Expr) -> Expr {
    Expr::new().i32_const(0).local_set(I).block(
        BlockType::Empty,
        Expr::new().loop_(
            BlockType::Empty,
            Expr::new()
                .local_get(I)
                .then(load(count_at))
                .op(op::I32_GE_U)
                .br_if(1)
                .then(body)
                .local_get(I)
                .i32_const(1)
                .op(op::I32_ADD)
                .local_set(I)
                .br(0),
        ),
    )
}

/// Load the `i`-th pointer of the array at `base` into local [`PTR`].
fn nth_pointer(base: i32) -> Expr {
    Expr::new()
        .local_get(I)
        .i32_const(4)
        .op(op::I32_MUL)
        .i32_const(base)
        .op(op::I32_ADD)
        .i32_load(0)
        .local_set(PTR)
}

/// Write the NUL-terminated string at local [`PTR`] to stdout, without its terminator.
///
/// The length is found by walking the bytes, which is the point: `args_get` promises the
/// strings are NUL-separated inside the buffer, and a module that wants one of them back has
/// to take the promise at its word. One `fd_write` of a handful of bytes is never written
/// short, so the return value is dropped.
fn put_cstr() -> Expr {
    Expr::new()
        .local_get(PTR)
        .local_set(CUR)
        .block(
            BlockType::Empty,
            Expr::new().loop_(
                BlockType::Empty,
                Expr::new()
                    .local_get(CUR)
                    .mem(op::I32_LOAD8_U, 0, 0)
                    .op(op::I32_EQZ)
                    .br_if(1)
                    .local_get(CUR)
                    .i32_const(1)
                    .op(op::I32_ADD)
                    .local_set(CUR)
                    .br(0),
            ),
        )
        .i32_const(wasi::IOV)
        .local_get(PTR)
        .i32_store(0)
        .i32_const(wasi::IOV + 4)
        .local_get(CUR)
        .local_get(PTR)
        .op(op::I32_SUB)
        .i32_store(0)
        .i32_const(wasi::STDOUT)
        .i32_const(wasi::IOV)
        .i32_const(1)
        .i32_const(wasi::NWRITTEN)
        .call(FD_WRITE)
        .drop()
}

/// A module that reports `args_sizes_get`, then `args_get`, then every argument on its own
/// line: errno, argc, argv_buf_size, errno, `argv[0]`, `argv[1]`, …
fn args_report() -> Module {
    let body = say(Expr::new()
        .i32_const(ARGC_OUT)
        .i32_const(ARGV_BUF_SIZE_OUT)
        .call(ARGS_SIZES_GET))
    .then(say(load(ARGC_OUT)))
    .then(say(load(ARGV_BUF_SIZE_OUT)))
    .then(say(Expr::new()
        .i32_const(ARGV_PTRS)
        .i32_const(ARGV_BUF)
        .call(ARGS_GET)))
    .then(for_each(
        ARGC_OUT,
        nth_pointer(ARGV_PTRS)
            .then(put_cstr())
            .then(wasi::puts(wasi::STDOUT, NL, 1, FD_WRITE)),
    ));
    command("args-report", &[], body)
}

/// A module that prints every argument separated by `|`, then the raw `argv` buffer with its
/// NULs turned into `.` — the same bytes, seen twice, once through the pointer array and
/// once as the host laid them out.
fn args_buffer_dump() -> Module {
    let body = Expr::new()
        .i32_const(ARGC_OUT)
        .i32_const(ARGV_BUF_SIZE_OUT)
        .call(ARGS_SIZES_GET)
        .drop()
        .i32_const(ARGV_PTRS)
        .i32_const(ARGV_BUF)
        .call(ARGS_GET)
        .drop()
        .then(for_each(
            ARGC_OUT,
            nth_pointer(ARGV_PTRS).then(put_cstr()).then(wasi::puts(
                wasi::STDOUT,
                BAR,
                1,
                FD_WRITE,
            )),
        ))
        .then(wasi::puts(wasi::STDOUT, NL, 1, FD_WRITE))
        // Turn every NUL in the buffer into a full stop, then write the lot in one go.
        .then(for_each(
            ARGV_BUF_SIZE_OUT,
            Expr::new()
                .local_get(I)
                .i32_const(ARGV_BUF)
                .op(op::I32_ADD)
                .local_set(PTR)
                .local_get(PTR)
                .mem(op::I32_LOAD8_U, 0, 0)
                .op(op::I32_EQZ)
                .if_(
                    BlockType::Empty,
                    Expr::new()
                        .local_get(PTR)
                        .i32_const(b'.' as i32)
                        .mem(op::I32_STORE8, 0, 0),
                ),
        ))
        .i32_const(wasi::IOV)
        .i32_const(ARGV_BUF)
        .i32_store(0)
        .i32_const(wasi::IOV + 4)
        .then(load(ARGV_BUF_SIZE_OUT))
        .i32_store(0)
        .i32_const(wasi::STDOUT)
        .i32_const(wasi::IOV)
        .i32_const(1)
        .i32_const(wasi::NWRITTEN)
        .call(FD_WRITE)
        .drop()
        .then(wasi::puts(wasi::STDOUT, NL, 1, FD_WRITE));
    command("args-buffer-dump", &[], body)
}

/// A module that reports the environment and then checks the host's own answer against
/// itself: errno, count, buf_size, errno, NULs found in the buffer, pointers landing in it.
fn environ_report() -> Module {
    let count_nuls = Expr::new().i32_const(0).local_set(N).then(for_each(
        ENV_BUF_SIZE_OUT,
        Expr::new()
            .local_get(I)
            .i32_const(ENV_BUF)
            .op(op::I32_ADD)
            .mem(op::I32_LOAD8_U, 0, 0)
            .op(op::I32_EQZ)
            .local_get(N)
            .op(op::I32_ADD)
            .local_set(N),
    ));
    let count_pointers = Expr::new().i32_const(0).local_set(N).then(for_each(
        ENV_COUNT_OUT,
        nth_pointer(ENV_PTRS)
            .local_get(PTR)
            .i32_const(ENV_BUF)
            .op(op::I32_GE_U)
            .local_get(PTR)
            .i32_const(ENV_BUF)
            .then(load(ENV_BUF_SIZE_OUT))
            .op(op::I32_ADD)
            .op(op::I32_LT_U)
            .op(op::I32_AND)
            .local_get(N)
            .op(op::I32_ADD)
            .local_set(N),
    ));
    let body = say(Expr::new()
        .i32_const(ENV_COUNT_OUT)
        .i32_const(ENV_BUF_SIZE_OUT)
        .call(ENVIRON_SIZES_GET))
    .then(say(load(ENV_COUNT_OUT)))
    .then(say(load(ENV_BUF_SIZE_OUT)))
    .then(say(Expr::new()
        .i32_const(ENV_PTRS)
        .i32_const(ENV_BUF)
        .call(ENVIRON_GET)))
    .then(count_nuls)
    .then(say(Expr::new().local_get(N)))
    .then(count_pointers)
    .then(say(Expr::new().local_get(N)));
    command("environ-report", &[], body)
}

/// Where the `proc_exit` modules keep "before" and "after".
const BEFORE: i32 = 0x10;
/// Where the `proc_exit` modules keep the line that must never be printed.
const AFTER: i32 = 0x20;
/// The line every `proc_exit` module prints before it goes.
const BEFORE_TEXT: &[u8] = b"before\n";
/// The line no `proc_exit` module may ever print.
const AFTER_TEXT: &[u8] = b"after\n";

/// A command that prints `before`, calls `proc_exit(code)`, and then tries to print `after`.
fn exiting_command(label: &str, code: i32) -> Module {
    let body = wasi::puts(wasi::STDOUT, BEFORE, BEFORE_TEXT.len() as i32, FD_WRITE)
        .i32_const(code)
        .call(PROC_EXIT)
        .then(wasi::puts(
            wasi::STDOUT,
            AFTER,
            AFTER_TEXT.len() as i32,
            FD_WRITE,
        ));
    command(label, &[(BEFORE, BEFORE_TEXT), (AFTER, AFTER_TEXT)], body)
}

/// A three-deep call chain whose innermost function calls `proc_exit(7)`.
///
/// Each level prints its own name on the way down and would print it again on the way up.
/// The way up never happens, which is the whole point.
fn nested_exit() -> Module {
    let (mut b, _) = wasi::wasi_command("proc-exit-nested", 1, IMPORTS);
    let ty = b.add_type(ftype(&[], &[]));
    // 0x10 "one\n", 0x20 "two\n", 0x30 "three\n", 0x40 "up\n"
    let inner = b.add_func(
        ty,
        Func::new(
            wasi::puts(wasi::STDOUT, 0x30, 6, FD_WRITE)
                .i32_const(7)
                .call(PROC_EXIT),
        ),
    );
    let middle = b.add_func(
        ty,
        Func::new(
            wasi::puts(wasi::STDOUT, 0x20, 4, FD_WRITE)
                .call(inner)
                .then(wasi::puts(wasi::STDOUT, 0x40, 3, FD_WRITE)),
        ),
    );
    let start = b.add_func(
        ty,
        Func::new(
            wasi::puts(wasi::STDOUT, 0x10, 4, FD_WRITE)
                .call(middle)
                .then(wasi::puts(wasi::STDOUT, 0x40, 3, FD_WRITE)),
        ),
    );
    b.export_func("_start", start)
        .data_active(0x10, b"one\n")
        .data_active(0x20, b"two\n")
        .data_active(0x30, b"three\n")
        .data_active(0x40, b"up\n")
        .build()
}

// ---------------------------------------------------------------------------------------
// Reading what a module printed
// ---------------------------------------------------------------------------------------

/// The `i`-th line as an unsigned number, or `None` when it is missing or not one.
fn num(lines: &[String], i: usize) -> Option<u32> {
    lines.get(i).and_then(|l| l.trim().parse::<u32>().ok())
}

/// The `i`-th line, or an empty string.
fn line(lines: &[String], i: usize) -> String {
    lines.get(i).cloned().unwrap_or_default()
}

/// How many bytes `args_get` needs for these strings: every one of them, plus a NUL each.
fn buffer_size(strings: &[String]) -> usize {
    strings.iter().map(|s| s.len() + 1).sum()
}

// ---------------------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------------------

wasm_test!(sizes_bare, |ctx| {
    let m = args_report();
    let run = ctx.start(&m, &[])?;
    let lines = run.lines();
    let argv0 = line(&lines, 4);
    let mut c = Check::new("args_sizes_get with nothing after the module path", &run);
    c.module(&m);
    c.eq("args_sizes_get errno", Some(0u32), num(&lines, 0));
    c.eq("argc", Some(1u32), num(&lines, 1));
    c.eq("args_get errno", Some(0u32), num(&lines, 3));
    c.that(
        "argv[0]",
        "the program name: the basename of the module path the runtime was given",
        argv0.ends_with(".wasm") && !argv0.contains('/'),
        argv0.clone(),
    );
    c.eq(
        "argv_buf_size",
        Some(argv0.len() as u32 + 1),
        num(&lines, 2),
    );
    c.eq("lines printed", 5usize, lines.len());
    c.note(
        "argv[0] changes from run to run, so what is asserted is that the buffer size is \
         exactly its length plus the one NUL that terminates it",
    );
    c.finish()?;
    ctx.note(format!("argv[0] was {argv0:?}"));
    Ok(())
});

wasm_test!(sizes_three, |ctx| {
    let m = args_report();
    let args = ["alpha", "beta", "gamma"];
    let run = ctx.start(&m, &args)?;
    let lines = run.lines();
    let argv: Vec<String> = lines.iter().skip(4).cloned().collect();
    let mut c = Check::new("args_sizes_get with three arguments", &run);
    c.module(&m);
    c.eq("args_sizes_get errno", Some(0u32), num(&lines, 0));
    c.eq("argc", Some(4u32), num(&lines, 1));
    c.eq(
        "argv[1..]",
        args.iter().map(|s| s.to_string()).collect::<Vec<String>>(),
        argv.iter().skip(1).cloned().collect::<Vec<String>>(),
    );
    c.eq(
        "argv_buf_size",
        Some(buffer_size(&argv) as u32),
        num(&lines, 2),
    );
    c.note(
        "the buffer size is the sum of every argument's length plus one NUL each, the \
         program name included — computed here from the file name the module was actually \
         given",
    );
    c.finish()?;
    ctx.note(format!(
        "argv[0] was {:?}, so argv_buf_size is {} + {} bytes of arguments and NULs",
        line(&lines, 4),
        line(&lines, 4).len() + 1,
        buffer_size(&argv) - line(&lines, 4).len() - 1
    ));
    Ok(())
});

wasm_test!(args_get_buffer, |ctx| {
    // Every argument read back through the pointer array, then the buffer itself with its
    // NULs made visible. The two must be the same text: that is what "the pointers point
    // into the NUL-separated buffer" means.
    let m = args_buffer_dump();
    let run = ctx.start(&m, &["one", "two"])?;
    let lines = run.lines();
    let joined = line(&lines, 0);
    let buffer = line(&lines, 1);
    let mut c = Check::new("args_get's pointer array against its buffer", &run);
    c.module(&m);
    c.that(
        "argv joined by '|'",
        "the program name, then \"one|two|\"",
        joined.ends_with("|one|two|"),
        joined.clone(),
    );
    c.eq(
        "the argv buffer, NULs shown as '.'",
        joined.replace('|', "."),
        buffer,
    );
    c.eq("lines printed", 2usize, lines.len());
    c.note(
        "the first line walks argv[i] and writes each string followed by '|'; the second \
         writes argv_buf_size bytes straight out of the buffer with every NUL replaced by \
         '.'. They agree only if args_get wrote the strings back to back, terminated each \
         one, and pointed at them in order",
    );
    c.finish()?;
    Ok(())
});

wasm_test!(args_are_not_reinterpreted, |ctx| {
    // An argument holding a space must not become two, and one holding multi-byte UTF-8 must
    // come back with the same bytes: preview1 moves bytes, it does not parse them.
    let m = args_report();
    let args = ["two words", "naïve café", "→ ünïcødé ←"];
    let run = ctx.start(&m, &args)?;
    let lines = run.lines();
    let argv: Vec<String> = lines.iter().skip(4).cloned().collect();
    let mut c = Check::new("arguments that a naive host would mangle", &run);
    c.module(&m);
    c.eq("argc", Some(4u32), num(&lines, 1));
    c.eq(
        "argv[1..]",
        args.iter().map(|s| s.to_string()).collect::<Vec<String>>(),
        argv.iter().skip(1).cloned().collect::<Vec<String>>(),
    );
    c.eq(
        "argv_buf_size",
        Some(buffer_size(&argv) as u32),
        num(&lines, 2),
    );
    c.note(
        "\"two words\" is one argument, not two, and the accented letters are two bytes \
         each — which is why the buffer size is counted in bytes and not in characters",
    );
    c.finish()?;
    ctx.note(format!(
        "{} bytes of UTF-8 in argv[2..] came back unchanged",
        args[1].len() + args[2].len()
    ));
    Ok(())
});

wasm_test!(environ_is_consistent, |ctx| {
    let m = environ_report();
    let run = ctx.start(&m, &[])?;
    let lines = run.lines();
    let count = num(&lines, 1);
    let size = num(&lines, 2);
    let mut c = Check::new("environ_sizes_get against environ_get", &run);
    c.module(&m);
    c.eq("environ_sizes_get errno", Some(0u32), num(&lines, 0));
    c.eq("environ_get errno", Some(0u32), num(&lines, 3));
    c.eq("NUL terminators inside the buffer", count, num(&lines, 4));
    c.eq("pointers landing inside the buffer", count, num(&lines, 5));
    c.that(
        "count and buf_size",
        "either both zero, or both not: one variable needs at least its NUL",
        (count == Some(0)) == (size == Some(0)),
        format!("count {count:?}, buf_size {size:?}"),
    );
    c.note(
        "whether a host passes its own environment through to the guest is the host's \
         choice, so the count is not compared to anything; what is compared is the host's \
         answer against itself",
    );
    c.finish()?;
    ctx.note(format!(
        "environ_sizes_get answered count {} and buf_size {} — wasmtime passes no \
         environment through unless it is asked with --env",
        count.unwrap_or(0),
        size.unwrap_or(0)
    ));
    Ok(())
});

wasm_test!(exit_zero, |ctx| {
    let m = exiting_command("proc-exit-0", 0);
    let run = ctx.start(&m, &[])?;
    let mut c = Check::new("proc_exit(0)", &run);
    c.module(&m);
    c.eq("exit", Some(0i32), run.exit.code);
    c.eq("stdout", "before\n", run.stdout.as_str());
    c.that(
        "the fd_write after proc_exit",
        "never reached: proc_exit does not return",
        !run.stdout.contains("after"),
        run.stdout.clone(),
    );
    c.finish()
});

wasm_test!(exit_seven, |ctx| {
    let m = exiting_command("proc-exit-7", 7);
    let run = ctx.start(&m, &[])?;
    let mut c = Check::new("proc_exit(7)", &run);
    c.module(&m);
    c.eq("exit", Some(7i32), run.exit.code);
    c.eq("stdout", "before\n", run.stdout.as_str());
    c.that(
        "the fd_write after proc_exit",
        "never reached: proc_exit does not return",
        !run.stdout.contains("after"),
        run.stdout.clone(),
    );
    c.note(
        "the exit status is the argument, not a generic failure code, and it is not a trap: \
         proc_exit is how a WASI command ends on purpose",
    );
    c.finish()
});

wasm_test!(exit_from_a_nested_call, |ctx| {
    let m = nested_exit();
    let run = ctx.start(&m, &[])?;
    let mut c = Check::new("proc_exit two calls deep", &run);
    c.module(&m);
    c.eq("exit", Some(7i32), run.exit.code);
    c.eq("stdout", "one\ntwo\nthree\n", run.stdout.as_str());
    c.that(
        "the two fd_writes on the way back up",
        "never reached: proc_exit unwinds nothing, it ends the program",
        !run.stdout.contains("up"),
        run.stdout.clone(),
    );
    c.finish()
});

/// Worked examples: 1-3, each a real module this stage builds.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("Every argument, read back out of memory", args_report)
            .summary(
                "calls args_sizes_get into 0x400, prints argc and the buffer size, calls \
                 args_get(0x500, 0x600), then walks the pointer array and writes each \
                 NUL-terminated string to stdout",
            )
            .command("run mod.wasm alpha beta gamma")
            .output("0\n4\n38\n0\n001-args-report.wasm\nalpha\nbeta\ngamma")
            .note(
                "argc counts the program name, so three arguments make four. argv[0] is the \
                 basename of the module path the runtime was given, which is why the third \
                 line — the buffer size — is only predictable as a sum: every string's \
                 length plus one NUL each.",
            ),
        ExampleSpec::module(
            "The argv buffer, with its NULs made visible",
            args_buffer_dump,
        )
        .summary(
            "the same two calls, but printed twice: once by following argv[i] and \
                 writing each string followed by '|', and once by writing argv_buf_size \
                 bytes straight out of the buffer with every NUL turned into '.'",
        )
        .command("run mod.wasm one two")
        .output("001-args-buffer-dump.wasm|one|two|\n001-args-buffer-dump.wasm.one.two.")
        .note(
            "The two lines are the same text with one character changed, and they can \
                 only be that if args_get laid the strings out back to back, terminated each \
                 one, and pointed at them in order.",
        ),
        ExampleSpec::module("proc_exit(7), and the code that never runs", || {
            exiting_command("proc-exit-7", 7)
        })
        .summary(
            "writes \"before\" to stdout, calls proc_exit(7), and then asks fd_write to \
             print \"after\"",
        )
        .command("run mod.wasm")
        .output("before        (stdout, then exit status 7)")
        .note(
            "proc_exit never returns, so \"after\" is dead code — but it is still validated, \
             because its imported type being (i32) -> () is a promise to the host and not to \
             the type checker. The exit status is the argument, and this is not a trap.",
        ),
    ]
}
