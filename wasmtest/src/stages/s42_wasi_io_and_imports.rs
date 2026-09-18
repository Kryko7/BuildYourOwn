//! Stage 42 — WASI preview1: `fd_read`, the clocks, `random_get`, and imports the host does
//! not have. **[ext]**
//!
//! Three of these expectations are deliberately loose, and it is worth knowing exactly how
//! loose.
//!
//! `fd_read`, like `fd_write` in stage 40, is allowed to come back with **fewer** bytes than
//! the iovec has room for and say so in `*nread` — a pipe hands over what has arrived, not
//! what will arrive — and the `fd_write` that echoes them may in turn be short. So the echo
//! test asserts the relationships that always hold (what reaches stdout is a non-empty
//! prefix of what was fed in, and `*nread` is at least that many bytes and at most the whole
//! input) and records both counts with `ctx.note`, rather than demanding the whole string
//! back in one call.
//!
//! The clocks belong to the host. `clock_time_get` with the monotonic id (1) counts
//! nanoseconds from an epoch the host chooses and at a resolution the host chooses, so the
//! only thing asserted is that a second reading is not *earlier* than the first and that the
//! gap between two adjacent calls is small; a coarse clock that answers the same value twice
//! passes, which is correct. The realtime id (0) does have a fixed epoch — the Unix one — so
//! that one is checked for a plausible number of seconds since 1970 and the value is noted.
//!
//! `random_get` is checked statistically, and only for the failure mode a stub actually has.
//! Sixty-four bytes are asked for; the test counts how many are non-zero and how many
//! distinct values appear, and the bounds (at least 48 non-zero, at least 16 distinct) are
//! so far from what real randomness produces — 63.75 non-zero and about 57 distinct on
//! average — that no correct implementation can fail them and no constant-returning stub can
//! pass them. **This is not a test of randomness quality**: it says nothing about
//! distribution, period or unpredictability, and it is not meant to.
//!
//! The memory map, on top of the one in [`crate::stages::wasi`]:
//!
//! | address | what |
//! |---|---|
//! | `0x0420` | the `nread` out-parameter, kept away from `nwritten` so the echo cannot eat it |
//! | `0x0430` | two `i64` clock readings |
//! | `0x0480` | the decimal-printing scratch buffer |
//! | `0x0500` | the `fd_read` buffer |
//! | `0x0700` | the `random_get` buffer |

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::stages::wasi::{self, W};
use crate::stages::{expect_start_rejected, first_meaningful_line, Stage, Test};
use crate::wasm::{ftype, op, BlockType, Expr, Func, ImportKind, Module, ValType};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 42,
        slug: "wasi_io_and_imports",
        name: "WASI: fd_read, clocks, randomness and missing imports",
        ext: true,
        hints: &[
            "fd_read(fd, iovs, iovs_len, nread) is fd_write backwards: fill the iovecs from the descriptor, store how many bytes you really moved at nread, and return errno 0 — end of input is nread 0 and errno 0, not an error",
            "clock_time_get(id, precision, time_out) writes an i64 count of nanoseconds: id 0 is the realtime clock, counted from the Unix epoch, and id 1 the monotonic one, counted from an epoch you choose but never going backwards",
            "random_get(buf, len) fills len bytes of the guest's memory and returns errno 0; a len of 0 is legal and must leave the memory exactly as it was",
            "A module that imports a name you do not provide must be refused before anything runs: nothing on stdout, a non-zero exit, and a message naming the import — instantiation fails, it does not trap",
        ],
        examples,
        tests: vec![
            Test::new("fd_read brings stdin into the guest's memory", read_stdin)
                .ext()
                .tag("wasi"),
            Test::new(
                "fd_read at end of input is nread 0 and errno 0, not an error",
                read_at_end_of_input,
            )
            .ext()
            .tag("wasi"),
            Test::new(
                "fd_read on a descriptor nobody opened is a non-zero errno",
                read_bad_fd,
            )
            .ext()
            .tag("wasi"),
            Test::new("the monotonic clock never goes backwards", monotonic_clock)
                .ext()
                .tag("wasi"),
            Test::new(
                "the realtime clock answers a plausible time since the Unix epoch",
                realtime_clock,
            )
            .ext()
            .tag("wasi"),
            Test::new(
                "random_get fills a buffer with something that is not one repeated byte",
                random_fills_a_buffer,
            )
            .ext()
            .tag("wasi"),
            Test::new(
                "random_get of length zero is legal and leaves the memory alone",
                random_of_length_zero,
            )
            .ext()
            .tag("wasi"),
            Test::new(
                "a module importing a name the host does not have never runs",
                missing_imports,
            )
            .ext()
            .tag("wasi"),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// The module's memory map and its imports
// ---------------------------------------------------------------------------------------

/// Where `fd_read` is asked to leave the byte count. Deliberately not [`wasi::NWRITTEN`]:
/// the `fd_write` that echoes the bytes would overwrite it.
const NREAD: i32 = 0x0420;
/// The first clock reading.
const TIME_A: i32 = 0x0430;
/// The second clock reading.
const TIME_B: i32 = 0x0438;
/// The scratch buffer the decimal printer fills.
const NUM: i32 = 0x0480;
/// Where `fd_read` puts what it read.
const READ_BUF: i32 = 0x0500;
/// How much room the read buffer has.
const READ_CAP: i32 = 256;
/// Where `random_get` is asked to put its bytes.
const RAND_BUF: i32 = 0x0700;
/// How many random bytes the statistical test asks for.
const RAND_LEN: i32 = 64;
/// The byte the zero-length test paints the buffer with first.
const PAINT: i32 = 0x5a;
/// How many painted bytes the zero-length test checks afterwards.
const PAINT_LEN: i32 = 8;

/// The monotonic clock's id in preview1.
const CLOCK_MONOTONIC: i32 = 1;
/// The realtime clock's id in preview1.
const CLOCK_REALTIME: i32 = 0;
/// The precision every `clock_time_get` here asks for: one microsecond, in nanoseconds.
const PRECISION: i64 = 1_000;

/// The imports every working module in this stage declares, in index order.
const IMPORTS: &[W] = &[W::FdWrite, W::FdRead, W::ClockTimeGet, W::RandomGet];

/// `fd_write`'s function index — imported functions come first, in the order of [`IMPORTS`].
const FD_WRITE: u32 = 0;
/// `fd_read`'s function index.
const FD_READ: u32 = 1;
/// `clock_time_get`'s function index.
const CLOCK_TIME_GET: u32 = 2;
/// `random_get`'s function index.
const RANDOM_GET: u32 = 3;

/// Locals 0 and 1 belong to the decimal printer; 2 to 6 are this stage's own.
const LOCALS: &[(u32, ValType)] = &[(7, ValType::I32)];

/// Local holding the outer loop counter.
const I: u32 = 2;
/// Local holding the inner loop counter.
const J: u32 = 3;
/// Local holding the byte, or the errno, currently being looked at.
const V: u32 = 4;
/// Local holding a running total.
const N: u32 = 5;
/// Local holding a yes-or-no flag.
const S: u32 = 6;

// ---------------------------------------------------------------------------------------
// Building the modules
// ---------------------------------------------------------------------------------------

/// A WASI command whose `_start` is `body`.
fn command(label: &str, body: Expr) -> Module {
    let (mut b, _) = wasi::wasi_command(label, 1, IMPORTS);
    let ty = b.add_type(ftype(&[], &[]));
    let start = b.add_func(ty, Func::with_locals(LOCALS, body));
    b.export_func("_start", start).build()
}

/// Print one unsigned decimal number and a newline on `fd`.
fn say(fd: i32, value: Expr) -> Expr {
    wasi::print_i32(fd, value, NUM, FD_WRITE)
}

/// `for <counter> in 0..limit { body }`, with `limit` re-evaluated every time round.
fn for_below(counter: u32, limit: Expr, body: Expr) -> Expr {
    Expr::new().i32_const(0).local_set(counter).block(
        BlockType::Empty,
        Expr::new().loop_(
            BlockType::Empty,
            Expr::new()
                .local_get(counter)
                .then(limit)
                .op(op::I32_GE_U)
                .br_if(1)
                .then(body)
                .local_get(counter)
                .i32_const(1)
                .op(op::I32_ADD)
                .local_set(counter)
                .br(0),
        ),
    )
}

/// The unsigned byte at `base + <counter>`.
fn byte_at(base: i32, counter: u32) -> Expr {
    Expr::new()
        .local_get(counter)
        .i32_const(base)
        .op(op::I32_ADD)
        .mem(op::I32_LOAD8_U, 0, 0)
}

/// Build one iovec at [`wasi::IOV`].
fn iovec(ptr: i32, len: Expr) -> Expr {
    Expr::new()
        .i32_const(wasi::IOV)
        .i32_const(ptr)
        .i32_store(0)
        .i32_const(wasi::IOV + 4)
        .then(len)
        .i32_store(0)
}

/// A command that reads `fd` once into [`READ_BUF`], echoes what it got to stdout, and then
/// reports the errno and the byte count on stderr — so stdout holds nothing but the echo.
fn read_echo(label: &str, fd: i32) -> Module {
    let body = iovec(READ_BUF, Expr::new().i32_const(READ_CAP))
        .i32_const(fd)
        .i32_const(wasi::IOV)
        .i32_const(1)
        .i32_const(NREAD)
        .call(FD_READ)
        .local_set(V)
        .i32_const(NREAD)
        .i32_load(0)
        .local_set(N)
        .then(iovec(READ_BUF, Expr::new().local_get(N)))
        .i32_const(wasi::STDOUT)
        .i32_const(wasi::IOV)
        .i32_const(1)
        .i32_const(wasi::NWRITTEN)
        .call(FD_WRITE)
        .drop()
        .then(say(wasi::STDERR, Expr::new().local_get(V)))
        .then(say(wasi::STDERR, Expr::new().local_get(N)));
    command(label, body)
}

/// A command that reads the monotonic clock twice and reports: errno, errno, whether the
/// second reading is not earlier than the first, whether the gap is under two seconds, and
/// the gap itself in nanoseconds.
fn monotonic_pair() -> Module {
    let gap = || {
        Expr::new()
            .i32_const(TIME_B)
            .i64_load(0)
            .i32_const(TIME_A)
            .i64_load(0)
            .op(op::I64_SUB)
    };
    let body = say(
        wasi::STDOUT,
        Expr::new()
            .i32_const(CLOCK_MONOTONIC)
            .i64_const(PRECISION)
            .i32_const(TIME_A)
            .call(CLOCK_TIME_GET),
    )
    .then(say(
        wasi::STDOUT,
        Expr::new()
            .i32_const(CLOCK_MONOTONIC)
            .i64_const(PRECISION)
            .i32_const(TIME_B)
            .call(CLOCK_TIME_GET),
    ))
    .then(say(
        wasi::STDOUT,
        Expr::new()
            .i32_const(TIME_B)
            .i64_load(0)
            .i32_const(TIME_A)
            .i64_load(0)
            .op(op::I64_GE_U),
    ))
    .then(say(
        wasi::STDOUT,
        gap().i64_const(2_000_000_000).op(op::I64_LT_U),
    ))
    .then(say(wasi::STDOUT, gap().op(op::I32_WRAP_I64)));
    command("clock-monotonic-pair", body)
}

/// A command that reads the realtime clock and reports: errno, whether it is non-zero, and
/// the reading divided down to whole seconds since the Unix epoch.
fn realtime_reading() -> Module {
    let body = say(
        wasi::STDOUT,
        Expr::new()
            .i32_const(CLOCK_REALTIME)
            .i64_const(PRECISION)
            .i32_const(TIME_A)
            .call(CLOCK_TIME_GET),
    )
    .then(say(
        wasi::STDOUT,
        Expr::new()
            .i32_const(TIME_A)
            .i64_load(0)
            .i64_const(0)
            .op(op::I64_NE),
    ))
    .then(say(
        wasi::STDOUT,
        Expr::new()
            .i32_const(TIME_A)
            .i64_load(0)
            .i64_const(1_000_000_000)
            .op(op::I64_DIV_U)
            .op(op::I32_WRAP_I64),
    ));
    command("clock-realtime", body)
}

/// A command that asks for [`RAND_LEN`] random bytes and reports: errno, how many are
/// non-zero, and how many distinct values are among them.
fn random_survey() -> Module {
    let count_non_zero = Expr::new().i32_const(0).local_set(N).then(for_below(
        I,
        Expr::new().i32_const(RAND_LEN),
        byte_at(RAND_BUF, I)
            .i32_const(0)
            .op(op::I32_NE)
            .local_get(N)
            .op(op::I32_ADD)
            .local_set(N),
    ));
    // A value is new when it does not appear anywhere earlier in the buffer, so the count of
    // first appearances is the count of distinct values. Sixty-four bytes make this 2016
    // comparisons, which is nothing.
    let count_distinct = Expr::new().i32_const(0).local_set(N).then(for_below(
        I,
        Expr::new().i32_const(RAND_LEN),
        byte_at(RAND_BUF, I)
            .local_set(V)
            .i32_const(0)
            .local_set(S)
            .then(for_below(
                J,
                Expr::new().local_get(I),
                byte_at(RAND_BUF, J)
                    .local_get(V)
                    .op(op::I32_EQ)
                    .if_(BlockType::Empty, Expr::new().i32_const(1).local_set(S)),
            ))
            .local_get(S)
            .op(op::I32_EQZ)
            .local_get(N)
            .op(op::I32_ADD)
            .local_set(N),
    ));
    let body = say(
        wasi::STDOUT,
        Expr::new()
            .i32_const(RAND_BUF)
            .i32_const(RAND_LEN)
            .call(RANDOM_GET),
    )
    .then(count_non_zero)
    .then(say(wasi::STDOUT, Expr::new().local_get(N)))
    .then(count_distinct)
    .then(say(wasi::STDOUT, Expr::new().local_get(N)));
    command("random-survey", body)
}

/// A command that paints [`PAINT_LEN`] bytes, asks `random_get` for **none**, and reports:
/// errno, and how many of the painted bytes survived.
fn random_zero_length() -> Module {
    let body = Expr::new()
        .i32_const(RAND_BUF)
        .i32_const(PAINT)
        .i32_const(PAINT_LEN)
        .memory_fill()
        .then(say(
            wasi::STDOUT,
            Expr::new()
                .i32_const(RAND_BUF)
                .i32_const(0)
                .call(RANDOM_GET),
        ))
        .i32_const(0)
        .local_set(N)
        .then(for_below(
            I,
            Expr::new().i32_const(PAINT_LEN),
            byte_at(RAND_BUF, I)
                .i32_const(PAINT)
                .op(op::I32_EQ)
                .local_get(N)
                .op(op::I32_ADD)
                .local_set(N),
        ))
        .then(say(wasi::STDOUT, Expr::new().local_get(N)));
    command("random-zero-length", body)
}

/// A command importing a name no version of preview1 has ever defined, alongside one that
/// exists. Its `_start` would print a line, which is how the test knows it never ran.
fn missing_preview1_name() -> Module {
    let (mut b, _) = wasi::wasi_command("missing-preview1-name", 1, &[W::FdWrite, W::Missing]);
    let ty = b.add_type(ftype(&[], &[]));
    let start = b.add_func(
        ty,
        Func::new(
            wasi::puts(wasi::STDOUT, 0, 14, FD_WRITE)
                .i32_const(0)
                .i32_const(0)
                .call(1)
                .drop(),
        ),
    );
    b.export_func("_start", start)
        .data_active(0, b"should not run")
        .build()
}

/// The same, but the import module itself is one the host has never heard of.
fn unknown_import_module() -> Module {
    let (mut b, _) = wasi::wasi_command("unknown-import-module", 1, &[W::FdWrite]);
    let ty = b.add_type(ftype(&[ValType::I32], &[ValType::I32]));
    b = b.import(
        "env",
        "definitely_not_a_host_function",
        ImportKind::Func(ty),
    );
    let void = b.add_type(ftype(&[], &[]));
    let start = b.add_func(
        void,
        Func::new(
            wasi::puts(wasi::STDOUT, 0, 14, FD_WRITE)
                .i32_const(0)
                .call(1)
                .drop(),
        ),
    );
    b.export_func("_start", start)
        .data_active(0, b"should not run")
        .build()
}

// ---------------------------------------------------------------------------------------
// Reading what a module printed
// ---------------------------------------------------------------------------------------

/// Every line of `text` that is a bare unsigned number, in order. The runtime's own warnings
/// are not numbers, so they fall out on their own.
fn numbers(text: &str) -> Vec<u32> {
    text.lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .collect()
}

/// The `i`-th such number, or `None`.
fn nth(text: &str, i: usize) -> Option<u32> {
    numbers(text).get(i).copied()
}

/// The most specific thing the runtime said about a failed instantiation.
///
/// `wasmtime` writes a headline naming the module file and then a `Caused by:` chain, the
/// last link of which is the one worth quoting: `unknown import: ... has not been defined`.
/// Nothing is asserted about any of it — this only decides which line goes into the note.
fn deepest_complaint(stderr: &str) -> String {
    stderr
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| first_meaningful_line(stderr))
}

// ---------------------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------------------

/// What the echo test feeds in on standard input.
const INPUT: &[u8] = b"the quick brown fox\n";

wasm_test!(read_stdin, |ctx| {
    let m = read_echo("fd-read-stdin", wasi::STDIN);
    let run = ctx.start_with_stdin(&m, &[], INPUT)?;
    let errno = nth(&run.stderr, 0);
    let nread = nth(&run.stderr, 1);
    let echoed = run.raw_stdout.clone();
    let mut c = Check::new("fd_read from stdin, echoed back to stdout", &run);
    c.module(&m);
    c.eq("errno", Some(0u32), errno);
    c.that(
        "stdout",
        "a non-empty prefix of what was fed in — the bytes arrive in order and unchanged",
        !echoed.is_empty() && INPUT.starts_with(&echoed),
        String::from_utf8_lossy(&echoed).to_string(),
    );
    c.that(
        "*nread",
        "at least what reached stdout and at most the whole input",
        nread.is_some_and(|n| n as usize >= echoed.len() && n as usize <= INPUT.len()),
        format!("{nread:?} for {} byte(s) echoed", echoed.len()),
    );
    c.note(
        "fd_read may hand over fewer bytes than the iovec has room for, and the fd_write \
         that echoes them may in turn be short, so what is asserted is the prefix and the \
         counts either side of it, not the whole line in one go",
    );
    c.finish()?;
    ctx.note(format!(
        "fd_read reported {} of {} byte(s); {} reached stdout",
        nread.unwrap_or(0),
        INPUT.len(),
        echoed.len()
    ));
    Ok(())
});

wasm_test!(read_at_end_of_input, |ctx| {
    // No standard input at all: the harness gives the runtime /dev/null, so the very first
    // read is already at the end. That is not an error — it is how a reader knows to stop.
    let m = read_echo("fd-read-eof", wasi::STDIN);
    let run = ctx.start(&m, &[])?;
    let mut c = Check::new("fd_read with nothing left to read", &run);
    c.module(&m);
    c.eq("errno", Some(0u32), nth(&run.stderr, 0));
    c.eq("*nread", Some(0u32), nth(&run.stderr, 1));
    c.eq("stdout", "", run.stdout.as_str());
    c.that(
        "exit",
        "exit status 0: end of input is not a failure",
        run.exit.success(),
        run.exit.label(),
    );
    c.finish()
});

wasm_test!(read_bad_fd, |ctx| {
    // fd 9 was never opened. EBADF is 8 in preview1, but only "not 0" is asserted and the
    // value is recorded, exactly as stage 40 does for fd_write.
    let m = read_echo("fd-read-bad-fd", 9);
    let run = ctx.start(&m, &[])?;
    let errno = nth(&run.stderr, 0);
    let mut c = Check::new("fd_read on a descriptor nobody opened", &run);
    c.module(&m);
    c.that(
        "errno",
        "a non-zero errno (8 EBADF in preview1)",
        errno.is_some_and(|e| e != 0),
        format!("{errno:?}"),
    );
    c.eq("stdout", "", run.stdout.as_str());
    c.finish()?;
    ctx.note(format!(
        "fd_read(9, ..) answered errno {}",
        errno.unwrap_or(0)
    ));
    Ok(())
});

wasm_test!(monotonic_clock, |ctx| {
    let m = monotonic_pair();
    let run = ctx.start(&m, &[])?;
    let lines = run.stdout.clone();
    let gap = nth(&lines, 4);
    let mut c = Check::new("two readings of the monotonic clock in one program", &run);
    c.module(&m);
    c.eq("errno of the first reading", Some(0u32), nth(&lines, 0));
    c.eq("errno of the second reading", Some(0u32), nth(&lines, 1));
    c.eq(
        "the second reading is not earlier than the first",
        Some(1u32),
        nth(&lines, 2),
    );
    c.eq(
        "the gap between two adjacent calls is under two seconds",
        Some(1u32),
        nth(&lines, 3),
    );
    c.note(
        "the epoch and the resolution of the monotonic clock are the host's own, so a clock \
         coarse enough to answer the same value twice passes: what is asserted is only that \
         time did not run backwards between two calls a few instructions apart",
    );
    c.finish()?;
    ctx.note(format!(
        "the monotonic clock advanced {} ns between two adjacent calls",
        gap.unwrap_or(0)
    ));
    Ok(())
});

wasm_test!(realtime_clock, |ctx| {
    // The realtime clock's epoch is fixed by the spec, so the reading can be checked for
    // plausibility: some time after 2001 and before 2106.
    let m = realtime_reading();
    let run = ctx.start(&m, &[])?;
    let lines = run.stdout.clone();
    let seconds = nth(&lines, 2);
    let mut c = Check::new("one reading of the realtime clock", &run);
    c.module(&m);
    c.eq("errno", Some(0u32), nth(&lines, 0));
    c.eq("the reading is not zero", Some(1u32), nth(&lines, 1));
    c.that(
        "seconds since the Unix epoch",
        "somewhere between 2001 and 2106 — the realtime clock counts nanoseconds from 1970",
        seconds.is_some_and(|s| (1_000_000_000..4_000_000_000).contains(&s)),
        format!("{seconds:?}"),
    );
    c.finish()?;
    ctx.note(format!(
        "the realtime clock answered {} seconds since the Unix epoch",
        seconds.unwrap_or(0)
    ));
    Ok(())
});

wasm_test!(random_fills_a_buffer, |ctx| {
    let m = random_survey();
    let run = ctx.start(&m, &[])?;
    let lines = run.stdout.clone();
    let non_zero = nth(&lines, 1);
    let distinct = nth(&lines, 2);
    let mut c = Check::new("sixty-four bytes out of random_get", &run);
    c.module(&m);
    c.eq("errno", Some(0u32), nth(&lines, 0));
    c.that(
        "non-zero bytes",
        "at least 48 of 64 — a buffer left untouched has none",
        non_zero.is_some_and(|n| n >= 48),
        format!("{non_zero:?}"),
    );
    c.that(
        "distinct values",
        "at least 16 of the 64 bytes differ from each other — one repeated byte gives 1",
        distinct.is_some_and(|n| n >= 16),
        format!("{distinct:?}"),
    );
    c.note(
        "this is a test of whether the bytes were written at all, not of how random they \
         are: 64 uniform bytes give about 63.75 non-zero and about 57 distinct values, so \
         the bounds are far enough away that only a stub can fail them",
    );
    c.finish()?;
    ctx.note(format!(
        "random_get filled 64 bytes: {} non-zero, {} distinct values",
        non_zero.unwrap_or(0),
        distinct.unwrap_or(0)
    ));
    Ok(())
});

wasm_test!(random_of_length_zero, |ctx| {
    // Eight bytes painted 0x5a, then random_get(buf, 0). Asking for no bytes is a legal
    // request, and the answer to it is to write nothing at all.
    let m = random_zero_length();
    let run = ctx.start(&m, &[])?;
    let lines = run.stdout.clone();
    let mut c = Check::new("random_get with a length of zero", &run);
    c.module(&m);
    c.eq("errno", Some(0u32), nth(&lines, 0));
    c.eq(
        "painted bytes still holding 0x5a",
        Some(PAINT_LEN as u32),
        nth(&lines, 1),
    );
    c.that(
        "exit",
        "exit status 0: asking for no bytes is not an error",
        run.exit.success(),
        run.exit.label(),
    );
    c.finish()
});

wasm_test!(missing_imports, |ctx| {
    // Two ways of asking for something the host does not have: a name that is not in
    // preview1, and a whole import module that does not exist. Both fail the same way, and
    // they fail at instantiation — before `_start`, which is why neither module gets to
    // print the line it holds in its data segment.
    let unknown_name = missing_preview1_name();
    let a = expect_start_rejected(
        ctx,
        &unknown_name,
        "wasi_snapshot_preview1 has no function called \
         definitely_not_a_preview1_function, so the module cannot be instantiated",
    )?;
    let unknown_module = unknown_import_module();
    let b = expect_start_rejected(
        ctx,
        &unknown_module,
        "there is no import module called 'env' when a WASI command is run, so the module \
         cannot be instantiated",
    )?;
    ctx.note(format!(
        "an unknown preview1 name: {}",
        deepest_complaint(&a.stderr)
    ));
    ctx.note(format!(
        "an unknown import module: {}",
        deepest_complaint(&b.stderr)
    ));
    Ok(())
});

/// Worked examples: 1-3, each a real module this stage builds.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("Read standard input, echo it back", || {
            read_echo("fd-read-stdin", wasi::STDIN)
        })
        .summary(
            "builds one iovec at 0x100 pointing at a 256-byte buffer, calls fd_read(0, \
             0x100, 1, 0x420), then hands the first *nread bytes of that buffer straight to \
             fd_write(1, ..) and reports the errno and the count on stderr",
        )
        .command("echo 'the quick brown fox' | run mod.wasm")
        .output(
            "the quick brown fox   (stdout)\n0\n20                    (stderr: errno, then *nread)",
        )
        .note(
            "fd_read is fd_write backwards, and it is short in the same way: *nread is how \
             many bytes actually arrived, and end of input is *nread 0 with errno 0 rather \
             than an error. The count goes to 0x420 and not to the usual 0x200 because the \
             echoing fd_write would overwrite it.",
        ),
        ExampleSpec::module("Sixty-four random bytes, surveyed", random_survey)
            .summary(
                "calls random_get(0x700, 64), then counts in WebAssembly how many of those \
                 bytes are non-zero and how many distinct values they hold",
            )
            .command("run mod.wasm")
            .output("0\n64\n57")
            .note(
                "Real randomness gives about 63.75 non-zero bytes and about 57 distinct \
                 values; a stub that returns zeroes gives 0 and 1, and one that returns a \
                 repeated byte gives 64 and 1. The test's bounds — 48 and 16 — sit in the \
                 gap, and claim nothing whatever about the quality of the randomness.",
            ),
        ExampleSpec::module("An import the host does not have", missing_preview1_name)
            .summary(
                "a WASI command importing wasi_snapshot_preview1::fd_write, which exists, \
                 and wasi_snapshot_preview1::definitely_not_a_preview1_function, which does \
                 not",
            )
            .command("run mod.wasm")
            .output("(nothing on stdout; exit status 1, and 'unknown import' on stderr)")
            .note(
                "The module decodes and validates perfectly well — nothing about it is \
                 malformed. It fails at instantiation, when the host is asked for each \
                 import by name, which is before `_start` runs: that is why not one byte of \
                 the line in its data segment ever reaches stdout.",
            ),
    ]
}
