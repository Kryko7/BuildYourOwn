//! The reference linker: GNU ld, already on the machine.
//!
//! Unlike the other tracks in this repo there is nothing to download. `ld` is part of
//! binutils, it is installed wherever a compiler is, and `linktest --linker gnu_ld
//! --validate --all` is the suite's self-check against it.

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where the reference linker is looked for, in order: `$LD`, then these paths.
pub const CANDIDATES: &[&str] = &["/usr/bin/ld", "/usr/local/bin/ld", "/bin/ld"];

/// Resolve GNU ld.
pub fn locate() -> Result<PathBuf> {
    if let Some(env) = std::env::var_os("LD") {
        let p = PathBuf::from(&env);
        if p.is_file() {
            return Ok(p);
        }
        if let Some(found) = crate::exec::which(&p.to_string_lossy()) {
            return Ok(found);
        }
        bail!(
            "$LD is set to '{}', which is not an executable",
            p.display()
        );
    }
    for c in CANDIDATES {
        let p = PathBuf::from(c);
        if p.is_file() {
            return Ok(p);
        }
    }
    if let Some(found) = crate::exec::which("ld") {
        return Ok(found);
    }
    bail!(
        "cannot find GNU ld (looked at $LD, {}, and $PATH). Install binutils, or point $LD \
         at a linker, to use --linker gnu_ld.",
        CANDIDATES.join(", ")
    )
}

/// The first line of `ld --version`, when it answers.
pub fn version(program: &Path) -> Option<String> {
    let out = crate::exec::run(&crate::exec::Spec::new(
        program,
        &["--version".to_string()],
        Path::new("."),
        Duration::from_secs(5),
    ))
    .ok()?;
    out.stdout.lines().next().map(str::trim).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_ld_is_an_error_with_advice() {
        // Point $LD at something that is definitely not there and check the wording.
        let previous = std::env::var_os("LD");
        // SAFETY: single-threaded test; restored immediately afterwards.
        unsafe { std::env::set_var("LD", "/definitely/not/a/linker") };
        let err = locate().expect_err("a bogus $LD must not resolve");
        assert!(err.to_string().contains("$LD"), "{err}");
        match previous {
            // SAFETY: as above.
            Some(v) => unsafe { std::env::set_var("LD", v) },
            None => unsafe { std::env::remove_var("LD") },
        }
    }
}
