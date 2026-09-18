//! Stage 40 — WASI preview1: `fd_write` to stdout and stderr. **[ext]**
//!
//! One thing here is looser than it looks. `fd_write` is allowed to write **fewer** bytes
//! than the iovecs describe and report that count in `nwritten` — exactly like POSIX
//! `writev` — and `wasmtime` really does it: given two iovecs it writes the first and
//! answers with its length. So the multi-iovec tests assert what the spec actually promises
//! (the bytes that come out are a prefix of the concatenation, and `nwritten` is how many
//! there were), not "everything you asked for was written". A runtime that writes the lot in
//! one go passes the same tests.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::stages::wasi::{self, W};
use crate::stages::{expect_stdout, Stage, Test};
use crate::wasm::{ftype, Expr, Func, Module};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 40,
        slug: "wasi_fd_write",
        name: "WASI: fd_write to stdout and stderr",
        ext: true,
        hints: &[
            "Import wasi_snapshot_preview1::fd_write with the signature (i32 i32 i32 i32) -> i32, and export your memory as `memory`: the host reads the buffers out of it",
            "The third argument counts iovecs, not bytes; each one is {buf: i32, len: i32}, eight bytes, and they go into the stream in order",
            "Store the number of bytes you really wrote at the fourth argument and return errno 0 — writing fewer than asked is allowed, and the caller is expected to loop",
            "stdout is fd 1, stderr is fd 2, and a descriptor nobody opened is errno 8 (EBADF) with nothing written",
        ],
        examples,
        tests: vec![
            Test::new("a string reaches stdout", stdout_one).ext().tag("wasi"),
            Test::new("stderr is a different stream from stdout", stderr_is_separate)
                .ext()
                .tag("wasi"),
            Test::new("two iovecs are consumed in order", two_iovecs)
                .ext()
                .tag("wasi"),
            Test::new("an iovec of length zero writes nothing and is not an error", empty_iovec)
                .ext()
                .tag("wasi"),
            Test::new("three fd_write calls append to one stream", three_calls)
                .ext()
                .tag("wasi"),
            Test::new("fd_write reports the byte count it wrote", nwritten)
                .ext()
                .tag("wasi"),
            Test::new("fd_write returns errno 0 when it succeeds", errno_zero)
                .ext()
                .tag("wasi"),
            Test::new("a descriptor nobody opened is a non-zero errno", bad_fd)
                .ext()
                .tag("wasi"),
        ],
    }
}

/// The message every module in this stage keeps at address 0. No newline: stdout is compared
/// byte for byte, and a trailing newline would hide a short write.
const MSG: &[u8] = b"hello wasi";

/// A WASI command whose `_start` is `body`, with `data` in memory at address 0.
fn command(label: &str, data: &[u8], body: Expr) -> Module {
    let (mut b, _) = wasi::wasi_command(label, 1, &[W::FdWrite]);
    let ty = b.add_type(ftype(&[], &[]));
    let start = b.add_func(ty, Func::with_locals(wasi::PRINT_I32_LOCALS, body));
    b.export_func("_start", start).data_active(0, data).build()
}

/// Build the iovecs for `pieces`, call `fd_write` on `fd`, and print `nwritten` on stderr.
fn write_and_report(fd: i32, pieces: &[(i32, i32)]) -> Expr {
    wasi::puts_many(fd, pieces, 0).then(wasi::print_i32(
        wasi::STDERR,
        Expr::new().i32_const(wasi::NWRITTEN).i32_load(0),
        wasi::SCRATCH,
        0,
    ))
}

/// The number the module printed on stderr, ignoring the runtime's own warnings.
fn reported_count(stderr: &str) -> Option<u32> {
    stderr
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .next_back()
}

wasm_test!(stdout_one, |ctx| {
    let m = command(
        "fd-write-stdout",
        MSG,
        wasi::puts(wasi::STDOUT, 0, MSG.len() as i32, 0),
    );
    expect_stdout(ctx, &m, &[], "hello wasi")?;
    Ok(())
});

wasm_test!(stderr_is_separate, |ctx| {
    let m = command(
        "fd-write-stderr",
        MSG,
        wasi::puts(wasi::STDERR, 0, MSG.len() as i32, 0),
    );
    let run = ctx.start(&m, &[])?;
    let mut c = Check::new("fd 2 going to stderr and nowhere else", &run);
    c.module(&m);
    c.eq("stdout", "", run.stdout.as_str());
    c.that(
        "stderr",
        "the message, because fd 2 is stderr",
        run.stderr.contains("hello wasi"),
        run.stderr.clone(),
    );
    c.finish()
});

wasm_test!(two_iovecs, |ctx| {
    // "hello " and "wasi" as two iovecs in one call. Whatever comes out must be a prefix of
    // the concatenation, and nwritten must say exactly how much came out.
    let m = command(
        "fd-write-two-iovecs",
        MSG,
        write_and_report(wasi::STDOUT, &[(0, 6), (6, 4)]),
    );
    let run = ctx.start(&m, &[])?;
    let count = reported_count(&run.stderr);
    let mut c = Check::new("two iovecs written with one fd_write", &run);
    c.module(&m);
    c.that(
        "stdout",
        "a non-empty prefix of \"hello wasi\" — the iovecs are consumed in order",
        !run.stdout.is_empty() && "hello wasi".starts_with(run.stdout.as_str()),
        run.stdout.clone(),
    );
    c.that(
        "*nwritten",
        "the number of bytes that really came out",
        count == Some(run.stdout.len() as u32),
        format!("{count:?} for {} bytes on stdout", run.stdout.len()),
    );
    c.note(
        "fd_write may write fewer bytes than the iovecs describe and report that in \
         nwritten, exactly like writev; what it must never do is write them out of order \
         or write bytes nobody asked for",
    );
    c.finish()?;
    ctx.note(format!(
        "fd_write reported {} byte(s) for two iovecs totalling 10",
        count.unwrap_or(0)
    ));
    Ok(())
});

wasm_test!(empty_iovec, |ctx| {
    // One iovec of length zero: errno 0, nwritten 0, and not a byte on stdout.
    let m = command(
        "fd-write-empty-iovec",
        MSG,
        write_and_report(wasi::STDOUT, &[(0, 0)]),
    );
    let run = ctx.start(&m, &[])?;
    let mut c = Check::new("an fd_write of zero bytes", &run);
    c.module(&m);
    c.eq("stdout", "", run.stdout.as_str());
    c.eq("*nwritten", Some(0u32), reported_count(&run.stderr));
    c.that(
        "exit",
        "exit status 0: writing nothing is not a failure",
        run.exit.success(),
        run.exit.label(),
    );
    c.finish()
});

wasm_test!(three_calls, |ctx| {
    let body = wasi::puts(wasi::STDOUT, 0, 6, 0)
        .then(wasi::puts(wasi::STDOUT, 6, 3, 0))
        .then(wasi::puts(wasi::STDOUT, 9, 1, 0));
    let m = command("fd-write-three-calls", MSG, body);
    expect_stdout(ctx, &m, &[], "hello wasi")?;
    Ok(())
});

wasm_test!(nwritten, |ctx| {
    // One iovec of ten bytes: a single buffer that small is never written short.
    let m = command(
        "fd-write-nwritten",
        MSG,
        write_and_report(wasi::STDOUT, &[(0, MSG.len() as i32)]),
    );
    let run = ctx.start(&m, &[])?;
    let mut c = Check::new("the byte count fd_write stores", &run);
    c.module(&m);
    c.eq("stdout", "hello wasi", run.stdout.as_str());
    c.eq("*nwritten", Some(10u32), reported_count(&run.stderr));
    c.finish()
});

wasm_test!(errno_zero, |ctx| {
    // fd_write leaves its errno on the stack; print it on stderr instead of dropping it.
    let body = Expr::new()
        .i32_const(wasi::IOV)
        .i32_const(0)
        .i32_store(0)
        .i32_const(wasi::IOV + 4)
        .i32_const(MSG.len() as i32)
        .i32_store(0)
        .then(wasi::print_i32(
            wasi::STDERR,
            Expr::new()
                .i32_const(wasi::STDOUT)
                .i32_const(wasi::IOV)
                .i32_const(1)
                .i32_const(wasi::NWRITTEN)
                .call(0),
            wasi::SCRATCH,
            0,
        ));
    let m = command("fd-write-errno", MSG, body);
    let run = ctx.start(&m, &[])?;
    let mut c = Check::new("the errno of a successful fd_write", &run);
    c.module(&m);
    c.eq("stdout", "hello wasi", run.stdout.as_str());
    c.eq("errno", Some(0u32), reported_count(&run.stderr));
    c.finish()
});

wasm_test!(bad_fd, |ctx| {
    // fd 9 was never opened. The errno is EBADF (8) in preview1, but a runtime may pick
    // another non-zero value, so only "not 0" is asserted and the value is recorded.
    let body = Expr::new()
        .i32_const(wasi::IOV)
        .i32_const(0)
        .i32_store(0)
        .i32_const(wasi::IOV + 4)
        .i32_const(MSG.len() as i32)
        .i32_store(0)
        .then(wasi::print_i32(
            wasi::STDERR,
            Expr::new()
                .i32_const(9)
                .i32_const(wasi::IOV)
                .i32_const(1)
                .i32_const(wasi::NWRITTEN)
                .call(0),
            wasi::SCRATCH,
            0,
        ));
    let m = command("fd-write-bad-fd", MSG, body);
    let run = ctx.start(&m, &[])?;
    let errno = reported_count(&run.stderr);
    let mut c = Check::new("fd_write on a descriptor nobody opened", &run);
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
        "fd_write(9, ..) answered errno {}",
        errno.unwrap_or(0)
    ));
    Ok(())
});

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::module("Hello, through one iovec", || {
            command(
                "fd-write-stdout",
                MSG,
                wasi::puts(wasi::STDOUT, 0, MSG.len() as i32, 0),
            )
        })
        .summary(
            "imports `wasi_snapshot_preview1::fd_write`, exports its memory, puts \
             \"hello wasi\" at address 0 with a data segment, builds one iovec at 0x100 and \
             calls fd_write(1, 0x100, 1, 0x200)",
        )
        .command("run mod.wasm")
        .output("hello wasi")
        .note(
            "The module exports `memory` because the host has to read the buffer out of it; \
             a WASI module that forgets is refused before `_start` runs. An iovec is two \
             i32s — a pointer and a length — not the bytes themselves.",
        ),
        ExampleSpec::module("Two iovecs, one call, and a byte count", || {
            command(
                "fd-write-two-iovecs",
                MSG,
                write_and_report(wasi::STDOUT, &[(0, 6), (6, 4)]),
            )
        })
        .summary(
            "the same message split across two iovecs at 0x100 and 0x108, written with one \
             fd_write(1, 0x100, 2, 0x200), after which the module prints the byte count the \
             host stored at 0x200 — on stderr, so stdout stays exactly what was written",
        )
        .command("run mod.wasm")
        .output("hello wasi   (stdout)\n10           (stderr: the byte count)")
        .note(
            "The third argument counts iovecs, not bytes. fd_write is allowed to write fewer \
             bytes than it was given and say so in *nwritten — wasmtime writes the first \
             iovec and answers 6 — so a caller that does not check the count loses data.",
        ),
    ]
}
