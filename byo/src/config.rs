//! `byo.toml` — the per-project config written by `byo init`.
//!
//! ```toml
//! track = "shell"
//! shell = "bash"          # a name registered in shells.yaml …
//! # command = "./my_shell"  # … or a path to the program you are building
//! # port = 9092
//! # log_dir = "/tmp/kraft-combined-logs"
//! ```

use crate::paths::Track;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The file name `byo` looks for, walking up from the current directory.
pub const FILE: &str = "byo.toml";

/// How the tester should be pointed at the program under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// A name registered in `shells.yaml` / `brokers.yaml` (e.g. `bash`).
    Registered,
    /// A path to an executable the tester runs directly.
    Command,
}

impl TargetKind {
    /// The word stored in the DB and printed by `byo status`.
    pub fn as_str(self) -> &'static str {
        match self {
            TargetKind::Registered => "registered",
            TargetKind::Command => "command",
        }
    }
}

/// The parsed `byo.toml`.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ProjectFile {
    /// `shell` or `kafka`.
    pub track: String,
    /// A registered shell name (shell track only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    /// A registered broker name (kafka track only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub broker: Option<String>,
    /// A path to the program under test.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Port the broker must listen on (kafka track).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Kafka log directory (kafka track).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_dir: Option<String>,
}

/// A validated project: the config plus the directory that holds `byo.toml`.
#[derive(Debug, Clone)]
pub struct Project {
    /// Directory containing `byo.toml`.
    pub root: PathBuf,
    /// Which track this project belongs to.
    pub track: Track,
    /// The value passed to `--shell` / `--broker`.
    pub target: String,
    /// Whether `target` is a registered name or a path.
    pub target_kind: TargetKind,
    /// Optional port override.
    pub port: Option<u16>,
    /// Optional log-directory override.
    pub log_dir: Option<String>,
}

impl Project {
    /// Validate a parsed file into a project rooted at `root`.
    pub fn from_file(root: PathBuf, f: ProjectFile) -> Result<Self> {
        let track = Track::parse(&f.track)?;
        let registered = match track {
            Track::Shell => {
                if f.broker.is_some() {
                    bail!(
                        "`broker` is a kafka-track key; a shell project uses `shell` or `command`"
                    );
                }
                f.shell.clone()
            }
            Track::Kafka => {
                if f.shell.is_some() {
                    bail!(
                        "`shell` is a shell-track key; a kafka project uses `broker` or `command`"
                    );
                }
                f.broker.clone()
            }
        };
        let (target, target_kind) = match (registered, f.command.clone()) {
            (Some(_), Some(_)) => bail!(
                "{FILE} sets both a registered name and `command`; keep exactly one so it is \
                 unambiguous which program is tested"
            ),
            (Some(name), None) if !name.trim().is_empty() => (name, TargetKind::Registered),
            (None, Some(cmd)) if !cmd.trim().is_empty() => (cmd, TargetKind::Command),
            _ => bail!(
                "{FILE} must set `command = \"./your_program.sh\"` or a registered name \
                 (`{} = \"...\"`)",
                match track {
                    Track::Shell => "shell",
                    Track::Kafka => "broker",
                }
            ),
        };
        Ok(Self {
            root,
            track,
            target,
            target_kind,
            port: f.port,
            log_dir: f.log_dir,
        })
    }

    /// Render the file this project would be written as.
    pub fn to_toml(&self) -> String {
        let mut s = String::new();
        s.push_str("# byo project config — created by `byo init`.\n");
        s.push_str("# Run `byo test --stage 1` here, `byo status` for progress, `byo site` for the map.\n\n");
        s.push_str(&format!("track = {:?}\n", self.track.as_str()));
        match (self.track, self.target_kind) {
            (Track::Shell, TargetKind::Registered) => {
                s.push_str(&format!(
                    "# a shell registered in shells.yaml\nshell = {:?}\n",
                    self.target
                ));
            }
            (Track::Kafka, TargetKind::Registered) => {
                s.push_str(&format!(
                    "# a broker registered in brokers.yaml\nbroker = {:?}\n",
                    self.target
                ));
            }
            (_, TargetKind::Command) => {
                s.push_str(&format!(
                    "# the program you are building\ncommand = {:?}\n",
                    self.target
                ));
            }
        }
        if let Some(p) = self.port {
            s.push_str(&format!("port = {p}\n"));
        }
        if let Some(d) = &self.log_dir {
            s.push_str(&format!("log_dir = {d:?}\n"));
        }
        s
    }

    /// Write `byo.toml` into the project root.
    pub fn save(&self) -> Result<PathBuf> {
        let path = self.root.join(FILE);
        std::fs::write(&path, self.to_toml())
            .with_context(|| format!("cannot write {}", path.display()))?;
        Ok(path)
    }
}

/// Parse a `byo.toml` from text.
pub fn parse(text: &str, root: PathBuf) -> Result<Project> {
    let f: ProjectFile = toml::from_str(text).context("invalid byo.toml")?;
    Project::from_file(root, f)
}

/// Walk up from `start` looking for `byo.toml`.
pub fn find(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        let cand = d.join(FILE);
        if cand.is_file() {
            return Some(cand);
        }
        dir = d.parent();
    }
    None
}

/// Load the project for the current directory, with a helpful error when there is none.
pub fn load_from(cwd: &Path) -> Result<Project> {
    let path = find(cwd).ok_or_else(|| {
        anyhow!(
            "no {FILE} here (or in any parent of {}).\n\n\
             `byo` works inside the repo where you are building your shell or broker.\n\
             Create one with:\n\
             \x20   byo init shell --command ./your_program.sh     # build your own shell\n\
             \x20   byo init kafka --command ./your_program.sh     # build your own Kafka broker\n\n\
             To try the harness against a shell that already works: byo init shell --shell bash",
            cwd.display()
        )
    })?;
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    let root = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    parse(&text, root).with_context(|| format!("in {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str) -> Result<Project> {
        parse(text, PathBuf::from("/proj"))
    }

    #[test]
    fn registered_shell() {
        let pr = p("track = \"shell\"\nshell = \"bash\"\n").unwrap();
        assert_eq!(pr.track, Track::Shell);
        assert_eq!(pr.target, "bash");
        assert_eq!(pr.target_kind, TargetKind::Registered);
    }

    #[test]
    fn command_shell() {
        let pr = p("track = \"shell\"\ncommand = \"./my_shell\"\n").unwrap();
        assert_eq!(pr.target_kind, TargetKind::Command);
        assert_eq!(pr.target, "./my_shell");
    }

    #[test]
    fn kafka_with_port_and_logdir() {
        let pr = p("track = \"kafka\"\ncommand = \"./your_program.sh\"\nport = 9092\nlog_dir = \"/tmp/l\"\n")
            .unwrap();
        assert_eq!(pr.track, Track::Kafka);
        assert_eq!(pr.port, Some(9092));
        assert_eq!(pr.log_dir.as_deref(), Some("/tmp/l"));
    }

    #[test]
    fn rejects_both_target_forms() {
        let e = p("track = \"shell\"\nshell = \"bash\"\ncommand = \"./x\"\n").unwrap_err();
        assert!(e.to_string().contains("exactly one"), "{e}");
    }

    #[test]
    fn rejects_missing_target() {
        assert!(p("track = \"shell\"\n").is_err());
    }

    #[test]
    fn rejects_unknown_track() {
        assert!(p("track = \"redis\"\ncommand = \"./x\"\n").is_err());
    }

    #[test]
    fn rejects_cross_track_key() {
        let e = p("track = \"shell\"\nbroker = \"my_broker\"\n").unwrap_err();
        assert!(e.to_string().contains("kafka-track key"), "{e}");
    }

    #[test]
    fn round_trips_through_toml() {
        for text in [
            "track = \"shell\"\nshell = \"bash\"\n",
            "track = \"kafka\"\ncommand = \"./b\"\nport = 9099\nlog_dir = \"/tmp/k\"\n",
        ] {
            let a = p(text).unwrap();
            let b = parse(&a.to_toml(), PathBuf::from("/proj")).unwrap();
            assert_eq!(a.track, b.track);
            assert_eq!(a.target, b.target);
            assert_eq!(a.target_kind, b.target_kind);
            assert_eq!(a.port, b.port);
            assert_eq!(a.log_dir, b.log_dir);
        }
    }

    #[test]
    fn find_walks_up() {
        let dir = tempfile::tempdir().unwrap();
        let deep = dir.path().join("a/b/c");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(
            dir.path().join(FILE),
            "track = \"shell\"\nshell = \"bash\"\n",
        )
        .unwrap();
        let found = find(&deep).unwrap();
        assert_eq!(found, dir.path().join(FILE));
        let pr = load_from(&deep).unwrap();
        assert_eq!(pr.root, dir.path());
    }

    #[test]
    fn missing_config_explains_init() {
        let dir = tempfile::tempdir().unwrap();
        let e = load_from(dir.path()).unwrap_err();
        let s = format!("{e:#}");
        assert!(s.contains("byo init shell"), "{s}");
        assert!(s.contains("byo init kafka"), "{s}");
    }
}
