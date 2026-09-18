use linktest::asm::{Code, Reg, STDOUT};
use linktest::elf::write::*;
use linktest::elf::*;

fn main() -> Result<(), String> {
    let mut code = Code::new();
    code.sys_write(STDOUT, "msg", 6);
    code.call("other");
    code.sys_exit_eax();
    let a = ObjectBuilder::new()
        .section(SectionSpec::text(".text", code.bytes.clone()).relocs(code.relocs.clone()))
        .section(SectionSpec::rodata(".rodata", b"hello\n".to_vec()))
        .symbol(SymbolSpec::global("_start", ".text", 0).func())
        .symbol(SymbolSpec::local("msg", ".rodata", 0).object(6))
        .symbol(SymbolSpec::undefined("other"))
        .build()?;
    let mut o = Code::new();
    o.mov_r32_imm32(Reg::Rax, 7);
    o.ret();
    let b = ObjectBuilder::new()
        .section(SectionSpec::text(".text", o.bytes.clone()))
        .symbol(SymbolSpec::global("other", ".text", 0).func())
        .build()?;
    std::fs::write("/tmp/ldx/my_a.o", &a).map_err(|e| e.to_string())?;
    std::fs::write("/tmp/ldx/my_b.o", &b).map_err(|e| e.to_string())?;
    println!("wrote {} and {} bytes", a.len(), b.len());
    let _ = EM_X86_64;
    Ok(())
}
