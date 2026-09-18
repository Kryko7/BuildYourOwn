//! Where `byo` keeps its data and how it finds the tester binaries.
//!
//! Everything lives under `$BYO_HOME` (default `~/.local/share/byo`), which `install.sh`
//! populates. Which files a track needs is registry data ([`crate::track::DataFile`]), so
//! this module only turns a `$BYO_HOME`-relative path into an absolute one. The tester
//! binaries are looked for next to the running `byo` binary first (so an uninstalled
//! `target/release/byo` finds its siblings) and then on `PATH`.

use crate::track::DataFile;
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

pub use crate::track::Track;

/// Resolved locations of the data directory and the files inside it.
#[derive(Debug, Clone)]
pub struct Paths {
    /// `$BYO_HOME`, the data directory.
    pub home: PathBuf,
}

impl Paths {
    /// Resolve `$BYO_HOME`, falling back to the platform data dir.
    pub fn resolve() -> Result<Self> {
        let home = match std::env::var_os("BYO_HOME") {
            Some(v) if !v.is_empty() => PathBuf::from(v),
            _ => dirs::data_dir()
                .ok_or_else(|| anyhow!("cannot determine a data directory; set BYO_HOME"))?
                .join("byo"),
        };
        Ok(Self { home })
    }

    /// The SQLite database.
    pub fn db(&self) -> PathBuf {
        self.home.join("byo.db")
    }

    /// The built site that `byo site` serves.
    pub fn site(&self) -> PathBuf {
        self.home.join("site")
    }

    /// A `$BYO_HOME`-relative path, absolute.
    pub fn data(&self, rel: &str) -> PathBuf {
        self.home.join(rel)
    }

    /// Where one of a track's data files lives.
    pub fn data_file(&self, f: &DataFile) -> PathBuf {
        self.data(f.rel)
    }

    /// True when a track's data file is installed (a non-empty directory, or a file).
    pub fn has_data_file(&self, f: &DataFile) -> bool {
        let p = self.data_file(f);
        if f.dir {
            std::fs::read_dir(&p).is_ok_and(|mut d| d.next().is_some())
        } else {
            p.is_file()
        }
    }

    /// The stage catalog for a track, as served by `GET /api/catalog/:track`.
    pub fn catalog(&self, track: Track) -> PathBuf {
        self.home.join(format!("catalog.{}.json", track.as_str()))
    }

    /// Create the data directory if it is missing.
    pub fn ensure(&self) -> Result<()> {
        std::fs::create_dir_all(&self.home)
            .with_context(|| format!("cannot create data directory {}", self.home.display()))?;
        Ok(())
    }
}

/// Find a tester binary: next to this executable, then on `PATH`.
pub fn find_tester(name: &str) -> Result<PathBuf> {
    locate_tester(name).ok_or_else(|| {
        anyhow!(
            "cannot find the '{name}' binary next to `byo` or on PATH.\n\
             Run ./install.sh from the BuildYourOwn repo, and make sure ~/.local/bin is on your PATH."
        )
    })
}

/// Find a tester binary, with "not installed" as an ordinary answer rather than an error.
pub fn locate_tester(name: &str) -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let cand = dir.join(name);
            if is_executable(&cand) {
                return Some(cand);
            }
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|cand| is_executable(cand))
}

/// True when a track's tester is installed.
pub fn tester_installed(track: Track) -> bool {
    locate_tester(track.tester()).is_some()
}

/// True when the path is a file we are allowed to execute.
pub fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(p) {
            Ok(m) => m.is_file() && m.permissions().mode() & 0o111 != 0,
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_hang_off_byo_home() {
        let p = Paths {
            home: PathBuf::from("/x"),
        };
        assert_eq!(p.db(), PathBuf::from("/x/byo.db"));
        assert_eq!(p.data("tests/stages"), PathBuf::from("/x/tests/stages"));
        assert_eq!(
            p.catalog(Track::KAFKA),
            PathBuf::from("/x/catalog.kafka.json")
        );
        assert_eq!(
            p.catalog(Track::WASM),
            PathBuf::from("/x/catalog.wasm.json")
        );
    }

    #[test]
    fn data_files_come_from_the_registry() {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths {
            home: d.path().to_path_buf(),
        };
        for f in Track::SHELL.def().data_files {
            assert!(!paths.has_data_file(f), "{} should be missing", f.rel);
        }
        std::fs::create_dir_all(d.path().join("tests/stages")).unwrap();
        // An empty directory is not an installed suite.
        let tests = Track::SHELL.def().data_files[0];
        assert!(!paths.has_data_file(&tests));
        std::fs::write(d.path().join("tests/stages/01_x.yaml"), "stage: 1").unwrap();
        assert!(paths.has_data_file(&tests));
        std::fs::write(d.path().join("shells.yaml"), "shells: {}").unwrap();
        assert!(paths.has_data_file(&Track::SHELL.def().data_files[1]));
    }

    #[test]
    fn a_missing_tester_is_an_answer_not_a_panic() {
        assert!(locate_tester("definitely-not-a-real-tester-binary").is_none());
        let e = find_tester("definitely-not-a-real-tester-binary")
            .unwrap_err()
            .to_string();
        assert!(e.contains("install.sh"), "{e}");
    }
}
