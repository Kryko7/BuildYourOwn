//! `byo.toml` — the per-project config written by `byo init`.
//!
//! ```toml
//! track = "shell"
//! shell = "bash"          # a name registered in shells.yaml …
//! # command = "./my_shell"  # … or a path to the program you are building
//! ```
//!
//! Which keys are legal is entirely [registry][crate::track] data: a track names its target
//! with `track.def().target_key` (`shell`, `broker`, `runtime`, `server`, `linker`) and may
//! declare extra keys (the kafka track's `port` and `log_dir`). Nothing here knows which
//! tracks exist, so a new track needs no changes in this file.

use crate::track::{ExtraKind, Track, TRACKS};
use anyhow::{anyhow, bail, Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use toml::Value;

/// The file name `byo` looks for, walking up from the current directory.
pub const FILE: &str = "byo.toml";

/// How the tester should be pointed at the program under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// A name registered in the track's catalog of targets (e.g. `bash` in `shells.yaml`).
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

/// A validated project: the config plus the directory that holds `byo.toml`.
#[derive(Debug, Clone)]
pub struct Project {
    /// Directory containing `byo.toml`.
    pub root: PathBuf,
    /// Which track this project belongs to.
    pub track: Track,
    /// The value passed to the track's target flag (`--shell`, `--broker`, …).
    pub target: String,
    /// Whether `target` is a registered name or a path.
    pub target_kind: TargetKind,
    /// The track's extra keys, validated, keyed by registry name (kafka: `port`, `log_dir`).
    pub extras: BTreeMap<String, String>,
}

impl Project {
    /// The value of one extra key, if this project sets it.
    pub fn extra(&self, name: &str) -> Option<&str> {
        self.extras.get(name).map(String::as_str)
    }

    /// Render the file this project would be written as.
    pub fn to_toml(&self) -> String {
        let def = self.track.def();
        let mut s = String::new();
        s.push_str("# byo project config — created by `byo init`.\n");
        s.push_str("# Run `byo test --stage 1` here, `byo status` for progress, `byo site` for the map.\n\n");
        s.push_str(&format!("track = {:?}   # {}\n", def.id, def.title));
        match self.target_kind {
            TargetKind::Registered => s.push_str(&format!(
                "# a target registered for this track, passed as `{} {}`\n{} = {:?}\n",
                def.target_flag, self.target, def.target_key, self.target
            )),
            TargetKind::Command => s.push_str(&format!(
                "# the program you are building\ncommand = {:?}\n",
                self.target
            )),
        }
        for key in def.extra_keys {
            if let Some(v) = self.extra(key.name) {
                s.push_str(&format!("# {}\n", key.doc));
                match key.kind {
                    ExtraKind::Int => s.push_str(&format!("{} = {v}\n", key.name)),
                    ExtraKind::Text => s.push_str(&format!("{} = {v:?}\n", key.name)),
                }
            }
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

/// Build a project from already-separated parts (what `byo init` has in hand).
///
/// `registered` and `command` are the two ways to name the program under test; exactly one
/// must be given. `extras` are checked against the track's registry keys.
pub fn build(
    root: PathBuf,
    track: Track,
    registered: Option<String>,
    command: Option<String>,
    extras: BTreeMap<String, String>,
) -> Result<Project> {
    let def = track.def();
    let (target, target_kind) = match (registered, command) {
        (Some(_), Some(_)) => bail!(
            "name the program under test once: either `{}` (a registered target) or `command` \
             (a path), not both",
            def.target_key
        ),
        (Some(name), None) if !name.trim().is_empty() => {
            (name.trim().to_string(), TargetKind::Registered)
        }
        (None, Some(cmd)) if !cmd.trim().is_empty() => {
            (cmd.trim().to_string(), TargetKind::Command)
        }
        _ => bail!(
            "a {} project must set `command = \"{}\"` or a registered target (`{} = \"...\"`)",
            def.id,
            def.default_command,
            def.target_key
        ),
    };
    let mut checked = BTreeMap::new();
    for (name, value) in extras {
        let key = track.extra_key(&name).ok_or_else(|| {
            anyhow!(
                "`{name}` is not a key of the {} track ({})",
                def.id,
                allowed_keys(track)
            )
        })?;
        checked.insert(name, key.validate(&value)?);
    }
    Ok(Project {
        root,
        track,
        target,
        target_kind,
        extras: checked,
    })
}

/// The keys a track accepts, for error messages.
fn allowed_keys(track: Track) -> String {
    let def = track.def();
    let mut keys = vec![
        "track".to_string(),
        "command".to_string(),
        def.target_key.to_string(),
    ];
    keys.extend(def.extra_keys.iter().map(|k| k.name.to_string()));
    format!("accepted here: {}", keys.join(", "))
}

/// Parse a `byo.toml` from text.
pub fn parse(text: &str, root: PathBuf) -> Result<Project> {
    let table: toml::Table = toml::from_str(text).context("invalid byo.toml")?;
    let track_name = table
        .get("track")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("{FILE} must start with `track = \"...\"`"))?;
    let track = Track::parse(track_name)?;
    let def = track.def();

    let mut registered = None;
    let mut command = None;
    let mut extras = BTreeMap::new();
    for (key, value) in &table {
        match key.as_str() {
            "track" => {}
            "command" => command = Some(as_text(key, value)?),
            k if k == def.target_key => registered = Some(as_text(key, value)?),
            k => {
                // A key that belongs to a *different* track is the common typo; say so.
                if let Some(other) = Track::by_target_key(k) {
                    bail!(
                        "`{k}` is the {} track's key; a {} project uses `{}` or `command`",
                        other.as_str(),
                        def.id,
                        def.target_key
                    );
                }
                if track.extra_key(k).is_none() {
                    bail!("unknown key `{k}` in {FILE} ({})", allowed_keys(track));
                }
                extras.insert(k.to_string(), as_text(key, value)?);
            }
        }
    }
    build(root, track, registered, command, extras)
}

/// Accept a string or an integer for any key, so `port = 9092` and `port = "9092"` agree.
fn as_text(key: &str, value: &Value) -> Result<String> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Integer(i) => Ok(i.to_string()),
        other => bail!(
            "`{key}` must be a string or a number in {FILE}, got {}",
            other.type_str()
        ),
    }
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
    let path = find(cwd).ok_or_else(|| anyhow!("{}", no_project_here(cwd)))?;
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    let root = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    parse(&text, root).with_context(|| format!("in {}", path.display()))
}

/// The "you are not in a project" message, listing every registered track.
fn no_project_here(cwd: &Path) -> String {
    let mut s = format!(
        "no {FILE} here (or in any parent of {}).\n\n\
         `byo` works inside the repo where you are building your own thing.\n\
         Create one with:\n",
        cwd.display()
    );
    for t in TRACKS {
        s.push_str(&format!(
            "    byo init {:<6} --command {:<20}  # {}\n",
            t.id, t.default_command, t.title
        ));
    }
    s.push_str(
        "\nTo try the harness against something that already works: byo init shell --shell bash",
    );
    s
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
        assert_eq!(pr.track, Track::SHELL);
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
        assert_eq!(pr.track, Track::KAFKA);
        assert_eq!(pr.extra("port"), Some("9092"));
        assert_eq!(pr.extra("log_dir"), Some("/tmp/l"));
    }

    #[test]
    fn every_registered_track_parses_a_project() {
        for t in Track::all() {
            let text = format!(
                "track = {:?}\ncommand = \"./your_program.sh\"\n",
                t.as_str()
            );
            let pr = p(&text).unwrap();
            assert_eq!(pr.track, t);
            let named = format!("track = {:?}\n{} = \"ref\"\n", t.as_str(), t.target_key());
            let pr = p(&named).unwrap();
            assert_eq!(pr.target, "ref");
            assert_eq!(pr.target_kind, TargetKind::Registered);
        }
    }

    #[test]
    fn rejects_both_target_forms() {
        let e = p("track = \"shell\"\nshell = \"bash\"\ncommand = \"./x\"\n").unwrap_err();
        assert!(e.to_string().contains("not both"), "{e}");
    }

    #[test]
    fn rejects_missing_target() {
        assert!(p("track = \"shell\"\n").is_err());
    }

    #[test]
    fn rejects_unknown_track() {
        let e = p("track = \"redis\"\ncommand = \"./x\"\n").unwrap_err();
        assert!(e.to_string().contains("wasm"), "{e}");
    }

    #[test]
    fn rejects_cross_track_key() {
        let e = p("track = \"shell\"\nbroker = \"my_broker\"\n").unwrap_err();
        assert!(e.to_string().contains("kafka track's key"), "{e}");
        let e = p("track = \"wasm\"\nserver = \"openssl\"\n").unwrap_err();
        assert!(e.to_string().contains("tls track's key"), "{e}");
    }

    #[test]
    fn rejects_extra_keys_of_another_track() {
        let e = p("track = \"wasm\"\ncommand = \"./x\"\nport = 9092\n").unwrap_err();
        assert!(e.to_string().contains("unknown key `port`"), "{e}");
        let e = p("track = \"kafka\"\ncommand = \"./x\"\nport = \"nine\"\n").unwrap_err();
        assert!(e.to_string().contains("must be a number"), "{e}");
    }

    #[test]
    fn round_trips_through_toml() {
        for text in [
            "track = \"shell\"\nshell = \"bash\"\n",
            "track = \"kafka\"\ncommand = \"./b\"\nport = 9099\nlog_dir = \"/tmp/k\"\n",
            "track = \"wasm\"\nruntime = \"wasmtime\"\n",
            "track = \"tls\"\ncommand = \"./your_program.sh\"\n",
            "track = \"link\"\nlinker = \"gnu_ld\"\n",
        ] {
            let a = p(text).unwrap();
            let b = parse(&a.to_toml(), PathBuf::from("/proj")).unwrap();
            assert_eq!(a.track, b.track);
            assert_eq!(a.target, b.target);
            assert_eq!(a.target_kind, b.target_kind);
            assert_eq!(a.extras, b.extras);
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
    fn missing_config_lists_every_track() {
        let dir = tempfile::tempdir().unwrap();
        let e = load_from(dir.path()).unwrap_err();
        let s = format!("{e:#}");
        for t in Track::all() {
            assert!(s.contains(&format!("byo init {}", t.as_str())), "{s}");
        }
    }

    #[test]
    fn build_rejects_unregistered_extras() {
        let extras = BTreeMap::from([("port".to_string(), "1".to_string())]);
        let e = build(
            PathBuf::from("/p"),
            Track::LINK,
            None,
            Some("./x".into()),
            extras,
        )
        .unwrap_err();
        assert!(e.to_string().contains("not a key of the link track"), "{e}");
    }
}
