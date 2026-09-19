//! The commands CI runs and the commands the docs promise must be the same commands.
//!
//! This exists because they were not. The workflow was written before the repo became a
//! cargo workspace and kept saying `./target/release/<tester>`; the READMEs were updated and
//! the workflow was not, so every `--validate` job failed on a path while every job that
//! only ran `cargo test` went green. The suites were fine. The claim about them was not
//! runnable, which is the failure this whole file is meant to make loud.
//!
//! So: every `--validate` command in the workflow must appear verbatim in `PLAN.md`'s
//! reproduce table, and every one in that table must be in the workflow. Neither file can
//! drift without the other noticing.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    // `byo/` is a workspace member, so the repo root is its parent.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("byo/ has a parent")
        .to_path_buf()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Every `target/release/<something> ... --validate ...` command in a file, normalised to
/// one line with single spaces so a table cell and a YAML line compare equal.
fn validate_commands(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in text.lines() {
        let Some(at) = line.find("target/release/") else {
            continue;
        };
        let rest = &line[at..];
        if !rest.contains("--validate") {
            continue;
        }
        // Stop at whatever the surrounding file wraps commands in.
        let end = rest
            .find(" #")
            .or_else(|| rest.find('`'))
            .or_else(|| rest.find('|'))
            .unwrap_or(rest.len());
        let cmd = rest[..end].split_whitespace().collect::<Vec<_>>().join(" ");
        out.insert(cmd);
    }
    out
}

#[test]
fn ci_runs_exactly_the_commands_the_plan_promises() {
    let root = repo_root();
    let workflow = read(&root.join(".github/workflows/ci.yml"));
    let plan = read(&root.join("PLAN.md"));

    let in_ci = validate_commands(&workflow);
    let in_plan = validate_commands(&plan);

    assert!(
        in_ci.len() >= 6,
        "expected a --validate command per track in CI, found {}: {in_ci:#?}",
        in_ci.len()
    );

    let ci_only: Vec<_> = in_ci.difference(&in_plan).collect();
    let plan_only: Vec<_> = in_plan.difference(&in_ci).collect();
    assert!(
        ci_only.is_empty() && plan_only.is_empty(),
        "the workflow and PLAN.md disagree about how to reproduce the numbers.\n\
         only in the workflow: {ci_only:#?}\nonly in PLAN.md: {plan_only:#?}"
    );
}

#[test]
fn no_document_still_points_at_a_per_crate_target_directory() {
    // The workspace has one target directory at the root. `./target/release/<tester>` was
    // right before it and is wrong now: it resolves to `<crate>/target/release`, which cargo
    // no longer writes to, and it fails only at run time.
    let root = repo_root();
    let mut offenders = Vec::new();
    let mut check = |rel: &str| {
        let p = root.join(rel);
        if !p.is_file() {
            return;
        }
        for (n, line) in read(&p).lines().enumerate() {
            if line.contains("./target/release/") {
                offenders.push(format!("{rel}:{}: {}", n + 1, line.trim()));
            }
        }
    };
    check(".github/workflows/ci.yml");
    check("README.md");
    check("PLAN.md");
    for crate_dir in [
        "shelltest",
        "kafkatest",
        "wasmtest",
        "tlstest",
        "linktest",
        "disttest",
        "byo",
    ] {
        check(&format!("{crate_dir}/README.md"));
    }
    assert!(
        offenders.is_empty(),
        "these point at a target directory the workspace does not use:\n{}",
        offenders.join("\n")
    );
}
