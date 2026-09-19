//! The reference server: the system OpenSSL, run as `s_server -tls1_3 -rev`.
//!
//! There is nothing to download and nothing to model. `--validate` points the suite at the
//! same binary a learner has on their own machine, which is the whole reason the track's
//! program contract is a subset of `s_server`'s flags.
//!
//! Two flags are added that the contract does not name:
//!
//! - `-tls1_3` pins the reference to the version the suite is about. Without it OpenSSL
//!   would happily negotiate TLS 1.2 with the "a 1.2-only client is refused" stage and the
//!   expectation would be wrong.
//! - `-quiet` stops `s_server` printing the peer's certificate and the session on every
//!   connection, which would otherwise be megabytes of captured output per stage.

use crate::config::ServerOptions;
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

/// The environment variable that overrides which `openssl` is used.
pub const OPENSSL_ENV: &str = "TLSTEST_OPENSSL";

/// The `openssl` binary the reference server runs, and the `s_server` subcommand.
pub fn program() -> Result<Vec<String>> {
    let exe = openssl_path()?;
    Ok(vec![exe.to_string_lossy().to_string(), "s_server".into()])
}

/// Where `openssl` is.
pub fn openssl_path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os(OPENSSL_ENV) {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Ok(p);
        }
        anyhow::bail!(
            "{OPENSSL_ENV} points at {}, which is not a file",
            p.display()
        );
    }
    which("openssl").context(
        "cannot find `openssl` on PATH; the reference server is the system OpenSSL 3.x \
         (set TLSTEST_OPENSSL to point at one)",
    )
}

/// The flags only the reference needs.
pub fn extra_flags(options: &ServerOptions) -> Vec<String> {
    let mut flags = vec!["-tls1_3".to_string(), "-quiet".to_string()];
    // `s_server` is a demo tool: its verification callback prints what went wrong and then
    // returns "carry on", so `-Verify` alone rejects a client that sends *no* certificate
    // but happily establishes a connection with one signed by nobody. `-verify_return_error`
    // makes it enforce what the contract's `-Verify` already means. A server under test is
    // expected to enforce it without being asked twice.
    if options.client_auth == Some(true) {
        flags.push("-verify_return_error".to_string());
    }
    flags.extend(options.reference_args.clone());
    flags
}

/// The version string of the `openssl` that will be used, e.g. `OpenSSL 3.6.4`.
pub fn version() -> Result<String> {
    let exe = openssl_path()?;
    let out = Command::new(&exe)
        .arg("version")
        .output()
        .with_context(|| format!("cannot run {} version", exe.display()))?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Whether this OpenSSL understands a flag, by asking `s_server -help`.
///
/// `-early_data` and `-num_tickets` are the two the advanced stages need and the two most
/// likely to be missing from an old build, so a stage can skip with a real reason rather
/// than fail.
pub fn supports_flag(flag: &str) -> bool {
    let Ok(exe) = openssl_path() else {
        return false;
    };
    let Ok(out) = Command::new(&exe).args(["s_server", "-help"]).output() else {
        return false;
    };
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    text.lines()
        .any(|l| l.split_whitespace().next() == Some(flag))
}

/// The `s_client` command line an interop stage runs, as words.
pub fn s_client_argv(port: u16, extra: &[&str]) -> Result<Vec<String>> {
    let exe = openssl_path()?;
    let mut argv = vec![
        exe.to_string_lossy().to_string(),
        "s_client".to_string(),
        "-connect".to_string(),
        format!("127.0.0.1:{port}"),
        "-tls1_3".to_string(),
        "-quiet".to_string(),
        "-verify_return_error".to_string(),
        "-CAfile".to_string(),
    ];
    argv.extend(extra.iter().map(|a| (*a).to_string()));
    Ok(argv)
}

fn which(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_is_openssl_s_server() {
        let Ok(argv) = program() else {
            eprintln!("skipping: no openssl on PATH");
            return;
        };
        assert_eq!(argv.len(), 2);
        assert!(argv[0].ends_with("openssl"), "{argv:?}");
        assert_eq!(argv[1], "s_server");
    }

    #[test]
    fn the_reference_is_pinned_to_tls_1_3_and_kept_quiet() {
        let flags = extra_flags(&ServerOptions::default());
        assert_eq!(flags, vec!["-tls1_3".to_string(), "-quiet".to_string()]);
        let flags =
            extra_flags(&ServerOptions::default().with_reference_args(&["-num_tickets", "0"]));
        assert_eq!(&flags[2..], &["-num_tickets".to_string(), "0".to_string()]);
    }

    #[test]
    fn the_system_openssl_is_a_3_x_that_has_the_flags_the_suite_needs() {
        let Ok(v) = version() else {
            eprintln!("skipping: no openssl on PATH");
            return;
        };
        assert!(
            v.starts_with("OpenSSL 3"),
            "the reference must be OpenSSL 3.x, got {v}"
        );
        for flag in ["-rev", "-naccept", "-tls1_3"] {
            assert!(supports_flag(flag), "this openssl has no {flag}: {v}");
        }
    }
}
