//! Hand-assembled x86-64 machine code, with the relocations it needs.
//!
//! The suite has no assembler: every byte of every fixture's `.text` is produced here, and
//! every builder is documented with the instruction it encodes and the encoding it uses, so
//! a reader can check it against the Intel manual without running anything.
//!
//! [`Code`] is the point of the module. It appends instruction bytes *and* records the
//! relocation each RIP-relative or absolute reference needs, at the right offset, so a
//! fixture never has to count bytes by hand:
//!
//! ```ignore
//! let mut c = Code::new();
//! c.lea_rip(Reg::Rsi, "message", 0);     // lea rsi, [rip+message]  → R_X86_64_PC32, A = -4
//! c.mov_r32_imm32(Reg::Edx, 6);          // mov edx, 6
//! c.syscall();                           // syscall
//! ```
//!
//! Only the eight legacy registers are used, so no REX.R/REX.B bit ever has to be set and
//! the encodings stay one line each. `tests/asm_bytes.rs` pins a known sequence to a literal
//! byte string, and cross-checks it against the system assembler when there is one.

use crate::elf::write::{RelTarget, Reloc};
use crate::elf::{R_X86_64_64, R_X86_64_PC32, R_X86_64_PLT32};

/// The eight legacy 64-bit registers, by their ModRM register number.
///
/// The 32-bit names (`eax`, `edi`, …) are the same numbers: whether an instruction operates
/// on 32 or 64 bits is the REX.W prefix, not the register number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Reg {
    /// `rax` / `eax` — the syscall number, and the syscall's return value.
    Rax = 0,
    /// `rcx` / `ecx` — clobbered by `syscall`.
    Rcx = 1,
    /// `rdx` / `edx` — third syscall argument.
    Rdx = 2,
    /// `rbx` / `ebx`.
    Rbx = 3,
    /// `rsp` / `esp` — the stack pointer; never a ModRM r/m of 100 in this module.
    Rsp = 4,
    /// `rbp` / `ebp`.
    Rbp = 5,
    /// `rsi` / `esi` — second syscall argument.
    Rsi = 6,
    /// `rdi` / `edi` — first syscall argument.
    Rdi = 7,
}

impl Reg {
    /// The register's 3-bit number.
    pub fn num(self) -> u8 {
        self as u8
    }

    /// The name used in reports.
    pub fn name64(self) -> &'static str {
        match self {
            Reg::Rax => "rax",
            Reg::Rcx => "rcx",
            Reg::Rdx => "rdx",
            Reg::Rbx => "rbx",
            Reg::Rsp => "rsp",
            Reg::Rbp => "rbp",
            Reg::Rsi => "rsi",
            Reg::Rdi => "rdi",
        }
    }
}

/// `REX.W`: the prefix that makes an instruction 64-bit.
const REX_W: u8 = 0x48;

/// ModRM byte for "register-direct": `mod = 11`, `reg` and `rm` both register numbers.
fn modrm_reg(reg: Reg, rm: Reg) -> u8 {
    0xc0 | (reg.num() << 3) | rm.num()
}

/// ModRM byte for RIP-relative addressing: `mod = 00`, `rm = 101`, a disp32 follows.
fn modrm_rip(reg: Reg) -> u8 {
    (reg.num() << 3) | 0b101
}

/// A block of machine code, plus the relocations its references need.
#[derive(Debug, Clone, Default)]
pub struct Code {
    /// The instruction bytes.
    pub bytes: Vec<u8>,
    /// Relocations whose `offset` is relative to the start of this block.
    pub relocs: Vec<Reloc>,
}

impl Code {
    /// An empty block.
    pub fn new() -> Code {
        Code::default()
    }

    /// How many bytes have been emitted so far — the offset the next instruction starts at.
    pub fn len(&self) -> u64 {
        self.bytes.len() as u64
    }

    /// True when nothing has been emitted.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Append raw bytes that are already encoded (used by the fuzz and "odd encoding"
    /// fixtures).
    pub fn raw(&mut self, bytes: &[u8]) -> &mut Code {
        self.bytes.extend_from_slice(bytes);
        self
    }

    /// Append another block, shifting its relocations by the current offset.
    pub fn append(&mut self, other: &Code) -> &mut Code {
        let base = self.len();
        for r in &other.relocs {
            let mut r = r.clone();
            r.offset += base;
            self.relocs.push(r);
        }
        self.bytes.extend_from_slice(&other.bytes);
        self
    }

    /// Record a relocation at the current offset without emitting anything.
    pub fn reloc_here(&mut self, target: RelTarget, kind: u32, addend: i64) -> &mut Code {
        self.relocs.push(Reloc {
            offset: self.len(),
            target,
            kind,
            addend,
        });
        self
    }

    // -----------------------------------------------------------------------------------
    // Instructions with no operands
    // -----------------------------------------------------------------------------------

    /// `syscall` — `0f 05`. Arguments in rdi, rsi, rdx, r10, r8, r9; number in eax; result
    /// in rax; rcx and r11 are clobbered.
    pub fn syscall(&mut self) -> &mut Code {
        self.raw(&[0x0f, 0x05])
    }

    /// `ret` — `c3`.
    pub fn ret(&mut self) -> &mut Code {
        self.raw(&[0xc3])
    }

    /// `nop` — `90`.
    pub fn nop(&mut self) -> &mut Code {
        self.raw(&[0x90])
    }

    /// `ud2` — `0f 0b`, the canonical "must never be reached" trap.
    pub fn ud2(&mut self) -> &mut Code {
        self.raw(&[0x0f, 0x0b])
    }

    /// `push <r64>` — `50+rd`.
    pub fn push(&mut self, r: Reg) -> &mut Code {
        self.raw(&[0x50 + r.num()])
    }

    /// `pop <r64>` — `58+rd`.
    pub fn pop(&mut self, r: Reg) -> &mut Code {
        self.raw(&[0x58 + r.num()])
    }

    // -----------------------------------------------------------------------------------
    // Immediates
    // -----------------------------------------------------------------------------------

    /// `mov <r32>, imm32` — `b8+rd id`. Writing a 32-bit register zeroes the upper half.
    pub fn mov_r32_imm32(&mut self, r: Reg, v: u32) -> &mut Code {
        self.raw(&[0xb8 + r.num()]);
        self.raw(&v.to_le_bytes())
    }

    /// `movabs <r64>, imm64` — `48 b8+rd io`, the only instruction taking a 64-bit immediate.
    pub fn mov_r64_imm64(&mut self, r: Reg, v: u64) -> &mut Code {
        self.raw(&[REX_W, 0xb8 + r.num()]);
        self.raw(&v.to_le_bytes())
    }

    /// `add <r32>, imm32` — `81 /0 id`.
    pub fn add_r32_imm32(&mut self, r: Reg, v: u32) -> &mut Code {
        self.raw(&[0x81, 0xc0 | r.num()]);
        self.raw(&v.to_le_bytes())
    }

    /// `cmp <r32>, imm32` — `81 /7 id`.
    pub fn cmp_r32_imm32(&mut self, r: Reg, v: u32) -> &mut Code {
        self.raw(&[0x81, 0xf8 | r.num()]);
        self.raw(&v.to_le_bytes())
    }

    // -----------------------------------------------------------------------------------
    // Register to register
    // -----------------------------------------------------------------------------------

    /// `mov <dst32>, <src32>` — `89 /r`, ModRM `11 src dst`.
    pub fn mov_r32_r32(&mut self, dst: Reg, src: Reg) -> &mut Code {
        self.raw(&[0x89, modrm_reg(src, dst)])
    }

    /// `mov <dst64>, <src64>` — `48 89 /r`.
    pub fn mov_r64_r64(&mut self, dst: Reg, src: Reg) -> &mut Code {
        self.raw(&[REX_W, 0x89, modrm_reg(src, dst)])
    }

    /// `add <dst32>, <src32>` — `01 /r`.
    pub fn add_r32_r32(&mut self, dst: Reg, src: Reg) -> &mut Code {
        self.raw(&[0x01, modrm_reg(src, dst)])
    }

    /// `sub <dst32>, <src32>` — `29 /r`.
    pub fn sub_r32_r32(&mut self, dst: Reg, src: Reg) -> &mut Code {
        self.raw(&[0x29, modrm_reg(src, dst)])
    }

    /// `imul <dst32>, <src32>` — `0f af /r`.
    pub fn imul_r32_r32(&mut self, dst: Reg, src: Reg) -> &mut Code {
        self.raw(&[0x0f, 0xaf, modrm_reg(dst, src)])
    }

    /// `xor <dst32>, <src32>` — `31 /r`. `xor eax, eax` is the idiomatic zero.
    pub fn xor_r32_r32(&mut self, dst: Reg, src: Reg) -> &mut Code {
        self.raw(&[0x31, modrm_reg(src, dst)])
    }

    /// `test <a64>, <b64>` — `48 85 /r`; sets ZF when the AND is zero.
    pub fn test_r64_r64(&mut self, a: Reg, b: Reg) -> &mut Code {
        self.raw(&[REX_W, 0x85, modrm_reg(b, a)])
    }

    // -----------------------------------------------------------------------------------
    // RIP-relative references — each one records its own relocation
    // -----------------------------------------------------------------------------------

    /// `lea <r64>, [rip + <target>]` — `48 8d /r disp32`.
    ///
    /// The disp32 is patched by an `R_X86_64_PC32` whose addend is `-4 + addend`: the
    /// relocation's `P` is the address of the disp32 field, but the processor adds the
    /// displacement to the address of the *next* instruction, four bytes further on.
    pub fn lea_rip(&mut self, r: Reg, target: &str, addend: i64) -> &mut Code {
        self.raw(&[REX_W, 0x8d, modrm_rip(r)]);
        self.reloc_here(
            RelTarget::Symbol(target.to_string()),
            R_X86_64_PC32,
            -4 + addend,
        );
        self.raw(&0u32.to_le_bytes())
    }

    /// `lea <r64>, [rip + <section> + addend]` — the same encoding, but against a section
    /// symbol, which is how a compiler refers to an anonymous string literal.
    pub fn lea_rip_section(&mut self, r: Reg, section: &str, addend: i64) -> &mut Code {
        self.raw(&[REX_W, 0x8d, modrm_rip(r)]);
        self.reloc_here(
            RelTarget::Section(section.to_string()),
            R_X86_64_PC32,
            -4 + addend,
        );
        self.raw(&0u32.to_le_bytes())
    }

    /// `mov <r64>, [rip + <target>]` — `48 8b /r disp32`: load eight bytes.
    ///
    /// With `kind = R_X86_64_GOTPCREL` this is the classic "load the symbol's address out of
    /// the GOT" sequence; with `R_X86_64_PC32` it simply loads the symbol's contents.
    pub fn mov_r64_rip(&mut self, r: Reg, target: &str, kind: u32, addend: i64) -> &mut Code {
        self.raw(&[REX_W, 0x8b, modrm_rip(r)]);
        self.reloc_here(RelTarget::Symbol(target.to_string()), kind, addend);
        self.raw(&0u32.to_le_bytes())
    }

    /// `mov <r32>, [rip + <target>]` — `8b /r disp32`: load four bytes.
    pub fn mov_r32_rip(&mut self, r: Reg, target: &str, addend: i64) -> &mut Code {
        self.raw(&[0x8b, modrm_rip(r)]);
        self.reloc_here(
            RelTarget::Symbol(target.to_string()),
            R_X86_64_PC32,
            -4 + addend,
        );
        self.raw(&0u32.to_le_bytes())
    }

    /// `mov [rip + <target>], <r32>` — `89 /r disp32`: store four bytes.
    pub fn mov_rip_r32(&mut self, target: &str, r: Reg, addend: i64) -> &mut Code {
        self.raw(&[0x89, modrm_rip(r)]);
        self.reloc_here(
            RelTarget::Symbol(target.to_string()),
            R_X86_64_PC32,
            -4 + addend,
        );
        self.raw(&0u32.to_le_bytes())
    }

    /// `add <r32>, [rip + <target>]` — `03 /r disp32`.
    pub fn add_r32_rip(&mut self, r: Reg, target: &str, addend: i64) -> &mut Code {
        self.raw(&[0x03, modrm_rip(r)]);
        self.reloc_here(
            RelTarget::Symbol(target.to_string()),
            R_X86_64_PC32,
            -4 + addend,
        );
        self.raw(&0u32.to_le_bytes())
    }

    /// `movzx <r32>, byte [rip + <target>]` — `0f b6 /r disp32`: load one byte, zero-extended.
    pub fn movzx_r32_byte_rip(&mut self, r: Reg, target: &str, addend: i64) -> &mut Code {
        self.raw(&[0x0f, 0xb6, modrm_rip(r)]);
        self.reloc_here(
            RelTarget::Symbol(target.to_string()),
            R_X86_64_PC32,
            -4 + addend,
        );
        self.raw(&0u32.to_le_bytes())
    }

    /// `mov <r32>, imm32` whose immediate is the *address* of a symbol — `b8+rd id` with an
    /// `R_X86_64_32` relocation. Only valid when the address fits 32 unsigned bits, which is
    /// exactly what makes it the instruction that proves the overflow check.
    pub fn mov_r32_symbol_addr32(&mut self, r: Reg, target: &str, addend: i64) -> &mut Code {
        self.raw(&[0xb8 + r.num()]);
        self.reloc_here(
            RelTarget::Symbol(target.to_string()),
            crate::elf::R_X86_64_32,
            addend,
        );
        self.raw(&0u32.to_le_bytes())
    }

    /// `mov <r64>, imm32-sign-extended` whose immediate is the address of a symbol —
    /// `48 c7 /0 id` with an `R_X86_64_32S` relocation.
    pub fn mov_r64_symbol_addr32s(&mut self, r: Reg, target: &str, addend: i64) -> &mut Code {
        self.raw(&[REX_W, 0xc7, 0xc0 | r.num()]);
        self.reloc_here(
            RelTarget::Symbol(target.to_string()),
            crate::elf::R_X86_64_32S,
            addend,
        );
        self.raw(&0u32.to_le_bytes())
    }

    // -----------------------------------------------------------------------------------
    // Control flow
    // -----------------------------------------------------------------------------------

    /// `call <target>` — `e8 cd`, patched by `R_X86_64_PLT32` with addend `-4`.
    ///
    /// In a static link with no PLT, `PLT32` and `PC32` compute the same thing; a linker that
    /// treats them differently fails stage 24.
    pub fn call(&mut self, target: &str) -> &mut Code {
        self.raw(&[0xe8]);
        self.reloc_here(RelTarget::Symbol(target.to_string()), R_X86_64_PLT32, -4);
        self.raw(&0u32.to_le_bytes())
    }

    /// `call <target>` encoded with `R_X86_64_PC32` instead, to prove the two are the same.
    pub fn call_pc32(&mut self, target: &str) -> &mut Code {
        self.raw(&[0xe8]);
        self.reloc_here(RelTarget::Symbol(target.to_string()), R_X86_64_PC32, -4);
        self.raw(&0u32.to_le_bytes())
    }

    /// `jmp <target>` — `e9 cd`, patched by `R_X86_64_PLT32` with addend `-4`.
    pub fn jmp(&mut self, target: &str) -> &mut Code {
        self.raw(&[0xe9]);
        self.reloc_here(RelTarget::Symbol(target.to_string()), R_X86_64_PLT32, -4);
        self.raw(&0u32.to_le_bytes())
    }

    /// `je rel8` — `74 cb`, a short forward jump measured from the next instruction.
    pub fn je_rel8(&mut self, rel: i8) -> &mut Code {
        self.raw(&[0x74, rel as u8])
    }

    /// `jne rel8` — `75 cb`.
    pub fn jne_rel8(&mut self, rel: i8) -> &mut Code {
        self.raw(&[0x75, rel as u8])
    }
}

// ---------------------------------------------------------------------------------------
// The two syscalls every fixture needs
// ---------------------------------------------------------------------------------------

/// `write(2)`.
pub const SYS_WRITE: u32 = 1;
/// `exit(2)`.
pub const SYS_EXIT: u32 = 60;
/// `read(2)`.
pub const SYS_READ: u32 = 0;
/// File descriptor 1.
pub const STDOUT: u32 = 1;
/// File descriptor 2.
pub const STDERR: u32 = 2;

impl Code {
    /// `write(fd, &<symbol>, len)` — five instructions, one relocation.
    ///
    /// ```text
    /// b8 01 00 00 00      mov  eax, 1        ; __NR_write
    /// bf 01 00 00 00      mov  edi, 1        ; fd
    /// 48 8d 35 xx xx xx xx lea rsi, [rip+buf]; R_X86_64_PC32, A = -4
    /// ba len              mov  edx, len
    /// 0f 05               syscall
    /// ```
    pub fn sys_write(&mut self, fd: u32, symbol: &str, len: u32) -> &mut Code {
        self.mov_r32_imm32(Reg::Rax, SYS_WRITE);
        self.mov_r32_imm32(Reg::Rdi, fd);
        self.lea_rip(Reg::Rsi, symbol, 0);
        self.mov_r32_imm32(Reg::Rdx, len);
        self.syscall()
    }

    /// `write(fd, <section> + offset, len)`, for data referenced through a section symbol.
    pub fn sys_write_section(
        &mut self,
        fd: u32,
        section: &str,
        offset: i64,
        len: u32,
    ) -> &mut Code {
        self.mov_r32_imm32(Reg::Rax, SYS_WRITE);
        self.mov_r32_imm32(Reg::Rdi, fd);
        self.lea_rip_section(Reg::Rsi, section, offset);
        self.mov_r32_imm32(Reg::Rdx, len);
        self.syscall()
    }

    /// `write(fd, <address already in rsi>, len)`, for a pointer computed some other way.
    pub fn sys_write_rsi(&mut self, fd: u32, len: u32) -> &mut Code {
        self.mov_r32_imm32(Reg::Rax, SYS_WRITE);
        self.mov_r32_imm32(Reg::Rdi, fd);
        self.mov_r32_imm32(Reg::Rdx, len);
        self.syscall()
    }

    /// `exit(code)` — three instructions, no relocation. Never returns.
    pub fn sys_exit(&mut self, code: u32) -> &mut Code {
        self.mov_r32_imm32(Reg::Rax, SYS_EXIT);
        self.mov_r32_imm32(Reg::Rdi, code);
        self.syscall()
    }

    /// `exit(<the low byte of eax>)` — the status a computation produced.
    pub fn sys_exit_eax(&mut self) -> &mut Code {
        self.mov_r32_r32(Reg::Rdi, Reg::Rax);
        self.mov_r32_imm32(Reg::Rax, SYS_EXIT);
        self.syscall()
    }
}

/// An eight-byte absolute pointer to a symbol, for `.data`: eight zero bytes and an
/// `R_X86_64_64` relocation that fills them in.
pub fn pointer_to(symbol: &str, addend: i64) -> (Vec<u8>, Reloc) {
    (vec![0u8; 8], Reloc::sym(0, symbol, R_X86_64_64, addend))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hello_world_encodes_to_known_bytes() {
        let mut c = Code::new();
        c.sys_write(STDOUT, "msg", 6);
        c.sys_exit(0);
        assert_eq!(
            c.bytes,
            vec![
                0xb8, 0x01, 0x00, 0x00, 0x00, // mov eax, 1
                0xbf, 0x01, 0x00, 0x00, 0x00, // mov edi, 1
                0x48, 0x8d, 0x35, 0x00, 0x00, 0x00, 0x00, // lea rsi, [rip+msg]
                0xba, 0x06, 0x00, 0x00, 0x00, // mov edx, 6
                0x0f, 0x05, // syscall
                0xb8, 0x3c, 0x00, 0x00, 0x00, // mov eax, 60
                0xbf, 0x00, 0x00, 0x00, 0x00, // mov edi, 0
                0x0f, 0x05, // syscall
            ]
        );
        assert_eq!(c.relocs.len(), 1);
        assert_eq!(c.relocs[0].offset, 13, "the disp32 sits three bytes in");
        assert_eq!(c.relocs[0].kind, R_X86_64_PC32);
        assert_eq!(c.relocs[0].addend, -4);
    }

    #[test]
    fn call_records_a_plt32_at_the_displacement() {
        let mut c = Code::new();
        c.nop();
        c.call("other");
        assert_eq!(c.bytes[0..2], [0x90, 0xe8]);
        assert_eq!(c.relocs[0].offset, 2);
        assert_eq!(c.relocs[0].kind, R_X86_64_PLT32);
        assert_eq!(c.relocs[0].addend, -4);
        assert_eq!(c.len(), 6);
    }

    #[test]
    fn appending_shifts_relocations() {
        let mut head = Code::new();
        head.nop();
        head.nop();
        let mut tail = Code::new();
        tail.call("f");
        head.append(&tail);
        assert_eq!(
            head.relocs[0].offset, 3,
            "1 byte of call opcode after 2 nops"
        );
    }

    #[test]
    fn register_encodings_match_the_manual() {
        let mut c = Code::new();
        c.mov_r32_r32(Reg::Rdi, Reg::Rax); // mov edi, eax
        assert_eq!(c.bytes, vec![0x89, 0xc7]);
        let mut c = Code::new();
        c.test_r64_r64(Reg::Rax, Reg::Rax); // test rax, rax
        assert_eq!(c.bytes, vec![0x48, 0x85, 0xc0]);
        let mut c = Code::new();
        c.lea_rip(Reg::Rdi, "x", 0); // lea rdi, [rip+x]
        assert_eq!(c.bytes[..3], [0x48, 0x8d, 0x3d]);
    }
}
