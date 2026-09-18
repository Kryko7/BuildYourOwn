//! Running one test: broker lifecycle, fixtures, timeout, crash detection.

use crate::assert::{Failure, FailureKind};
use crate::broker::{self, reference, BrokerHandle};
use crate::config::{BrokerDef, BrokerKind, FixtureStrategy, RestartPolicy};
use crate::fixtures::{self, FixtureHandle, FixtureSpec};
use crate::proto::meta::uuid_to_kafka_string;
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
    /// At least one check failed, or the broker misbehaved.
    Fail,
    /// The test does not apply to this broker.
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
    /// Force this port instead of the one in `brokers.yaml`.
    pub port: Option<u16>,
    /// Force this log directory instead of `{TMP}/kraft-combined-logs`.
    pub log_dir: Option<PathBuf>,
    /// Print what the harness is doing between tests.
    pub verbose: bool,
}

/// Owns the broker process and the temporary directories of a run.
pub struct Runner {
    def: BrokerDef,
    opts: RunOptions,
    broker: Option<BrokerHandle>,
    tmp_root: PathBuf,
    instance: u32,
    dist: Option<PathBuf>,
    cluster_id: String,
    keep: Vec<PathBuf>,
    /// Number of broker processes started so far (reported at the end of a run).
    pub starts: u32,
}

impl Runner {
    /// Prepare a runner; downloads the reference distribution if this is a reference broker.
    pub fn new(def: BrokerDef, opts: RunOptions) -> Result<Runner> {
        let dist = if def.kind == BrokerKind::Reference {
            let version = def
                .version
                .clone()
                .unwrap_or_else(|| reference::DEFAULT_VERSION.to_string());
            Some(reference::ensure_installed(&version)?)
        } else {
            None
        };
        let cluster_id = match &dist {
            Some(d) => reference::cluster_id(d)?,
            None => uuid_to_kafka_string(&fixtures::derive_topic_id(opts.seed, "cluster")),
        };
        let tmp_root = std::env::temp_dir().join(format!("kafkatest-{}", std::process::id()));
        std::fs::create_dir_all(&tmp_root)
            .with_context(|| format!("cannot create {}", tmp_root.display()))?;
        Ok(Runner {
            def,
            opts,
            broker: None,
            tmp_root,
            instance: 0,
            dist,
            cluster_id,
            keep: Vec::new(),
            starts: 0,
        })
    }

    /// The broker definition being tested.
    pub fn def(&self) -> &BrokerDef {
        &self.def
    }

    /// Temporary directories kept because of `--keep-tmp`.
    pub fn kept_dirs(&self) -> &[PathBuf] {
        &self.keep
    }

    /// Called before a stage's first test.
    pub async fn begin_stage(&mut self, _stage: &Stage) -> Result<(), Failure> {
        if self.def.effective_restart() == RestartPolicy::PerStage {
            self.restart(&FixtureSpec::none(), "stage").await?;
        }
        Ok(())
    }

    /// Start (or restart) the broker for a run that is not a stage test, and return the
    /// address it listens on. `--capture-examples` uses it.
    pub async fn boot(&mut self) -> Result<std::net::SocketAddr, Failure> {
        self.restart(&FixtureSpec::none(), "examples").await?;
        Ok(self.broker_mut()?.addr)
    }

    /// Where the running broker listens, if one is running.
    pub fn addr(&self) -> Option<std::net::SocketAddr> {
        self.broker.as_ref().map(|b| b.addr)
    }

    /// Put a fixture spec in place against the broker, whichever strategy it uses.
    ///
    /// The `api` strategy creates the topics on the running process; the `files` strategy
    /// has to write them before the broker starts, so it restarts it.
    pub async fn materialize(
        &mut self,
        spec: &FixtureSpec,
        salt: &str,
    ) -> Result<FixtureHandle, Failure> {
        if self.def.fixtures == FixtureStrategy::Files || self.broker.is_none() {
            return self.restart(spec, salt).await;
        }
        let addr = self.broker_mut()?.addr;
        let log_dir = self.broker_mut()?.spec.log_dir.clone();
        let mut conn =
            crate::proto::Conn::connect(addr, Duration::from_millis(self.opts.timeout_ms))
                .await
                .map_err(|e| {
                    Failure::proto(e, None).note("connecting to create example fixtures")
                })?;
        fixtures::api::materialize(&mut conn, spec, salt, &log_dir).await
    }

    /// Called after a stage's last test when the broker is per-stage.
    pub fn end_run(&mut self) {
        if let Some(mut b) = self.broker.take() {
            b.stop();
        }
    }

    fn next_tmp(&mut self) -> PathBuf {
        self.instance += 1;
        let dir = self.tmp_root.join(format!("i{:04}", self.instance));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    /// Stop the current broker, materialize `files` fixtures, and start a fresh one.
    async fn restart(&mut self, spec: &FixtureSpec, salt: &str) -> Result<FixtureHandle, Failure> {
        if let Some(mut b) = self.broker.take() {
            b.stop();
        }
        let tmp = self.next_tmp();
        let handle = match self.def.kind {
            BrokerKind::Reference => {
                let version = self
                    .def
                    .version
                    .clone()
                    .unwrap_or_else(|| reference::DEFAULT_VERSION.to_string());
                let spec_b = reference::prepare(
                    &version,
                    &tmp,
                    Duration::from_millis(self.def.boot_timeout_ms),
                )
                .map_err(|e| {
                    Failure::harness(format!("cannot prepare the reference broker: {e:#}"))
                })?;
                let log_dir = spec_b.log_dir.clone();
                let b =
                    BrokerHandle::start(spec_b).map_err(|e| Failure::harness(format!("{e:#}")))?;
                self.starts += 1;
                reference::wait_for_marker(
                    &tmp,
                    reference::READY_MARKER,
                    Duration::from_millis(self.def.boot_timeout_ms),
                )
                .map_err(|e| Failure::harness(format!("{e:#}")))?;
                self.broker = Some(b);
                FixtureHandle {
                    log_dir,
                    broker_id: 1,
                    ..Default::default()
                }
            }
            BrokerKind::External => {
                let port = match self.opts.port.or(self.def.port) {
                    Some(p) if p != 0 => p,
                    _ => broker::free_port()
                        .map_err(|e| Failure::harness(format!("cannot pick a port: {e:#}")))?,
                };
                let mut spec_b = broker::external_spec(&self.def, &tmp, port)
                    .map_err(|e| Failure::harness(format!("{e:#}")))?;
                if let Some(d) = &self.opts.log_dir {
                    spec_b.log_dir = d.clone();
                }
                broker::write_external_properties(&spec_b.props, &spec_b.log_dir, port)
                    .map_err(|e| Failure::harness(format!("{e:#}")))?;
                let handle = if self.def.fixtures == FixtureStrategy::Files {
                    fixtures::files::materialize(
                        spec,
                        salt,
                        self.opts.seed,
                        &spec_b.log_dir,
                        1,
                        &self.cluster_id,
                    )
                    .map_err(|e| Failure::harness(format!("cannot write fixtures: {e:#}")))?
                } else {
                    FixtureHandle {
                        log_dir: spec_b.log_dir.clone(),
                        broker_id: 1,
                        ..Default::default()
                    }
                };
                let b =
                    BrokerHandle::start(spec_b).map_err(|e| Failure::harness(format!("{e:#}")))?;
                self.starts += 1;
                self.broker = Some(b);
                handle
            }
        };
        if self.opts.keep_tmp {
            self.keep.push(tmp);
        }
        Ok(handle)
    }

    fn broker_mut(&mut self) -> Result<&mut BrokerHandle, Failure> {
        self.broker
            .as_mut()
            .ok_or_else(|| Failure::harness("no broker is running"))
    }

    /// Run one test end to end.
    pub async fn run_test(&mut self, stage: &Stage, test: &Test, index: u64) -> TestResult {
        let started = Instant::now();
        if let Some(reason) = test.skip_reason(&self.def.name) {
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
        let spec = (test.fixtures)();
        let salt = format!("s{:02}x{index}", stage.number);
        let mut notes = Vec::new();
        let result = self
            .prepare_and_run(stage, test, &spec, &salt, index, &mut notes)
            .await;
        let mut failure = result.err();
        // A broker that died turns any other failure into the crash that caused it.
        if let Ok(b) = self.broker_mut() {
            if let Some(crash) = b.crash_failure() {
                failure = Some(crash);
                if let Some(mut b) = self.broker.take() {
                    b.stop();
                }
            }
        }
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

    async fn prepare_and_run(
        &mut self,
        stage: &Stage,
        test: &Test,
        spec: &FixtureSpec,
        salt: &str,
        index: u64,
        notes: &mut Vec<String>,
    ) -> Result<(), Failure> {
        let per_test = self.def.effective_restart() == RestartPolicy::PerTest;
        let needs_restart = per_test || test.force_restart || self.broker.is_none();
        let mut handle = if needs_restart {
            self.restart(spec, salt).await?
        } else {
            let log_dir = self.broker_mut()?.spec.log_dir.clone();
            FixtureHandle {
                log_dir,
                broker_id: 1,
                ..Default::default()
            }
        };
        if self.def.fixtures == FixtureStrategy::Api && !spec.is_empty() {
            let addr = self.broker_mut()?.addr;
            let mut conn =
                crate::proto::Conn::connect(addr, Duration::from_millis(self.opts.timeout_ms))
                    .await
                    .map_err(|e| Failure::proto(e, None).note("connecting to create fixtures"))?;
            handle = fixtures::api::materialize(&mut conn, spec, salt, &handle.log_dir).await?;
        }
        if self.opts.verbose && !spec.is_empty() {
            let names: Vec<String> = handle
                .all()
                .map(|t| format!("{} ({})", t.name, t.id))
                .collect();
            println!("      fixtures: {}", names.join(", "));
        }
        let timeout = Duration::from_millis(self.opts.timeout_ms);
        let seed = self.opts.seed;
        let dist = self.dist.clone();
        let broker = self.broker_mut()?;
        let mut ctx = Ctx::new(broker, handle, timeout, seed, index, dist);
        let _ = stage;
        // `ctx.timeout` stays the per-request timeout; the deadline for the whole body is
        // the test's own: an explicit `timeout_ms` override, or `--timeout-ms` raised to a
        // slow stage's `min_timeout_ms` floor.
        let deadline = test.timeout(timeout);
        // Lend the broker process to the test so `Ctx::restart_broker` can replace it, and
        // take back whatever is running when the body returns (or times out).
        ctx.broker = self.broker.take();
        let outcome = match tokio::time::timeout(deadline, (test.run)(&mut ctx)).await {
            Ok(r) => r,
            Err(_) => Err(Failure::new(
                FailureKind::Timeout,
                format!(
                    "the test did not finish within {} ms (--timeout-ms)",
                    deadline.as_millis()
                ),
            )),
        };
        self.broker = ctx.broker.take();
        notes.append(&mut ctx.notes);
        outcome
    }

    /// Broker output for a report block.
    pub fn broker_output(&self) -> Option<String> {
        self.broker.as_ref().map(|b| b.output_tail(20))
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        if let Some(mut b) = self.broker.take() {
            b.stop();
        }
        if !self.opts.keep_tmp {
            let _ = std::fs::remove_dir_all(&self.tmp_root);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_serializes_like_shelltest() {
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
