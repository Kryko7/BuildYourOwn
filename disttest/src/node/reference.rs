//! The references: real etcd for the `node` and `cluster` ladders, and this crate's own
//! example binaries for the `primitives` and `algorithms` ladders.
//!
//! The etcd release tarball lands in `~/.cache/disttest` (override with `DISTTEST_CACHE`)
//! behind an `flock`ed lock file, so parallel runs share one copy, and its SHA-256 is
//! checked against a pinned digest before it is unpacked. Nothing is ever downloaded when
//! the distribution is already there, which is why a `--validate` run does no network I/O.

use anyhow::{bail, Context, Result};
use sha2::Digest;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

/// Default etcd version when `targets.yaml` does not pin one.
pub const DEFAULT_VERSION: &str = "3.7.1";
/// SHA-256 of `etcd-v3.7.1-linux-amd64.tar.gz` as published on the release page.
const PINNED_SHA256_3_7_1: &str =
    "e8cd3fa8064c98137c5dbd78b76f969417ace84efb83c481041d7a52ffdd8fb9";
const RELEASES: &str = "https://github.com/etcd-io/etcd/releases/download";

/// The cache directory holding the tarball and its unpacked tree.
pub fn cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("DISTTEST_CACHE") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".cache").join("disttest")
}

/// The platform slug etcd names its release assets with.
pub fn platform() -> &'static str {
    // The suite has only ever been run on 64-bit Linux; anything else is a clear error
    // rather than a download that quietly fetches the wrong binary.
    match std::env::consts::ARCH {
        "aarch64" => "linux-arm64",
        _ => "linux-amd64",
    }
}

/// Where an already-downloaded distribution of `version` lives.
pub fn dist_dir(version: &str) -> PathBuf {
    cache_dir().join(format!("etcd-v{version}-{}", platform()))
}

/// True when the distribution is unpacked and usable.
pub fn is_installed(version: &str) -> bool {
    dist_dir(version).join("etcd").is_file()
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
                "cannot lock the disttest cache: {}",
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

/// The digest this suite expects for a version, when it pins one.
pub fn pinned_sha256(version: &str) -> Option<&'static str> {
    match version {
        "3.7.1" => Some(PINNED_SHA256_3_7_1),
        _ => None,
    }
}

/// Download and unpack etcd `version` if it is not already in the cache.
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
    let name = format!("etcd-v{version}-{}.tar.gz", platform());
    // An earlier hand-primed cache may have used the short name.
    let tgz = [cache.join(&name), cache.join("etcd.tgz")]
        .into_iter()
        .find(|p| p.is_file())
        .unwrap_or_else(|| cache.join(&name));
    let url = format!("{RELEASES}/v{version}/{name}");
    if !tgz.is_file() {
        eprintln!("disttest: downloading {url} (~23 MB, once)");
        let part = cache.join(format!("{name}.part"));
        fetch(&url, &part)?;
        std::fs::rename(&part, &tgz)?;
    }
    verify_sha256(&tgz, version, &cache, &name)
        .with_context(|| format!("checksum of {} does not match", tgz.display()))?;
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
    let unpacked = staging.join(format!("etcd-v{version}-{}", platform()));
    if !unpacked.join("etcd").is_file() {
        bail!("{} does not look like an etcd release", unpacked.display());
    }
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&unpacked, &dir)?;
    let _ = std::fs::remove_dir_all(&staging);
    Ok(dir)
}

fn fetch(url: &str, to: &Path) -> Result<()> {
    // curl/wget rather than an HTTP client crate: this runs once, and a machine that can
    // reach github.com always has one of them.
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

/// Check the tarball against the pinned digest, falling back to the release `SHA256SUMS`
/// for a version this build does not pin.
fn verify_sha256(file: &Path, version: &str, cache: &Path, asset: &str) -> Result<()> {
    let actual = sha256_of(file)?;
    if let Some(expected) = pinned_sha256(version) {
        if actual != expected {
            bail!("sha256 mismatch\n  expected {expected}\n  actual   {actual}");
        }
        return Ok(());
    }
    let sums = cache.join(format!("SHA256SUMS-{version}"));
    if !sums.is_file() {
        fetch(&format!("{RELEASES}/v{version}/SHA256SUMS"), &sums)?;
    }
    let text = std::fs::read_to_string(&sums)?;
    let expected = text
        .lines()
        .find(|l| l.ends_with(asset) || l.ends_with(&format!("./{asset}")))
        .and_then(|l| l.split_whitespace().next())
        .map(str::to_lowercase)
        .with_context(|| format!("{asset} is not listed in {}", sums.display()))?;
    if actual != expected {
        bail!("sha256 mismatch\n  expected {expected}\n  actual   {actual}");
    }
    Ok(())
}

/// Lower-case SHA-256 of a file.
pub fn sha256_of(file: &Path) -> Result<String> {
    let bytes = std::fs::read(file).with_context(|| format!("cannot read {}", file.display()))?;
    let mut h = sha2::Sha256::new();
    h.update(&bytes);
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The version string the cached distribution reports, for the run header.
pub fn dist_version(dist: &Path) -> Option<String> {
    let out = std::process::Command::new(dist.join("etcd"))
        .arg("--version")
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("etcd Version: "))
        .map(|s| s.trim().to_string())
}

// ---------------------------------------------------------------------------------------
// The crate's own example binaries
// ---------------------------------------------------------------------------------------

/// The crate root: the directory holding `Cargo.toml`, found from the cwd or the binary.
pub fn crate_root() -> Result<PathBuf> {
    static ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
    let found = ROOT.get_or_init(|| {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Ok(cwd) = std::env::current_dir() {
            candidates.push(cwd);
        }
        if let Ok(exe) = std::env::current_exe() {
            let mut d = exe.parent().map(Path::to_path_buf);
            for _ in 0..5 {
                if let Some(dir) = d {
                    candidates.push(dir.clone());
                    d = dir.parent().map(Path::to_path_buf);
                }
            }
        }
        for c in candidates {
            let mut dir = Some(c);
            while let Some(d) = dir {
                if d.join("Cargo.toml").is_file() && d.join("src/stages").is_dir() {
                    return Some(d);
                }
                dir = d.parent().map(Path::to_path_buf);
            }
        }
        None
    });
    found
        .clone()
        .context("cannot find the disttest crate root (run from the disttest directory)")
}

/// Locate one of this crate's example binaries, building it once if it is missing.
///
/// `reference_primitives` and `reference_algorithms` are the two CLI ladders' references, and
/// `broken_node` is the deliberately wrong node the README uses to show red output.
pub fn example_binary(name: &str) -> Result<PathBuf> {
    let root = crate_root()?;
    for profile in ["release", "debug"] {
        let p = root
            .join("target")
            .join(profile)
            .join("examples")
            .join(name);
        if p.is_file() {
            return Ok(p);
        }
    }
    eprintln!("disttest: building the {name} example (once)");
    let status = std::process::Command::new(std::env::var("CARGO").unwrap_or("cargo".into()))
        .arg("build")
        .arg("--release")
        .arg("--example")
        .arg(name)
        .current_dir(&root)
        .status()
        .with_context(|| format!("cannot run cargo to build the {name} example"))?;
    if !status.success() {
        bail!("cargo could not build the {name} example");
    }
    let p = root.join("target/release/examples").join(name);
    if !p.is_file() {
        bail!(
            "cargo built the {name} example but {} is missing",
            p.display()
        );
    }
    Ok(p)
}

/// How long a reference node may take to answer its first request.
pub const REFERENCE_BOOT_TIMEOUT: Duration = Duration::from_millis(20_000);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_and_dist_paths_are_derived_consistently() {
        let dist = dist_dir("3.7.1");
        assert!(dist.starts_with(cache_dir()));
        assert!(dist.ends_with(format!("etcd-v3.7.1-{}", platform())));
    }

    #[test]
    fn the_pinned_version_carries_a_digest() {
        assert_eq!(pinned_sha256(DEFAULT_VERSION).map(str::len), Some(64));
        assert!(pinned_sha256("0.0.1").is_none());
    }

    #[test]
    fn sha256_matches_coreutils() {
        let dir = tempfile::tempdir().expect("tempdir");
        let f = dir.path().join("x");
        std::fs::write(&f, b"hello").expect("write");
        assert_eq!(
            sha256_of(&f).expect("digest"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn the_crate_root_is_found_from_the_test_binary() {
        let root = crate_root().expect("crate root");
        assert!(root.join("Cargo.toml").is_file());
        assert!(root.join("targets.yaml").is_file());
    }
}
