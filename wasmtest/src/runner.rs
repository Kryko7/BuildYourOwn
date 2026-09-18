//! Running one test: a fresh temporary directory, the runtime handle, the deadline.

use crate::assert::Failure;
use crate::config::RuntimeDef;
use crate::runtime::RuntimeHandle;
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
    /// At least one check failed, or the runtime misbehaved.
    Fail,
    /// The test does not apply to this runtime.
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
    /// Informational lines the test attached with `ctx.note(..)`, shown whatever the outcome.
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
    /// Default per-test timeout.
    pub timeout_ms: u64,
    /// Seed for every random choice.
    pub seed: u64,
    /// Print what the harness is doing between invocations.
    pub verbose: bool,
}

/// Owns the runtime handle and the temporary directories of a run.
pub struct Runner {
    handle: RuntimeHandle,
    opts: RunOptions,
    tmp_root: PathBuf,
    instance: u32,
    keep: Vec<PathBuf>,
}

impl Runner {
    /// Prepare a runner; downloads the reference runtime if this is a reference definition.
    pub fn new(def: &RuntimeDef, opts: RunOptions) -> Result<Runner> {
        let handle = RuntimeHandle::prepare(def)?;
        let tmp_root = std::env::temp_dir().join(format!("wasmtest-{}", std::process::id()));
        std::fs::create_dir_all(&tmp_root)
            .with_context(|| format!("cannot create {}", tmp_root.display()))?;
        Ok(Runner {
            handle,
            opts,
            tmp_root,
            instance: 0,
            keep: Vec::new(),
        })
    }

    /// The runtime being tested.
    pub fn handle(&self) -> &RuntimeHandle {
        &self.handle
    }

    /// Temporary directories kept because of `--keep-tmp`.
    pub fn kept_dirs(&self) -> &[PathBuf] {
        &self.keep
    }

    fn next_tmp(&mut self, stage: u32) -> PathBuf {
        self.instance += 1;
        let dir = self
            .tmp_root
            .join(format!("s{stage:02}-t{:04}", self.instance));
        let _ = std::fs::create_dir_all(&dir);
        dir
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
        let tmp = self.next_tmp(stage.number);
        let deadline = test.timeout(Duration::from_millis(self.opts.timeout_ms));
        let mut ctx = Ctx::new(
            self.handle.clone(),
            tmp.clone(),
            deadline,
            self.opts.seed,
            index,
            self.opts.verbose,
        );
        let outcome = (test.run)(&mut ctx);
        let notes = std::mem::take(&mut ctx.notes);
        if self.opts.keep_tmp {
            self.keep.push(tmp);
        } else {
            let _ = std::fs::remove_dir_all(&tmp);
        }
        let failure = outcome.err();
        TestResult {
            name: test.name.to_string(),
            status: if failure.is_none() {
                Status::Pass
            } else {
                Status::Fail
            },
            ext: test.is_ext(),
            duration_ms: started.elapsed().as_millis(),
            failure,
            skip_reason: None,
            notes,
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
