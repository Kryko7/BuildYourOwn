//! The stage registry: what a stage and a test are, and the context a test runs in.
//!
//! Adding a stage means writing `src/stages/sNN_slug.rs` with a `pub fn stage() -> Stage`
//! and adding two lines here (the `mod` and the entry in [`all`]). Nothing else in the
//! harness changes, which is what lets several people work on different stages at once.

use crate::assert::{Failure, FailureKind};
use crate::certs::{CertKind, CertStore, Material};
use crate::config::ServerOptions;
use crate::server::ServerHandle;
use crate::tls::client::{Client, ClientConfig};
use crate::tls::conn::TlsConn;
use crate::tls::TlsError;
use rand::rngs::StdRng;
use rand::SeedableRng;
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

mod helpers;
pub use helpers::*;

// ---------------------------------------------------------------------------------------
// Stage modules. Keep them in numeric order.
// ---------------------------------------------------------------------------------------
mod s01_accept;
mod s02_record_framing;
mod s03_record_limits;
mod s04_garbage_on_connect;
mod s05_split_client_hello;
mod s06_coalesced_handshake;
mod s07_half_close;
mod s08_supported_versions;
mod s09_tls12_client;
mod s10_cipher_suites;
mod s11_handshake_failure;
mod s12_key_share_groups;
mod s13_signature_algorithms;
mod s14_grease_and_unknown;
mod s15_sni_and_alpn;
mod s16_bad_extensions;
mod s17_server_hello;
mod s18_handshake_secret;
mod s19_transcript_hash;
mod s20_traffic_keys;
mod s21_change_cipher_spec;
mod s22_encrypted_extensions;
mod s23_sequence_numbers;
mod s24_certificate;
mod s25_certificate_verify;
mod s26_signature_schemes;
mod s27_server_finished;
mod s28_client_finished;
mod s29_flight_order;
mod s30_echo;
mod s31_echo_all_suites;
mod s32_inner_plaintext;
mod s33_record_sizes;
mod s34_close_notify;
mod s35_bad_record_mac;
mod s36_alerts;
mod s37_bad_handshake;
mod s38_no_renegotiation;
mod s39_concurrency;
mod s40_fuzz;
mod s41_hello_retry_request;
mod s42_key_update;
mod s43_resumption;
mod s44_early_data;
mod s45_interop;
mod s46_client_certificate_request;
mod s47_client_certificate;
mod s48_client_auth_policy;

/// Every implemented stage, in ascending order.
pub fn all() -> Vec<Stage> {
    let mut v = vec![
        s01_accept::stage(),
        s02_record_framing::stage(),
        s03_record_limits::stage(),
        s04_garbage_on_connect::stage(),
        s05_split_client_hello::stage(),
        s06_coalesced_handshake::stage(),
        s07_half_close::stage(),
        s08_supported_versions::stage(),
        s09_tls12_client::stage(),
        s10_cipher_suites::stage(),
        s11_handshake_failure::stage(),
        s12_key_share_groups::stage(),
        s13_signature_algorithms::stage(),
        s14_grease_and_unknown::stage(),
        s15_sni_and_alpn::stage(),
        s16_bad_extensions::stage(),
        s17_server_hello::stage(),
        s18_handshake_secret::stage(),
        s19_transcript_hash::stage(),
        s20_traffic_keys::stage(),
        s21_change_cipher_spec::stage(),
        s22_encrypted_extensions::stage(),
        s23_sequence_numbers::stage(),
        s24_certificate::stage(),
        s25_certificate_verify::stage(),
        s26_signature_schemes::stage(),
        s27_server_finished::stage(),
        s28_client_finished::stage(),
        s29_flight_order::stage(),
        s30_echo::stage(),
        s31_echo_all_suites::stage(),
        s32_inner_plaintext::stage(),
        s33_record_sizes::stage(),
        s34_close_notify::stage(),
        s35_bad_record_mac::stage(),
        s36_alerts::stage(),
        s37_bad_handshake::stage(),
        s38_no_renegotiation::stage(),
        s39_concurrency::stage(),
        s40_fuzz::stage(),
        s41_hello_retry_request::stage(),
        s42_key_update::stage(),
        s43_resumption::stage(),
        s44_early_data::stage(),
        s45_interop::stage(),
        s46_client_certificate_request::stage(),
        s47_client_certificate::stage(),
        s48_client_auth_policy::stage(),
    ];
    v.sort_by_key(|s| s.number);
    v
}

/// A section of the plan; `stages` lists every number the section holds.
pub struct Section {
    /// Short id, `a`..`g`.
    pub id: &'static str,
    /// Human title.
    pub title: &'static str,
    /// Every stage number belonging to this section.
    pub stages: &'static [u32],
}

/// The seven sections of the TLS track.
pub fn sections() -> &'static [Section] {
    &[
        Section {
            id: "a",
            title: "TCP & the record layer",
            stages: &[1, 2, 3, 4, 5, 6, 7],
        },
        Section {
            id: "b",
            title: "ClientHello, extensions, negotiation",
            stages: &[8, 9, 10, 11, 12, 13, 14, 15, 16],
        },
        Section {
            id: "c",
            title: "Key schedule & handshake encryption",
            stages: &[17, 18, 19, 20, 21, 22, 23],
        },
        Section {
            id: "d",
            title: "Authentication",
            stages: &[24, 25, 26, 27, 28, 29],
        },
        Section {
            id: "e",
            title: "Application data & AEAD",
            stages: &[30, 31, 32, 33, 34, 35],
        },
        Section {
            id: "f",
            title: "Alerts, errors, robustness",
            stages: &[36, 37, 38, 39, 40],
        },
        Section {
            id: "g",
            title: "Advanced",
            stages: &[41, 42, 43, 44, 45],
        },
        Section {
            id: "h",
            title: "Client authentication",
            stages: &[46, 47, 48],
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
/// tls_test!(echoes_a_line, |ctx| {
///     let mut client = ctx.handshake().await?;
///     ...
///     Ok(())
/// });
/// ```
#[macro_export]
macro_rules! tls_test {
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
    /// `(server name, reason)` pairs: the test is skipped for those servers.
    pub skip_on: Vec<(&'static str, &'static str)>,
    /// How the server must be started for this test.
    pub server_options: fn() -> ServerOptions,
    /// Force a fresh server process before this test even under `restart: per_stage`.
    pub force_restart: bool,
    /// Per-test timeout override; wins outright.
    pub timeout_ms: Option<u64>,
    /// A floor under the per-test timeout, for tests that are inherently slow.
    pub min_timeout_ms: Option<u64>,
    /// The test body.
    pub run: TestFn,
}

fn default_options() -> ServerOptions {
    ServerOptions::default()
}

impl Test {
    /// A test with no tags, the default server options and no skips.
    pub fn new(name: &'static str, run: TestFn) -> Test {
        Test {
            name,
            tags: Vec::new(),
            skip_on: Vec::new(),
            server_options: default_options,
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

    /// Skip this test for one server, with a reason that is always printed.
    pub fn skip_on(mut self, server: &'static str, reason: &'static str) -> Test {
        self.skip_on.push((server, reason));
        self
    }

    /// Start the server differently for this test (another certificate, `-naccept`, ...).
    pub fn with_server(mut self, f: fn() -> ServerOptions) -> Test {
        self.server_options = f;
        self
    }

    /// Demand a fresh server process for this test.
    pub fn restart(mut self) -> Test {
        self.force_restart = true;
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

    /// The reason this test is skipped for `server`, if it is.
    pub fn skip_reason(&self, server: &str) -> Option<&'static str> {
        self.skip_on
            .iter()
            .find(|(s, _)| *s == server)
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
    /// 1–3 worked examples: the exact bytes a learner should expect to see.
    pub examples: fn() -> Vec<crate::examples::ExampleSpec>,
    /// The stage's tests.
    pub tests: Vec<Test>,
}

impl Stage {
    /// The source file the stage lives in.
    pub fn file_name(&self) -> String {
        format!("src/stages/s{:02}_{}.rs", self.number, self.slug)
    }

    /// Whether `test` is beyond the core track, which it is if either the test or the whole
    /// stage says so.
    ///
    /// `--skip-ext` and the catalog both go through here, so a stage marked `ext: true`
    /// really does hide every one of its tests rather than only the individually tagged
    /// ones.
    pub fn test_is_ext(&self, test: &Test) -> bool {
        self.ext || test.is_ext()
    }
}

/// Everything a test body can reach.
pub struct Ctx {
    /// The server under test, by name.
    pub server_name: String,
    /// True when this is the reference (`openssl s_server`).
    pub is_reference: bool,
    /// Where to connect.
    pub addr: SocketAddr,
    /// Per-operation timeout.
    pub timeout: Duration,
    /// The run's seed; every random choice must come from it.
    pub seed: u64,
    /// A seeded RNG, reset per test so runs are reproducible.
    pub rng: StdRng,
    /// A per-test value derived from the seed, used to vary ClientHello randoms.
    pub salt: u64,
    /// The certificate and key the server was started with.
    pub material: Material,
    /// The certificate store, for tests that need another kind.
    pub certs: Arc<CertStore>,
    /// The options the server was started with.
    pub options: ServerOptions,
    /// The `openssl` binary, when one is on PATH (interop stages).
    pub openssl: Option<PathBuf>,
    /// The server process, lent to the test by the runner for the duration of the body.
    pub server: Option<ServerHandle>,
    /// Informational lines the test wants in the report even when it passes.
    pub notes: Vec<String>,
    /// Set by [`Ctx::skip`] when the test decided at run time that it does not apply.
    pub skip: Option<String>,
}

impl Ctx {
    /// Build a context for one test.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        server: &ServerHandle,
        is_reference: bool,
        material: Material,
        certs: Arc<CertStore>,
        timeout: Duration,
        seed: u64,
        test_index: u64,
        openssl: Option<PathBuf>,
    ) -> Ctx {
        let salt = seed ^ test_index.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        Ctx {
            server_name: server.spec.name.clone(),
            is_reference,
            addr: server.addr,
            timeout,
            seed,
            rng: StdRng::seed_from_u64(salt),
            salt,
            material,
            certs,
            options: server.spec.options.clone(),
            openssl,
            server: None,
            notes: Vec::new(),
            skip: None,
        }
    }

    /// Add an informational line to the test's report entry, pass or fail.
    pub fn note(&mut self, line: impl Into<String>) {
        self.notes.push(line.into());
    }

    /// Decide at run time that this test does not apply, with a reason that is always
    /// printed.
    ///
    /// `Test::skip_on` covers "this server never passes this test"; this covers "the thing
    /// this test needs is not installed here" — a missing `openssl` for the interop stage,
    /// say. The body should return `Ok(())` straight after calling it.
    pub fn skip(&mut self, reason: impl Into<String>) {
        self.skip = Some(reason.into());
    }

    /// The default ClientHello configuration for this test: seeded, and different from
    /// every other test's so two connections never look identical on the wire.
    pub fn config(&self) -> ClientConfig {
        ClientConfig::seeded(self.salt)
    }

    /// A configuration whose randomness differs from [`Ctx::config`]'s, for tests that
    /// open several connections.
    pub fn config_n(&self, n: u64) -> ClientConfig {
        ClientConfig::seeded(self.salt.wrapping_add(n.wrapping_mul(0x9e37_79b9)))
    }

    /// Open a raw TCP connection with no TLS state.
    pub async fn connect(&self) -> Result<TlsConn, Failure> {
        TlsConn::connect(self.addr, self.timeout)
            .await
            .map_err(|e| Failure::tls(e).note(format!("connecting to {}", self.addr)))
    }

    /// A client that has connected but sent nothing.
    pub async fn client(&self) -> Result<Client, Failure> {
        self.client_with(self.config()).await
    }

    /// A client with an explicit configuration, connected but silent.
    pub async fn client_with(&self, config: ClientConfig) -> Result<Client, Failure> {
        let conn = self.connect().await?;
        Ok(Client::over(conn, config))
    }

    /// A client that has completed a full handshake.
    pub async fn handshake(&self) -> Result<Client, Failure> {
        self.handshake_with(self.config()).await
    }

    /// A client that has completed a full handshake with an explicit configuration.
    pub async fn handshake_with(&self, config: ClientConfig) -> Result<Client, Failure> {
        let mut client = self.client_with(config).await?;
        match client.handshake().await {
            Ok(()) => Ok(client),
            Err(e) => Err(handshake_failure(e, &client)),
        }
    }

    /// A client that has handshaked and presented the suite's client certificate.
    ///
    /// Pair it with `ServerOptions::requesting_client_cert()` or
    /// `requiring_client_cert()`; without one of those the server never asks, and this
    /// fails saying so.
    pub async fn handshake_authenticated(&self) -> Result<Client, Failure> {
        let auth = self
            .certs
            .client_auth()
            .map_err(|e| harness(format!("cannot generate the client certificate: {e:#}")))?;
        let mut client = self.client().await?;
        match client.handshake_with_client_auth(&auth).await {
            Ok(()) => Ok(client),
            Err(e) => Err(handshake_failure(e, &client)),
        }
    }

    /// A client that handshakes but answers the CertificateRequest with an empty list.
    pub async fn handshake_declining(&self) -> Result<Client, Failure> {
        let mut client = self.client().await?;
        match client.handshake_declining_client_auth().await {
            Ok(()) => Ok(client),
            Err(e) => Err(handshake_failure(e, &client)),
        }
    }

    /// Prove the server is still *accepting* after whatever the test just did to it,
    /// without requiring a handshake.
    ///
    /// Stage 01 uses this rather than [`Ctx::expect_still_serving`], so that a learner who
    /// has written an accept loop and nothing else can get the first stage green.
    pub async fn expect_accepting(&self, after: &str) -> Result<(), Failure> {
        let conn = self.connect().await.map_err(|f| {
            f.note(format!(
                "the server stopped accepting TCP connections after {after}"
            ))
        })?;
        drop(conn);
        Ok(())
    }

    /// Prove the server still answers a ClientHello with a handshake record, without
    /// requiring the whole handshake.
    ///
    /// This is the liveness check sections A and B use: a learner working on the record
    /// layer has a ServerHello and nothing after it, and a robustness test has no business
    /// demanding a Finished.
    pub async fn expect_still_answering(&self, after: &str) -> Result<(), Failure> {
        let message =
            crate::stages::hello_message(&self.config_n(0xa11e)).map_err(crate::stages::harness)?;
        let mut conn = self.connect().await.map_err(|f| {
            f.note(format!(
                "the server stopped accepting connections after {after}"
            ))
        })?;
        conn.write_record(crate::tls::ContentType::Handshake, &message)
            .await
            .map_err(Failure::tls)?;
        let record = conn.read_record().await.map_err(|e| {
            Failure::tls(e).note(format!(
                "a fresh connection must still be answered after {after}"
            ))
        })?;
        if record.content_type != crate::tls::ContentType::Handshake {
            return Err(Failure::new(
                FailureKind::Protocol,
                format!(
                    "after {after}, a fresh ClientHello was answered with a {} record instead \
                     of a handshake record",
                    record.content_type.name()
                ),
            )
            .block("the record the server sent", &record.raw));
        }
        Ok(())
    }

    /// Prove the server is still able to serve a fresh connection after whatever the test
    /// just did to it.
    pub async fn expect_still_serving(&self, after: &str) -> Result<(), Failure> {
        let mut client = self.client_with(self.config_n(0xfeed)).await.map_err(|f| {
            f.note(format!(
                "the server stopped accepting connections after {after}"
            ))
        })?;
        match client.handshake().await {
            Ok(()) => Ok(()),
            Err(e) => Err(handshake_failure(e, &client).note(format!(
                "a clean handshake on a new connection must still work after {after}"
            ))),
        }
    }

    /// The server's captured output, for a note.
    pub fn server_output(&self) -> String {
        self.server
            .as_ref()
            .map(|s| s.output_tail(10))
            .unwrap_or_default()
    }
}

/// Turn a handshake error into a failure carrying the connection's trace, the transcript,
/// the key schedule and the bytes of every message that had been seen.
pub fn handshake_failure(e: TlsError, client: &Client) -> Failure {
    let mut f = Failure::tls(e);
    if !client.conn.trace.is_empty() {
        f.notes.push("connection trace:".to_string());
        f.notes.extend(client.conn.trace_lines());
    }
    let transcript = client.transcript_lines();
    if !transcript.is_empty() {
        f.notes.push("transcript hashes:".to_string());
        f.notes.extend(transcript);
    }
    let schedule = client.schedule_lines();
    if !schedule.is_empty() {
        f.notes.push("key schedule:".to_string());
        f.notes.extend(schedule);
    }
    for (title, bytes) in [
        ("client_hello", &client.client_hello_bytes),
        ("server_hello", &client.server_hello_bytes),
        ("encrypted_extensions", &client.encrypted_extensions_bytes),
        ("certificate", &client.certificate_bytes),
        ("certificate_verify", &client.certificate_verify_bytes),
        ("server finished", &client.server_finished_bytes),
    ] {
        f = f.block(title, bytes);
    }
    if let Some(record) = client.conn.last_record_bytes() {
        f = f.block("the last record the server sent", &record);
    }
    f
}

/// A failure for a test whose own set-up went wrong.
pub fn harness(message: impl Into<String>) -> Failure {
    Failure::new(FailureKind::Harness, message)
}

/// The certificate kind a test's server options asked for.
pub fn cert_kind(options: &ServerOptions) -> CertKind {
    options.cert
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stage_is_unique_and_ordered() {
        let stages = all();
        // Counted from the sections rather than written here, so growing the suite does not
        // mean editing a number in two places.
        let planned: usize = sections().iter().map(|s| s.stages.len()).sum();
        assert_eq!(
            stages.len(),
            planned,
            "every planned stage is implemented, and no more"
        );
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
    fn the_suite_is_at_least_as_big_as_the_plan_promises() {
        let total: usize = all().iter().map(|s| s.tests.len()).sum();
        assert!(
            total >= 320,
            "the plan promises at least 320 tests, the registry has {total}"
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
    fn an_ext_stage_makes_every_one_of_its_tests_ext() {
        for s in all() {
            if !s.ext {
                continue;
            }
            for t in &s.tests {
                assert!(
                    s.test_is_ext(t),
                    "stage {} is ext but test '{}' is not hidden by --skip-ext",
                    s.number,
                    t.name
                );
            }
        }
    }

    #[test]
    fn skips_always_carry_a_reason() {
        for s in all() {
            for t in &s.tests {
                for (server, reason) in &t.skip_on {
                    assert!(
                        !reason.trim().is_empty(),
                        "stage {} test '{}' skips {server} with no reason",
                        s.number,
                        t.name
                    );
                }
            }
        }
    }

    #[test]
    fn timeout_override_wins_and_min_only_raises_the_floor() {
        crate::tls_test!(nothing, |_ctx| { Ok(()) });
        let default = Duration::from_millis(10_000);
        let plain = Test::new("plain", nothing);
        assert_eq!(plain.timeout(default), default);
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
    fn a_seeded_context_config_varies_per_test_index() {
        let a = ClientConfig::seeded(1);
        let b = ClientConfig::seeded(1u64 ^ 2u64.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        assert_ne!(a.random(), b.random());
    }
}
