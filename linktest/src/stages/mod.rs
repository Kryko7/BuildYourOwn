//! The stage registry: what a stage and a test are, and the context a test runs in.
//!
//! Adding a stage means writing `src/stages/sNN_slug.rs` with a `pub fn stage() -> Stage`
//! and adding two lines here (the `mod` and the `push`). Nothing else in the harness
//! changes, which is what lets several people work on different stages at once.
//!
//! A test body is a plain function — there is no async anywhere in `linktest`, because a
//! link is a subprocess and a linked program is a subprocess, and both are waited on with a
//! deadline ([`crate::exec`]).

use crate::assert::{Check, Failure, FailureKind};
use crate::elf::read::Elf;
use crate::exec;
use crate::link::{Link, LinkRun, Linked};
use crate::linker::LinkerHandle;
use rand::rngs::StdRng;
use rand::SeedableRng;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub mod helpers;
pub use helpers::*;

// ---------------------------------------------------------------------------------------
// Stage modules. Keep them in numeric order; gaps are expected while stages are unwritten.
// ---------------------------------------------------------------------------------------
mod s01_link_one_object;
mod s02_reject_foreign_input;
mod s03_read_the_elf_header;
mod s04_string_tables;
mod s05_section_header_table;
mod s06_truncated_objects;
mod s07_odd_but_legal_objects;
mod s08_executable_header;
mod s09_program_headers;
mod s10_entry_point;
mod s11_segment_permissions;
mod s12_page_alignment;
mod s13_bss;
mod s14_section_concatenation;
mod s15_alignment_and_empty_sections;
mod s16_global_across_objects;
mod s17_undefined_symbol;
mod s18_duplicate_definition;
mod s19_local_symbols;
mod s20_weak_symbols;
mod s21_common_symbols;
mod s22_visibility_and_absolute;
mod s23_entry_symbol;
mod s24_pc32_and_plt32;
mod s25_absolute_64;
mod s26_absolute_32;
mod s27_relocation_overflow;
mod s28_addends;
mod s29_section_symbols;
mod s30_cross_section_references;
mod s31_gotpcrel;
mod s32_archive_basics;
mod s33_archive_member_selection;
mod s34_link_order;
mod s35_library_search_path;
mod s36_archive_edge_cases;
mod s37_multi_object_program;
mod s38_many_objects;
mod s39_large_sections;
mod s40_fuzz;
mod s41_determinism;
mod s42_toolchain_interop;
mod s43_init_arrays;
mod s44_gc_sections;

/// Every implemented stage, in ascending order.
pub fn all() -> Vec<Stage> {
    let mut v = vec![
        s01_link_one_object::stage(),
        s02_reject_foreign_input::stage(),
        s03_read_the_elf_header::stage(),
        s04_string_tables::stage(),
        s05_section_header_table::stage(),
        s06_truncated_objects::stage(),
        s07_odd_but_legal_objects::stage(),
        s08_executable_header::stage(),
        s09_program_headers::stage(),
        s10_entry_point::stage(),
        s11_segment_permissions::stage(),
        s12_page_alignment::stage(),
        s13_bss::stage(),
        s14_section_concatenation::stage(),
        s15_alignment_and_empty_sections::stage(),
        s16_global_across_objects::stage(),
        s17_undefined_symbol::stage(),
        s18_duplicate_definition::stage(),
        s19_local_symbols::stage(),
        s20_weak_symbols::stage(),
        s21_common_symbols::stage(),
        s22_visibility_and_absolute::stage(),
        s23_entry_symbol::stage(),
        s24_pc32_and_plt32::stage(),
        s25_absolute_64::stage(),
        s26_absolute_32::stage(),
        s27_relocation_overflow::stage(),
        s28_addends::stage(),
        s29_section_symbols::stage(),
        s30_cross_section_references::stage(),
        s31_gotpcrel::stage(),
        s32_archive_basics::stage(),
        s33_archive_member_selection::stage(),
        s34_link_order::stage(),
        s35_library_search_path::stage(),
        s36_archive_edge_cases::stage(),
        s37_multi_object_program::stage(),
        s38_many_objects::stage(),
        s39_large_sections::stage(),
        s40_fuzz::stage(),
        s41_determinism::stage(),
        s42_toolchain_interop::stage(),
        s43_init_arrays::stage(),
        s44_gc_sections::stage(),
    ];
    v.sort_by_key(|s| s.number);
    v
}

/// A section of the plan; `stages` lists every number the section will ever hold, whether or
/// not it is implemented yet, so the site can draw the whole journey from day one.
pub struct Section {
    /// Short id, `a`..`f`.
    pub id: &'static str,
    /// Human title.
    pub title: &'static str,
    /// Every stage number belonging to this section.
    pub stages: &'static [u32],
}

/// The six sections of the linker track.
pub fn sections() -> &'static [Section] {
    &[
        Section {
            id: "a",
            title: "Reading relocatable objects",
            stages: &[1, 2, 3, 4, 5, 6, 7],
        },
        Section {
            id: "b",
            title: "Emitting a runnable executable",
            stages: &[8, 9, 10, 11, 12, 13, 14, 15],
        },
        Section {
            id: "c",
            title: "Symbol resolution",
            stages: &[16, 17, 18, 19, 20, 21, 22, 23],
        },
        Section {
            id: "d",
            title: "Relocations",
            stages: &[24, 25, 26, 27, 28, 29, 30, 31],
        },
        Section {
            id: "e",
            title: "Archives and link order",
            stages: &[32, 33, 34, 35, 36],
        },
        Section {
            id: "f",
            title: "Real programs, robustness, scale",
            stages: &[37, 38, 39, 40, 41, 42],
        },
        Section {
            id: "g",
            title: "What real toolchains expect",
            stages: &[43, 44],
        },
    ]
}

/// A test body: a plain function over the shared [`Ctx`].
pub type TestFn = fn(&mut Ctx) -> Result<(), Failure>;

/// Declare a test body.
///
/// ```ignore
/// link_test!(entry_is_start, |ctx| {
///     let linked = ctx.link_ok(&Link::new().object("a.o", obj))?;
///     ...
///     Ok(())
/// });
/// ```
#[macro_export]
macro_rules! link_test {
    ($fn_name:ident, |$ctx:ident| $body:block) => {
        fn $fn_name(
            $ctx: &mut $crate::stages::Ctx,
        ) -> ::std::result::Result<(), $crate::assert::Failure> {
            $body
        }
    };
}

/// One test inside a stage.
pub struct Test {
    /// Test name, shown in the report and used by `--only`.
    pub name: &'static str,
    /// Tags; `ext` marks tests beyond the core track.
    pub tags: Vec<&'static str>,
    /// `(linker name, reason)` pairs: the test is skipped for those linkers.
    pub skip_on: Vec<(&'static str, &'static str)>,
    /// Per-test timeout override. When set it wins outright, `--timeout-ms` included.
    pub timeout_ms: Option<u64>,
    /// A floor under the per-test timeout, for tests that are inherently slow (200 objects,
    /// a 5 MB section, 400 fuzz links). `--timeout-ms` still wins when it is larger.
    pub min_timeout_ms: Option<u64>,
    /// The test body.
    pub run: TestFn,
}

impl Test {
    /// A test with no tags and no skips.
    pub fn new(name: &'static str, run: TestFn) -> Test {
        Test {
            name,
            tags: Vec::new(),
            skip_on: Vec::new(),
            timeout_ms: None,
            min_timeout_ms: None,
            run,
        }
    }

    /// Mark the test as beyond the core track (`--skip-ext` hides it).
    pub fn ext(mut self) -> Test {
        self.tags.push("ext");
        self
    }

    /// Add an arbitrary tag (`--tag` selects it).
    pub fn tag(mut self, tag: &'static str) -> Test {
        self.tags.push(tag);
        self
    }

    /// Skip this test for one linker, with a reason that is always printed.
    pub fn skip_on(mut self, linker: &'static str, reason: &'static str) -> Test {
        self.skip_on.push((linker, reason));
        self
    }

    /// Give this test its own deadline, overriding both `--timeout-ms` and
    /// [`Test::min_timeout_ms`].
    pub fn timeout_ms(mut self, ms: u64) -> Test {
        self.timeout_ms = Some(ms);
        self
    }

    /// Raise the floor of this test's deadline; `--timeout-ms` still wins when it is larger.
    pub fn min_timeout_ms(mut self, ms: u64) -> Test {
        self.min_timeout_ms = Some(ms);
        self
    }

    /// The deadline this test runs under, given the run's default (`--timeout-ms`).
    pub fn timeout(&self, default: Duration) -> Duration {
        match self.timeout_ms {
            Some(ms) => Duration::from_millis(ms),
            None => match self.min_timeout_ms {
                Some(ms) => default.max(Duration::from_millis(ms)),
                None => default,
            },
        }
    }

    /// True when the test carries the `ext` tag.
    pub fn is_ext(&self) -> bool {
        self.tags.contains(&"ext")
    }

    /// The reason this test is skipped for `linker`, if it is.
    pub fn skip_reason(&self, linker: &str) -> Option<&'static str> {
        self.skip_on
            .iter()
            .find(|(l, _)| *l == linker)
            .map(|(_, r)| *r)
    }
}

/// One stage: a numbered group of tests with implementation hints.
pub struct Stage {
    /// Stage number, 1-based.
    pub number: u32,
    /// File-name slug, `sNN_<slug>.rs`.
    pub slug: &'static str,
    /// Human title.
    pub name: &'static str,
    /// True when the whole stage is beyond the core track.
    pub ext: bool,
    /// 2–4 lines of "what to implement, where the trap is".
    pub hints: &'static [&'static str],
    /// 1–3 worked examples: the input objects' relevant bytes, annotated, next to what the
    /// linked output must contain. See [`crate::examples`].
    pub examples: fn() -> Vec<crate::examples::ExampleSpec>,
    /// The stage's tests.
    pub tests: Vec<Test>,
}

impl Stage {
    /// The source file the stage lives in.
    pub fn file_name(&self) -> String {
        format!("src/stages/s{:02}_{}.rs", self.number, self.slug)
    }
}

/// Everything a test body can reach.
pub struct Ctx {
    /// The name of the linker under test, as `--linker` spelled it.
    pub linker_name: String,
    /// The linker under test.
    pub linker: LinkerHandle,
    /// GNU ld, when it is on the machine: the third leg of the verification, used by the
    /// handful of stages that compare an invariant against the reference.
    pub reference: Option<LinkerHandle>,
    /// The test's own temporary directory. Every link gets a fresh subdirectory of it.
    pub dir: PathBuf,
    /// The per-subprocess timeout.
    pub timeout: Duration,
    /// The run's seed; every random choice must come from it.
    pub seed: u64,
    /// A seeded RNG, reset per test so runs are reproducible.
    pub rng: StdRng,
    /// Informational lines the test wants in the report even when it passes.
    pub notes: Vec<String>,
    /// Set by [`Ctx::skip`]: the test decided at run time that it does not apply.
    pub skipped: Option<String>,
    /// When the test must be finished.
    pub deadline: Instant,
    /// How many links this test has run, so each gets its own directory.
    links: u32,
}

impl Ctx {
    /// Build a context for one test.
    pub fn new(
        linker: LinkerHandle,
        reference: Option<LinkerHandle>,
        dir: PathBuf,
        timeout: Duration,
        deadline: Duration,
        seed: u64,
        test_index: u64,
    ) -> Ctx {
        Ctx {
            linker_name: linker.def.name.clone(),
            linker,
            reference,
            dir,
            timeout,
            seed,
            rng: StdRng::seed_from_u64(seed ^ (test_index.wrapping_mul(0x9e37_79b9_7f4a_7c15))),
            notes: Vec::new(),
            skipped: None,
            deadline: Instant::now() + deadline,
            links: 0,
        }
    }

    /// Add an informational line to the test's report entry, whatever the outcome.
    pub fn note(&mut self, line: impl Into<String>) {
        self.notes.push(line.into());
    }

    /// Decide at run time that this test does not apply — an optional tool is missing, the
    /// linker legitimately took the other of two allowed roads. Always give a reason: it is
    /// printed, put in the JSON report, and (for `--validate`) has to be in the README.
    ///
    /// Written as `return ctx.skip("readelf is not installed");`.
    pub fn skip(&mut self, reason: impl Into<String>) -> Result<(), Failure> {
        self.skipped = Some(reason.into());
        Ok(())
    }

    /// A name unique to this run but stable for a given seed.
    pub fn unique(&self, prefix: &str) -> String {
        format!("{prefix}_{:x}", self.seed & 0xffff_ffff)
    }

    /// How long is left before the test's deadline.
    fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    fn budget(&self) -> Duration {
        self.timeout
            .min(self.remaining().max(Duration::from_millis(1)))
    }

    /// Run one link and hand back whatever happened, with no expectation at all.
    ///
    /// This is the entry point every other `link_*` method goes through: it creates a fresh
    /// directory, writes the inputs, runs the linker with the remaining time budget and
    /// reads the output file back if one appeared.
    pub fn link(&mut self, link: &Link) -> Result<LinkRun, Failure> {
        self.links += 1;
        let dir = self.dir.join(format!("l{:02}", self.links));
        std::fs::create_dir_all(&dir)
            .map_err(|e| Failure::harness(format!("cannot create {}: {e}", dir.display())))?;
        for (name, bytes) in &link.files {
            let path = dir.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    Failure::harness(format!("cannot create {}: {e}", parent.display()))
                })?;
            }
            std::fs::write(&path, bytes)
                .map_err(|e| Failure::harness(format!("cannot write {}: {e}", path.display())))?;
        }
        if self.remaining().is_zero() {
            return Err(Failure::new(
                FailureKind::Timeout,
                "the test ran out of time before this link started",
            ));
        }
        let output = self
            .linker
            .invoke(&dir, &absolute_argv(&dir, &link.argv()), self.budget())
            .map_err(|e| {
                Failure::harness(format!(
                    "cannot run the linker {}: {e}",
                    self.linker.program.display()
                ))
            })?;
        let out_path = dir.join(&link.out);
        let produced = std::fs::read(&out_path).ok();
        Ok(LinkRun {
            dir,
            output,
            out_path,
            produced,
        })
    }

    /// Link, and insist it worked: exit 0, an output file, and an output that parses.
    pub fn link_ok(&mut self, link: &Link) -> Result<Linked, Failure> {
        let doing = if link.label.is_empty() {
            "the inputs".to_string()
        } else {
            link.label.clone()
        };
        let run = self.link(link)?;
        if !run.succeeded() {
            return Err(run.failed(&doing));
        }
        let bytes = run.produced.clone().unwrap_or_default();
        let elf = Elf::parse(&bytes).map_err(|e| {
            run.attach(
                Failure::new(
                    FailureKind::MalformedOutput,
                    format!("the output is not an ELF64 file this parser can read: {e}"),
                )
                .note(format!("while linking {doing}")),
            )
            .hex("output bytes", &bytes, &[])
        })?;
        Ok(Linked {
            path: run.out_path.clone(),
            bytes,
            elf,
            run,
        })
    }

    /// Link, and insist it was refused: a non-zero exit and a diagnostic.
    ///
    /// The wording is never pinned — only that the linker said *something* and failed. Use
    /// [`Check::mentions`] afterwards to assert the offending symbol is named.
    pub fn link_fails(&mut self, link: &Link, doing: &str) -> Result<LinkRun, Failure> {
        let run = self.link(link)?;
        if run.output.timed_out {
            return Err(run.attach(Failure::new(
                FailureKind::Timeout,
                format!("the linker never finished {doing}"),
            )));
        }
        if run.output.signal.is_some() {
            return Err(run.attach(Failure::new(
                FailureKind::LinkerCrash,
                format!(
                    "the linker was killed by a signal while {doing}: {}",
                    run.output.status_line()
                ),
            )));
        }
        if run.output.success() {
            return Err(run.attach(
                Failure::new(
                    FailureKind::LinkAccepted,
                    format!("the linker exited 0 while {doing}; this must be an error"),
                )
                .note(
                    "a linker that accepts a malformed input goes on to produce a binary the \
                     kernel will refuse, or worse, run",
                ),
            ));
        }
        let mut c = Check::new(format!("the diagnostic for {doing}"));
        c.that(
            "linker.stderr",
            "a diagnostic on stderr",
            !run.output.stderr.trim().is_empty(),
            "(empty)",
        );
        c.block("linker command", run.output.command_line());
        c.block("linker output", run.diagnostics());
        c.finish()?;
        Ok(run)
    }

    /// Link the same inputs with GNU ld, for the comparison leg.
    ///
    /// `Ok(None)` when there is no reference linker on the machine, or when the linker under
    /// test *is* the reference — the caller then notes that the comparison was skipped.
    pub fn reference_link(&mut self, link: &Link) -> Result<Option<Linked>, Failure> {
        let Some(reference) = self.reference.clone() else {
            return Ok(None);
        };
        if reference.program == self.linker.program {
            return Ok(None);
        }
        let mut ctx_linker = reference;
        std::mem::swap(&mut self.linker, &mut ctx_linker);
        let result = self.link_ok(link);
        std::mem::swap(&mut self.linker, &mut ctx_linker);
        result.map(Some)
    }

    /// Run a linked program with no arguments and no stdin.
    pub fn run(&mut self, linked: &Linked) -> Result<exec::Output, Failure> {
        self.run_with(linked, &[], &[])
    }

    /// Run a linked program with arguments and stdin.
    ///
    /// The binary is freestanding and statically linked, so this is a plain `execve` in the
    /// link's own directory, with a deadline and never with elevated privileges.
    pub fn run_with(
        &mut self,
        linked: &Linked,
        args: &[&str],
        stdin: &[u8],
    ) -> Result<exec::Output, Failure> {
        if !is_executable(&linked.path) {
            return Err(linked.attach(
                Failure::new(
                    FailureKind::ProgramCrash,
                    format!(
                        "the output file {} is not marked executable, so the kernel refuses to \
                         run it",
                        linked.path.display()
                    ),
                )
                .note(
                    "the linker has to chmod its output 0755 (or 0777 & ~umask); ELF alone is \
                     not enough",
                ),
            ));
        }
        if self.remaining().is_zero() {
            return Err(Failure::new(
                FailureKind::Timeout,
                "the test ran out of time before the linked program could be run",
            ));
        }
        let spec = exec::Spec::new(
            &linked.path,
            &args.iter().map(|a| (*a).to_string()).collect::<Vec<_>>(),
            &linked.run.dir,
            self.budget(),
        )
        .stdin(stdin.to_vec());
        let out = exec::run(&spec).map_err(|e| {
            linked.attach(Failure::new(
                FailureKind::ProgramCrash,
                format!("cannot run {}: {e}", linked.path.display()),
            ))
        })?;
        if out.timed_out {
            return Err(linked.attach(Failure::new(
                FailureKind::Timeout,
                "the linked program never exited",
            )));
        }
        if let Some(sig) = out.signal {
            return Err(linked
                .attach(Failure::new(
                    FailureKind::ProgramCrash,
                    format!(
                        "the linked program died with signal {sig} ({})",
                        exec::signal_name(sig)
                    ),
                ))
                .note(
                    "a freestanding binary that dies this way usually means a relocation was \
                     patched with the wrong value, or a segment is mapped without the \
                     permission the code needs",
                )
                .block("program stdout", out.stdout.clone())
                .block("program stderr", out.stderr.clone()));
        }
        Ok(out)
    }

    /// Run a linked program and assert its stdout and exit status.
    ///
    /// This is leg one of the three-legged verification, and the one a learner feels first:
    /// the program the linker produced does what its source said it would.
    pub fn expect_output(
        &mut self,
        linked: &Linked,
        stdout: &str,
        exit: i32,
    ) -> Result<exec::Output, Failure> {
        let out = self.run(linked)?;
        let mut c = Check::new("what the linked program printed and exited with");
        c.eq("program.stdout", stdout.to_string(), out.stdout.clone());
        c.eq("program.exit_status", Some(exit), out.code);
        if !c.ok() {
            c.block("linker command", linked.run.output.command_line());
            c.block("output program headers", linked.elf.program_header_table());
            if !out.stderr.trim().is_empty() {
                c.block("program stderr", out.stderr.clone());
            }
        }
        c.finish()?;
        Ok(out)
    }

    /// An optional toolchain program (`readelf`, `nm`, `objdump`) for the interop leg.
    pub fn tool(&self, name: &str) -> Option<PathBuf> {
        exec::which(name)
    }

    /// Run an optional toolchain program over a file.
    pub fn run_tool(&mut self, name: &str, args: &[&str]) -> Result<exec::Output, Failure> {
        let program = self
            .tool(name)
            .ok_or_else(|| Failure::harness(format!("{name} is not on PATH")))?;
        let spec = exec::Spec::new(
            &program,
            &args.iter().map(|a| (*a).to_string()).collect::<Vec<_>>(),
            &self.dir,
            self.budget(),
        );
        exec::run(&spec).map_err(|e| Failure::harness(format!("cannot run {name}: {e}")))
    }
}

/// Rewrite a link's argument list so every path in it is absolute.
///
/// A linker may have a working directory of its own (`cwd: "."` in `linkers.yaml` is what
/// `./your_program.sh` needs, because that is where the rest of its project lives), so the
/// harness can never assume the linker runs in the directory the inputs were written to.
/// Anything that names a file which exists in the link's directory — and the `-o` and `-L`
/// operands, which need not exist yet — becomes an absolute path. `-e main` and `-l x` name
/// a symbol and a library, not files, and are left exactly as they were.
pub fn absolute_argv(dir: &Path, args: &[String]) -> Vec<String> {
    let abs = |s: &str| dir.join(s).to_string_lossy().to_string();
    let mut out = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if (a == "-o" || a == "-L") && i + 1 < args.len() {
            out.push(a.clone());
            out.push(abs(&args[i + 1]));
            i += 2;
            continue;
        }
        if let Some(rest) = a.strip_prefix("-L") {
            if !rest.is_empty() {
                out.push(format!("-L{}", abs(rest)));
                i += 1;
                continue;
            }
        }
        if !a.starts_with('-') && dir.join(a).exists() {
            out.push(abs(a));
        } else {
            out.push(a.clone());
        }
        i += 1;
    }
    out
}

/// True when the file has any execute bit set.
pub fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stage_is_unique_and_ordered() {
        let stages = all();
        assert!(!stages.is_empty());
        let mut last = 0;
        for s in &stages {
            assert!(s.number > last, "stage {} is out of order", s.number);
            last = s.number;
            assert!(!s.tests.is_empty(), "stage {} has no tests", s.number);
            assert!(
                (2..=4).contains(&s.hints.len()),
                "stage {} must carry 2-4 hints, has {}",
                s.number,
                s.hints.len()
            );
            assert!(
                s.slug
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()),
                "stage {} slug '{}' must be snake_case",
                s.number,
                s.slug
            );
        }
    }

    #[test]
    fn test_names_are_unique_within_a_stage() {
        for s in all() {
            let mut names: Vec<&str> = s.tests.iter().map(|t| t.name).collect();
            names.sort_unstable();
            let before = names.len();
            names.dedup();
            assert_eq!(
                before,
                names.len(),
                "stage {} has duplicate test names",
                s.number
            );
        }
    }

    #[test]
    fn skips_always_carry_a_reason() {
        for s in all() {
            for t in &s.tests {
                for (linker, reason) in &t.skip_on {
                    assert!(
                        !reason.trim().is_empty(),
                        "stage {} test '{}' skips {linker} with no reason",
                        s.number,
                        t.name
                    );
                }
            }
        }
    }

    #[test]
    fn timeout_override_wins_and_min_only_raises_the_floor() {
        crate::link_test!(nothing, |_ctx| { Ok(()) });
        let default = Duration::from_millis(10_000);

        let plain = Test::new("plain", nothing);
        assert_eq!(plain.timeout(default), default, "no opinion, no change");

        let slow = Test::new("slow", nothing).min_timeout_ms(60_000);
        assert_eq!(slow.timeout(default), Duration::from_millis(60_000));
        assert_eq!(
            slow.timeout(Duration::from_millis(90_000)),
            Duration::from_millis(90_000),
            "--timeout-ms still wins when it is larger than the floor"
        );

        let pinned = Test::new("pinned", nothing).timeout_ms(120_000);
        assert_eq!(pinned.timeout(default), Duration::from_millis(120_000));
        assert_eq!(
            pinned.timeout(Duration::from_millis(300_000)),
            Duration::from_millis(120_000),
            "an explicit override wins outright"
        );
    }

    #[test]
    fn sections_cover_the_whole_plan_once() {
        let mut all_numbers: Vec<u32> = sections().iter().flat_map(|s| s.stages.to_vec()).collect();
        all_numbers.sort_unstable();
        assert_eq!(
            all_numbers,
            (1..=all_numbers.len() as u32).collect::<Vec<u32>>(),
            "the sections must cover 1..=n once each, with no gap and no repeat"
        );
    }

    #[test]
    fn implemented_stages_belong_to_a_section() {
        for s in all() {
            assert!(
                sections().iter().any(|sec| sec.stages.contains(&s.number)),
                "stage {} is in no section",
                s.number
            );
        }
    }
}
