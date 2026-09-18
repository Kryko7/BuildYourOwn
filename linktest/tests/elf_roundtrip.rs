//! The ELF writer and the ELF reader, checked against each other, against an independent
//! parser (the `object` crate) and — when binutils happens to be installed — against the
//! real thing: `ld` links the objects this suite writes, and the binary runs.

use linktest::elf::read::Elf;
use linktest::elf::write::*;
use linktest::elf::*;
use linktest::stages::helpers;
use object::{Object, ObjectSection, ObjectSymbol};

fn two_section_object() -> Vec<u8> {
    let mut code = linktest::asm::Code::new();
    code.sys_write(linktest::asm::STDOUT, "message", 6);
    code.call("other");
    code.sys_exit_eax();
    ObjectBuilder::new()
        .section(SectionSpec::text(".text", code.bytes.clone()).relocs(code.relocs.clone()))
        .section(SectionSpec::rodata(".rodata", b"hello\n".to_vec()))
        .section(SectionSpec::data(".data", vec![1, 2, 3, 4]).align(8))
        .section(SectionSpec::bss(".bss", 128).align(16))
        .symbol(SymbolSpec::global("_start", ".text", 0).func())
        .symbol(SymbolSpec::local("message", ".rodata", 0).object(6))
        .symbol(SymbolSpec::global("counter", ".data", 0).object(4))
        .symbol(SymbolSpec::undefined("other"))
        .build()
        .expect("the writer must produce an object")
}

#[test]
fn what_the_writer_wrote_is_what_the_reader_reads() {
    let bytes = two_section_object();
    let elf = Elf::parse(&bytes).expect("parse");

    assert_eq!(elf.e_type, ET_REL);
    assert_eq!(elf.e_machine, EM_X86_64);
    assert_eq!(elf.ehsize, EHDR_SIZE);
    assert_eq!(elf.shentsize, SHDR_SIZE);
    assert_eq!(elf.phnum, 0, "a relocatable object has no program headers");

    let text = elf.section(".text").expect(".text");
    assert!(text.is_alloc() && text.is_exec() && !text.is_write());
    assert_eq!(text.align, 16);

    let bss = elf.section(".bss").expect(".bss");
    assert!(bss.is_nobits());
    assert_eq!(bss.size, 128);
    assert_eq!(
        elf.section_data(".bss").expect("nobits data"),
        &[] as &[u8],
        ".bss occupies no file space"
    );

    assert_eq!(elf.section_data(".rodata").expect(".rodata"), b"hello\n");
    assert_eq!(elf.section_data(".data").expect(".data"), &[1, 2, 3, 4]);

    let start = elf.symbol("_start").expect("_start");
    assert_eq!(start.bind, STB_GLOBAL);
    assert_eq!(start.stype, STT_FUNC);
    assert_eq!(start.shndx, text.index as u16);

    let other = elf.symbol("other").expect("other");
    assert!(other.is_undefined());

    assert_eq!(elf.relocations.len(), 2);
    let kinds: Vec<u32> = elf.relocations.iter().map(|r| r.kind).collect();
    assert!(kinds.contains(&R_X86_64_PC32) && kinds.contains(&R_X86_64_PLT32));
    for r in &elf.relocations {
        assert_eq!(r.addend, -4, "both references are RIP-relative");
        assert_eq!(r.target_section, text.index);
    }
}

#[test]
fn locals_come_before_globals_and_sh_info_says_where() {
    let bytes = two_section_object();
    let elf = Elf::parse(&bytes).expect("parse");
    let symtab = elf
        .sections
        .iter()
        .find(|s| s.sh_type == SHT_SYMTAB)
        .expect(".symtab");
    let first_global = symtab.info as usize;
    for (i, s) in elf.symbols.iter().enumerate() {
        if i < first_global {
            assert_eq!(s.bind, STB_LOCAL, "symbol {i} ('{}') must be local", s.name);
        } else {
            assert_ne!(
                s.bind, STB_LOCAL,
                "symbol {i} ('{}') must not be local",
                s.name
            );
        }
    }
    assert!(first_global > 1, "the section symbols are locals too");
}

#[test]
fn an_independent_parser_agrees() {
    // The `object` crate is a dev-dependency and a cross-check only: the bytes are ours.
    let bytes = two_section_object();
    let file = object::File::parse(&bytes[..]).expect("the object crate must parse our object");
    assert_eq!(file.format(), object::BinaryFormat::Elf);
    assert_eq!(file.architecture(), object::Architecture::X86_64);
    assert!(file.is_little_endian());

    let names: Vec<String> = file
        .sections()
        .map(|s| s.name().unwrap_or_default().to_string())
        .collect();
    for want in [
        ".text",
        ".rodata",
        ".data",
        ".bss",
        ".symtab",
        ".strtab",
        ".shstrtab",
    ] {
        assert!(
            names.contains(&want.to_string()),
            "{want} missing from {names:?}"
        );
    }

    let start = file
        .symbols()
        .find(|s| s.name().unwrap_or_default() == "_start")
        .expect("_start");
    assert!(start.is_global());
    assert_eq!(start.address(), 0);

    let undefined: Vec<String> = file
        .symbols()
        .filter(|s| s.is_undefined())
        .map(|s| s.name().unwrap_or_default().to_string())
        .collect();
    assert!(undefined.contains(&"other".to_string()));
}

#[test]
fn header_overrides_do_exactly_what_they_say() {
    let bytes = ObjectBuilder::new()
        .section(SectionSpec::text(".text", vec![0xc3]))
        .symbol(SymbolSpec::global("_start", ".text", 0))
        .header(HeaderOverrides {
            e_machine: Some(EM_386),
            e_type: Some(ET_EXEC),
            e_shnum: Some(999),
            ..Default::default()
        })
        .build()
        .expect("build");
    assert_eq!(u16::from_le_bytes([bytes[18], bytes[19]]), EM_386);
    assert_eq!(u16::from_le_bytes([bytes[16], bytes[17]]), ET_EXEC);
    assert_eq!(u16::from_le_bytes([bytes[60], bytes[61]]), 999);
    // A section header table that claims 999 entries runs off the end: an error, not a panic.
    assert!(Elf::parse(&bytes).is_err() || Elf::parse(&bytes).map(|e| e.sections.len()) == Ok(0));
}

#[test]
fn a_truncated_object_is_an_error_at_every_length() {
    let bytes = two_section_object();
    for n in [0usize, 1, 4, 16, 40, 63] {
        let short = &bytes[..n.min(bytes.len())];
        assert!(
            Elf::parse(short).is_err(),
            "a {n}-byte file must not parse as ELF64"
        );
    }
    // Past the header, parsing may succeed but every lookup has to stay bounded.
    for n in [64usize, 100, 200] {
        if n >= bytes.len() {
            continue;
        }
        if let Ok(elf) = Elf::parse(&bytes[..n]) {
            let _ = elf.section_data(".text");
            let _ = elf.symbol("_start");
            let _ = elf.read_at_vaddr(0x401000, 8);
        }
    }
}

#[test]
fn alternative_layouts_still_parse() {
    for builder in [
        ObjectBuilder::new().rela_before_target(),
        ObjectBuilder::new().symtab_first(),
    ] {
        let mut code = linktest::asm::Code::new();
        code.lea_rip(linktest::asm::Reg::Rsi, "message", 0);
        code.ret();
        let bytes = builder
            .section(SectionSpec::text(".text", code.bytes.clone()).relocs(code.relocs.clone()))
            .section(SectionSpec::rodata(".rodata", b"hi".to_vec()))
            .symbol(SymbolSpec::global("_start", ".text", 0))
            .symbol(SymbolSpec::local("message", ".rodata", 0))
            .build()
            .expect("build");
        let elf = Elf::parse(&bytes).expect("parse");
        assert_eq!(elf.section_data(".rodata").expect("rodata"), b"hi");
        assert_eq!(elf.relocations.len(), 1);
        assert_eq!(
            elf.relocations[0].target_section,
            elf.section(".text").expect(".text").index
        );
    }
}

/// The end-to-end proof: the objects this suite writes are objects a real linker accepts.
#[test]
fn gnu_ld_links_what_this_suite_writes() {
    let Some(ld) = linktest::exec::which("ld") else {
        eprintln!("skipping: no `ld` on PATH, so the real-toolchain cross-check cannot run");
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let a = dir.path().join("a.o");
    let b = dir.path().join("b.o");
    let out = dir.path().join("prog");
    std::fs::write(
        &a,
        helpers::caller("hello from linktest\n", "other").expect("caller"),
    )
    .expect("write a.o");
    std::fs::write(&b, helpers::callee_returning("other", 7).expect("callee")).expect("write b.o");

    let link = linktest::exec::run(&linktest::exec::Spec::new(
        &ld,
        &[
            "-o".into(),
            out.to_string_lossy().to_string(),
            a.to_string_lossy().to_string(),
            b.to_string_lossy().to_string(),
        ],
        dir.path(),
        std::time::Duration::from_secs(30),
    ))
    .expect("run ld");
    assert!(
        link.success(),
        "ld refused our objects: {}\n{}",
        link.status_line(),
        link.stderr
    );

    let run = linktest::exec::run(&linktest::exec::Spec::new(
        &out,
        &[],
        dir.path(),
        std::time::Duration::from_secs(30),
    ))
    .expect("run the linked program");
    assert_eq!(run.stdout, "hello from linktest\n");
    assert_eq!(run.code, Some(7));

    // And our reader agrees with what ld produced.
    let bytes = std::fs::read(&out).expect("read the output");
    let exe = Elf::parse(&bytes).expect("our reader must read ld's output");
    assert_eq!(exe.e_type, ET_EXEC);
    assert_eq!(
        exe.entry,
        exe.symbol_address("_start").expect("_start in ld's output")
    );
    assert!(exe.loads().count() >= 1);
}
