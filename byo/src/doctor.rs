//! `byo doctor` — check that everything the CLI depends on is in place.

use crate::config;
use crate::db;
use crate::paths::{find_tester, Paths, Track};
use crate::server;
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
    for t in Track::ALL {
        out.push(tester_check(t));
    }
    out.push(data_dir_check(paths));
    out.extend(data_file_checks(paths));
    out.push(db_check(paths));
    out.push(site_check(paths));
    out.push(java_check());
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

fn tester_check(track: Track) -> Check {
    let name = track.tester();
    match find_tester(name) {
        Err(_) => check(Level::Bad, name, "not found next to `byo` or on PATH"),
        Ok(p) => match Command::new(&p).arg("--version").output() {
            Ok(o) if o.status.success() => {
                let v = String::from_utf8_lossy(&o.stdout).trim().to_string();
                check(Level::Ok, name, format!("{v} ({})", p.display()))
            }
            Ok(o) => check(
                Level::Bad,
                name,
                format!("{} exists but `--version` exited {}", p.display(), o.status),
            ),
            Err(e) => check(
                Level::Bad,
                name,
                format!("{} cannot be run: {e}", p.display()),
            ),
        },
    }
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

fn data_file_checks(paths: &Paths) -> Vec<Check> {
    let mut out = Vec::new();
    let stages = paths.tests_dir();
    let n = std::fs::read_dir(&stages)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|x| x == "yaml"))
                .count()
        })
        .unwrap_or(0);
    out.push(if n > 0 {
        check(
            Level::Ok,
            "shell stages",
            format!("{n} yaml files in {}", stages.display()),
        )
    } else {
        check(
            Level::Bad,
            "shell stages",
            format!("no stage files in {}", stages.display()),
        )
    });
    for (label, p) in [
        ("shells.yaml", paths.shells_file()),
        ("brokers.yaml", paths.brokers_file()),
    ] {
        out.push(if p.is_file() {
            check(Level::Ok, label, p.display().to_string())
        } else {
            check(Level::Bad, label, format!("missing: {}", p.display()))
        });
    }
    for t in Track::ALL {
        let p = paths.catalog(t);
        let label = format!("catalog.{t}");
        out.push(if p.is_file() {
            check(Level::Ok, &label, p.display().to_string())
        } else {
            check(
                Level::Warn,
                &label,
                format!("missing: {} — /api/catalog/{t} will 404", p.display()),
            )
        });
    }
    out
}

fn db_check(paths: &Paths) -> Check {
    let p = paths.db();
    match db::open(&p) {
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

fn java_check() -> Check {
    match Command::new("java").arg("-version").output() {
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stderr);
            let first = text.lines().next().unwrap_or("").trim().to_string();
            check(
                Level::Ok,
                "java",
                format!("{first} (kafkatest's reference broker)"),
            )
        }
        Err(_) => check(
            Level::Warn,
            "java",
            "not on PATH — needed only for `kafkatest --broker apache_kafka`",
        ),
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

    #[test]
    fn an_empty_data_dir_produces_failures_not_panics() {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths {
            home: d.path().join("nope"),
        };
        let checks = collect(&paths);
        assert!(checks
            .iter()
            .any(|c| c.name == "data dir" && c.level == Level::Bad));
        assert!(checks
            .iter()
            .any(|c| c.name == "shell stages" && c.level == Level::Bad));
        assert!(checks
            .iter()
            .any(|c| c.name == "site" && c.level == Level::Warn));
    }

    #[test]
    fn a_populated_data_dir_passes_the_file_checks() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("tests/stages")).unwrap();
        std::fs::write(d.path().join("tests/stages/01_x.yaml"), "stage: 1").unwrap();
        std::fs::write(d.path().join("shells.yaml"), "shells: {}").unwrap();
        std::fs::write(d.path().join("brokers.yaml"), "brokers: {}").unwrap();
        std::fs::write(d.path().join("catalog.shell.json"), "{}").unwrap();
        std::fs::create_dir_all(d.path().join("site")).unwrap();
        std::fs::write(d.path().join("site/index.html"), "<html>").unwrap();
        let paths = Paths {
            home: d.path().to_path_buf(),
        };
        let checks = collect(&paths);
        let by = |n: &str| checks.iter().find(|c| c.name == n).expect(n).level;
        assert_eq!(by("data dir"), Level::Ok);
        assert_eq!(by("shell stages"), Level::Ok);
        assert_eq!(by("shells.yaml"), Level::Ok);
        assert_eq!(by("brokers.yaml"), Level::Ok);
        assert_eq!(by("catalog.shell"), Level::Ok);
        assert_eq!(by("catalog.kafka"), Level::Warn);
        assert_eq!(by("site"), Level::Ok);
        assert_eq!(by("database"), Level::Ok);
    }
}
