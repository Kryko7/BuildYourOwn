//! The hand-assembled instruction encoders.
//!
//! Two checks. The first pins a known sequence to a literal byte string, which is what makes
//! a mistake in an encoder visible in a diff rather than three stages later as "the program
//! segfaults". The second assembles the same source with the system assembler and compares —
//! it is skipped, with a reason, on a machine that has no `as`.

use linktest::asm::{Code, Reg, STDOUT};
use linktest::elf::{R_X86_64_PC32, R_X86_64_PLT32};
use linktest::exec;
use std::time::Duration;

/// `write(1, message, 6); call other; mov edi, eax; mov eax, 60; syscall`
fn reference_sequence() -> Code {
    let mut c = Code::new();
    c.sys_write(STDOUT, "message", 6);
    c.call("other");
    c.sys_exit_eax();
    c
}

#[test]
fn the_reference_sequence_is_these_exact_bytes() {
    let c = reference_sequence();
    assert_eq!(
        c.bytes,
        vec![
            // write(1, message, 6)
            0xb8, 0x01, 0x00, 0x00, 0x00, //       mov  eax, 1
            0xbf, 0x01, 0x00, 0x00, 0x00, //       mov  edi, 1
            0x48, 0x8d, 0x35, 0x00, 0x00, 0x00, 0x00, // lea rsi, [rip+message]
            0xba, 0x06, 0x00, 0x00, 0x00, //       mov  edx, 6
            0x0f, 0x05, //                         syscall
            // call other
            0xe8, 0x00, 0x00, 0x00, 0x00, //       call other
            // exit(eax)
            0x89, 0xc7, //                         mov  edi, eax
            0xb8, 0x3c, 0x00, 0x00, 0x00, //       mov  eax, 60
            0x0f, 0x05, //                         syscall
        ]
    );
    assert_eq!(c.relocs.len(), 2);
    assert_eq!(
        (c.relocs[0].offset, c.relocs[0].kind, c.relocs[0].addend),
        (13, R_X86_64_PC32, -4)
    );
    assert_eq!(
        (c.relocs[1].offset, c.relocs[1].kind, c.relocs[1].addend),
        (25, R_X86_64_PLT32, -4)
    );
}

#[test]
fn every_rip_relative_encoder_puts_the_relocation_on_the_displacement() {
    // Each of these ends with a four-byte displacement, and the relocation must point at it.
    let cases: Vec<(&str, Code)> = vec![
        ("lea rsi", {
            let mut c = Code::new();
            c.lea_rip(Reg::Rsi, "x", 0);
            c
        }),
        ("mov rax, [rip+x]", {
            let mut c = Code::new();
            c.mov_r64_rip(Reg::Rax, "x", R_X86_64_PC32, -4);
            c
        }),
        ("mov eax, [rip+x]", {
            let mut c = Code::new();
            c.mov_r32_rip(Reg::Rax, "x", 0);
            c
        }),
        ("mov [rip+x], eax", {
            let mut c = Code::new();
            c.mov_rip_r32("x", Reg::Rax, 0);
            c
        }),
        ("add eax, [rip+x]", {
            let mut c = Code::new();
            c.add_r32_rip(Reg::Rax, "x", 0);
            c
        }),
        ("movzx eax, byte [rip+x]", {
            let mut c = Code::new();
            c.movzx_r32_byte_rip(Reg::Rax, "x", 0);
            c
        }),
    ];
    for (what, c) in cases {
        assert_eq!(c.relocs.len(), 1, "{what}");
        let r = &c.relocs[0];
        assert_eq!(
            r.offset + 4,
            c.len(),
            "{what}: the displacement must be the last four bytes"
        );
        assert_eq!(
            &c.bytes[r.offset as usize..],
            &[0, 0, 0, 0],
            "{what}: the displacement is written as zero and patched by the linker"
        );
    }
}

#[test]
fn register_numbers_land_in_the_modrm_byte() {
    for (reg, modrm) in [
        (Reg::Rax, 0x05u8),
        (Reg::Rcx, 0x0d),
        (Reg::Rdx, 0x15),
        (Reg::Rbx, 0x1d),
        (Reg::Rbp, 0x2d),
        (Reg::Rsi, 0x35),
        (Reg::Rdi, 0x3d),
    ] {
        let mut c = Code::new();
        c.lea_rip(reg, "x", 0);
        assert_eq!(
            c.bytes[..3],
            [0x48, 0x8d, modrm],
            "lea {}, [rip+x]",
            reg.name64()
        );
    }
}

/// The optional cross-check: `as` assembles the same source and must agree byte for byte.
#[test]
fn the_system_assembler_agrees() {
    let Some(assembler) = exec::which("as") else {
        eprintln!(
            "skipping: no `as` on PATH, so the assembler cross-check cannot run — the literal \
             byte string above is still checked"
        );
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("t.s");
    let obj = dir.path().join("t.o");
    // The same instructions, with local labels so nothing needs relocating: the encoders
    // under test are about the opcodes and the ModRM bytes, not the displacements.
    std::fs::write(
        &src,
        ".text\n\
         mov $1, %eax\n\
         mov $1, %edi\n\
         mov $6, %edx\n\
         syscall\n\
         mov %eax, %edi\n\
         mov $60, %eax\n\
         syscall\n\
         ret\n\
         test %rax, %rax\n\
         xor %eax, %eax\n\
         add %eax, %edi\n\
         push %rbp\n\
         pop %rbp\n",
    )
    .expect("write the assembly source");

    let run = exec::run(&exec::Spec::new(
        &assembler,
        &[
            "--64".into(),
            "-o".into(),
            obj.to_string_lossy().to_string(),
            src.to_string_lossy().to_string(),
        ],
        dir.path(),
        Duration::from_secs(30),
    ))
    .expect("run as");
    assert!(run.success(), "as failed: {}", run.stderr);

    let bytes = std::fs::read(&obj).expect("read t.o");
    let elf = linktest::elf::read::Elf::parse(&bytes).expect("our reader reads gas's output");
    let text = elf.section_data(".text").expect(".text");

    let mut ours = Code::new();
    ours.mov_r32_imm32(Reg::Rax, 1);
    ours.mov_r32_imm32(Reg::Rdi, 1);
    ours.mov_r32_imm32(Reg::Rdx, 6);
    ours.syscall();
    ours.mov_r32_r32(Reg::Rdi, Reg::Rax);
    ours.mov_r32_imm32(Reg::Rax, 60);
    ours.syscall();
    ours.ret();
    ours.test_r64_r64(Reg::Rax, Reg::Rax);
    ours.xor_r32_r32(Reg::Rax, Reg::Rax);
    ours.add_r32_r32(Reg::Rdi, Reg::Rax);
    ours.push(Reg::Rbp);
    ours.pop(Reg::Rbp);

    assert_eq!(
        &text[..ours.bytes.len()],
        &ours.bytes[..],
        "the system assembler and src/asm/mod.rs disagree"
    );
}
