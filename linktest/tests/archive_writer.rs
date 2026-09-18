//! The archive writer, round-tripped through its own parser and — when binutils is
//! installed — through `ar` and `nm`, which is the check that keeps the format honest.

use linktest::elf::archive::{self, ArchiveBuilder, IndexMode, Member};
use linktest::exec;
use linktest::stages::helpers;
use std::time::Duration;

fn library() -> Vec<u8> {
    let m1 = helpers::callee_returning("other", 7).expect("member 1");
    let m2 = helpers::callee_returning("unused_helper", 3).expect("member 2");
    ArchiveBuilder::new()
        .member(Member::new("m1.o", m1, &["other"]))
        .member(Member::new(
            "a_member_with_a_very_long_name.o",
            m2,
            &["unused_helper"],
        ))
        .build()
        .expect("build the archive")
}

#[test]
fn the_archive_round_trips_through_its_own_parser() {
    let bytes = library();
    assert_eq!(&bytes[..8], archive::ARCHIVE_MAGIC);
    let (members, index) = archive::parse(&bytes).expect("parse");
    assert_eq!(members.len(), 2);
    assert_eq!(members[0].name, "m1.o");
    assert_eq!(members[1].name, "a_member_with_a_very_long_name.o");
    let index = index.expect("the default index mode writes one");
    let names: Vec<&str> = index.entries.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, vec!["other", "unused_helper"]);
    for (name, offset) in &index.entries {
        assert!(
            members.iter().any(|m| m.header_offset == *offset as usize),
            "'{name}' points at offset {offset}, which is not a member header"
        );
    }
}

#[test]
fn every_member_is_still_a_readable_object() {
    let bytes = library();
    let (members, _) = archive::parse(&bytes).expect("parse");
    for m in &members {
        let elf = linktest::elf::read::Elf::parse(&m.data)
            .unwrap_or_else(|e| panic!("member '{}' is not an object: {e}", m.name));
        assert_eq!(elf.e_type, linktest::elf::ET_REL);
    }
}

#[test]
fn members_start_on_even_offsets() {
    let bytes = ArchiveBuilder::new()
        .member(Member::new("odd.o", vec![1, 2, 3], &["a"]))
        .member(Member::new("next.o", vec![4, 5], &["b"]))
        .build()
        .expect("build");
    let (members, _) = archive::parse(&bytes).expect("parse");
    for m in &members {
        assert!(
            m.header_offset.is_multiple_of(2),
            "member '{}' starts at {}, which is odd",
            m.name,
            m.header_offset
        );
    }
}

#[test]
fn the_index_modes_differ_the_way_they_say() {
    let object = helpers::callee_returning("other", 1).expect("object");
    let build = |mode: IndexMode| {
        ArchiveBuilder::new()
            .member(Member::new("m1.o", object.clone(), &["other"]))
            .member(Member::new("m2.o", object.clone(), &["second"]))
            .index(mode)
            .build()
            .expect("build")
    };
    let (_, none) = archive::parse(&build(IndexMode::Omitted)).expect("parse");
    assert!(none.is_none());

    let (members, stale) = archive::parse(&build(IndexMode::Stale)).expect("parse");
    let stale = stale.expect("index");
    let first = members[0].header_offset as u32;
    assert!(stale.entries.iter().all(|(_, o)| *o == first));

    let (_, phantom) = archive::parse(&build(IndexMode::Phantom)).expect("parse");
    let phantom = phantom.expect("index");
    assert!(phantom
        .entries
        .iter()
        .any(|(n, _)| n == "linktest_phantom_symbol"));
}

#[test]
fn a_truncated_or_corrupt_archive_is_an_error_not_a_panic() {
    let bytes = library();
    for n in [0usize, 4, 8, 20, 59, 61, bytes.len() - 1] {
        let _ = archive::parse(&bytes[..n.min(bytes.len())]);
    }
    assert!(archive::parse(&bytes[..4]).is_err());
    let broken = ArchiveBuilder::new()
        .member(Member::new("m.o", vec![1, 2], &[]))
        .magic_override(*b"!<arch>!")
        .build()
        .expect("build");
    assert!(archive::parse(&broken).is_err());
}

/// The cross-check: real `ar` and `nm` read the archives this suite writes.
#[test]
fn binutils_reads_our_archive() {
    let (Some(ar), Some(nm)) = (exec::which("ar"), exec::which("nm")) else {
        eprintln!("skipping: `ar` or `nm` is not on PATH, so the binutils cross-check cannot run");
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("libx.a");
    std::fs::write(&path, library()).expect("write the archive");

    let listed = exec::run(&exec::Spec::new(
        &ar,
        &["t".into(), path.to_string_lossy().to_string()],
        dir.path(),
        Duration::from_secs(30),
    ))
    .expect("run ar");
    assert!(listed.success(), "ar t failed: {}", listed.stderr);
    assert!(listed.stdout.contains("m1.o"), "{}", listed.stdout);
    assert!(
        listed.stdout.contains("a_member_with_a_very_long_name.o"),
        "the long-name table is wrong:\n{}",
        listed.stdout
    );

    let symbols = exec::run(&exec::Spec::new(
        &nm,
        &["-s".into(), path.to_string_lossy().to_string()],
        dir.path(),
        Duration::from_secs(30),
    ))
    .expect("run nm");
    assert!(symbols.success(), "nm -s failed: {}", symbols.stderr);
    assert!(
        symbols.stdout.contains("other in m1.o"),
        "nm does not see our symbol index:\n{}",
        symbols.stdout
    );
}

/// And the real proof: `ld` pulls a member out of our archive and the program runs.
#[test]
fn gnu_ld_links_from_our_archive() {
    let Some(ld) = exec::which("ld") else {
        eprintln!("skipping: no `ld` on PATH");
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let main = dir.path().join("main.o");
    let lib = dir.path().join("libx.a");
    let out = dir.path().join("prog");
    std::fs::write(
        &main,
        helpers::caller("from an archive\n", "other").expect("caller"),
    )
    .expect("write main.o");
    std::fs::write(&lib, library()).expect("write libx.a");

    let link = exec::run(&exec::Spec::new(
        &ld,
        &[
            "-o".into(),
            out.to_string_lossy().to_string(),
            main.to_string_lossy().to_string(),
            lib.to_string_lossy().to_string(),
        ],
        dir.path(),
        Duration::from_secs(30),
    ))
    .expect("run ld");
    assert!(link.success(), "ld refused our archive: {}", link.stderr);

    let run = exec::run(&exec::Spec::new(
        &out,
        &[],
        dir.path(),
        Duration::from_secs(30),
    ))
    .expect("run");
    assert_eq!(run.stdout, "from an archive\n");
    assert_eq!(
        run.code,
        Some(7),
        "the member defining `other` was pulled in"
    );
}
