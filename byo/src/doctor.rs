//! `byo doctor` — check that everything the CLI depends on is in place.
//!
//! The per-track rows are generated from the [registry][crate::track], and a track that is
//! not installed yet is a *warning*, never a failure: the five testers land in the repo one
//! at a time, and `byo doctor` has to stay useful while that happens. Only things that make
//! `byo` itself unusable (no data directory, a broken database, not one tester installed)
//! are `Bad`.

use crate::catalog;
use crate::config;
use crate::db;
use crate::paths::{self, Paths};
use crate::server;
use crate::track::{Requirement, Track};
use anyhow::Result;
use std::path::Path;
use std::process::Command;

/// How serious a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Everything is fine.
    Ok,
    /// Works, but something is missing or degraded.
    Warn,
    /// Broken; `byo doctor` exits non-zero.
    Bad,
}

/// One line of the report.
#[derive(Debug, Clone)]
pub struct Check {
    /// How serious this finding is.
    pub level: Level,
    /// What was checked.
    pub name: String,
    /// What was found.
    pub detail: String,
}

fn check(level: Level, name: &str, detail: impl Into<String>) -> Check {
    Check {
        level,
        name: name.to_string(),
        detail: detail.into(),
    }
}

/// Run every check and print the report; returns the process exit code.
pub fn run(paths: &Paths) -> Result<i32> {
    let checks = collect(paths);
    println!("\nbyo doctor — {}\n", env!("CARGO_PKG_VERSION"));
    let mut bad = 0;
    let mut warn = 0;
    for c in &checks {
        let mark = match c.level {
            Level::Ok => "\x1b[32m✔\x1b[0m",
            Level::Warn => "\x1b[33m!\x1b[0m",
            Level::Bad => "\x1b[31m✘\x1b[0m",
        };
        let mark = if std::env::var_os("NO_COLOR").is_some() {
            match c.level {
                Level::Ok => "ok  ",
                Level::Warn => "warn",
                Level::Bad => "FAIL",
            }
        } else {
            mark
        };
        println!("{mark} {:<22} {}", c.name, c.detail);
        match c.level {
            Level::Bad => bad += 1,
            Level::Warn => warn += 1,
            Level::Ok => {}
        }
    }
    println!();
    if bad > 0 {
        println!(
            "{bad} problem(s), {warn} warning(s). Re-run ./install.sh from the BuildYourOwn repo."
        );
        Ok(1)
    } else {
        println!("all good ({warn} warning(s)).");
        Ok(0)
    }
}

/// Every check, in report order.
pub fn collect(paths: &Paths) -> Vec<Check> {
    let mut out = Vec::new();
    out.push(check(
        Level::Ok,
        "byo",
        format!("{} at {}", env!("CARGO_PKG_VERSION"), exe_path()),
    ));
    out.push(data_dir_check(paths));
    let mut installed = 0;
    for t in Track::all() {
        let rows = track_checks(paths, t);
        if rows
            .iter()
            .any(|c| c.name.ends_with("tester") && c.level == Level::Ok)
        {
            installed += 1;
        }
        out.extend(rows);
    }
    if installed == 0 {
        out.push(check(
            Level::Bad,
            "testers",
            "not one tester is installed — run ./install.sh from the BuildYourOwn repo",
        ));
    } else {
        out.push(check(
            Level::Ok,
            "testers",
            format!("{installed} of {} track(s) installed", Track::all().len()),
        ));
    }
    out.push(db_check(paths));
    out.push(site_check(paths));
    out.push(node_check());
    out.push(port_check(4321));
    out.push(path_check());
    out.push(project_check());
    out
}

fn exe_path() -> String {
    std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "?".into())
}

/// The rows for one track: binary, data files, catalog, external requirement.
///
/// A track with nothing at all collapses into a single "not installed yet" line so that the
/// report of a half-built repo stays readable.
pub fn track_checks(paths: &Paths, track: Track) -> Vec<Check> {
    let def = track.def();
    let binary = paths::locate_tester(def.tester);
    let data_missing: Vec<&str> = def
        .data_files
        .iter()
        .filter(|f| !paths.has_data_file(f))
        .map(|f| f.rel)
        .collect();
    let catalog_path = paths.catalog(track);

    if binary.is_none() && data_missing.len() == def.data_files.len() && !catalog_path.is_file() {
        return vec![check(
            Level::Warn,
            def.id,
            format!(
                "not installed yet — ./install.sh builds {} once {}/ exists",
                def.tester, def.dir
            ),
        )];
    }

    let mut out = Vec::new();
    let tester_row = format!("{} tester", def.id);
    out.push(match &binary {
        None => check(
            Level::Warn,
            &tester_row,
            format!("{} not found next to `byo` or on PATH", def.tester),
        ),
        Some(p) => match Command::new(p).arg("--version").output() {
            Ok(o) if o.status.success() => check(
                Level::Ok,
                &tester_row,
                format!(
                    "{} ({})",
                    String::from_utf8_lossy(&o.stdout).trim(),
                    p.display()
                ),
            ),
            Ok(o) => check(
                Level::Warn,
                &tester_row,
                format!("{} exists but `--version` exited {}", p.display(), o.status),
            ),
            Err(e) => check(
                Level::Warn,
                &tester_row,
                format!("{} cannot be run: {e}", p.display()),
            ),
        },
    });

    if !def.data_files.is_empty() {
        let data_row = format!("{} data", def.id);
        out.push(if data_missing.is_empty() {
            let listed: Vec<String> = def
                .data_files
                .iter()
                .map(|f| {
                    if f.dir {
                        format!("{}/ ({} files)", f.rel, count_files(&paths.data_file(f)))
                    } else {
                        f.rel.to_string()
                    }
                })
                .collect();
            check(Level::Ok, &data_row, listed.join(", "))
        } else {
            check(
                Level::Warn,
                &data_row,
                format!(
                    "missing in {}: {}",
                    paths.home.display(),
                    data_missing.join(", ")
                ),
            )
        });
    }

    let catalog_row = format!("{} catalog", def.id);
    out.push(if !catalog_path.is_file() {
        check(
            Level::Warn,
            &catalog_row,
            format!(
                "missing: {} — /api/catalog/{} will 404",
                catalog_path.display(),
                def.id
            ),
        )
    } else {
        match catalog::load(&catalog_path) {
            Some(c) => check(
                Level::Ok,
                &catalog_row,
                format!(
                    "{} stage(s), {} section(s) — {}",
                    c.stages.len(),
                    c.sections.len(),
                    catalog_path.display()
                ),
            ),
            None => check(
                Level::Warn,
                &catalog_row,
                format!("{} is not valid catalog JSON", catalog_path.display()),
            ),
        }
    });

    if let Some(req) = def.requirement {
        out.push(requirement_check(def.id, req));
    }
    out
}

fn count_files(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .filter(|e| e.path().is_file())
                .count()
        })
        .unwrap_or(0)
}

/// The external tool a track's *reference* implementation needs (never your own program).
fn requirement_check(id: &str, req: Requirement) -> Check {
    let name = format!("{id} reference");
    match req {
        Requirement::Program { bin, args, why } => match Command::new(bin).args(args).output() {
            Ok(o) => {
                let text = String::from_utf8_lossy(&o.stdout);
                let text = if text.trim().is_empty() {
                    String::from_utf8_lossy(&o.stderr).into_owned()
                } else {
                    text.into_owned()
                };
                let first = text.lines().next().unwrap_or(bin).trim().to_string();
                check(Level::Ok, &name, format!("{first} — for {why}"))
            }
            Err(_) => check(
                Level::Warn,
                &name,
                format!("{bin} is not on PATH — needed only for {why}"),
            ),
        },
        Requirement::Cached { dir, bin, why } => {
            let root = dirs::cache_dir().map(|c| c.join(dir));
            match root.as_ref().and_then(|r| find_in(r, bin)) {
                Some(p) => check(Level::Ok, &name, format!("{} — for {why}", p.display())),
                None => check(
                    Level::Warn,
                    &name,
                    format!(
                        "no cached {bin} in {} — needed only for {why}",
                        root.map(|r| r.display().to_string())
                            .unwrap_or_else(|| format!("~/.cache/{dir}"))
                    ),
                ),
            }
        }
    }
}

/// Look for `name` in `dir` or one level below it (the shape of an unpacked release).
fn find_in(dir: &Path, name: &str) -> Option<std::path::PathBuf> {
    let direct = dir.join(name);
    if paths::is_executable(&direct) {
        return Some(direct);
    }
    for entry in std::fs::read_dir(dir).ok()? {
        let entry = entry.ok()?;
        if entry.file_type().ok()?.is_dir() {
            let cand = entry.path().join(name);
            if paths::is_executable(&cand) {
                return Some(cand);
            }
            let cand = entry.path().join("bin").join(name);
            if paths::is_executable(&cand) {
                return Some(cand);
            }
        }
    }
    None
}

fn data_dir_check(paths: &Paths) -> Check {
    if paths.home.is_dir() {
        check(Level::Ok, "data dir", paths.home.display().to_string())
    } else {
        check(
            Level::Bad,
            "data dir",
            format!("{} does not exist — run ./install.sh", paths.home.display()),
        )
    }
}

fn db_check(paths: &Paths) -> Check {
    let p = paths.db();
    if !p.is_file() {
        return check(
            Level::Warn,
            "database",
            format!(
                "{} does not exist yet — ./install.sh creates it",
                p.display()
            ),
        );
    }
    match db::open_readonly(&p) {
        Err(e) => check(Level::Bad, "database", format!("{}: {e:#}", p.display())),
        Ok(conn) => {
            let v = db::schema_version(&conn).unwrap_or(0);
            let runs = db::runs(&conn, None, 1).map(|r| r.len()).unwrap_or(0);
            if v == db::SCHEMA_VERSION {
                check(
                    Level::Ok,
                    "database",
                    format!(
                        "{} (schema v{v}, {})",
                        p.display(),
                        if runs > 0 { "has runs" } else { "no runs yet" }
                    ),
                )
            } else {
                check(
                    Level::Bad,
                    "database",
                    format!(
                        "{} has schema v{v}, this build expects v{}",
                        p.display(),
                        db::SCHEMA_VERSION
                    ),
                )
            }
        }
    }
}

fn site_check(paths: &Paths) -> Check {
    let index = paths.site().join("index.html");
    if index.is_file() {
        check(
            Level::Ok,
            "site",
            format!("built at {}", paths.site().display()),
        )
    } else {
        check(
            Level::Warn,
            "site",
            format!(
                "no build at {} — `byo site --rebuild`",
                paths.site().display()
            ),
        )
    }
}

fn node_check() -> Check {
    match Command::new("npm").arg("--version").output() {
        Ok(o) if o.status.success() => check(
            Level::Ok,
            "npm",
            format!(
                "{} (for `byo site --rebuild`)",
                String::from_utf8_lossy(&o.stdout).trim()
            ),
        ),
        _ => check(
            Level::Warn,
            "npm",
            "not on PATH — `byo site --rebuild` will not work",
        ),
    }
}

fn port_check(port: u16) -> Check {
    match server::pick_port(port, 50) {
        Ok(p) if p == port => check(Level::Ok, "port", format!("{port} is free for `byo site`")),
        Ok(p) => check(
            Level::Warn,
            "port",
            format!("{port} is busy; `byo site` would use {p}"),
        ),
        Err(e) => check(Level::Bad, "port", format!("{e:#}")),
    }
}

fn path_check() -> Check {
    let Some(dir) = dirs::home_dir().map(|h| h.join(".local/bin")) else {
        return check(Level::Warn, "PATH", "cannot determine your home directory");
    };
    let on_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d == dir))
        .unwrap_or(false);
    if on_path || !dir.exists() {
        check(Level::Ok, "PATH", format!("{} is on PATH", dir.display()))
    } else {
        check(
            Level::Warn,
            "PATH",
            format!(
                "{} is not on PATH — add `export PATH=\"$HOME/.local/bin:$PATH\"`",
                dir.display()
            ),
        )
    }
}

fn project_check() -> Check {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    match config::find(&cwd) {
        None => check(
            Level::Ok,
            "project",
            "no byo.toml here (that is fine outside a project)",
        ),
        Some(p) => match config::load_from(&cwd) {
            Ok(pr) => check(
                Level::Ok,
                "project",
                format!(
                    "{} — {} track, {} `{}`",
                    p.display(),
                    pr.track,
                    pr.target_kind.as_str(),
                    pr.target
                ),
            ),
            Err(e) => check(Level::Bad, "project", format!("{}: {e:#}", p.display())),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level_of(checks: &[Check], name: &str) -> Option<Level> {
        checks.iter().find(|c| c.name == name).map(|c| c.level)
    }

    #[test]
    fn an_empty_data_dir_warns_per_track_and_fails_only_on_the_essentials() {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths {
            home: d.path().join("nope"),
        };
        let checks = collect(&paths);
        assert_eq!(level_of(&checks, "data dir"), Some(Level::Bad));
        assert_eq!(level_of(&checks, "site"), Some(Level::Warn));
        // Tracks whose tester is nowhere collapse to one warning line each.
        for t in [Track::WASM, Track::TLS, Track::LINK] {
            let row = level_of(&checks, t.as_str());
            assert!(
                row.is_none() || row == Some(Level::Warn),
                "{t} should never be Bad, got {row:?}"
            );
        }
    }

    #[test]
    fn diagnosing_creates_nothing_in_the_users_home() {
        // A diagnostic that writes is a diagnostic that lies: `doctor` used to call
        // `db::open`, which creates the data directory and the database, and then reported
        // on the world it had just made.
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("never-created");
        let paths = Paths { home: home.clone() };
        let _ = collect(&paths);
        assert!(!home.exists(), "doctor created {}", home.display());
    }

    #[test]
    fn a_missing_track_never_fails_the_others() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("tests/stages")).unwrap();
        std::fs::write(d.path().join("tests/stages/01_x.yaml"), "stage: 1").unwrap();
        std::fs::write(d.path().join("shells.yaml"), "shells: {}").unwrap();
        std::fs::write(
            d.path().join("catalog.shell.json"),
            r#"{"track":"shell","sections":[{"id":"A","title":"x","stages":[1]}],
                "stages":[{"number":1,"name":"Prompt","ext":false}]}"#,
        )
        .unwrap();
        let paths = Paths {
            home: d.path().to_path_buf(),
        };
        let checks = collect(&paths);
        assert_eq!(level_of(&checks, "data dir"), Some(Level::Ok));
        assert_eq!(level_of(&checks, "shell data"), Some(Level::Ok));
        assert_eq!(level_of(&checks, "shell catalog"), Some(Level::Ok));
        // No install has run in this temp home, so there is no database — and `doctor`
        // must say so rather than conjuring one while it looks.
        assert_eq!(level_of(&checks, "database"), Some(Level::Warn));
        assert!(!paths.db().exists(), "doctor must not create the database");
        // wasm/tls/link are absent from this data dir; that is a warning, not a failure.
        assert_eq!(level_of(&checks, "wasm"), Some(Level::Warn));
        assert_eq!(level_of(&checks, "tls"), Some(Level::Warn));
        assert!(
            checks
                .iter()
                .filter(|c| c.level == Level::Bad && c.name != "testers")
                .count()
                == 0,
            "{checks:#?}"
        );
    }

    #[test]
    fn a_half_installed_track_reports_each_piece() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("catalog.wasm.json"), "{ not json").unwrap();
        let paths = Paths {
            home: d.path().to_path_buf(),
        };
        let rows = track_checks(&paths, Track::WASM);
        assert_eq!(level_of(&rows, "wasm tester"), Some(Level::Warn));
        assert_eq!(level_of(&rows, "wasm data"), Some(Level::Warn));
        assert_eq!(level_of(&rows, "wasm catalog"), Some(Level::Warn));
        assert!(
            rows.iter().any(|c| c.name == "wasm reference"),
            "wasm needs a cached wasmtime row: {rows:#?}"
        );
        assert!(rows.iter().all(|c| c.level != Level::Bad));
    }

    #[test]
    fn every_track_produces_rows_without_panicking() {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths {
            home: d.path().to_path_buf(),
        };
        for t in Track::all() {
            let rows = track_checks(&paths, t);
            assert!(!rows.is_empty(), "{t} produced no rows");
            assert!(rows.iter().all(|c| c.level != Level::Bad), "{t}: {rows:#?}");
        }
    }
}
