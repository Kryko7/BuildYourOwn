//! Running one test: a fresh directory, the linker under test, and the deadline.
//!
//! There is no long-lived process to manage — a link is a subprocess and a linked program is
//! a subprocess — so the runner is mostly about isolation and time. Each test gets its own
//! directory under `$TMPDIR/linktest-<pid>/`, each *link* inside a test gets a subdirectory
//! of that, and everything is removed at the end unless `--keep-tmp` says otherwise.
//!
//! The deadline (`--timeout-ms`, raised by [`crate::stages::Test::min_timeout_ms`]) is
//! enforced where a linker can actually hang: on every subprocess, and again between them.
//! A test body that loops forever in Rust would not be caught — but that would be a bug in
//! the suite, not in the linker, and `cargo test` is where those are caught.

use crate::assert::{Failure, FailureKind};
use crate::config::LinkerDef;
use crate::linker::{reference, LinkerHandle};
use crate::stages::{Ctx, Stage, Test};
use anyhow::{Context, Result};
use serde::Serialize;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How a test ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Every check passed.
    Pass,
    /// At least one check failed, or the linker misbehaved.
    Fail,
    /// The test does not apply to this linker, or an optional tool was missing.
    Skip,
}

/// The outcome of one test.
#[derive(Debug, Clone)]
pub struct TestResult {
    /// Test name.
    pub name: String,
    /// Pass, fail or skip.
    pub status: Status,
    /// True when the test carries the `ext` tag.
    pub ext: bool,
    /// Wall-clock duration.
    pub duration_ms: u128,
    /// The failure, when the test failed.
    pub failure: Option<Failure>,
    /// Why it was skipped, when it was.
    pub skip_reason: Option<String>,
    /// Informational lines the test attached with `ctx.note(..)`.
    pub notes: Vec<String>,
}

impl TestResult {
    /// The failure messages, or an empty list.
    pub fn messages(&self) -> Vec<String> {
        self.failure
            .as_ref()
            .map(|f| f.messages.clone())
            .unwrap_or_default()
    }
}

/// Knobs that come from the command line.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// Keep every test's temporary directory.
    pub keep_tmp: bool,
    /// Default per-test deadline.
    pub timeout_ms: u64,
    /// Seed for every random choice.
    pub seed: u64,
    /// Print what the harness is doing between tests.
    pub verbose: bool,
}

/// Owns the resolved linker and the temporary directories of a run.
pub struct Runner {
    handle: LinkerHandle,
    reference: Option<LinkerHandle>,
    opts: RunOptions,
    tmp_root: PathBuf,
    keep: Vec<PathBuf>,
}

impl Runner {
    /// Resolve the linker and prepare the run's temporary root.
    pub fn new(def: LinkerDef, opts: RunOptions) -> Result<Runner> {
        let handle = LinkerHandle::resolve(&def)
            .with_context(|| format!("cannot use the linker '{}'", def.name))?;
        // The comparison leg. It is optional: a machine with no binutils still runs every
        // stage, the handful of comparison checks simply say they had nothing to compare to.
        let reference = reference::locate().ok().map(|program| LinkerHandle {
            def: LinkerDef {
                name: "gnu_ld".to_string(),
                kind: crate::config::LinkerKind::Reference,
                command: Vec::new(),
                cwd: None,
                env: Default::default(),
            },
            version: reference::version(&program),
            program,
            base_args: Vec::new(),
        });
        let tmp_root = std::env::temp_dir().join(format!("linktest-{}", std::process::id()));
        std::fs::create_dir_all(&tmp_root)
            .with_context(|| format!("cannot create {}", tmp_root.display()))?;
        Ok(Runner {
            handle,
            reference,
            opts,
            tmp_root,
            keep: Vec::new(),
        })
    }

    /// The linker being tested.
    pub fn handle(&self) -> &LinkerHandle {
        &self.handle
    }

    /// Temporary directories kept because of `--keep-tmp`.
    pub fn kept_dirs(&self) -> &[PathBuf] {
        &self.keep
    }

    /// Run one test end to end.
    pub fn run_test(&mut self, stage: &Stage, test: &Test, index: u64) -> TestResult {
        let started = Instant::now();
        if let Some(reason) = test.skip_reason(&self.handle.def.name) {
            return TestResult {
                name: test.name.to_string(),
                status: Status::Skip,
                ext: test.is_ext(),
                duration_ms: 0,
                failure: None,
                skip_reason: Some(reason.to_string()),
                notes: Vec::new(),
            };
        }
        let dir = self
            .tmp_root
            .join(format!("s{:02}", stage.number))
            .join(format!("t{index:03}"));
        let outcome = match std::fs::create_dir_all(&dir) {
            Ok(()) => None,
            Err(e) => Some(Failure::harness(format!(
                "cannot create the test directory {}: {e}",
                dir.display()
            ))),
        };
        let default = Duration::from_millis(self.opts.timeout_ms);
        let deadline = test.timeout(default);
        let mut ctx = Ctx::new(
            self.handle.clone(),
            self.reference.clone(),
            dir.clone(),
            default,
            deadline,
            self.opts.seed,
            index,
        );
        if self.opts.verbose {
            println!("      dir {}", dir.display());
        }
        let result = match outcome {
            Some(f) => Err(f),
            None => (test.run)(&mut ctx),
        };
        let status = match (&result, &ctx.skipped) {
            (Ok(()), Some(_)) => Status::Skip,
            (Ok(()), None) => Status::Pass,
            (Err(_), _) => Status::Fail,
        };
        if self.opts.keep_tmp {
            self.keep.push(dir);
        } else {
            let _ = std::fs::remove_dir_all(&dir);
        }
        TestResult {
            name: test.name.to_string(),
            status,
            ext: test.is_ext(),
            duration_ms: started.elapsed().as_millis(),
            failure: result.err(),
            skip_reason: ctx.skipped.clone(),
            notes: ctx.notes.clone(),
        }
    }

    /// Called once when the run is over.
    pub fn end_run(&mut self) {
        if !self.opts.keep_tmp {
            let _ = std::fs::remove_dir_all(&self.tmp_root);
        }
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        if !self.opts.keep_tmp {
            let _ = std::fs::remove_dir_all(&self.tmp_root);
        }
    }
}

/// A failure kind for a test that never even got to run its first link.
pub fn setup_failure(message: impl Into<String>) -> Failure {
    Failure::new(FailureKind::Harness, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_serializes_like_the_other_testers() {
        assert_eq!(
            serde_json::to_string(&Status::Pass).unwrap_or_default(),
            "\"pass\""
        );
        assert_eq!(
            serde_json::to_string(&Status::Fail).unwrap_or_default(),
            "\"fail\""
        );
        assert_eq!(
            serde_json::to_string(&Status::Skip).unwrap_or_default(),
            "\"skip\""
        );
    }
}
