//! The stage registry: what a stage and a test are, and the context a test runs in.
//!
//! Adding a stage means writing `src/stages/sNN_slug.rs` with a `pub fn stage() -> Stage`
//! and adding two lines here (the `mod` and the `push`). Nothing else in the harness
//! changes, which is what lets several people work on different stages at once.

use crate::assert::{Failure, FailureKind};
use crate::runtime::{Run, RuntimeHandle};
use crate::wasm::Module;
use rand::rngs::StdRng;
use rand::SeedableRng;
use std::path::PathBuf;
use std::time::{Duration, Instant};

mod helpers;
pub mod wasi;
pub use helpers::*;

// ---------------------------------------------------------------------------------------
// Stage modules. Keep them in numeric order.
// ---------------------------------------------------------------------------------------
mod s01_magic_and_version;
mod s02_leb128;
mod s03_section_framing;
mod s04_custom_sections;
mod s05_truncated_modules;
mod s06_minimal_modules;
mod s07_function_bodies;
mod s08_stack_discipline;
mod s09_unreachable_typing;
mod s10_block_types;
mod s11_unknown_indices;
mod s12_validation_before_execution;
mod s13_i32_arithmetic;
mod s14_i64_arithmetic;
mod s15_division_traps;
mod s16_comparisons_and_shifts;
mod s17_bit_counting;
mod s18_float_arithmetic;
mod s19_float_special_values;
mod s20_conversions;
mod s21_blocks_and_loops;
mod s22_if_else;
mod s23_br_table;
mod s24_return_and_unreachable;
mod s25_select;
mod s26_calls_and_recursion;
mod s27_multi_value;
mod s28_loads_and_stores;
mod s29_offset_and_alignment;
mod s30_memory_size_and_grow;
mod s31_memory_bounds;
mod s32_data_segments;
mod s33_memory_init;
mod s34_memory_copy_fill;
mod s35_globals;
mod s36_call_indirect;
mod s37_table_operations;
mod s38_element_segments;
mod s39_references_and_start;
mod s40_wasi_fd_write;
mod s41_wasi_args_and_exit;
mod s42_wasi_io_and_imports;
mod s43_fuzz;
mod s44_scale;
mod s45_soak;

/// Every implemented stage, in ascending order.
pub fn all() -> Vec<Stage> {
    let mut v = vec![
        s01_magic_and_version::stage(),
        s02_leb128::stage(),
        s03_section_framing::stage(),
        s04_custom_sections::stage(),
        s05_truncated_modules::stage(),
        s06_minimal_modules::stage(),
        s07_function_bodies::stage(),
        s08_stack_discipline::stage(),
        s09_unreachable_typing::stage(),
        s10_block_types::stage(),
        s11_unknown_indices::stage(),
        s12_validation_before_execution::stage(),
        s13_i32_arithmetic::stage(),
        s14_i64_arithmetic::stage(),
        s15_division_traps::stage(),
        s16_comparisons_and_shifts::stage(),
        s17_bit_counting::stage(),
        s18_float_arithmetic::stage(),
        s19_float_special_values::stage(),
        s20_conversions::stage(),
        s21_blocks_and_loops::stage(),
        s22_if_else::stage(),
        s23_br_table::stage(),
        s24_return_and_unreachable::stage(),
        s25_select::stage(),
        s26_calls_and_recursion::stage(),
        s27_multi_value::stage(),
        s28_loads_and_stores::stage(),
        s29_offset_and_alignment::stage(),
        s30_memory_size_and_grow::stage(),
        s31_memory_bounds::stage(),
        s32_data_segments::stage(),
        s33_memory_init::stage(),
        s34_memory_copy_fill::stage(),
        s35_globals::stage(),
        s36_call_indirect::stage(),
        s37_table_operations::stage(),
        s38_element_segments::stage(),
        s39_references_and_start::stage(),
        s40_wasi_fd_write::stage(),
        s41_wasi_args_and_exit::stage(),
        s42_wasi_io_and_imports::stage(),
        s43_fuzz::stage(),
        s44_scale::stage(),
        s45_soak::stage(),
    ];
    v.sort_by_key(|s| s.number);
    v
}

/// A section of the plan; `stages` lists every number the section will ever hold, whether or
/// not it is implemented yet, so the site can draw the whole journey from day one.
pub struct Section {
    /// Short id, `a`..`h`.
    pub id: &'static str,
    /// Human title.
    pub title: &'static str,
    /// Every stage number belonging to this section.
    pub stages: &'static [u32],
}

/// The eight sections of the WebAssembly track.
pub fn sections() -> &'static [Section] {
    &[
        Section {
            id: "a",
            title: "Binary format & decoding",
            stages: &[1, 2, 3, 4, 5, 6],
        },
        Section {
            id: "b",
            title: "Validation & type checking",
            stages: &[7, 8, 9, 10, 11, 12],
        },
        Section {
            id: "c",
            title: "Numeric instructions & traps",
            stages: &[13, 14, 15, 16, 17, 18, 19, 20],
        },
        Section {
            id: "d",
            title: "Control flow",
            stages: &[21, 22, 23, 24, 25, 26, 27],
        },
        Section {
            id: "e",
            title: "Memory & bulk operations",
            stages: &[28, 29, 30, 31, 32, 33, 34],
        },
        Section {
            id: "f",
            title: "Tables, globals, indirect calls",
            stages: &[35, 36, 37, 38, 39],
        },
        Section {
            id: "g",
            title: "WASI preview1",
            stages: &[40, 41, 42],
        },
        Section {
            id: "h",
            title: "Robustness & scale",
            stages: &[43, 44, 45],
        },
    ]
}

/// A test body.
pub type TestFn = fn(&mut Ctx) -> Result<(), Failure>;

/// Define a test body with a name, so a stage's `tests` list reads like its report.
///
/// ```ignore
/// wasm_test!(add_two_numbers, |ctx| {
///     let m = single("add", ftype(&[I32, I32], &[I32]), Expr::new()...);
///     expect_line(ctx, &m, "add", &["3", "4"], "7")
/// });
/// ```
#[macro_export]
macro_rules! wasm_test {
    ($fn_name:ident, |$ctx:ident| $body:block) => {
        fn $fn_name($ctx: &mut $crate::stages::Ctx) -> Result<(), $crate::assert::Failure> {
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
    /// `(runtime name, reason)` pairs: the test is skipped for those runtimes.
    pub skip_on: Vec<(&'static str, &'static str)>,
    /// Per-test timeout override; it wins outright, `--timeout-ms` included.
    pub timeout_ms: Option<u64>,
    /// A floor under the per-test timeout, for tests that are inherently slow (fuzz, soak,
    /// a multi-megabyte module). `--timeout-ms` still wins when it is larger.
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

    /// Skip this test for one runtime, with a reason that is always printed.
    pub fn skip_on(mut self, runtime: &'static str, reason: &'static str) -> Test {
        self.skip_on.push((runtime, reason));
        self
    }

    /// Give this test its own timeout, overriding both `--timeout-ms` and
    /// [`Test::min_timeout_ms`].
    pub fn timeout_ms(mut self, ms: u64) -> Test {
        self.timeout_ms = Some(ms);
        self
    }

    /// Raise the floor of this test's timeout; `--timeout-ms` still wins when it is larger.
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

    /// The reason this test is skipped for `runtime`, if it is.
    pub fn skip_reason(&self, runtime: &str) -> Option<&'static str> {
        self.skip_on
            .iter()
            .find(|(r, _)| *r == runtime)
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
    /// 1–3 worked examples: a module, its annotated bytes, and what a correct runtime
    /// prints. See [`crate::examples`].
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
    /// The name of the runtime under test, for `skip_on` and failure wording.
    pub runtime_name: String,
    /// How to spawn it.
    pub runtime: RuntimeHandle,
    /// This test's temporary directory; every module is written here.
    pub tmp: PathBuf,
    /// The timeout of one invocation.
    pub timeout: Duration,
    /// When the whole test body must be done.
    pub deadline: Instant,
    /// The run's seed; every random choice must come from it.
    pub seed: u64,
    /// A seeded RNG, reset per test so runs are reproducible.
    pub rng: StdRng,
    /// Print what the harness is doing between invocations.
    pub verbose: bool,
    /// Informational lines the test wants in the report even when it passes.
    pub notes: Vec<String>,
    /// How many times the runtime has been started in this test.
    pub invocations: u32,
}

impl Ctx {
    /// Build a context for one test.
    pub fn new(
        runtime: RuntimeHandle,
        tmp: PathBuf,
        timeout: Duration,
        seed: u64,
        test_index: u64,
        verbose: bool,
    ) -> Ctx {
        Ctx {
            runtime_name: runtime.def.name.clone(),
            runtime,
            tmp,
            timeout,
            deadline: Instant::now() + timeout,
            seed,
            rng: StdRng::seed_from_u64(seed ^ (test_index.wrapping_mul(0x9e37_79b9_7f4a_7c15))),
            verbose,
            notes: Vec::new(),
            invocations: 0,
        }
    }

    /// Add an informational line to the test's report entry.
    ///
    /// Unlike [`crate::assert::Check::note`], which only ever surfaces under a failure,
    /// these lines are printed (and put into the JSON report) whatever the outcome.
    pub fn note(&mut self, line: impl Into<String>) {
        self.notes.push(line.into());
    }

    /// Write a module into this test's temporary directory and return its path.
    pub fn write_module(&mut self, m: &Module) -> Result<PathBuf, Failure> {
        self.invocations += 1;
        let path = self.tmp.join(format!(
            "{:03}-{}.wasm",
            self.invocations,
            safe_stem(&m.label)
        ));
        std::fs::write(&path, &m.bytes).map_err(|e| {
            Failure::harness(format!("cannot write {}: {e}", path.display())).with_module(m)
        })?;
        Ok(path)
    }

    fn check_deadline(&self, m: &Module) -> Result<(), Failure> {
        if Instant::now() >= self.deadline {
            return Err(Failure::new(
                FailureKind::Timeout,
                "the test ran past its deadline before this invocation (raise --timeout-ms)",
            )
            .with_module(m));
        }
        Ok(())
    }

    fn spawn(
        &mut self,
        m: &Module,
        export: Option<&str>,
        args: &[&str],
        stdin: Option<&[u8]>,
    ) -> Result<Run, Failure> {
        self.check_deadline(m)?;
        let path = self.write_module(m)?;
        let owned: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        if self.verbose {
            println!(
                "      {} {} ({} bytes)",
                export
                    .map(|e| format!("--invoke {e}"))
                    .unwrap_or_else(|| "_start".to_string()),
                m.label,
                m.len()
            );
        }
        let left = self.deadline.saturating_duration_since(Instant::now());
        let run = self
            .runtime
            .invoke(
                &path,
                export,
                &owned,
                stdin,
                self.timeout.min(left).max(Duration::from_millis(50)),
                &self.tmp,
            )
            .map_err(|e| {
                Failure::new(
                    FailureKind::Harness,
                    format!("cannot start the runtime: {e:#}"),
                )
                .with_module(m)
            })?;
        if run.timed_out {
            return Err(Failure::new(
                FailureKind::Timeout,
                format!(
                    "the runtime did not finish within {} ms and was killed",
                    self.timeout.as_millis()
                ),
            )
            .with_module(m)
            .with_run(&run));
        }
        if run.exit.signal.is_some() {
            return Err(Failure::new(
                FailureKind::RuntimeCrash,
                format!(
                    "the runtime was killed by {} — a module is input, never a reason to die",
                    run.exit.label()
                ),
            )
            .with_module(m)
            .with_run(&run));
        }
        Ok(run)
    }

    /// `run --invoke <export> <module> [args...]`.
    pub fn invoke(&mut self, m: &Module, export: &str, args: &[&str]) -> Result<Run, Failure> {
        self.spawn(m, Some(export), args, None)
    }

    /// `run <module> [args...]` — the WASI command path, which calls `_start`.
    pub fn start(&mut self, m: &Module, args: &[&str]) -> Result<Run, Failure> {
        self.spawn(m, None, args, None)
    }

    /// `run <module>` with bytes on standard input.
    pub fn start_with_stdin(
        &mut self,
        m: &Module,
        args: &[&str],
        stdin: &[u8],
    ) -> Result<Run, Failure> {
        self.spawn(m, None, args, Some(stdin))
    }

    /// A name that is unique to this run but stable for a given seed.
    pub fn unique(&self, prefix: &str) -> String {
        format!("{prefix}-{:x}", self.seed & 0xffff_ffff)
    }
}

fn safe_stem(label: &str) -> String {
    label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stage_is_unique_and_ordered() {
        let stages = all();
        assert_eq!(stages.len(), 45);
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
    fn the_suite_is_as_big_as_the_plan_says() {
        let total: usize = all().iter().map(|s| s.tests.len()).sum();
        assert!(
            total >= 330,
            "the plan asks for at least 330 tests, the registry has {total}"
        );
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
                for (runtime, reason) in &t.skip_on {
                    assert!(
                        !reason.trim().is_empty(),
                        "stage {} test '{}' skips {runtime} with no reason",
                        s.number,
                        t.name
                    );
                }
            }
        }
    }

    #[test]
    fn an_ext_stage_only_holds_ext_tests() {
        for s in all().iter().filter(|s| s.ext) {
            for t in &s.tests {
                assert!(
                    t.is_ext(),
                    "stage {} is ext, so its test '{}' must be too",
                    s.number,
                    t.name
                );
            }
        }
    }

    #[test]
    fn wasi_tests_carry_the_wasi_tag_so_they_can_be_selected() {
        for s in all().iter().filter(|s| (40..=42).contains(&s.number)) {
            for t in &s.tests {
                assert!(
                    t.tags.contains(&"wasi"),
                    "stage {} test '{}' must carry the wasi tag",
                    s.number,
                    t.name
                );
            }
        }
    }

    #[test]
    fn timeout_override_wins_and_min_only_raises_the_floor() {
        crate::wasm_test!(nothing, |_ctx| { Ok(()) });
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
        let mut numbers: Vec<u32> = sections().iter().flat_map(|s| s.stages.to_vec()).collect();
        numbers.sort_unstable();
        assert_eq!(numbers, (1..=45).collect::<Vec<u32>>());
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
