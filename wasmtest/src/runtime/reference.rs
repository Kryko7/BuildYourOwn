//! The reference runtime: `wasmtime`, downloaded once and cached.
//!
//! The release tarball lands in `~/.cache/wasmtest` (override with `WASMTEST_CACHE`) behind
//! an `flock`ed lock file, so parallel runs share one copy, and its SHA-256 is checked
//! against a digest pinned in this file before it is unpacked. GitHub publishes no checksum
//! file next to the asset, so pinning the digest here is both the integrity check and the
//! version lock: a tarball that does not hash to this value is not the build this suite was
//! validated against, and the harness refuses it.

use anyhow::{bail, Context, Result};
use sha2::Digest;
use std::path::{Path, PathBuf};

/// The `wasmtime` version `runtimes.yaml` pins when it does not say otherwise.
pub const DEFAULT_VERSION: &str = "48.0.2";

/// The platform triple of the release asset this build downloads.
pub const TARGET: &str = "x86_64-linux";

/// SHA-256 of `wasmtime-v48.0.2-x86_64-linux.tar.xz`, as published by the Bytecode Alliance.
pub const SHA256_48_0_2: &str = "f2b0ad1ce9253f2f9a38793c2c42cd1cba4e90b27dc40d685eaf723dc8438d94";

/// The digest this suite expects for `version`, when it knows one.
pub fn expected_sha256(version: &str) -> Option<&'static str> {
    match version {
        "48.0.2" => Some(SHA256_48_0_2),
        _ => None,
    }
}

/// The cache directory holding the tarball and its unpacked tree.
pub fn cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("WASMTEST_CACHE") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".cache").join("wasmtest")
}

/// The name of the release asset for `version`, which is also the directory inside it.
pub fn asset_stem(version: &str) -> String {
    format!("wasmtime-v{version}-{TARGET}")
}

/// Where an already-unpacked distribution of `version` lives.
pub fn dist_dir(version: &str) -> PathBuf {
    cache_dir().join(asset_stem(version))
}

/// The `wasmtime` binary of an unpacked distribution.
pub fn exe_path(version: &str) -> PathBuf {
    dist_dir(version).join("wasmtime")
}

/// True when the distribution is unpacked and usable.
pub fn is_installed(version: &str) -> bool {
    exe_path(version).is_file()
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
                "cannot lock the wasmtest cache: {}",
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

/// Download and unpack `wasmtime` `version` if it is not already in the cache, and return
/// the path of the binary.
pub fn ensure_installed(version: &str) -> Result<PathBuf> {
    if is_installed(version) {
        return Ok(exe_path(version));
    }
    let cache = cache_dir();
    let _lock = CacheLock::acquire(&cache)?;
    if is_installed(version) {
        return Ok(exe_path(version));
    }
    let stem = asset_stem(version);
    // The tarball keeps a version-free name so an existing cache populated by hand is
    // reused; the unpacked directory carries the version, which is what `is_installed` asks.
    let tarball = cache.join("wasmtime.tar.xz");
    let url = format!(
        "https://github.com/bytecodealliance/wasmtime/releases/download/v{version}/{stem}.tar.xz"
    );
    if !tarball.is_file() {
        eprintln!("wasmtest: downloading {url} (~11 MB, once)");
        let part = cache.join("wasmtime.tar.xz.part");
        fetch(&url, &part)?;
        std::fs::rename(&part, &tarball)?;
    }
    if let Some(expected) = expected_sha256(version) {
        verify_sha256(&tarball, expected).with_context(|| {
            format!(
                "{} is not the wasmtime {version} release this suite was validated against; \
                 delete it and let the harness download it again",
                tarball.display()
            )
        })?;
    }
    let staging = cache.join(format!(".extract-{version}"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let status = std::process::Command::new("tar")
        .arg("xJf")
        .arg(&tarball)
        .arg("-C")
        .arg(&staging)
        .status()
        .context("cannot run tar (the release asset is a .tar.xz)")?;
    if !status.success() {
        bail!("tar failed to unpack {}", tarball.display());
    }
    let unpacked = staging.join(&stem);
    if !unpacked.join("wasmtime").is_file() {
        bail!(
            "{} does not look like a wasmtime distribution",
            unpacked.display()
        );
    }
    let dir = dist_dir(version);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&unpacked, &dir)?;
    let _ = std::fs::remove_dir_all(&staging);
    Ok(exe_path(version))
}

fn fetch(url: &str, to: &Path) -> Result<()> {
    // curl/wget rather than an HTTP client crate: this runs once, and it keeps reqwest and a
    // TLS stack out of the dependency tree.
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

fn verify_sha256(file: &Path, expected: &str) -> Result<()> {
    let bytes = std::fs::read(file).with_context(|| format!("cannot read {}", file.display()))?;
    let mut h = sha2::Sha256::new();
    h.update(&bytes);
    let actual = h
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if actual != expected.to_lowercase() {
        bail!("sha256 mismatch\n  expected {expected}\n  actual   {actual}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_and_dist_paths_are_derived_consistently() {
        let dist = dist_dir("48.0.2");
        assert!(dist.starts_with(cache_dir()));
        assert!(dist.ends_with("wasmtime-v48.0.2-x86_64-linux"));
        assert!(exe_path("48.0.2").ends_with("wasmtime"));
    }

    #[test]
    fn the_pinned_digest_is_a_sha256() {
        let d = expected_sha256("48.0.2").unwrap_or_default();
        assert_eq!(d.len(), 64, "a SHA-256 is 64 hex characters");
        assert!(d.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(expected_sha256("1.0.0").is_none(), "only pinned versions");
    }

    #[test]
    fn sha256_verification_rejects_a_tampered_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("x.tar.xz");
        std::fs::write(&file, b"hello").expect("write");
        let mut h = sha2::Sha256::new();
        h.update(b"hello");
        let digest = h
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        verify_sha256(&file, &digest).expect("must verify");
        std::fs::write(&file, b"tampered").expect("write");
        assert!(verify_sha256(&file, &digest).is_err());
    }
}
