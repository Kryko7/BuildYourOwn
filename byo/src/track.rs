//! The track registry — the one place that knows a track exists.
//!
//! Every track in this repo (build your own shell, Kafka broker, WebAssembly runtime,
//! TLS 1.3 server, ELF linker, distributed key-value store) is one [`TrackDef`] in
//! [`TRACKS`]. Everything else in `byo`
//! — `byo init`, `byo test`, `byo status`, `byo doctor`, the JSON API and the database —
//! reads that table instead of matching on a track name, so adding a sixth track is a data
//! change here plus a row in `install.sh`, not new code in six modules.
//!
//! A [`Track`] is a handle into the table (an index), so it stays `Copy`, `Ord` and
//! hashable exactly like the two-value enum it replaced. `Track::SHELL` and friends are
//! constants rather than enum variants; `Track::all()` iterates the registry in display
//! order.
//!
//! The tracks that are not built yet are still registered. That is deliberate: `byo` has to
//! answer "is the `wasm` track installed?" with "not yet" rather than "no such track", and
//! `install.sh` has to skip a missing directory instead of failing.

use anyhow::{anyhow, Result};

/// The type of a per-track `byo.toml` key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtraKind {
    /// A non-negative integer (a port, a count).
    Int,
    /// Free text (a path, a name).
    Text,
}

/// A track-specific key in `byo.toml`, forwarded to the tester as a flag.
///
/// Only the kafka track has any today (`port`, `log_dir`); the shape exists so the next
/// track that needs one does not have to grow a new `match`.
#[derive(Debug, Clone, Copy)]
pub struct ExtraKey {
    /// The key as written in `byo.toml`, e.g. `port`.
    pub name: &'static str,
    /// The tester flag it becomes, e.g. `--port`.
    pub flag: &'static str,
    /// What values are accepted.
    pub kind: ExtraKind,
    /// The value `byo init` writes when the user gives none.
    pub default: Option<&'static str>,
    /// One line of explanation, used as a comment in the generated `byo.toml`.
    pub doc: &'static str,
}

/// A data file (or directory) `install.sh` copies into `$BYO_HOME` and `byo test` points
/// the tester at, so the tester works from any working directory.
#[derive(Debug, Clone, Copy)]
pub struct DataFile {
    /// The tester flag, e.g. `--shells-file`.
    pub flag: &'static str,
    /// Path relative to `$BYO_HOME`, e.g. `shells.yaml`.
    pub rel: &'static str,
    /// True when `rel` is a directory rather than a file.
    pub dir: bool,
}

/// Something outside this repo that a track's *reference* implementation needs.
///
/// Never needed to work on your own program — only to run the tester's `--validate`
/// self-check — so `byo doctor` reports a missing one as a warning.
#[derive(Debug, Clone, Copy)]
pub enum Requirement {
    /// A program that must be on `PATH`.
    Program {
        /// Binary name.
        bin: &'static str,
        /// Arguments that make it print its version.
        args: &'static [&'static str],
        /// What it is needed for.
        why: &'static str,
    },
    /// A binary the tester downloads once and caches under `~/.cache/<dir>`.
    Cached {
        /// Directory under the user's cache dir.
        dir: &'static str,
        /// The binary's file name inside it.
        bin: &'static str,
        /// What it is needed for.
        why: &'static str,
    },
}

/// Everything `byo` knows about one track.
#[derive(Debug, Clone, Copy)]
pub struct TrackDef {
    /// The id used in `byo.toml`, the database, the API and the site, e.g. `shell`.
    pub id: &'static str,
    /// Human title, e.g. "Build your own shell".
    pub title: &'static str,
    /// One line for the terminal and the site's track cards.
    pub blurb: &'static str,
    /// The repo directory holding the tester, e.g. `shelltest`.
    pub dir: &'static str,
    /// The tester binary, e.g. `shelltest`.
    pub tester: &'static str,
    /// The tester flag naming the program under test, e.g. `--shell`.
    pub target_flag: &'static str,
    /// How that target is spelled in `byo.toml`, e.g. `shell` / `broker` / `runtime`.
    pub target_key: &'static str,
    /// What `byo init <track>` writes as `command` when the user names nothing.
    pub default_command: &'static str,
    /// Data files passed to the tester when they exist in `$BYO_HOME`.
    pub data_files: &'static [DataFile],
    /// Track-specific `byo.toml` keys.
    pub extra_keys: &'static [ExtraKey],
    /// The external tool the track's reference implementation needs, if any.
    pub requirement: Option<Requirement>,
    /// CSS colour the site paints this track with.
    pub accent: &'static str,
    /// SGR parameters `byo status` paints this track's heading with.
    pub ansi: &'static str,
}

/// Every registered track, in display order. **This is the registry.**
pub static TRACKS: &[TrackDef] = &[
    TrackDef {
        id: "shell",
        title: "Build your own shell",
        blurb: "A POSIX shell: parsing, quoting, expansion, pipelines, redirection, job control.",
        dir: "shelltest",
        tester: "shelltest",
        target_flag: "--shell",
        target_key: "shell",
        default_command: "./your_program.sh",
        data_files: &[
            DataFile {
                flag: "--tests-dir",
                rel: "tests/stages",
                dir: true,
            },
            DataFile {
                flag: "--shells-file",
                rel: "shells.yaml",
                dir: false,
            },
        ],
        extra_keys: &[],
        requirement: None,
        accent: "#a78bfa",
        ansi: "1;35",
    },
    TrackDef {
        id: "kafka",
        title: "Build your own Kafka broker",
        blurb: "A Kafka broker: the binary wire protocol, the on-disk log, consumer groups.",
        dir: "kafkatest",
        tester: "kafkatest",
        target_flag: "--broker",
        target_key: "broker",
        default_command: "./your_program.sh",
        data_files: &[DataFile {
            flag: "--brokers-file",
            rel: "brokers.yaml",
            dir: false,
        }],
        extra_keys: &[
            ExtraKey {
                name: "port",
                flag: "--port",
                kind: ExtraKind::Int,
                default: Some("9092"),
                doc: "the port your broker listens on",
            },
            ExtraKey {
                name: "log_dir",
                flag: "--log-dir",
                kind: ExtraKind::Text,
                default: Some("/tmp/kraft-combined-logs"),
                doc: "where your broker keeps its log segments",
            },
        ],
        requirement: Some(Requirement::Program {
            bin: "java",
            args: &["-version"],
            why: "`kafkatest --broker apache_kafka --validate`, the suite's self-check",
        }),
        accent: "#f9a8d4",
        ansi: "1;36",
    },
    TrackDef {
        id: "wasm",
        title: "Build your own WebAssembly runtime",
        blurb: "A WebAssembly runtime: binary decoding, validation, execution, WASI preview1.",
        dir: "wasmtest",
        tester: "wasmtest",
        target_flag: "--runtime",
        target_key: "runtime",
        default_command: "./your_program.sh",
        data_files: &[DataFile {
            flag: "--runtimes-file",
            rel: "runtimes.yaml",
            dir: false,
        }],
        extra_keys: &[],
        requirement: Some(Requirement::Cached {
            dir: "wasmtest",
            bin: "wasmtime",
            why: "`wasmtest --runtime wasmtime --validate` (wasmtest downloads it on first use)",
        }),
        accent: "#86efac",
        ansi: "1;32",
    },
    TrackDef {
        id: "tls",
        title: "Build your own TLS 1.3 server",
        blurb: "A TLS 1.3 server: the record layer, the key schedule, certificates, AEAD.",
        dir: "tlstest",
        tester: "tlstest",
        target_flag: "--server",
        target_key: "server",
        default_command: "./your_program.sh",
        data_files: &[DataFile {
            flag: "--servers-file",
            rel: "servers.yaml",
            dir: false,
        }],
        extra_keys: &[],
        requirement: Some(Requirement::Program {
            bin: "openssl",
            args: &["version"],
            why: "`tlstest --server openssl --validate`, and the certificates each run needs",
        }),
        accent: "#7dd3fc",
        ansi: "1;34",
    },
    TrackDef {
        id: "link",
        title: "Build your own ELF linker",
        blurb:
            "A static ELF64 linker for x86-64: sections, symbols, relocations, a binary that runs.",
        dir: "linktest",
        tester: "linktest",
        target_flag: "--linker",
        target_key: "linker",
        default_command: "./your_program.sh",
        data_files: &[DataFile {
            flag: "--linkers-file",
            rel: "linkers.yaml",
            dir: false,
        }],
        extra_keys: &[],
        requirement: None,
        accent: "#fcd34d",
        ansi: "1;33",
    },
    TrackDef {
        id: "dist",
        title: "Build your own distributed system",
        blurb: "A replicated, linearizable key-value store: clocks, quorums, consensus, CRDTs.",
        dir: "disttest",
        tester: "disttest",
        target_flag: "--target",
        target_key: "target",
        default_command: "./your_program.sh",
        data_files: &[DataFile {
            flag: "--targets-file",
            rel: "targets.yaml",
            dir: false,
        }],
        extra_keys: &[],
        requirement: Some(Requirement::Cached {
            dir: "disttest",
            bin: "etcd",
            why: "`disttest --target etcd --validate` (etcd 3.7.1, downloaded on first use)",
        }),
        accent: "#fdba74",
        ansi: "1;95",
    },
];

/// A handle into [`TRACKS`]: an index, so it stays `Copy` and cheap to pass around.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Track(usize);

impl Track {
    /// Build your own shell, tested by `shelltest`.
    pub const SHELL: Track = Track(0);
    /// Build your own Kafka broker, tested by `kafkatest`.
    pub const KAFKA: Track = Track(1);
    /// Build your own WebAssembly runtime, tested by `wasmtest`.
    pub const WASM: Track = Track(2);
    /// Build your own TLS 1.3 server, tested by `tlstest`.
    pub const TLS: Track = Track(3);
    /// Build your own ELF linker, tested by `linktest`.
    pub const LINK: Track = Track(4);
    /// Build your own distributed key-value store, tested by `disttest`.
    pub const DIST: Track = Track(5);

    /// Every registered track, in registry order.
    pub fn all() -> impl ExactSizeIterator<Item = Track> + Clone {
        (0..TRACKS.len()).map(Track)
    }

    /// This track's registry entry.
    pub fn def(self) -> &'static TrackDef {
        &TRACKS[self.0]
    }

    /// The lowercase id used in configs, the database and the API.
    pub fn as_str(self) -> &'static str {
        self.def().id
    }

    /// The tester binary that drives this track.
    pub fn tester(self) -> &'static str {
        self.def().tester
    }

    /// The tester flag that selects the program under test.
    pub fn target_flag(self) -> &'static str {
        self.def().target_flag
    }

    /// The `byo.toml` key naming a registered target (`shell`, `broker`, `runtime`, …).
    pub fn target_key(self) -> &'static str {
        self.def().target_key
    }

    /// Human title, e.g. "Build your own shell".
    pub fn title(self) -> &'static str {
        self.def().title
    }

    /// Parse a track id, listing the registered ones when it is not one.
    pub fn parse(s: &str) -> Result<Self> {
        Self::find(s).ok_or_else(|| anyhow!("unknown track '{s}' (expected one of: {})", names()))
    }

    /// Look a track up by id, without producing an error.
    pub fn find(s: &str) -> Option<Self> {
        TRACKS.iter().position(|t| t.id == s).map(Track)
    }

    /// Look a track up by the `byo.toml` key naming its target (`broker` → kafka).
    pub fn by_target_key(key: &str) -> Option<Self> {
        TRACKS.iter().position(|t| t.target_key == key).map(Track)
    }

    /// The registry entry for one of this track's extra keys.
    pub fn extra_key(self, name: &str) -> Option<&'static ExtraKey> {
        self.def().extra_keys.iter().find(|k| k.name == name)
    }
}

/// Every registered id, comma-separated — for error messages and `--help`.
pub fn names() -> String {
    TRACKS.iter().map(|t| t.id).collect::<Vec<_>>().join(", ")
}

impl std::fmt::Display for Track {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl ExtraKey {
    /// Check a value against this key's type, returning it normalised.
    pub fn validate(&self, value: &str) -> Result<String> {
        let v = value.trim();
        match self.kind {
            ExtraKind::Int => {
                let n: u64 = v.parse().map_err(|_| {
                    anyhow!("`{}` must be a number ({}), got '{v}'", self.name, self.doc)
                })?;
                Ok(n.to_string())
            }
            ExtraKind::Text if v.is_empty() => {
                Err(anyhow!("`{}` must not be empty ({})", self.name, self.doc))
            }
            ExtraKind::Text => Ok(v.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_flags_and_keys_are_unique() {
        let mut ids = HashSet::new();
        let mut testers = HashSet::new();
        let mut flags = HashSet::new();
        let mut keys = HashSet::new();
        for t in TRACKS {
            assert!(ids.insert(t.id), "duplicate track id {}", t.id);
            assert!(testers.insert(t.tester), "duplicate tester {}", t.tester);
            assert!(
                flags.insert(t.target_flag),
                "duplicate flag {}",
                t.target_flag
            );
            assert!(keys.insert(t.target_key), "duplicate key {}", t.target_key);
            assert!(t.target_flag.starts_with("--"));
            assert!(!t.blurb.is_empty() && !t.title.is_empty());
            assert!(t.accent.starts_with('#'), "{} needs a CSS accent", t.id);
            for d in t.data_files {
                assert!(d.flag.starts_with("--"));
                assert!(
                    !d.rel.starts_with('/'),
                    "{} must be $BYO_HOME-relative",
                    d.rel
                );
            }
            for k in t.extra_keys {
                assert!(k.flag.starts_with("--"));
                assert!(!k.doc.is_empty());
            }
        }
    }

    #[test]
    fn handles_round_trip_through_ids() {
        for t in Track::all() {
            assert_eq!(Track::parse(t.as_str()).unwrap(), t);
            assert_eq!(Track::by_target_key(t.target_key()), Some(t));
        }
        assert_eq!(Track::all().len(), TRACKS.len());
        assert_eq!(Track::SHELL.as_str(), "shell");
        assert_eq!(Track::KAFKA.tester(), "kafkatest");
        assert_eq!(Track::WASM.target_flag(), "--runtime");
        assert_eq!(Track::TLS.target_key(), "server");
        assert_eq!(Track::LINK.def().dir, "linktest");
        assert_eq!(Track::DIST.target_flag(), "--target");
        assert_eq!(Track::DIST.def().extra_keys.len(), 0);
    }

    #[test]
    fn unknown_tracks_list_the_registered_ones() {
        let e = Track::parse("redis").unwrap_err().to_string();
        for t in TRACKS {
            assert!(e.contains(t.id), "{e} should mention {}", t.id);
        }
    }

    #[test]
    fn extra_keys_validate_their_values() {
        let port = Track::KAFKA
            .extra_key("port")
            .expect("kafka has a port key");
        assert_eq!(port.validate(" 9092 ").unwrap(), "9092");
        assert!(port.validate("nine").is_err());
        let dir = Track::KAFKA.extra_key("log_dir").unwrap();
        assert_eq!(dir.validate("/tmp/l").unwrap(), "/tmp/l");
        assert!(dir.validate("  ").is_err());
        assert!(Track::SHELL.extra_key("port").is_none());
    }
}
