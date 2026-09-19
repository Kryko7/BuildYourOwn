//! The stage registry: what a stage and a test are, which ladder each belongs to, and the
//! context a test body runs in.
//!
//! Adding a stage means writing `src/stages/sNN_slug.rs` with a `pub fn stage() -> Stage`
//! and adding two lines here (the `mod` and the `push`). Nothing else in the harness
//! changes, which is what lets several people work on different stages at once. See
//! README.md, "Adding a stage".

use crate::assert::Failure;
use crate::cluster::Cluster;
use crate::etcd::Client;
use crate::node::NodeHandle;
use crate::prim::PrimProc;
use rand::rngs::StdRng;
use rand::SeedableRng;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

mod helpers;
pub use helpers::*;

// ---------------------------------------------------------------------------------------
// Stage modules. Keep them in numeric order.
// ---------------------------------------------------------------------------------------
mod s01_lamport_clocks;
mod s02_vector_clocks;
mod s03_version_vectors;
mod s04_hybrid_logical_clocks;
mod s05_consistent_hashing;
mod s06_ring_key_movement;
mod s07_rendezvous_hashing;
mod s08_quorum_math;
mod s09_sloppy_quorums;
mod s10_bloom_filter;
mod s11_hyperloglog;
mod s12_merkle_tree;
mod s13_merkle_diff;
mod s14_counters;
mod s15_registers_and_sets;
mod s16_rga_sequence;
mod s17_chandy_lamport;
mod s18_causal_broadcast;
mod s19_rate_limiting;
mod s20_failure_detection;
mod s21_bind_version_health;
mod s22_put_header;
mod s23_range_one_key;
mod s24_range_over_ranges;
mod s25_range_options;
mod s26_mvcc_past_revisions;
mod s27_prev_kv_and_delete;
mod s28_txn_branches;
mod s29_txn_compare_and_swap;
mod s30_compaction;
mod s31_leases;
mod s32_lease_keepalive;
mod s33_watches;
mod s34_error_responses;
mod s35_durability;
mod s36_cluster_forms;
mod s37_replicated_writes;
mod s38_linearizable_reads;
mod s39_revision_agreement;
mod s40_minority_refuses_writes;
mod s41_majority_keeps_serving;
mod s42_partition_heals;
mod s43_leader_election;
mod s44_no_acknowledged_write_lost;
mod s45_old_leader_rejoins;
mod s46_follower_catch_up;
mod s47_snapshot_catch_up;
mod s48_member_add;
mod s49_member_remove;
mod s50_whole_cluster_restart;
mod s51_clock_skew;
mod s52_lossy_link;
mod s53_duplicate_and_reorder;
mod s54_linearizability;
mod s55_five_node_soak;
mod s56_raft_leader_election;
mod s57_raft_log_replication;
mod s58_raft_commit_safety;
mod s59_raft_snapshots;
mod s60_raft_membership;
mod s61_paxos_single_decree;
mod s62_multi_paxos;
mod s63_two_phase_commit;
mod s64_three_phase_commit;
mod s65_commit_recovery;
mod s66_saga_orchestrated;
mod s67_saga_choreographed;
mod s68_transactional_outbox;
mod s69_idempotent_consumer;
mod s70_idempotency_keys;
mod s71_fencing_tokens;
mod s72_leader_leases;
mod s73_anti_entropy;
mod s74_gossip_dissemination;
mod s75_circuit_breaker;
mod s76_hedged_requests;
mod s77_bulkheads;
mod s78_consistency_models;
mod s79_session_guarantees;
mod s80_abd_register;
mod s81_chain_replication;
mod s82_raft_reads_and_prevote;
mod s83_reliable_broadcast;
mod s84_ordered_broadcast;
mod s85_failure_detectors;
mod s86_byzantine_quorums;
mod s87_commit_wait;
mod s88_snapshot_isolation;

/// Every implemented stage, in ascending order.
pub fn all() -> Vec<Stage> {
    let mut v = vec![
        s01_lamport_clocks::stage(),
        s02_vector_clocks::stage(),
        s03_version_vectors::stage(),
        s04_hybrid_logical_clocks::stage(),
        s05_consistent_hashing::stage(),
        s06_ring_key_movement::stage(),
        s07_rendezvous_hashing::stage(),
        s08_quorum_math::stage(),
        s09_sloppy_quorums::stage(),
        s10_bloom_filter::stage(),
        s11_hyperloglog::stage(),
        s12_merkle_tree::stage(),
        s13_merkle_diff::stage(),
        s14_counters::stage(),
        s15_registers_and_sets::stage(),
        s16_rga_sequence::stage(),
        s17_chandy_lamport::stage(),
        s18_causal_broadcast::stage(),
        s19_rate_limiting::stage(),
        s20_failure_detection::stage(),
        s21_bind_version_health::stage(),
        s22_put_header::stage(),
        s23_range_one_key::stage(),
        s24_range_over_ranges::stage(),
        s25_range_options::stage(),
        s26_mvcc_past_revisions::stage(),
        s27_prev_kv_and_delete::stage(),
        s28_txn_branches::stage(),
        s29_txn_compare_and_swap::stage(),
        s30_compaction::stage(),
        s31_leases::stage(),
        s32_lease_keepalive::stage(),
        s33_watches::stage(),
        s34_error_responses::stage(),
        s35_durability::stage(),
        s36_cluster_forms::stage(),
        s37_replicated_writes::stage(),
        s38_linearizable_reads::stage(),
        s39_revision_agreement::stage(),
        s40_minority_refuses_writes::stage(),
        s41_majority_keeps_serving::stage(),
        s42_partition_heals::stage(),
        s43_leader_election::stage(),
        s44_no_acknowledged_write_lost::stage(),
        s45_old_leader_rejoins::stage(),
        s46_follower_catch_up::stage(),
        s47_snapshot_catch_up::stage(),
        s48_member_add::stage(),
        s49_member_remove::stage(),
        s50_whole_cluster_restart::stage(),
        s51_clock_skew::stage(),
        s52_lossy_link::stage(),
        s53_duplicate_and_reorder::stage(),
        s54_linearizability::stage(),
        s55_five_node_soak::stage(),
        s56_raft_leader_election::stage(),
        s57_raft_log_replication::stage(),
        s58_raft_commit_safety::stage(),
        s59_raft_snapshots::stage(),
        s60_raft_membership::stage(),
        s61_paxos_single_decree::stage(),
        s62_multi_paxos::stage(),
        s63_two_phase_commit::stage(),
        s64_three_phase_commit::stage(),
        s65_commit_recovery::stage(),
        s66_saga_orchestrated::stage(),
        s67_saga_choreographed::stage(),
        s68_transactional_outbox::stage(),
        s69_idempotent_consumer::stage(),
        s70_idempotency_keys::stage(),
        s71_fencing_tokens::stage(),
        s72_leader_leases::stage(),
        s73_anti_entropy::stage(),
        s74_gossip_dissemination::stage(),
        s75_circuit_breaker::stage(),
        s76_hedged_requests::stage(),
        s77_bulkheads::stage(),
        s78_consistency_models::stage(),
        s79_session_guarantees::stage(),
        s80_abd_register::stage(),
        s81_chain_replication::stage(),
        s82_raft_reads_and_prevote::stage(),
        s83_reliable_broadcast::stage(),
        s84_ordered_broadcast::stage(),
        s85_failure_detectors::stage(),
        s86_byzantine_quorums::stage(),
        s87_commit_wait::stage(),
        s88_snapshot_isolation::stage(),
    ];
    v.sort_by_key(|s| s.number);
    v
}

/// Which of the four ladders a stage belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ladder {
    /// Deterministic algorithm exercises in a single line-oriented process.
    Primitives,
    /// Consensus, atomic commit and the integration patterns, driven the same way.
    Algorithms,
    /// One server speaking the etcd v3 HTTP/JSON subset.
    Node,
    /// Three or five of those servers, with faults injected between them.
    Cluster,
}

impl Ladder {
    /// Every ladder, in the order a learner climbs them.
    ///
    /// This is the order the site and the run header show, not the order the stage numbers
    /// run in: the algorithms ladder was appended as stages 56 and upward so that the
    /// numbers already cited elsewhere could not move, but it is the natural second rung.
    pub const ALL: [Ladder; 4] = [
        Ladder::Primitives,
        Ladder::Algorithms,
        Ladder::Node,
        Ladder::Cluster,
    ];

    /// The tag this ladder puts on every one of its tests.
    pub fn as_str(self) -> &'static str {
        match self {
            Ladder::Primitives => "primitives",
            Ladder::Algorithms => "algorithms",
            Ladder::Node => "node",
            Ladder::Cluster => "cluster",
        }
    }

    /// True when this ladder drives a line-oriented CLI rather than a server.
    pub fn is_cli(self) -> bool {
        matches!(self, Ladder::Primitives | Ladder::Algorithms)
    }

    /// Parse a `--tag` value.
    pub fn parse(s: &str) -> Option<Ladder> {
        Ladder::ALL.into_iter().find(|l| l.as_str() == s)
    }
}

/// A section of the plan; `stages` lists every number the section holds.
pub struct Section {
    /// Short id, `a`..`l`.
    pub id: &'static str,
    /// Human title.
    pub title: &'static str,
    /// Every stage number belonging to this section.
    pub stages: &'static [u32],
}

/// The twelve sections of the distributed track.
pub fn sections() -> &'static [Section] {
    &[
        Section {
            id: "a",
            title: "Primitives: clocks and causality",
            stages: &[1, 2, 3, 4],
        },
        Section {
            id: "b",
            title: "Primitives: placement, quorums and sketches",
            stages: &[5, 6, 7, 8, 9, 10, 11, 12, 13],
        },
        Section {
            id: "c",
            title: "Primitives: CRDTs, snapshots and timing",
            stages: &[14, 15, 16, 17, 18, 19, 20],
        },
        Section {
            id: "d",
            title: "Single node: the key/value API",
            stages: &[21, 22, 23, 24, 25, 26, 27, 28, 29, 30],
        },
        Section {
            id: "e",
            title: "Single node: leases, watches and durability",
            stages: &[31, 32, 33, 34, 35],
        },
        Section {
            id: "f",
            title: "Cluster: replication and agreement",
            stages: &[36, 37, 38, 39, 40, 41, 42],
        },
        Section {
            id: "g",
            title: "Cluster: failure, recovery and membership",
            stages: &[43, 44, 45, 46, 47, 48, 49, 50, 51],
        },
        Section {
            id: "h",
            title: "Cluster: linearizability under fault injection",
            stages: &[52, 53, 54, 55],
        },
        Section {
            id: "i",
            title: "Algorithms: consensus",
            stages: &[56, 57, 58, 59, 60, 61, 62],
        },
        Section {
            id: "j",
            title: "Algorithms: atomic commit",
            stages: &[63, 64, 65],
        },
        Section {
            id: "k",
            title: "Algorithms: sagas and messaging",
            stages: &[66, 67, 68, 69, 70],
        },
        Section {
            id: "l",
            title: "Algorithms: coordination and resilience",
            stages: &[71, 72, 73, 74, 75, 76, 77],
        },
        Section {
            id: "m",
            title: "Algorithms: what consistency means",
            stages: &[78, 79],
        },
        Section {
            id: "n",
            title: "Algorithms: replication strategies",
            stages: &[80, 81, 82],
        },
        Section {
            id: "o",
            title: "Algorithms: broadcast and detection",
            stages: &[83, 84, 85],
        },
        Section {
            id: "p",
            title: "Algorithms: trust, time and transactions",
            stages: &[86, 87, 88],
        },
    ]
}

/// The future a test body returns.
pub type TestFuture<'a> = Pin<Box<dyn Future<Output = Result<(), Failure>> + Send + 'a>>;
/// A test body: an async function over the shared [`Ctx`].
pub type TestFn = for<'a> fn(&'a mut Ctx) -> TestFuture<'a>;

/// Wrap an async block as a [`TestFn`].
///
/// ```ignore
/// dist_test!(a_put_answers_with_a_revision, |ctx| {
///     let put = ok(ctx.kv().put(b"k", b"v").await, "put k")?;
///     let mut c = Check::new("the header of a put");
///     c.at_least("put.header.revision", 1, put.header.revision);
///     c.finish()
/// });
/// ```
#[macro_export]
macro_rules! dist_test {
    ($fn_name:ident, |$ctx:ident| $body:block) => {
        // A handful of tests check something about the suite itself rather than about the
        // program, and never touch the context; that is not a mistake worth a warning.
        #[allow(unused_variables)]
        fn $fn_name<'a>($ctx: &'a mut $crate::stages::Ctx) -> $crate::stages::TestFuture<'a> {
            Box::pin(async move { $body })
        }
    };
}

/// One test inside a stage.
pub struct Test {
    /// Test name, shown in the report and used by `--only`.
    pub name: &'static str,
    /// Tags; `ext` marks tests beyond the core track, and the ladder is always one of them.
    pub tags: Vec<&'static str>,
    /// `(target name, reason)` pairs: the test is skipped for those targets.
    pub skip_on: Vec<(&'static str, &'static str)>,
    /// Demand a cluster (or node) nobody else has touched.
    pub fresh: bool,
    /// How many members a cluster test wants; 0 means the stage default of three.
    pub cluster_size: usize,
    /// How many further members are configured but left stopped, for membership changes.
    pub spare: usize,
    /// Per-test timeout override; it wins outright.
    pub timeout_ms: Option<u64>,
    /// A floor under the per-test timeout, for tests that are inherently slow.
    pub min_timeout_ms: Option<u64>,
    /// The test body.
    pub run: TestFn,
}

impl Test {
    /// A test with no tags, no skips and the stage's defaults.
    pub fn new(name: &'static str, run: TestFn) -> Test {
        Test {
            name,
            tags: Vec::new(),
            skip_on: Vec::new(),
            fresh: false,
            cluster_size: 0,
            spare: 0,
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

    /// Skip this test for one target, with a reason that is always printed.
    pub fn skip_on(mut self, target: &'static str, reason: &'static str) -> Test {
        self.skip_on.push((target, reason));
        self
    }

    /// Demand a node or cluster no earlier test has touched.
    ///
    /// Any test that kills a member, cuts the network or changes membership needs this;
    /// the runner also throws a cluster away by itself once a test has dirtied it.
    pub fn fresh(mut self) -> Test {
        self.fresh = true;
        self
    }

    /// Ask for a cluster of this size (and a fresh one, necessarily).
    pub fn cluster(mut self, size: usize) -> Test {
        self.cluster_size = size;
        self.fresh = true;
        self
    }

    /// Ask for extra members that are configured but not started.
    pub fn spare(mut self, spare: usize) -> Test {
        self.spare = spare;
        self.fresh = true;
        self
    }

    /// Give this test its own timeout, overriding both `--timeout-ms` and the floor.
    pub fn timeout_ms(mut self, ms: u64) -> Test {
        self.timeout_ms = Some(ms);
        self
    }

    /// Raise the floor of this test's timeout; `--timeout-ms` still wins when it is larger.
    pub fn min_timeout_ms(mut self, ms: u64) -> Test {
        self.min_timeout_ms = Some(ms);
        self
    }

    /// The timeout this test runs under, given the run's default (`--timeout-ms`).
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

    /// The reason this test is skipped for `target`, if it is.
    pub fn skip_reason(&self, target: &str) -> Option<&'static str> {
        self.skip_on
            .iter()
            .find(|(t, _)| *t == target)
            .map(|(_, r)| *r)
    }

    /// Every tag this test reports, the ladder included.
    pub fn all_tags(&self, ladder: Ladder) -> Vec<String> {
        let mut tags = vec![ladder.as_str().to_string()];
        tags.extend(self.tags.iter().map(|t| (*t).to_string()));
        tags
    }
}

/// One stage: a numbered group of tests on one ladder, with implementation hints.
pub struct Stage {
    /// Stage number, 1-based.
    pub number: u32,
    /// File-name slug, `sNN_<slug>.rs`.
    pub slug: &'static str,
    /// Human title.
    pub name: &'static str,
    /// True when the whole stage is beyond the core track.
    pub ext: bool,
    /// Which ladder this stage belongs to.
    pub ladder: Ladder,
    /// 2–4 lines of "what to implement, where the trap is".
    pub hints: &'static [&'static str],
    /// 1–3 worked examples; see [`crate::examples`].
    pub examples: fn() -> Vec<crate::examples::ExampleSpec>,
    /// The stage's tests.
    pub tests: Vec<Test>,
}

impl Stage {
    /// The source file the stage lives in.
    pub fn file_name(&self) -> String {
        format!("src/stages/s{:02}_{}.rs", self.number, self.slug)
    }

    /// How many members this stage's cluster tests want by default.
    pub fn default_cluster_size(&self) -> usize {
        3
    }
}

/// Everything a test body can reach.
pub struct Ctx {
    /// The target under test, for report lines and `skip_on`.
    pub target_name: String,
    /// The ladder this test belongs to.
    pub ladder: Ladder,
    /// The run's seed; every random choice must come from it.
    pub seed: u64,
    /// A seeded RNG, reset per test so runs are reproducible.
    pub rng: StdRng,
    /// The per-request timeout.
    pub timeout: Duration,
    /// This test's scratch directory.
    pub tmp: PathBuf,
    /// Informational lines the test wants in the report whatever the outcome.
    pub notes: Vec<String>,
    /// argv of the program under test, without any ladder arguments.
    pub argv: Vec<String>,
    /// Working directory of the program under test.
    pub cwd: PathBuf,
    /// Extra environment for the program under test.
    pub env: Vec<(String, String)>,
    /// The single node of a `node` stage, lent by the runner for the body's duration.
    pub node: Option<NodeHandle>,
    /// A client bound to that node.
    pub client: Option<Client>,
    /// The cluster of a `cluster` stage, lent by the runner for the body's duration.
    pub cluster: Option<Cluster>,
    /// The primitives process, started on first use and reused while the topic is the same.
    pub prim_proc: Option<PrimProc>,
    /// A per-test index, so keys never collide between tests sharing a cluster.
    pub index: u64,
}

impl Ctx {
    /// Build a context for one test.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        target_name: &str,
        ladder: Ladder,
        seed: u64,
        index: u64,
        timeout: Duration,
        tmp: PathBuf,
        argv: Vec<String>,
        cwd: PathBuf,
        env: Vec<(String, String)>,
    ) -> Ctx {
        Ctx {
            target_name: target_name.to_string(),
            ladder,
            seed,
            rng: StdRng::seed_from_u64(seed ^ index.wrapping_mul(0x9e37_79b9_7f4a_7c15)),
            timeout,
            tmp,
            notes: Vec::new(),
            argv,
            cwd,
            env,
            node: None,
            client: None,
            cluster: None,
            prim_proc: None,
            index,
        }
    }

    /// Add an informational line to the test's report entry.
    pub fn note(&mut self, line: impl Into<String>) {
        self.notes.push(line.into());
    }

    /// A key prefix unique to this test, so tests that share a cluster never collide.
    pub fn prefix(&self) -> String {
        format!("t{:03}", self.index)
    }

    /// A key inside this test's own prefix.
    pub fn key(&self, name: &str) -> Vec<u8> {
        format!("{}/{name}", self.prefix()).into_bytes()
    }

    /// The primitives process for a topic, started on first use.
    ///
    /// Asking for a different topic closes the old process and starts a new one, which is
    /// what a stage covering two topics wants.
    pub async fn prim(&mut self, topic: &'static str) -> Result<&mut PrimProc, Failure> {
        let restart = match &self.prim_proc {
            Some(p) => p.topic != topic,
            None => true,
        };
        if restart {
            if let Some(mut old) = self.prim_proc.take() {
                old.close().await;
            }
            let proc = PrimProc::start(
                &self.argv,
                &self.cwd,
                &self.env,
                topic,
                &self.tmp,
                self.timeout,
            )
            .await?;
            self.prim_proc = Some(proc);
        }
        self.prim_proc
            .as_mut()
            .ok_or_else(|| Failure::harness("the primitives process vanished"))
    }

    /// A *fresh* primitives process for a topic, even when one is already open.
    ///
    /// Stages that prove a replica's state is per-process — every CRDT stage — need two
    /// conversations that cannot see each other.
    pub async fn prim_fresh(&mut self, topic: &'static str) -> Result<PrimProc, Failure> {
        PrimProc::start(
            &self.argv,
            &self.cwd,
            &self.env,
            topic,
            &self.tmp,
            self.timeout,
        )
        .await
    }

    /// The client of the single node a `node` stage runs against.
    pub fn kv(&self) -> Result<&Client, Failure> {
        self.client
            .as_ref()
            .ok_or_else(|| Failure::harness("this test has no node (is it on the `node` ladder?)"))
    }

    /// The single node's process.
    pub fn node_mut(&mut self) -> Result<&mut NodeHandle, Failure> {
        self.node
            .as_mut()
            .ok_or_else(|| Failure::harness("this test has no node (is it on the `node` ladder?)"))
    }

    /// The cluster a `cluster` stage runs against.
    pub fn cluster(&mut self) -> Result<&mut Cluster, Failure> {
        self.cluster.as_mut().ok_or_else(|| {
            Failure::harness("this test has no cluster (is it on the `cluster` ladder?)")
        })
    }

    /// `SIGKILL` the single node: a power cut, nothing flushed.
    pub async fn kill_node(&mut self) -> Result<(), Failure> {
        self.node_mut()?.kill_hard();
        if let Some(c) = &self.client {
            c.reset().await;
        }
        Ok(())
    }

    /// Start the single node again from its own data directory.
    pub async fn start_node(&mut self) -> Result<(), Failure> {
        let spec = self.node_mut()?.spec.clone();
        let handle = tokio::task::spawn_blocking(move || NodeHandle::start(spec))
            .await
            .map_err(|e| Failure::harness(format!("the restart task did not finish: {e}")))?
            .map_err(|e| {
                Failure::harness(format!("the node did not come back after a restart: {e:#}"))
            })?;
        self.node = Some(handle);
        if let Some(c) = &self.client {
            c.reset().await;
        }
        Ok(())
    }

    /// Kill the node and start it again: the durability stages' whole move.
    pub async fn restart_node(&mut self) -> Result<(), Failure> {
        self.kill_node().await?;
        self.start_node().await
    }

    /// The node's output, for a failure block.
    pub fn node_output(&self) -> String {
        match (&self.node, &self.cluster) {
            (Some(n), _) => n.output_tail(15),
            (None, Some(c)) => c.output(),
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stage_is_unique_ordered_and_documented() {
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
    fn the_suite_is_as_large_as_the_plan_says() {
        let stages = all();
        // Counted from the sections, so growing the plan does not mean editing a literal.
        let planned: usize = sections().iter().map(|s| s.stages.len()).sum();
        assert_eq!(
            stages.len(),
            planned,
            "every planned stage is implemented, and no more"
        );
        let tests: usize = stages.iter().map(|s| s.tests.len()).sum();
        assert!(
            tests >= 700,
            "the plan is at least 700 tests, found {tests}"
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
                for (target, reason) in &t.skip_on {
                    assert!(
                        !reason.trim().is_empty(),
                        "stage {} test '{}' skips {target} with no reason",
                        s.number,
                        t.name
                    );
                }
            }
        }
    }

    #[test]
    fn every_ladder_owns_one_contiguous_block_of_numbers() {
        // Stage numbers are append-only — the algorithms ladder sits at 56.. even though it
        // is the second rung a learner climbs — but a ladder must still be one unbroken run
        // of numbers, or `--tag` would select a ragged range.
        let stages = all();
        for ladder in Ladder::ALL {
            let mine: Vec<u32> = stages
                .iter()
                .filter(|s| s.ladder == ladder)
                .map(|s| s.number)
                .collect();
            assert!(!mine.is_empty(), "{} has no stages", ladder.as_str());
            let (lo, hi) = (mine[0], mine[mine.len() - 1]);
            assert_eq!(
                mine,
                (lo..=hi).collect::<Vec<u32>>(),
                "the {} ladder is not one contiguous block",
                ladder.as_str()
            );
        }
        assert!(
            stages
                .iter()
                .filter(|s| s.ladder == Ladder::Algorithms)
                .all(|s| s.number >= 56),
            "stage numbers 1-55 are frozen; the algorithms ladder appends"
        );
    }

    #[test]
    fn only_cluster_tests_ask_for_members() {
        for s in all() {
            for t in &s.tests {
                if t.cluster_size > 0 || t.spare > 0 {
                    assert_eq!(
                        s.ladder,
                        Ladder::Cluster,
                        "stage {} test '{}' asks for members but is not a cluster stage",
                        s.number,
                        t.name
                    );
                    assert!(t.fresh, "asking for members implies a fresh cluster");
                }
            }
        }
    }

    #[test]
    fn the_ladder_is_always_a_tag() {
        crate::dist_test!(nothing, |_ctx| { Ok(()) });
        let t = Test::new("t", nothing).ext().tag("slow");
        assert_eq!(
            t.all_tags(Ladder::Cluster),
            vec!["cluster".to_string(), "ext".into(), "slow".into()]
        );
        assert_eq!(Ladder::parse("node"), Some(Ladder::Node));
        assert_eq!(Ladder::parse("algorithms"), Some(Ladder::Algorithms));
        assert_eq!(Ladder::parse("nonsense"), None);
        assert!(Ladder::Algorithms.is_cli() && Ladder::Primitives.is_cli());
        assert!(!Ladder::Node.is_cli() && !Ladder::Cluster.is_cli());
    }

    #[test]
    fn timeout_override_wins_and_min_only_raises_the_floor() {
        crate::dist_test!(nothing, |_ctx| { Ok(()) });
        let default = Duration::from_millis(10_000);
        assert_eq!(Test::new("plain", nothing).timeout(default), default);
        let slow = Test::new("slow", nothing).min_timeout_ms(60_000);
        assert_eq!(slow.timeout(default), Duration::from_millis(60_000));
        assert_eq!(
            slow.timeout(Duration::from_millis(90_000)),
            Duration::from_millis(90_000)
        );
        let pinned = Test::new("pinned", nothing).timeout_ms(120_000);
        assert_eq!(
            pinned.timeout(Duration::from_millis(300_000)),
            Duration::from_millis(120_000)
        );
    }

    #[test]
    fn sections_cover_the_whole_plan_once() {
        let mut numbers: Vec<u32> = sections().iter().flat_map(|s| s.stages.to_vec()).collect();
        numbers.sort_unstable();
        assert_eq!(
            numbers,
            (1..=numbers.len() as u32).collect::<Vec<u32>>(),
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

    #[test]
    fn a_context_gives_each_test_its_own_key_space() {
        let ctx = Ctx::new(
            "my_node",
            Ladder::Node,
            7,
            12,
            Duration::from_secs(1),
            PathBuf::from("/tmp"),
            vec!["./x".into()],
            PathBuf::from("."),
            vec![],
        );
        assert_eq!(ctx.prefix(), "t012");
        assert_eq!(ctx.key("a"), b"t012/a".to_vec());
        assert!(ctx.kv().is_err(), "a context without a node says so");
    }
}
