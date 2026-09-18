//! The linker under test: how it is resolved, and how one link is invoked.
//!
//! The contract is the GNU ld flag subset, which is why the reference is literally
//! `/usr/bin/ld`:
//!
//! ```text
//! ./your_program.sh -o <out> [-e <entry>] [-L <dir>] [-l <name>] <input.o|input.a>...
//! ```
//!
//! The harness never asks for anything else — no `-shared`, no `-pie`, no `--dynamic-linker`.

pub mod reference;

use crate::config::{LinkerDef, LinkerKind, Placeholders};
use crate::exec;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A resolved linker, ready to be invoked.
#[derive(Debug, Clone)]
pub struct LinkerHandle {
    /// The definition it came from.
    pub def: LinkerDef,
    /// The program to run.
    pub program: PathBuf,
    /// Arguments that come before everything the harness adds.
    pub base_args: Vec<String>,
    /// The version string, when the linker was willing to say (reference linkers only).
    pub version: Option<String>,
}

impl LinkerHandle {
    /// Resolve a definition into something runnable.
    ///
    /// A `reference` linker is GNU ld: `$LD` when set, `/usr/bin/ld` otherwise, and its
    /// version is recorded so the report can say what the suite was validated against.
    pub fn resolve(def: &LinkerDef) -> Result<LinkerHandle> {
        match def.kind {
            LinkerKind::Reference => {
                let program = reference::locate()?;
                let version = reference::version(&program);
                Ok(LinkerHandle {
                    def: def.clone(),
                    program,
                    base_args: Vec::new(),
                    version,
                })
            }
            LinkerKind::External => {
                let (first, rest) = def
                    .command
                    .split_first()
                    .context("the linker definition has an empty command")?;
                let program = PathBuf::from(first);
                let resolved = if program.is_absolute() {
                    program
                } else {
                    // Relative to the current directory, which is where `your_program.sh`
                    // lives when a learner runs `linktest --linker ./your_program.sh`.
                    let cwd = std::env::current_dir()?;
                    let candidate = cwd.join(&program);
                    if candidate.exists() {
                        candidate
                    } else {
                        exec::which(first).unwrap_or(program)
                    }
                };
                if !resolved.exists() {
                    bail!(
                        "the linker '{}' does not exist (from linkers.yaml entry '{}')",
                        resolved.display(),
                        def.name
                    );
                }
                Ok(LinkerHandle {
                    def: def.clone(),
                    program: resolved,
                    base_args: rest.to_vec(),
                    version: None,
                })
            }
        }
    }

    /// Run one link in `dir`, with `args` appended to the base arguments.
    pub fn invoke(&self, dir: &Path, args: &[String], timeout: Duration) -> std::io::Result<exec::Output> {
        let ph = Placeholders {
            tmp: dir.to_path_buf(),
        };
        let mut argv = self.base_args.clone();
        argv.extend(args.iter().cloned());
        let cwd = match &self.def.cwd {
            // A linker with an explicit cwd keeps it (that is how `./your_program.sh` finds
            // the rest of its project); the input paths the harness passes are absolute, so
            // the working directory never changes what gets linked.
            Some(c) if !c.trim().is_empty() && c != "." => PathBuf::from(ph.apply(c)),
            Some(_) => std::env::current_dir().unwrap_or_else(|_| dir.to_path_buf()),
            None => dir.to_path_buf(),
        };
        let mut spec = exec::Spec::new(&self.program, &argv, &cwd, timeout);
        for (k, v) in &self.def.env {
            spec = spec.env(k, &ph.apply(v));
        }
        exec::run(&spec)
    }

    /// How the report names this linker.
    pub fn describe(&self) -> String {
        match &self.version {
            Some(v) => format!("{} ({})", self.program.display(), v),
            None => self.program.display().to_string(),
        }
    }
}
