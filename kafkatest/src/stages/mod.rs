//! The stage registry: what a stage and a test are, and the context a test runs in.
//!
//! Adding a stage means writing `src/stages/sNN_slug.rs` with a `pub fn stage() -> Stage`
//! and adding two lines here (the `mod` and the `push`). Nothing else in the harness
//! changes, which is what lets several agents work on different stages at once.

use crate::assert::Failure;
use crate::broker::BrokerHandle;
use crate::fixtures::{FixtureHandle, FixtureSpec, TopicInfo};
use crate::proto::{Conn, ProtoError};
use rand::rngs::StdRng;
use rand::SeedableRng;
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

pub(crate) mod group_protocol;
mod helpers;
pub(crate) mod transactions;
pub use helpers::*;

// ---------------------------------------------------------------------------------------
// Stage modules. Keep them in numeric order; gaps are expected while stages are unwritten.
// ---------------------------------------------------------------------------------------
mod s01_bind;
mod s02_correlation_id;
mod s03_request_header;
mod s04_unsupported_version;
mod s05_api_versions_body;
mod s06_sequential_requests;
mod s07_concurrent_connections;
mod s08_pipelined_requests;
mod s09_framing_robustness;
mod s10_advertise_describe_topic_partitions;
mod s11_describe_unknown_topic;
mod s12_describe_single_partition;
mod s13_describe_multiple_partitions;
mod s14_describe_multiple_topics;
mod s15_describe_pagination;
mod s16_metadata_v12;
mod s17_create_topics;
mod s18_delete_topics;
mod s19_advertise_fetch;
mod s20_fetch_no_topics;
mod s21_fetch_unknown_topic_id;
mod s22_fetch_empty_topic;
mod s23_fetch_single_record;
mod s24_fetch_multiple_batches;
mod s25_fetch_from_offset;
mod s26_fetch_max_bytes;
mod s27_compressed_batches;
mod s28_long_poll;
mod s29_advertise_produce;
mod s30_produce_unknown_topic;
mod s31_produce_one_record;
mod s32_produce_many;
mod s33_produce_acks;
mod s34_produce_persisted;
mod s35_produce_fetch_restart;
mod s36_idempotent_producer;
mod s37_record_validation;
mod s38_list_offsets;
mod s39_find_coordinator;
mod s40_consumer_group;
mod s41_offset_commit_fetch;
mod s42_group_errors;
mod s43_cli_interop;
mod s44_fuzz;
mod s45_soak;
mod s46_transaction_coordinator;
mod s47_transactional_writes;

/// Every implemented stage, in ascending order.
pub fn all() -> Vec<Stage> {
    let mut v = vec![
        s01_bind::stage(),
        s02_correlation_id::stage(),
        s03_request_header::stage(),
        s04_unsupported_version::stage(),
        s05_api_versions_body::stage(),
        s06_sequential_requests::stage(),
        s07_concurrent_connections::stage(),
        s08_pipelined_requests::stage(),
        s09_framing_robustness::stage(),
        s10_advertise_describe_topic_partitions::stage(),
        s11_describe_unknown_topic::stage(),
        s12_describe_single_partition::stage(),
        s13_describe_multiple_partitions::stage(),
        s14_describe_multiple_topics::stage(),
        s15_describe_pagination::stage(),
        s16_metadata_v12::stage(),
        s17_create_topics::stage(),
        s18_delete_topics::stage(),
        s19_advertise_fetch::stage(),
        s20_fetch_no_topics::stage(),
        s21_fetch_unknown_topic_id::stage(),
        s22_fetch_empty_topic::stage(),
        s23_fetch_single_record::stage(),
        s24_fetch_multiple_batches::stage(),
        s25_fetch_from_offset::stage(),
        s26_fetch_max_bytes::stage(),
        s27_compressed_batches::stage(),
        s28_long_poll::stage(),
        s29_advertise_produce::stage(),
        s30_produce_unknown_topic::stage(),
        s31_produce_one_record::stage(),
        s32_produce_many::stage(),
        s33_produce_acks::stage(),
        s34_produce_persisted::stage(),
        s35_produce_fetch_restart::stage(),
        s36_idempotent_producer::stage(),
        s37_record_validation::stage(),
        s38_list_offsets::stage(),
        s39_find_coordinator::stage(),
        s40_consumer_group::stage(),
        s41_offset_commit_fetch::stage(),
        s42_group_errors::stage(),
        s43_cli_interop::stage(),
        s44_fuzz::stage(),
        s45_soak::stage(),
        s46_transaction_coordinator::stage(),
        s47_transactional_writes::stage(),
    ];
    v.sort_by_key(|s| s.number);
    v
}

/// A section of the plan; `stages` lists every number the section will ever hold, whether
/// or not it is implemented yet, so the site can draw the whole journey from day one.
pub struct Section {
    /// Short id, `a`..`f`.
    pub id: &'static str,
    /// Human title.
    pub title: &'static str,
    /// Every stage number belonging to this section.
    pub stages: &'static [u32],
}

/// The six sections of the Kafka track.
pub fn sections() -> &'static [Section] {
    &[
        Section {
            id: "a",
            title: "Bootstrap & framing",
            stages: &[1, 2, 3, 4, 5, 6, 7, 8, 9],
        },
        Section {
            id: "b",
            title: "Metadata & topics",
            stages: &[10, 11, 12, 13, 14, 15, 16, 17, 18],
        },
        Section {
            id: "c",
            title: "Fetch",
            stages: &[19, 20, 21, 22, 23, 24, 25, 26, 27, 28],
        },
        Section {
            id: "d",
            title: "Produce",
            stages: &[29, 30, 31, 32, 33, 34, 35, 36, 37],
        },
        Section {
            id: "e",
            title: "Offsets & consumer groups",
            stages: &[38, 39, 40, 41, 42],
        },
        Section {
            id: "f",
            title: "Interop, robustness, performance",
            stages: &[43, 44, 45],
        },
        Section {
            id: "g",
            title: "Transactions",
            stages: &[46, 47],
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
/// kafka_test!(unknown_topic, |ctx| {
///     let mut conn = ctx.connect().await?;
///     ...
///     Ok(())
/// });
/// ```
#[macro_export]
macro_rules! kafka_test {
    ($fn_name:ident, |$ctx:ident| $body:block) => {
        fn $fn_name<'a>($ctx: &'a mut $crate::stages::Ctx) -> $crate::stages::TestFuture<'a> {
            Box::pin(async move { $body })
        }
    };
}

/// One test inside a stage.
pub struct Test {
    /// Test name, shown in the report and used by `--only`.
    pub name: &'static str,
    /// Tags; `ext` marks tests beyond the core track.
    pub tags: Vec<&'static str>,
    /// `(broker name, reason)` pairs: the test is skipped for those brokers.
    pub skip_on: Vec<(&'static str, &'static str)>,
    /// The topics/records that must exist before the test runs.
    pub fixtures: fn() -> FixtureSpec,
    /// Force a fresh broker process before this test even under `restart: per_stage`.
    pub force_restart: bool,
    /// Per-test timeout override, for tests that restart the broker mid-flight. When set it
    /// wins outright, `--timeout-ms` and `min_timeout_ms` included.
    pub timeout_ms: Option<u64>,
    /// A floor under the per-test timeout, for tests that are inherently slow (soak, fuzz,
    /// anything that shells out to a JVM). `--timeout-ms` still wins when it is larger.
    pub min_timeout_ms: Option<u64>,
    /// The test body.
    pub run: TestFn,
}

fn no_fixtures() -> FixtureSpec {
    FixtureSpec::none()
}

impl Test {
    /// A test with no tags, no fixtures and no skips.
    pub fn new(name: &'static str, run: TestFn) -> Test {
        Test {
            name,
            tags: Vec::new(),
            skip_on: Vec::new(),
            fixtures: no_fixtures,
            force_restart: false,
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

    /// Skip this test for one broker, with a reason that is always printed.
    pub fn skip_on(mut self, broker: &'static str, reason: &'static str) -> Test {
        self.skip_on.push((broker, reason));
        self
    }

    /// Declare the fixtures the test needs.
    pub fn with_fixtures(mut self, f: fn() -> FixtureSpec) -> Test {
        self.fixtures = f;
        self
    }

    /// Demand a fresh broker process for this test.
    pub fn restart(mut self) -> Test {
        self.force_restart = true;
        self
    }

    /// Give this test its own timeout, overriding both `--timeout-ms` and
    /// [`Test::min_timeout_ms`].
    ///
    /// Only for tests that legitimately take longer than a request round trip — the ones
    /// that stop and start the broker in the middle of the body ([`Ctx::restart_broker`]).
    pub fn timeout_ms(mut self, ms: u64) -> Test {
        self.timeout_ms = Some(ms);
        self
    }

    /// Raise the floor of this test's timeout; `--timeout-ms` still wins when it is larger.
    ///
    /// Stages 43-45 need it: a JVM command-line tool, 500 fuzz probes or a 10 000-record
    /// soak cannot finish inside the 10 s default.
    pub fn min_timeout_ms(mut self, ms: u64) -> Test {
        self.min_timeout_ms = Some(ms);
        self
    }

    /// The timeout this test runs under, given the run's default (`--timeout-ms`).
    ///
    /// [`Test::timeout_ms`] is an override and wins outright; otherwise the deadline is the
    /// run's default raised to [`Test::min_timeout_ms`] when the test asked for a floor.
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

    /// The reason this test is skipped for `broker`, if it is.
    pub fn skip_reason(&self, broker: &str) -> Option<&'static str> {
        self.skip_on
            .iter()
            .find(|(b, _)| *b == broker)
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
    /// 1–3 worked examples: the exact bytes a learner should expect to receive and to
    /// answer. See [`crate::examples`]; `--capture-examples` turns them into
    /// `examples/captured.json`, which `--list --json` merges into the catalog.
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
    /// The broker under test (for crash detection and output).
    pub broker_name: String,
    /// Where to connect.
    pub addr: SocketAddr,
    /// The fixtures that were materialized for this test.
    pub fixtures: FixtureHandle,
    /// Per-request timeout.
    pub timeout: Duration,
    /// The run's seed; every random choice must come from it.
    pub seed: u64,
    /// A seeded RNG, reset per test so runs are reproducible.
    pub rng: StdRng,
    /// The unpacked reference distribution, when one is available (interop tests).
    pub dist: Option<PathBuf>,
    /// The broker's log directory.
    pub log_dir: PathBuf,
    /// The broker process, lent to the test by the runner for the duration of the body so
    /// that [`Ctx::restart_broker`] can stop and start it in place (stage 35). The runner
    /// takes it back when the body returns; a test never touches this field directly.
    pub broker: Option<BrokerHandle>,
    /// Informational lines the test wants in the report even when it passes: timings,
    /// throughput, why something was skipped. See [`Ctx::note`].
    pub notes: Vec<String>,
}

impl Ctx {
    /// Build a context for one test.
    pub fn new(
        broker: &BrokerHandle,
        fixtures: FixtureHandle,
        timeout: Duration,
        seed: u64,
        test_index: u64,
        dist: Option<PathBuf>,
    ) -> Ctx {
        Ctx {
            broker_name: broker.spec.name.clone(),
            addr: broker.addr,
            log_dir: broker.spec.log_dir.clone(),
            fixtures,
            timeout,
            seed,
            rng: StdRng::seed_from_u64(seed ^ (test_index.wrapping_mul(0x9e37_79b9_7f4a_7c15))),
            dist,
            broker: None,
            notes: Vec::new(),
        }
    }

    /// Add an informational line to the test's report entry.
    ///
    /// Unlike [`crate::assert::Check::note`], which only ever surfaces under a failure, these
    /// lines are printed (and put into the JSON report) whatever the outcome. Stage 45 uses
    /// them for latency percentiles, stage 43 to say why a tool run was skipped.
    pub fn note(&mut self, line: impl Into<String>) {
        self.notes.push(line.into());
    }

    /// Stop the broker and start it again from the same spec: same port, same properties
    /// file, same log directory, no reformat. What is on disk is all the new process has.
    ///
    /// This is how stage 35 proves that offsets continue across a restart. The reference
    /// broker takes several seconds to come back, so a test using it needs
    /// [`Test::timeout_ms`].
    pub async fn restart_broker(&mut self) -> Result<(), Failure> {
        let Some(handle) = self.broker.take() else {
            return Err(Failure::harness(
                "this test cannot restart the broker: the harness did not lend it the process \
                 (restart_broker only works from a stage test body)",
            ));
        };
        // The reference broker accepts connections well before it can serve them, so wait
        // for the same marker line the runner waits for at boot.
        let wait_for_marker = self.dist.is_some();
        let started = tokio::task::spawn_blocking(move || -> anyhow::Result<BrokerHandle> {
            let spec = handle.spec.clone();
            // Dropping the handle SIGTERMs (then SIGKILLs) the whole process group.
            drop(handle);
            let fresh = BrokerHandle::start(spec.clone())?;
            if wait_for_marker {
                crate::broker::reference::wait_for_marker(
                    &spec.tmp,
                    crate::broker::reference::READY_MARKER,
                    spec.boot_timeout,
                )?;
            }
            Ok(fresh)
        })
        .await;
        match started {
            Ok(Ok(fresh)) => {
                self.addr = fresh.addr;
                self.broker = Some(fresh);
                Ok(())
            }
            Ok(Err(e)) => Err(Failure::harness(format!(
                "the broker did not come back after being restarted in place: {e:#}"
            ))),
            Err(e) => Err(Failure::harness(format!(
                "the restart task did not finish: {e}"
            ))),
        }
    }

    /// Open a connection to the broker.
    pub async fn connect(&self) -> Result<Conn, Failure> {
        Conn::connect(self.addr, self.timeout)
            .await
            .map_err(|e| Failure::proto(e, None).note(format!("connecting to {}", self.addr)))
    }

    /// Look a fixture topic up by its logical key.
    pub fn topic(&self, key: &str) -> Result<&TopicInfo, Failure> {
        self.fixtures.topic(key)
    }

    /// A name that is unique to this run but stable for a given seed.
    pub fn unique(&self, prefix: &str) -> String {
        format!("{prefix}-{:x}", self.seed & 0xffff_ffff)
    }
}

/// Turn a protocol error into a failure that carries the connection's bytes.
pub fn proto_fail(e: ProtoError, conn: &Conn) -> Failure {
    Failure::proto(e, Some(conn))
}

/// Ask the broker for its supported API versions (v4, the flexible version).
pub async fn api_versions(
    conn: &mut Conn,
) -> Result<kafka_protocol::messages::ApiVersionsResponse, Failure> {
    let req = helpers::api_versions_request();
    let resp = conn
        .request(4, &req)
        .await
        .map_err(|e| proto_fail(e, conn))?;
    Ok(resp.body)
}

/// The advertised version range for one API key, if the broker lists it.
pub fn advertised(
    resp: &kafka_protocol::messages::ApiVersionsResponse,
    key: i16,
) -> Option<(i16, i16)> {
    resp.api_keys
        .iter()
        .find(|k| k.api_key == key)
        .map(|k| (k.min_version, k.max_version))
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
                for (broker, reason) in &t.skip_on {
                    assert!(
                        !reason.trim().is_empty(),
                        "stage {} test '{}' skips {broker} with no reason",
                        s.number,
                        t.name
                    );
                }
            }
        }
    }

    #[test]
    fn timeout_override_wins_and_min_only_raises_the_floor() {
        crate::kafka_test!(nothing, |_ctx| { Ok(()) });
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

        let both = Test::new("both", nothing)
            .min_timeout_ms(60_000)
            .timeout_ms(20_000);
        assert_eq!(
            both.timeout(default),
            Duration::from_millis(20_000),
            "the override beats the floor too"
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
