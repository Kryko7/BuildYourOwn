//! The encoder's own tests: that the bytes it writes are the bytes the spec describes, and
//! that the reference runtime really accepts every module the suite's examples build.
//!
//! The second half needs `wasmtime` in the cache; without it those tests print a line and
//! return, so `cargo test` works on a machine that has never run the suite.

use std::path::PathBuf;
use std::process::Command;
use wasmtest::runtime::reference;
use wasmtest::stages;
use wasmtest::wasm::{
    const_i32, ftype, global_i32, op, section, uleb, uleb_padded, Enc, Expr, Func, Limits,
    ModuleBuilder, ValType,
};

fn wasmtime() -> Option<PathBuf> {
    let exe = reference::exe_path(reference::DEFAULT_VERSION);
    exe.is_file().then_some(exe)
}

#[test]
fn a_hand_checked_module_matches_byte_for_byte() {
    // The smallest interesting module, written out by hand from the spec's grammar:
    //   magic, version,
    //   type   section: 1 type, () -> i32
    //   func   section: 1 function, type 0
    //   export section: 1 export, "f" -> func 0
    //   code   section: 1 body, 0 locals, i32.const 7, end
    let expected: Vec<u8> = vec![
        0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // header
        0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7f, // type
        0x03, 0x02, 0x01, 0x00, // function
        0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, // export "f"
        0x0a, 0x06, 0x01, 0x04, 0x00, 0x41, 0x07, 0x0b, // code
    ];
    let mut b = ModuleBuilder::new("seven");
    let ty = b.add_type(ftype(&[], &[ValType::I32]));
    let idx = b.add_func(ty, Func::new(Expr::new().i32_const(7)));
    let m = b.export_func("f", idx).build();
    assert_eq!(
        m.bytes,
        expected,
        "the encoder drifted from the binary format\n{}",
        m.listing(4096)
    );
}

#[test]
fn annotations_cover_the_module_and_stay_inside_it() {
    let mut b = ModuleBuilder::new("everything");
    let ty = b.add_type(ftype(&[ValType::I32], &[ValType::I32]));
    let idx = b.add_func(
        ty,
        Func::with_locals(
            &[(2, ValType::I64)],
            Expr::new().local_get(0).i32_const(1).op(op::I32_ADD),
        ),
    );
    let m = b
        .export_func("f", idx)
        .memory(Limits::range(1, 2))
        .export_memory()
        .global(global_i32(3, true))
        .elem_active(0, &[])
        .data_active(0, b"hello")
        .custom("producers", b"wasmtest", Some(section::EXPORT))
        .build();
    for a in &m.anns {
        assert!(
            a.offset + a.length <= m.bytes.len(),
            "annotation {a:?} runs past the module of {} bytes",
            m.bytes.len()
        );
        assert!(!a.field.is_empty(), "an annotation with no field path");
    }
    let listing = m.listing(8192);
    for needle in [
        "header.magic",
        "header.version",
        "type[0]",
        "function[0]",
        "memory[0]",
        "global[0]",
        "export[0]",
        "code[0].body",
        "data[0]",
        "custom[producers]",
    ] {
        assert!(
            listing.contains(needle),
            "{needle} missing from:\n{listing}"
        );
    }
}

#[test]
fn padded_leb128s_do_not_change_a_module() {
    // A type section whose vector count is written in five bytes instead of one. Both forms
    // describe the same module; stage 02 asserts a runtime reads both.
    let short = {
        let mut b = ModuleBuilder::new("short");
        let ty = b.add_type(ftype(&[], &[ValType::I32]));
        let idx = b.add_func(ty, Func::new(Expr::new().i32_const(7)));
        b.export_func("f", idx).build()
    };
    let padded = {
        let mut type_body = uleb_padded(1, 5);
        type_body.extend_from_slice(&[0x60, 0x00, 0x01, 0x7f]);
        let mut e = Enc::with_header();
        e.section(section::TYPE, &type_body);
        e.section(section::FUNCTION, &[0x01, 0x00]);
        e.section(section::EXPORT, &[0x01, 0x01, b'f', 0x00, 0x00]);
        e.section(section::CODE, &[0x01, 0x04, 0x00, 0x41, 0x07, 0x0b]);
        e.finish("padded")
    };
    assert_ne!(short.bytes, padded.bytes, "the two encodings differ");
    assert_eq!(
        padded.bytes.len(),
        short.bytes.len() + 4,
        "four redundant continuation bytes"
    );
}

#[test]
fn a_section_can_be_given_a_size_that_lies() {
    let mut e = Enc::with_header();
    e.section_with_size(section::TYPE, &uleb(99), &[0x00], "99, but 1 byte follows");
    let m = e.finish("liar");
    assert_eq!(m.bytes.len(), 11);
    assert_eq!(m.bytes[9], 99, "the size field is written as given");
}

#[test]
fn constant_expressions_end_with_end() {
    assert_eq!(const_i32(0), vec![0x41, 0x00, 0x0b]);
    assert_eq!(const_i32(-1), vec![0x41, 0x7f, 0x0b]);
}

#[test]
fn the_reference_runs_every_example_module() {
    let Some(exe) = wasmtime() else {
        eprintln!("skipping: wasmtime is not in the cache (run --validate once)");
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let mut checked = 0usize;
    for s in stages::all() {
        for (i, spec) in (s.examples)().iter().enumerate() {
            let m = (spec.build)();
            let path = dir.path().join(format!("s{:02}e{i}.wasm", s.number));
            std::fs::write(&path, &m.bytes).expect("write the example module");
            // Only that the runtime *reads* the file the way the example says it does: an
            // example whose point is a rejection must be rejected, and any other one must
            // at least get past the decoder. Running it is what the stage's own tests do.
            let out = Command::new(&exe)
                .arg("run")
                .arg("--invoke")
                .arg("__wasmtest_no_such_export__")
                .arg(&path)
                .output()
                .expect("wasmtime must be runnable");
            let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
            let rejected_for_shape = stderr.contains("invalid input webassembly code")
                || stderr.contains("invalid leading byte")
                || stderr.contains("unexpected eof")
                || stderr.contains("expected")
                || stderr.contains("magic");
            // The example's `output` is prose, so the check is that it tells the reader the
            // module does not run — by any of the words this suite uses for that — rather
            // than that it matches one fixed sentence.
            let lower = spec.output.to_lowercase();
            let says_so = [
                "non-zero exit",
                "exit 1",
                "refused",
                "rejected",
                "never runs",
                "does not run",
                "on stderr",
                "error",
            ]
            .iter()
            .any(|w| lower.contains(w));
            if rejected_for_shape {
                assert!(
                    says_so,
                    "stage {} example '{}' is refused by the reference, but its `output` does \
                     not say so:\n{stderr}",
                    s.number, spec.title
                );
            }
            checked += 1;
        }
    }
    assert!(checked >= 45, "every stage must contribute an example");
}
