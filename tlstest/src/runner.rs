//! Running one test: server lifecycle, certificates, timeout, crash detection.

use crate::assert::Failure;
use crate::certs::CertStore;
use crate::config::{RestartPolicy, ServerDef, ServerKind, ServerOptions};
use crate::server::{self, reference, ServerHandle};
use crate::stages::{Ctx, Stage, Test};
use anyhow::{Context, Result};
use serde::Serialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How a test ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Every check passed.
    Pass,
    /// At least one check failed, or the server misbehaved.
    Fail,
    /// The test does not apply to this server.
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
    /// Keep every server instance's temporary directory.
    pub keep_tmp: bool,
    /// Default per-test timeout.
    pub timeout_ms: u64,
    /// Seed for every random choice.
    pub seed: u64,
    /// Print what the harness is doing between tests.
    pub verbose: bool,
}

/// Owns the server process, the certificates and the temporary directories of a run.
pub struct Runner {
    def: ServerDef,
    opts: RunOptions,
    server: Option<ServerHandle>,
    tmp_root: PathBuf,
    instance: u32,
    certs: Arc<CertStore>,
    openssl: Option<PathBuf>,
    keep: Vec<PathBuf>,
    /// Number of server processes started so far.
    pub starts: u32,
}

impl Runner {
    /// Prepare a runner and generate the default certificate.
    pub fn new(def: ServerDef, opts: RunOptions) -> Result<Runner> {
        let tmp_root = std::env::temp_dir().join(format!("tlstest-{}", std::process::id()));
        std::fs::create_dir_all(&tmp_root)
            .with_context(|| format!("cannot create {}", tmp_root.display()))?;
        let certs = Arc::new(CertStore::new(&tmp_root.join("certs"))?);
        certs
            .default_material()
            .context("cannot generate the default certificate")?;
        let openssl = reference::openssl_path().ok();
        if def.kind == ServerKind::Reference && openssl.is_none() {
            anyhow::bail!(
                "the reference server needs `openssl` on PATH (or {} set)",
                reference::OPENSSL_ENV
            );
        }
        Ok(Runner {
            def,
            opts,
            server: None,
            tmp_root,
            instance: 0,
            certs,
            openssl,
            keep: Vec::new(),
            starts: 0,
        })
    }

    /// The server definition being tested.
    pub fn def(&self) -> &ServerDef {
        &self.def
    }

    /// The certificate store, so `--capture-examples` can reach it.
    pub fn certs(&self) -> Arc<CertStore> {
        Arc::clone(&self.certs)
    }

    /// Temporary directories kept because of `--keep-tmp`.
    pub fn kept_dirs(&self) -> &[PathBuf] {
        &self.keep
    }

    /// Where the running server listens, if one is running.
    pub fn addr(&self) -> Option<SocketAddr> {
        self.server.as_ref().map(|s| s.addr)
    }

    /// Called before a stage's first test.
    pub async fn begin_stage(&mut self, _stage: &Stage) -> Result<(), Failure> {
        if self.def.restart == RestartPolicy::PerStage {
            self.restart(&ServerOptions::default())?;
        }
        Ok(())
    }

    /// Start a server for a run that is not a stage test; `--capture-examples` uses it.
    pub fn boot(&mut self, options: &ServerOptions) -> Result<SocketAddr, Failure> {
        self.restart(options)?;
        self.server
            .as_ref()
            .map(|s| s.addr)
            .ok_or_else(|| Failure::harness("no server is running"))
    }

    /// Stop whatever is running.
    pub fn end_run(&mut self) {
        if let Some(mut s) = self.server.take() {
            s.stop();
        }
    }

    fn next_tmp(&mut self) -> PathBuf {
        self.instance += 1;
        let dir = self.tmp_root.join(format!("i{:04}", self.instance));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    /// Stop the current server and start a fresh one with these options.
    fn restart(&mut self, options: &ServerOptions) -> Result<(), Failure> {
        if let Some(mut s) = self.server.take() {
            s.stop();
        }
        let tmp = self.next_tmp();
        let material = self
            .certs
            .get(options.cert)
            .map_err(|e| Failure::harness(format!("cannot generate a certificate: {e:#}")))?;
        let port = server::free_port()
            .map_err(|e| Failure::harness(format!("cannot pick a free port: {e:#}")))?;
        let spec = server::build_spec(
            &self.def,
            &tmp,
            port,
            &material,
            options,
            Duration::from_millis(self.def.boot_timeout_ms),
        )
        .map_err(|e| Failure::harness(format!("{e:#}")))?;
        if self.opts.verbose {
            println!("      starting: {}", spec.command_line());
        }
        let handle = ServerHandle::start(spec).map_err(|e| Failure::harness(format!("{e:#}")))?;
        self.starts += 1;
        self.server = Some(handle);
        if self.opts.keep_tmp {
            self.keep.push(tmp);
        }
        Ok(())
    }

    /// Run one test end to end.
    pub async fn run_test(&mut self, _stage: &Stage, test: &Test, index: u64) -> TestResult {
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
        let mut notes = Vec::new();
        let result = self.prepare_and_run(test, index, &mut notes).await;
        let mut failure = result.err();
        // A server that died turns any other failure into the crash that caused it.
        if let Some(s) = self.server.as_mut() {
            if let Some(crash) = s.crash_failure() {
                let expected_exit = s.spec.options.naccept.is_some();
                if !expected_exit || failure.is_some() {
                    failure = Some(crash);
                }
                if let Some(mut s) = self.server.take() {
                    s.stop();
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
        test: &Test,
        index: u64,
        notes: &mut Vec<String>,
    ) -> Result<(), Failure> {
        let options = (test.server_options)();
        let per_test = self.def.restart == RestartPolicy::PerTest;
        let differs = self
            .server
            .as_ref()
            .map(|s| s.spec.options != options)
            .unwrap_or(true);
        if per_test || test.force_restart || differs || self.server.is_none() {
            self.restart(&options)?;
        }
        let material = self
            .certs
            .get(options.cert)
            .map_err(|e| Failure::harness(format!("cannot generate a certificate: {e:#}")))?;
        let timeout = Duration::from_millis(self.opts.timeout_ms);
        let handle = self
            .server
            .as_ref()
            .ok_or_else(|| Failure::harness("no server is running"))?;
        let mut ctx = Ctx::new(
            handle,
            self.def.kind == ServerKind::Reference,
            material,
            Arc::clone(&self.certs),
            timeout,
            self.opts.seed,
            index,
            self.openssl.clone(),
        );
        let deadline = test.timeout(timeout);
        // Lend the server process to the test so it can watch the process exit.
        ctx.server = self.server.take();
        let outcome = match tokio::time::timeout(deadline, (test.run)(&mut ctx)).await {
            Ok(r) => r,
            Err(_) => Err(Failure::new(
                crate::assert::FailureKind::Timeout,
                format!(
                    "the test did not finish within {} ms (--timeout-ms)",
                    deadline.as_millis()
                ),
            )),
        };
        self.server = ctx.server.take();
        notes.append(&mut ctx.notes);
        outcome
    }

    /// The server's output for a report block.
    pub fn server_output(&self) -> Option<String> {
        self.server.as_ref().map(|s| s.output_tail(20))
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        if let Some(mut s) = self.server.take() {
            s.stop();
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
