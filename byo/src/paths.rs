//! Where `byo` keeps its data and how it finds the tester binaries.
//!
//! Everything lives under `$BYO_HOME` (default `~/.local/share/byo`), which `install.sh`
//! populates. The tester binaries are looked for next to the running `byo` binary first
//! (so an uninstalled `target/release/byo` finds its siblings) and then on `PATH`.

use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

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

    /// `shelltest --tests-dir` wants the directory of `NN_name.yaml` files.
    pub fn tests_dir(&self) -> PathBuf {
        self.home.join("tests/stages")
    }

    /// `shelltest --shells-file`.
    pub fn shells_file(&self) -> PathBuf {
        self.home.join("shells.yaml")
    }

    /// `kafkatest --brokers-file`.
    pub fn brokers_file(&self) -> PathBuf {
        self.home.join("brokers.yaml")
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

/// One of the two tracks this repo supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Track {
    /// Build your own shell, tested by `shelltest`.
    Shell,
    /// Build your own Kafka broker, tested by `kafkatest`.
    Kafka,
}

impl Track {
    /// The lowercase name used in configs, the DB and the API.
    pub fn as_str(self) -> &'static str {
        match self {
            Track::Shell => "shell",
            Track::Kafka => "kafka",
        }
    }

    /// The tester binary that drives this track.
    pub fn tester(self) -> &'static str {
        match self {
            Track::Shell => "shelltest",
            Track::Kafka => "kafkatest",
        }
    }

    /// The tester flag that selects the program under test.
    pub fn target_flag(self) -> &'static str {
        match self {
            Track::Shell => "--shell",
            Track::Kafka => "--broker",
        }
    }

    /// Parse a track name.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "shell" => Ok(Track::Shell),
            "kafka" => Ok(Track::Kafka),
            other => Err(anyhow!(
                "unknown track '{other}' (expected 'shell' or 'kafka')"
            )),
        }
    }

    /// Both tracks, in display order.
    pub const ALL: [Track; 2] = [Track::Shell, Track::Kafka];
}

impl std::fmt::Display for Track {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Find a tester binary: next to this executable, then on `PATH`.
pub fn find_tester(name: &str) -> Result<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let cand = dir.join(name);
            if is_executable(&cand) {
                return Ok(cand);
            }
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let cand = dir.join(name);
            if is_executable(&cand) {
                return Ok(cand);
            }
        }
    }
    Err(anyhow!(
        "cannot find the '{name}' binary next to `byo` or on PATH.\n\
         Run ./install.sh from the BuildYourOwn repo, and make sure ~/.local/bin is on your PATH."
    ))
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
    fn track_names_round_trip() {
        for t in Track::ALL {
            assert_eq!(Track::parse(t.as_str()).unwrap(), t);
        }
        assert!(Track::parse("redis").is_err());
    }

    #[test]
    fn paths_hang_off_byo_home() {
        let p = Paths {
            home: PathBuf::from("/x"),
        };
        assert_eq!(p.db(), PathBuf::from("/x/byo.db"));
        assert_eq!(p.tests_dir(), PathBuf::from("/x/tests/stages"));
        assert_eq!(
            p.catalog(Track::Kafka),
            PathBuf::from("/x/catalog.kafka.json")
        );
    }
}
