//! WASI preview1: the imports, their signatures, and the small amount of glue every WASI
//! module in this suite needs.
//!
//! `wasi_snapshot_preview1` is a plain import module: every call is a function whose
//! arguments are `i32`s into the module's **exported** linear memory, and whose result is an
//! `errno` (`0` on success). A runtime that does not export `memory` cannot be given a WASI
//! module at all — `wasmtime` answers `missing required memory export` — so
//! [`wasi_command`] always exports it.
//!
//! The memory map every module here uses, so a failing hex listing is readable:
//!
//! | address | what |
//! |---|---|
//! | `0x0000` | the module's own data (strings, buffers declared by data segments) |
//! | `0x0100` | one `iovec` (`{buf: i32, len: i32}`), built at run time by [`puts`] |
//! | `0x0200` | the `nwritten` / `nread` out-parameter |
//! | `0x0400` | scratch: `args_sizes_get`, `environ_sizes_get`, clocks, randomness |

use crate::wasm::{ftype, Expr, FuncType, ImportKind, Limits, ModuleBuilder, ValType};

/// The import module name every preview1 function lives in.
pub const MODULE: &str = "wasi_snapshot_preview1";

/// Where [`puts`] builds its `iovec`.
pub const IOV: i32 = 0x0100;
/// Where [`puts`] asks the runtime to write the byte count.
pub const NWRITTEN: i32 = 0x0200;
/// Scratch space for the out-parameters of the `*_sizes_get` calls and friends.
pub const SCRATCH: i32 = 0x0400;

/// File descriptor 0.
pub const STDIN: i32 = 0;
/// File descriptor 1.
pub const STDOUT: i32 = 1;
/// File descriptor 2.
pub const STDERR: i32 = 2;

/// The preview1 functions this suite imports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum W {
    /// `fd_write(fd, iovs, iovs_len, nwritten) -> errno`.
    FdWrite,
    /// `fd_read(fd, iovs, iovs_len, nread) -> errno`.
    FdRead,
    /// `fd_close(fd) -> errno`.
    FdClose,
    /// `args_get(argv, argv_buf) -> errno`.
    ArgsGet,
    /// `args_sizes_get(argc_out, argv_buf_size_out) -> errno`.
    ArgsSizesGet,
    /// `environ_get(environ, environ_buf) -> errno`.
    EnvironGet,
    /// `environ_sizes_get(count_out, buf_size_out) -> errno`.
    EnvironSizesGet,
    /// `proc_exit(code)` — never returns.
    ProcExit,
    /// `clock_time_get(id, precision: i64, time_out) -> errno`.
    ClockTimeGet,
    /// `random_get(buf, len) -> errno`.
    RandomGet,
    /// A name no version of preview1 defines, for the missing-import test.
    Missing,
}

impl W {
    /// The name inside `wasi_snapshot_preview1`.
    pub fn name(self) -> &'static str {
        match self {
            W::FdWrite => "fd_write",
            W::FdRead => "fd_read",
            W::FdClose => "fd_close",
            W::ArgsGet => "args_get",
            W::ArgsSizesGet => "args_sizes_get",
            W::EnvironGet => "environ_get",
            W::EnvironSizesGet => "environ_sizes_get",
            W::ProcExit => "proc_exit",
            W::ClockTimeGet => "clock_time_get",
            W::RandomGet => "random_get",
            W::Missing => "definitely_not_a_preview1_function",
        }
    }

    /// Its signature, exactly as the preview1 witx says.
    pub fn sig(self) -> FuncType {
        use ValType::{I32, I64};
        match self {
            W::FdWrite | W::FdRead => ftype(&[I32, I32, I32, I32], &[I32]),
            W::FdClose => ftype(&[I32], &[I32]),
            W::ArgsGet
            | W::ArgsSizesGet
            | W::EnvironGet
            | W::EnvironSizesGet
            | W::RandomGet
            | W::Missing => ftype(&[I32, I32], &[I32]),
            W::ProcExit => ftype(&[I32], &[]),
            W::ClockTimeGet => ftype(&[I32, I64, I32], &[I32]),
        }
    }
}

/// A module that imports the named WASI functions, has `pages` pages of memory and exports
/// it as `memory`.
///
/// Returns the builder and the function index of each import, in the order asked for.
/// Imported functions come **before** defined ones in the function index space, so the
/// first function this module defines has index `fns.len()`.
pub fn wasi_command(label: &str, pages: u32, fns: &[W]) -> (ModuleBuilder, Vec<u32>) {
    let mut b = ModuleBuilder::new(label);
    let mut indices = Vec::new();
    for (i, f) in fns.iter().enumerate() {
        let ty = b.add_type(f.sig());
        b = b.import(MODULE, f.name(), ImportKind::Func(ty));
        indices.push(i as u32);
    }
    b = b.memory(Limits::min(pages)).export_memory();
    (b, indices)
}

/// Instructions that write the `len` bytes at `ptr` to `fd` through one `iovec`, and throw
/// the errno away.
pub fn puts(fd: i32, ptr: i32, len: i32, fd_write: u32) -> Expr {
    Expr::new()
        .i32_const(IOV)
        .i32_const(ptr)
        .i32_store(0)
        .i32_const(IOV + 4)
        .i32_const(len)
        .i32_store(0)
        .i32_const(fd)
        .i32_const(IOV)
        .i32_const(1)
        .i32_const(NWRITTEN)
        .call(fd_write)
        .drop()
}

/// Instructions that write `n` iovecs, built from `(ptr, len)` pairs starting at [`IOV`], to
/// `fd` in one `fd_write` call — the thing a runtime that only ever handles one iovec gets
/// wrong.
pub fn puts_many(fd: i32, pieces: &[(i32, i32)], fd_write: u32) -> Expr {
    let mut e = Expr::new();
    for (i, (ptr, len)) in pieces.iter().enumerate() {
        let at = IOV + (i as i32) * 8;
        e = e
            .i32_const(at)
            .i32_const(*ptr)
            .i32_store(0)
            .i32_const(at + 4)
            .i32_const(*len)
            .i32_store(0);
    }
    e.i32_const(fd)
        .i32_const(IOV)
        .i32_const(pieces.len() as i32)
        .i32_const(NWRITTEN)
        .call(fd_write)
        .drop()
}

/// Instructions that print one decimal number followed by a newline on `fd`.
///
/// Written in WebAssembly rather than in the harness because the point of several stages is
/// that a WASI module can do its own work: the digits are produced by repeated division into
/// a buffer at `buf`, then handed to `fd_write` in one go. Only values that fit in 15 digits
/// are printed, and the number is treated as unsigned.
///
/// The function needs [`PRINT_I32_LOCALS`] declared, and it uses locals 0 and 1.
pub fn print_i32(fd: i32, value: Expr, buf: i32, fd_write: u32) -> Expr {
    // locals: 0 = the value, 1 = the write cursor.
    // The buffer is filled from the right; `buf + 15` holds the newline.
    Expr::new()
        .then(value)
        .local_set(0)
        .i32_const(buf + 15)
        .i32_const(10)
        .bytes(&[0x3a, 0x00, 0x00], "i32.store8 align=1 offset=0")
        .i32_const(buf + 15)
        .local_set(1)
        .block(
            crate::wasm::BlockType::Empty,
            Expr::new().loop_(
                crate::wasm::BlockType::Empty,
                Expr::new()
                    // cursor -= 1
                    .local_get(1)
                    .i32_const(1)
                    .op(crate::wasm::op::I32_SUB)
                    .local_set(1)
                    // *cursor = '0' + value % 10
                    .local_get(1)
                    .i32_const(48)
                    .local_get(0)
                    .i32_const(10)
                    .op(crate::wasm::op::I32_REM_U)
                    .op(crate::wasm::op::I32_ADD)
                    .bytes(&[0x3a, 0x00, 0x00], "i32.store8 align=1 offset=0")
                    // value /= 10
                    .local_get(0)
                    .i32_const(10)
                    .op(crate::wasm::op::I32_DIV_U)
                    .local_set(0)
                    // keep going while value != 0
                    .local_get(0)
                    .op(crate::wasm::op::I32_EQZ)
                    .br_if(1)
                    .br(0),
            ),
        )
        // fd_write(1, {buf: cursor, len: buf + 16 - cursor}, 1, NWRITTEN)
        .i32_const(IOV)
        .local_get(1)
        .i32_store(0)
        .i32_const(IOV + 4)
        .i32_const(buf + 16)
        .local_get(1)
        .op(crate::wasm::op::I32_SUB)
        .i32_store(0)
        .i32_const(fd)
        .i32_const(IOV)
        .i32_const(1)
        .i32_const(NWRITTEN)
        .call(fd_write)
        .drop()
}

/// The locals [`print_i32`] needs.
pub const PRINT_I32_LOCALS: &[(u32, ValType)] = &[(2, ValType::I32)];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_functions_come_first_in_the_index_space() {
        let (b, idx) = wasi_command("t", 1, &[W::FdWrite, W::ProcExit]);
        assert_eq!(idx, vec![0, 1]);
        assert_eq!(b.imported_funcs(), 2);
    }

    #[test]
    fn signatures_match_the_witx() {
        assert_eq!(
            W::FdWrite.sig(),
            ftype(
                &[ValType::I32, ValType::I32, ValType::I32, ValType::I32],
                &[ValType::I32]
            )
        );
        assert_eq!(W::ProcExit.sig(), ftype(&[ValType::I32], &[]));
        assert_eq!(
            W::ClockTimeGet.sig(),
            ftype(&[ValType::I32, ValType::I64, ValType::I32], &[ValType::I32])
        );
    }

    #[test]
    fn puts_builds_one_iovec_then_calls_fd_write() {
        let e = puts(STDOUT, 0, 3, 0);
        assert!(e
            .text()
            .starts_with("i32.const 256, i32.const 0, i32.store"));
        assert!(e.text().ends_with("call 0, drop"), "{}", e.text());
    }

    #[test]
    fn puts_many_writes_one_iovec_per_piece() {
        let e = puts_many(STDERR, &[(0, 2), (2, 1)], 0);
        assert!(e.text().contains("i32.const 264"), "{}", e.text());
        assert!(e.text().contains("i32.const 2, i32.const 256, i32.const 2"));
    }

    #[test]
    fn print_i32_can_be_pointed_at_either_stream() {
        let out = print_i32(STDOUT, Expr::new().i32_const(7), SCRATCH, 0);
        let err = print_i32(STDERR, Expr::new().i32_const(7), SCRATCH, 0);
        assert_ne!(out.as_bytes(), err.as_bytes());
        assert!(out.text().contains("i32.rem_u"), "{}", out.text());
    }
}
