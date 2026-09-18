//! Running one test: which reference a ladder uses, the node or cluster it needs, the
//! timeout it runs under, and the cleanup that happens whatever the outcome.

use crate::assert::{Failure, FailureKind};
use crate::cluster::Cluster;
use crate::config::{self, TargetDef, TargetKind};
use crate::etcd::Client;
use crate::node::{self, reference, NodeHandle};
use crate::stages::{Ctx, Ladder, Stage, Test};
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How a test ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Every check passed.
    Pass,
    /// At least one check failed, or the program misbehaved.
    Fail,
    /// The test does not apply to this target.
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
    /// Every tag, the ladder included.
    pub tags: Vec<String>,
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
    /// Default per-test timeout.
    pub timeout_ms: u64,
    /// Seed for every random choice.
    pub seed: u64,
    /// Print what the harness is doing between tests.
    pub verbose: bool,
}

/// Owns the processes and the temporary directories of a run.
pub struct Runner {
    targets: BTreeMap<String, TargetDef>,
    chosen: TargetDef,
    validate: bool,
    opts: RunOptions,
    tmp_root: PathBuf,
    instance: u32,
    dist: Option<PathBuf>,
    node: Option<NodeHandle>,
    client: Option<Client>,
    cluster: Option<Cluster>,
    cluster_shape: Option<(usize, usize)>,
    /// The scratch directory the live cluster's data lives in, which must outlive the test
    /// that created it because the next test may reuse the cluster.
    cluster_tmp: Option<PathBuf>,
    keep: Vec<PathBuf>,
    /// How many node processes have been started so far.
    pub starts: u32,
}

impl Runner {
    /// Prepare a runner. The reference distribution is only downloaded when a ladder that
    /// needs it is actually selected.
    pub fn new(
        targets: BTreeMap<String, TargetDef>,
        chosen: TargetDef,
        validate: bool,
        opts: RunOptions,
    ) -> Result<Runner> {
        let tmp_root = std::env::temp_dir().join(format!("disttest-{}", std::process::id()));
        std::fs::create_dir_all(&tmp_root)
            .with_context(|| format!("cannot create {}", tmp_root.display()))?;
        sweep_abandoned_scratch_dirs();
        Ok(Runner {
            targets,
            chosen,
            validate,
            opts,
            tmp_root,
            instance: 0,
            dist: None,
            node: None,
            client: None,
            cluster: None,
            cluster_shape: None,
            cluster_tmp: None,
            keep: Vec::new(),
            starts: 0,
        })
    }

    /// Which target a ladder runs against.
    ///
    /// Without `--validate` that is always the `--target` the command line named. With
    /// `--validate` it is the reference the *ladder* names, whatever `--target` said, so
    /// one command can prove the whole suite: the primitives ladder is checked against
    /// `reference_primitives` and the node and cluster ladders against real etcd.
    pub fn target_for(&self, ladder: Ladder) -> Result<TargetDef, Failure> {
        if self.validate {
            let want = config::reference_for(ladder);
            return self
                .targets
                .get(want)
                .cloned()
                .ok_or_else(|| Failure::harness(format!("targets.yaml has no '{want}'")));
        }
        Ok(self.chosen.clone())
    }

    /// The name shown in the run header and the JSON report.
    pub fn target_name(&self) -> &str {
        &self.chosen.name
    }

    /// Which targets `--validate` routed each ladder to, for the run header.
    pub fn routing(&self) -> Vec<(Ladder, String)> {
        Ladder::ALL
            .into_iter()
            .map(|l| {
                (
                    l,
                    self.target_for(l)
                        .map(|t| t.name)
                        .unwrap_or_else(|_| "?".into()),
                )
            })
            .collect()
    }

    fn ensure_dist(&mut self, def: &TargetDef) -> Result<Option<PathBuf>, Failure> {
        if def.kind != TargetKind::Reference {
            return Ok(None);
        }
        if self.dist.is_none() {
            let version = def
                .version
                .clone()
                .unwrap_or_else(|| reference::DEFAULT_VERSION.to_string());
            let dir = reference::ensure_installed(&version).map_err(|e| {
                Failure::harness(format!("cannot install the reference etcd: {e:#}"))
            })?;
            self.dist = Some(dir);
        }
        Ok(self.dist.clone())
    }

    fn next_tmp(&mut self) -> PathBuf {
        self.instance += 1;
        let dir = self.tmp_root.join(format!("i{:04}", self.instance));
        let _ = std::fs::create_dir_all(&dir);
        if self.opts.keep_tmp {
            self.keep.push(dir.clone());
        }
        dir
    }

    /// Delete one test's scratch directory, unless `--keep-tmp` asked for it.
    ///
    /// This matters more than it looks: a node preallocates a 64 MB write-ahead log the
    /// moment it starts, and a full run starts several hundred of them. Waiting until the
    /// end of the run to clean up fills `/tmp` and the nodes start failing to boot, which
    /// looks like a bug in the suite and is really a bug in the harness's housekeeping.
    fn release_tmp(&self, dir: &std::path::Path) {
        if self.opts.keep_tmp {
            return;
        }
        if self.cluster_tmp.as_deref() == Some(dir) {
            return; // still in use by the cluster the next test may reuse
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Temporary directories kept because of `--keep-tmp`.
    pub fn kept_dirs(&self) -> &[PathBuf] {
        &self.keep
    }

    /// Called before a stage's first test: a new stage never inherits a cluster.
    pub async fn begin_stage(&mut self, stage: &Stage) {
        let _ = stage;
        self.drop_node();
        self.drop_cluster();
    }

    /// Called after the last test of the run.
    pub fn end_run(&mut self) {
        self.drop_node();
        self.drop_cluster();
    }

    fn drop_node(&mut self) {
        if let Some(mut n) = self.node.take() {
            n.stop();
        }
        self.client = None;
    }

    fn drop_cluster(&mut self) {
        if let Some(mut c) = self.cluster.take() {
            c.shutdown();
        }
        self.cluster_shape = None;
        if let Some(dir) = self.cluster_tmp.take() {
            if !self.opts.keep_tmp {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
    }

    /// The program's output for a report block.
    pub fn output(&self) -> Option<String> {
        if let Some(n) = &self.node {
            return Some(n.output_tail(15));
        }
        self.cluster.as_ref().map(|c| c.output())
    }

    /// Run one test end to end.
    pub async fn run_test(&mut self, stage: &Stage, test: &Test, index: u64) -> TestResult {
        let started = Instant::now();
        let tags = test.all_tags(stage.ladder);
        let skipped = |reason: String| TestResult {
            name: test.name.to_string(),
            status: Status::Skip,
            ext: test.is_ext(),
            tags: tags.clone(),
            duration_ms: 0,
            failure: None,
            skip_reason: Some(reason),
            notes: Vec::new(),
        };
        if let Some(reason) = test.skip_reason(&self.chosen.name) {
            return skipped(reason.to_string());
        }
        let def = match self.target_for(stage.ladder) {
            Ok(d) => d,
            Err(f) => return self.failed(test, tags, started, f),
        };
        if !def.serves(stage.ladder) {
            return skipped(format!(
                "target '{}' does not serve the {} ladder",
                def.name,
                stage.ladder.as_str()
            ));
        }
        let mut notes = Vec::new();
        let result = self
            .prepare_and_run(stage, test, &def, index, &mut notes)
            .await;
        let mut failure = result.err();
        // A node that died turns any other failure into the crash that caused it.
        if let Some(n) = self.node.as_mut() {
            if let Some(crash) = n.crash_failure() {
                failure = Some(crash);
                self.drop_node();
            }
        }
        let status = if failure.is_none() {
            Status::Pass
        } else {
            Status::Fail
        };
        if status == Status::Fail {
            // Never hand a suspect cluster or node to the next test.
            self.drop_node();
            self.drop_cluster();
        }
        TestResult {
            name: test.name.to_string(),
            status,
            ext: test.is_ext(),
            tags,
            duration_ms: started.elapsed().as_millis(),
            failure,
            skip_reason: None,
            notes,
        }
    }

    fn failed(
        &self,
        test: &Test,
        tags: Vec<String>,
        started: Instant,
        failure: Failure,
    ) -> TestResult {
        TestResult {
            name: test.name.to_string(),
            status: Status::Fail,
            ext: test.is_ext(),
            tags,
            duration_ms: started.elapsed().as_millis(),
            failure: Some(failure),
            skip_reason: None,
            notes: Vec::new(),
        }
    }

    async fn prepare_and_run(
        &mut self,
        stage: &Stage,
        test: &Test,
        def: &TargetDef,
        index: u64,
        notes: &mut Vec<String>,
    ) -> Result<(), Failure> {
        let dist = self.ensure_dist(def)?;
        let tmp = self.next_tmp();
        let timeout = Duration::from_millis(self.opts.timeout_ms);
        let argv = self.program_argv(def)?;
        let cwd = def
            .cwd
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let env: Vec<(String, String)> = def
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let mut ctx = Ctx::new(
            &def.name,
            stage.ladder,
            self.opts.seed,
            index,
            timeout,
            tmp.clone(),
            argv,
            cwd,
            env,
        );

        match stage.ladder {
            Ladder::Primitives => {}
            Ladder::Node => {
                self.start_single_node(def, dist.as_deref(), &tmp, timeout)?;
                ctx.node = self.node.take();
                ctx.client = self.client.take();
            }
            Ladder::Cluster => {
                let size = if test.cluster_size == 0 {
                    stage.default_cluster_size()
                } else {
                    test.cluster_size
                };
                self.ensure_cluster(
                    def,
                    dist.as_deref(),
                    &tmp,
                    size,
                    test.spare,
                    test.fresh,
                    timeout,
                )
                .await?;
                ctx.cluster = self.cluster.take();
            }
        }
        if self.opts.verbose {
            println!(
                "      {} {} in {}",
                def.name,
                stage.ladder.as_str(),
                tmp.display()
            );
        }

        let deadline = test.timeout(timeout);
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

        if let Some(mut p) = ctx.prim_proc.take() {
            p.close().await;
        }
        self.node = ctx.node.take();
        self.client = ctx.client.take();
        self.cluster = ctx.cluster.take();
        notes.append(&mut ctx.notes);

        // A node is never reused; a cluster is, unless the test dirtied it.
        if stage.ladder == Ladder::Node {
            // A node that died during a test that otherwise passed is still a failure: the
            // durability stages are the only ones allowed to leave it stopped, and they do
            // that by killing it themselves and starting it again.
            let crash = self.node.as_mut().and_then(NodeHandle::crash_failure);
            self.drop_node();
            if outcome.is_ok() {
                if let Some(c) = crash {
                    return Err(c);
                }
            }
        }
        if stage.ladder == Ladder::Cluster {
            let dirty = self.cluster.as_mut().is_some_and(Cluster::is_dirty);
            if dirty {
                self.drop_cluster();
            }
        }
        self.release_tmp(&tmp);
        outcome
    }

    /// The argv the program under test is started with, before any ladder arguments.
    fn program_argv(&self, def: &TargetDef) -> Result<Vec<String>, Failure> {
        match def.kind {
            TargetKind::External => Ok(def.command.clone()),
            TargetKind::Example => Ok(vec![reference::example_binary(&def.name)
                .map_err(|e| Failure::harness(format!("{e:#}")))?
                .to_string_lossy()
                .to_string()]),
            TargetKind::Reference => {
                let dist = self
                    .dist
                    .clone()
                    .ok_or_else(|| Failure::harness("the reference is not installed"))?;
                Ok(vec![dist.join("etcd").to_string_lossy().to_string()])
            }
        }
    }

    fn start_single_node(
        &mut self,
        def: &TargetDef,
        dist: Option<&std::path::Path>,
        tmp: &std::path::Path,
        timeout: Duration,
    ) -> Result<(), Failure> {
        self.drop_node();
        let ports = node::free_ports(2)
            .map_err(|e| Failure::harness(format!("cannot allocate ports: {e:#}")))?;
        let (client_port, peer_port) = (ports[0], ports[1]);
        let data_dir = tmp.join("m1").join("data");
        let peer_url = format!("http://127.0.0.1:{peer_port}");
        let spec = node::spec_for(
            def,
            dist,
            "m1",
            &tmp.join("m1"),
            &data_dir,
            client_port,
            peer_port,
            &format!("m1={peer_url}"),
            &peer_url,
        )
        .map_err(|e| Failure::harness(format!("cannot describe the node: {e:#}")))?;
        let handle = NodeHandle::start(spec)
            .map_err(|e| Failure::harness(format!("cannot start the node: {e:#}")))?;
        self.starts += 1;
        let client = Client::new(&handle.spec.client_url(), "m1", timeout)
            .map_err(|e| Failure::harness(format!("cannot build a client: {e}")))?;
        self.node = Some(handle);
        self.client = Some(client);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn ensure_cluster(
        &mut self,
        def: &TargetDef,
        dist: Option<&std::path::Path>,
        tmp: &std::path::Path,
        size: usize,
        spare: usize,
        fresh: bool,
        timeout: Duration,
    ) -> Result<(), Failure> {
        let want = (size, spare);
        let reusable = !fresh
            && self.cluster_shape == Some(want)
            && self.cluster.as_mut().is_some_and(|c| !c.is_dirty());
        if reusable {
            return Ok(());
        }
        self.drop_cluster();
        let cluster = Cluster::start(def, dist, tmp, size, spare, timeout, self.opts.seed).await?;
        self.starts += size as u32;
        self.cluster = Some(cluster);
        self.cluster_shape = Some(want);
        self.cluster_tmp = Some(tmp.to_path_buf());
        Ok(())
    }
}

/// Delete the scratch directories of `disttest` runs that are no longer running.
///
/// A run that was killed with `SIGKILL` (or that ran out of `/tmp` and gave up) leaves its
/// nodes' data directories behind, and each of those holds a preallocated 64 MB write-ahead
/// log. Sweeping them at the start of the next run keeps one bad afternoon from making the
/// machine unusable.
fn sweep_abandoned_scratch_dirs() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_string_lossy()
            .strip_prefix("disttest-")
            .and_then(|p| p.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == std::process::id() {
            continue;
        }
        // `/proc/<pid>` is the cheapest "is anyone still using this" there is.
        if std::path::Path::new(&format!("/proc/{pid}")).exists() {
            continue;
        }
        let _ = std::fs::remove_dir_all(entry.path());
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.drop_node();
        self.drop_cluster();
        if !self.opts.keep_tmp {
            let _ = std::fs::remove_dir_all(&self.tmp_root);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets() -> BTreeMap<String, TargetDef> {
        let mut m = BTreeMap::new();
        let mut etcd = TargetDef::example("etcd");
        etcd.kind = TargetKind::Reference;
        etcd.version = Some("3.7.1".into());
        m.insert("etcd".to_string(), etcd);
        m.insert(
            "reference_primitives".to_string(),
            TargetDef::example("reference_primitives"),
        );
        let mut mine = TargetDef::example("my_node");
        mine.kind = TargetKind::External;
        mine.command = vec!["./your_program.sh".into()];
        m.insert("my_node".to_string(), mine);
        m
    }

    fn runner(validate: bool, chosen: &str) -> Runner {
        let t = targets();
        Runner::new(
            t.clone(),
            t[chosen].clone(),
            validate,
            RunOptions {
                keep_tmp: false,
                timeout_ms: 1000,
                seed: 1,
                verbose: false,
            },
        )
        .expect("runner")
    }

    #[test]
    fn status_serializes_like_the_other_testers() {
        assert_eq!(
            serde_json::to_string(&Status::Pass).unwrap_or_default(),
            "\"pass\""
        );
        assert_eq!(
            serde_json::to_string(&Status::Skip).unwrap_or_default(),
            "\"skip\""
        );
    }

    #[test]
    fn validate_routes_each_ladder_to_its_own_reference() {
        let r = runner(true, "etcd");
        assert_eq!(
            r.target_for(Ladder::Primitives).expect("target").name,
            "reference_primitives"
        );
        assert_eq!(r.target_for(Ladder::Node).expect("target").name, "etcd");
        assert_eq!(r.target_for(Ladder::Cluster).expect("target").name, "etcd");
        // The same is true when --target names the primitives reference.
        let r = runner(true, "reference_primitives");
        assert_eq!(r.target_for(Ladder::Node).expect("target").name, "etcd");
        assert_eq!(
            r.routing()
                .iter()
                .map(|(_, n)| n.clone())
                .collect::<Vec<_>>(),
            vec!["reference_primitives", "etcd", "etcd"]
        );
    }

    #[test]
    fn without_validate_every_ladder_uses_the_named_target() {
        let r = runner(false, "my_node");
        for l in Ladder::ALL {
            assert_eq!(r.target_for(l).expect("target").name, "my_node");
        }
        assert_eq!(r.target_name(), "my_node");
    }
}
