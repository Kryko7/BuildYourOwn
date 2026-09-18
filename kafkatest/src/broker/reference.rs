//! The reference broker: Apache Kafka, downloaded once, formatted per stage, run in KRaft
//! combined mode on random free ports.
//!
//! The tarball lands in `~/.cache/kafkatest` (override with `KAFKATEST_CACHE`) behind an
//! `flock`ed lock file, so parallel runs share one copy. Its SHA-512 is checked against the
//! `.sha512` published next to it before it is unpacked.

use super::{free_port, BrokerSpec};
use anyhow::{bail, Context, Result};
use sha2::Digest;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Scala version of the binary distribution the harness downloads.
pub const SCALA_VERSION: &str = "2.13";
/// Default Apache Kafka version when `brokers.yaml` does not pin one.
pub const DEFAULT_VERSION: &str = "4.1.2";
const MIRROR: &str = "https://archive.apache.org/dist/kafka";

/// The cache directory holding the tarball and its unpacked tree.
pub fn cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("KAFKATEST_CACHE") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".cache").join("kafkatest")
}

/// Where an already-downloaded distribution of `version` lives.
pub fn dist_dir(version: &str) -> PathBuf {
    cache_dir().join(format!("kafka_{SCALA_VERSION}-{version}"))
}

/// True when the distribution is unpacked and usable.
pub fn is_installed(version: &str) -> bool {
    dist_dir(version)
        .join("bin/kafka-server-start.sh")
        .is_file()
}

/// A lock file held for the whole download+extract, so parallel runs do not race.
struct CacheLock(std::fs::File);

impl CacheLock {
    fn acquire(dir: &Path) -> Result<CacheLock> {
        std::fs::create_dir_all(dir)?;
        let f = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(dir.join(".download.lock"))?;
        // SAFETY: flock on a file descriptor we own; LOCK_EX blocks until we get it.
        let rc = unsafe { libc::flock(std::os::unix::io::AsRawFd::as_raw_fd(&f), libc::LOCK_EX) };
        if rc != 0 {
            bail!(
                "cannot lock the kafkatest cache: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(CacheLock(f))
    }
}

impl Drop for CacheLock {
    fn drop(&mut self) {
        // SAFETY: the fd is still open and owned by this struct.
        unsafe {
            libc::flock(
                std::os::unix::io::AsRawFd::as_raw_fd(&self.0),
                libc::LOCK_UN,
            );
        }
    }
}

/// Download and unpack Apache Kafka `version` if it is not already in the cache.
pub fn ensure_installed(version: &str) -> Result<PathBuf> {
    let dir = dist_dir(version);
    if is_installed(version) {
        return Ok(dir);
    }
    let cache = cache_dir();
    let _lock = CacheLock::acquire(&cache)?;
    if is_installed(version) {
        return Ok(dir);
    }
    let name = format!("kafka_{SCALA_VERSION}-{version}.tgz");
    let tgz = cache.join(&name);
    let url = format!("{MIRROR}/{version}/{name}");
    if !tgz.is_file() {
        eprintln!("kafkatest: downloading {url} (~130 MB, once)");
        let part = cache.join(format!("{name}.part"));
        fetch(&url, &part)?;
        std::fs::rename(&part, &tgz)?;
    }
    let sums = cache.join(format!("{name}.sha512"));
    if !sums.is_file() {
        fetch(&format!("{url}.sha512"), &sums)?;
    }
    verify_sha512(&tgz, &sums).with_context(|| {
        format!(
            "checksum of {} does not match {}",
            tgz.display(),
            sums.display()
        )
    })?;
    let staging = cache.join(format!(".extract-{version}"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let status = std::process::Command::new("tar")
        .arg("xzf")
        .arg(&tgz)
        .arg("-C")
        .arg(&staging)
        .status()
        .context("cannot run tar")?;
    if !status.success() {
        bail!("tar failed to unpack {}", tgz.display());
    }
    let unpacked = staging.join(format!("kafka_{SCALA_VERSION}-{version}"));
    if !unpacked.join("bin/kafka-server-start.sh").is_file() {
        bail!(
            "{} does not look like a Kafka distribution",
            unpacked.display()
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&unpacked, &dir)?;
    let _ = std::fs::remove_dir_all(&staging);
    Ok(dir)
}

fn fetch(url: &str, to: &Path) -> Result<()> {
    // curl/wget rather than an HTTP client crate: this runs once, and the machine that can
    // reach archive.apache.org always has one of them.
    let attempts: [(&str, Vec<String>); 2] = [
        (
            "curl",
            vec![
                "-sSfL".into(),
                "--retry".into(),
                "3".into(),
                "-o".into(),
                to.to_string_lossy().to_string(),
                url.to_string(),
            ],
        ),
        (
            "wget",
            vec![
                "-q".into(),
                "-O".into(),
                to.to_string_lossy().to_string(),
                url.to_string(),
            ],
        ),
    ];
    let mut last = String::new();
    for (prog, args) in attempts {
        match std::process::Command::new(prog).args(&args).status() {
            Ok(s) if s.success() => return Ok(()),
            Ok(s) => last = format!("{prog} exited with {s}"),
            Err(e) => last = format!("cannot run {prog}: {e}"),
        }
    }
    bail!("cannot download {url}: {last}")
}

fn verify_sha512(file: &Path, sums: &Path) -> Result<()> {
    let text = std::fs::read_to_string(sums)?;
    // The published file is either "<hex>  <name>" or GNU-coreutils' spaced-uppercase form.
    let expected: String = text
        .split(':')
        .next_back()
        .unwrap_or(&text)
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect::<String>()
        .to_lowercase();
    let expected = expected
        .get(..128)
        .context("the .sha512 file does not contain a 512-bit digest")?
        .to_string();
    let bytes = std::fs::read(file)?;
    let mut h = sha2::Sha512::new();
    h.update(&bytes);
    let actual = h
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if actual != expected {
        bail!("sha512 mismatch\n  expected {expected}\n  actual   {actual}");
    }
    Ok(())
}

/// Ask `kafka-storage.sh random-uuid` for a cluster id, once per process.
pub fn cluster_id(dist: &Path) -> Result<String> {
    static ID: OnceLock<String> = OnceLock::new();
    if let Some(id) = ID.get() {
        return Ok(id.clone());
    }
    let out = std::process::Command::new(dist.join("bin/kafka-storage.sh"))
        .arg("random-uuid")
        .env("KAFKA_HEAP_OPTS", "-Xmx128M -Xms32M")
        .output()
        .context("cannot run kafka-storage.sh random-uuid")?;
    if !out.status.success() {
        bail!(
            "kafka-storage.sh random-uuid failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if id.is_empty() {
        bail!("kafka-storage.sh random-uuid printed nothing");
    }
    Ok(ID.get_or_init(|| id).clone())
}

/// The `message.max.bytes` both generated properties files carry.
///
/// Well above anything else the suite produces and far below Kafka's 1 MB default, so
/// stage 37 can send an oversized batch without moving a megabyte around.
pub const MESSAGE_MAX_BYTES: usize = 65_536;

/// Write the KRaft combined-mode `server.properties` for one reference broker instance.
pub fn write_properties(
    path: &Path,
    log_dir: &Path,
    port: u16,
    controller_port: u16,
) -> Result<()> {
    let text = format!(
        "# written by kafkatest for the reference broker\n\
         process.roles=broker,controller\n\
         node.id=1\n\
         controller.quorum.voters=1@127.0.0.1:{controller_port}\n\
         listeners=PLAINTEXT://0.0.0.0:{port},CONTROLLER://0.0.0.0:{controller_port}\n\
         advertised.listeners=PLAINTEXT://127.0.0.1:{port}\n\
         listener.security.protocol.map=CONTROLLER:PLAINTEXT,PLAINTEXT:PLAINTEXT\n\
         controller.listener.names=CONTROLLER\n\
         inter.broker.listener.name=PLAINTEXT\n\
         log.dirs={log}\n\
         num.partitions=1\n\
         default.replication.factor=1\n\
         offsets.topic.replication.factor=1\n\
         offsets.topic.num.partitions=1\n\
         share.coordinator.state.topic.replication.factor=1\n\
         share.coordinator.state.topic.min.isr=1\n\
         transaction.state.log.replication.factor=1\n\
         transaction.state.log.num.partitions=1\n\
         transaction.state.log.min.isr=1\n\
         group.initial.rebalance.delay.ms=0\n\
         auto.create.topics.enable=false\n\
         num.network.threads=2\n\
         num.io.threads=2\n\
         background.threads=4\n\
         log.flush.interval.messages=1\n\
         message.max.bytes={max_message}\n\
         metadata.log.max.record.bytes.between.snapshots=104857600\n",
        log = log_dir.display(),
        max_message = MESSAGE_MAX_BYTES,
    );
    std::fs::write(path, text).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(())
}

/// Run `kafka-storage.sh format` over a freshly written properties file.
pub fn format_storage(dist: &Path, props: &Path, cluster: &str) -> Result<()> {
    let out = std::process::Command::new(dist.join("bin/kafka-storage.sh"))
        .arg("format")
        .arg("-t")
        .arg(cluster)
        .arg("-c")
        .arg(props)
        .env("KAFKA_HEAP_OPTS", "-Xmx256M -Xms64M")
        .output()
        .context("cannot run kafka-storage.sh format")?;
    if !out.status.success() {
        bail!(
            "kafka-storage.sh format failed:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

/// Prepare (download, write properties, format) and describe one reference broker instance.
pub fn prepare(version: &str, tmp: &Path, boot_timeout: Duration) -> Result<BrokerSpec> {
    let dist = ensure_installed(version)?;
    let log_dir = tmp.join("kraft-combined-logs");
    let props = tmp.join("server.properties");
    std::fs::create_dir_all(&log_dir)?;
    let port = free_port()?;
    let controller_port = free_port()?;
    write_properties(&props, &log_dir, port, controller_port)?;
    format_storage(&dist, &props, &cluster_id(&dist)?)?;
    let jvm_logs = tmp.join("jvm-logs");
    std::fs::create_dir_all(&jvm_logs)?;
    Ok(BrokerSpec {
        name: "apache_kafka".to_string(),
        argv: vec![
            dist.join("bin/kafka-server-start.sh")
                .to_string_lossy()
                .to_string(),
            props.to_string_lossy().to_string(),
        ],
        cwd: dist.clone(),
        env: vec![
            ("KAFKA_HEAP_OPTS".into(), "-Xmx640M -Xms256M".into()),
            (
                "KAFKA_JVM_PERFORMANCE_OPTS".into(),
                "-XX:TieredStopAtLevel=1 -XX:+UseSerialGC -XX:CICompilerCount=2 -Xshare:auto \
                 -Djava.awt.headless=true"
                    .into(),
            ),
            ("LOG_DIR".into(), jvm_logs.to_string_lossy().to_string()),
        ],
        port,
        log_dir,
        props,
        tmp: tmp.to_path_buf(),
        boot_timeout,
    })
}

/// Wait for a line in the broker's stdout, so tests never race a half-started broker.
///
/// Apache Kafka accepts TCP connections well before the broker is in state STARTED; without
/// this a stage's first request would sometimes see `COORDINATOR_NOT_AVAILABLE`.
pub fn wait_for_marker(tmp: &Path, marker: &str, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let path = tmp.join("broker.stdout");
    loop {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if text.contains(marker) {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            bail!(
                "the reference broker never logged {marker:?} within {} ms",
                timeout.as_millis()
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// The marker line Apache Kafka prints once the broker is serving.
pub const READY_MARKER: &str = "Kafka Server started";

/// Run one of the tarball's CLI tools (`kafka-topics.sh`, `kafka-dump-log.sh`, ...).
///
/// Returns (stdout, stderr); errors only when the tool could not be started.
pub fn run_tool(dist: &Path, tool: &str, args: &[String]) -> Result<(String, String, bool)> {
    let out = std::process::Command::new(dist.join("bin").join(tool))
        .args(args)
        .env("KAFKA_HEAP_OPTS", "-Xmx320M -Xms64M")
        .output()
        .with_context(|| format!("cannot run {tool}"))?;
    Ok((
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.success(),
    ))
}

/// Write a note into a file, used by tests that keep their tmp dir.
pub fn write_note(path: &Path, text: &str) {
    if let Ok(mut f) = std::fs::File::create(path) {
        let _ = f.write_all(text.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_and_dist_paths_are_derived_consistently() {
        let dist = dist_dir("4.1.2");
        assert!(dist.starts_with(cache_dir()));
        assert!(dist.ends_with("kafka_2.13-4.1.2"));
    }

    #[test]
    fn sha512_verification_accepts_the_apache_format() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("x.tgz");
        std::fs::write(&file, b"hello").expect("write");
        let mut h = sha2::Sha512::new();
        h.update(b"hello");
        let digest = h
            .finalize()
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<String>();
        // Apache publishes the digest in spaced, upper-case groups.
        let spaced = digest
            .as_bytes()
            .chunks(8)
            .map(|c| String::from_utf8_lossy(c).to_string())
            .collect::<Vec<_>>()
            .join(" ");
        let sums = dir.path().join("x.tgz.sha512");
        std::fs::write(&sums, format!("x.tgz: {spaced}\n")).expect("write sums");
        verify_sha512(&file, &sums).expect("must verify");

        std::fs::write(&file, b"tampered").expect("write");
        assert!(verify_sha512(&file, &sums).is_err());
    }

    #[test]
    fn properties_pin_single_replica_settings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("server.properties");
        write_properties(&p, &dir.path().join("logs"), 1111, 2222).expect("write");
        let text = std::fs::read_to_string(&p).expect("read");
        assert!(
            text.contains("controller.quorum.voters=1@127.0.0.1:2222"),
            "{text}"
        );
        assert!(text.contains("auto.create.topics.enable=false"), "{text}");
        assert!(
            text.contains("offsets.topic.replication.factor=1"),
            "{text}"
        );
        assert!(
            text.contains(&format!("message.max.bytes={MESSAGE_MAX_BYTES}")),
            "stage 37 needs a small message.max.bytes\n{text}"
        );
    }
}
