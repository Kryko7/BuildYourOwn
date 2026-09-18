//! `byo test` — run the right tester, stream its output, then ingest its JSON report.

use crate::config::Project;
use crate::db;
use crate::paths::{find_tester, Paths, Track};
use crate::report;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The tester arguments `byo` computed, plus where the report will land.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// Arguments passed to the tester, in order.
    pub args: Vec<String>,
    /// The `--json` path the report is read back from.
    pub json: PathBuf,
    /// True when the tester will not produce a report (e.g. `--list`).
    pub no_report: bool,
}

/// True when `args` already contains `flag` (as `--flag` or `--flag=value`).
fn has(args: &[String], flag: &str) -> bool {
    args.iter()
        .any(|a| a == flag || a.starts_with(&format!("{flag}=")))
}

/// The value the user gave a flag, if any.
fn value_of(args: &[String], flag: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == flag {
            return it.next().cloned();
        }
        if let Some(v) = a.strip_prefix(&format!("{flag}=")) {
            return Some(v.to_string());
        }
    }
    None
}

/// Build the tester command line: the user's flags verbatim, plus whatever `byo` knows.
pub fn plan(paths: &Paths, project: &Project, user: &[String], temp_json: PathBuf) -> Invocation {
    let mut args: Vec<String> = Vec::new();
    let track = project.track;

    if !has(user, track.target_flag()) {
        args.push(track.target_flag().to_string());
        args.push(project.target.clone());
    }
    match track {
        Track::Shell => {
            let tests = paths.tests_dir();
            if !has(user, "--tests-dir") && tests.is_dir() {
                args.push("--tests-dir".into());
                args.push(tests.to_string_lossy().into_owned());
            }
            let shells = paths.shells_file();
            if !has(user, "--shells-file") && shells.is_file() {
                args.push("--shells-file".into());
                args.push(shells.to_string_lossy().into_owned());
            }
        }
        Track::Kafka => {
            let brokers = paths.brokers_file();
            if !has(user, "--brokers-file") && brokers.is_file() {
                args.push("--brokers-file".into());
                args.push(brokers.to_string_lossy().into_owned());
            }
            if let Some(p) = project.port {
                if !has(user, "--port") {
                    args.push("--port".into());
                    args.push(p.to_string());
                }
            }
            if let Some(d) = &project.log_dir {
                if !has(user, "--log-dir") {
                    args.push("--log-dir".into());
                    args.push(d.clone());
                }
            }
        }
    }

    // The testers insist on an explicit stage selection; `byo test` means "everything".
    let selectors = [
        "--stage",
        "--until",
        "--from",
        "--all",
        "--validate",
        "--list",
    ];
    if !selectors.iter().any(|s| has(user, s)) {
        args.push("--all".into());
    }

    let json = match value_of(user, "--json") {
        Some(p) => PathBuf::from(p),
        None => {
            args.push("--json".into());
            args.push(temp_json.to_string_lossy().into_owned());
            temp_json
        }
    };
    let no_report = has(user, "--list");

    args.extend(user.iter().cloned());
    Invocation {
        args,
        json,
        no_report,
    }
}

/// Run the tester for `project` and ingest the result. Returns the tester's exit code.
pub fn test(paths: &Paths, project: &Project, user: &[String]) -> Result<i32> {
    let track = project.track;
    let binary = find_tester(track.tester())?;
    let tmp = tempfile::Builder::new()
        .prefix("byo-report-")
        .suffix(".json")
        .tempfile()
        .context("cannot create a temporary file for the tester's JSON report")?;
    let inv = plan(paths, project, user, tmp.path().to_path_buf());

    let started_at = db::now();
    let start = std::time::Instant::now();
    let status = Command::new(&binary)
        .args(&inv.args)
        .current_dir(&project.root)
        .status()
        .with_context(|| format!("cannot run {}", binary.display()))?;
    let code = exit_code(&status);

    if inv.no_report {
        return Ok(code);
    }
    let elapsed = start.elapsed();
    match ingest_file(paths, project, &inv, user, &started_at, elapsed) {
        Ok(Some(line)) => println!("{line}"),
        Ok(None) => {}
        Err(e) => eprintln!("byo: could not record this run: {e:#}"),
    }
    Ok(code)
}

fn ingest_file(
    paths: &Paths,
    project: &Project,
    inv: &Invocation,
    user: &[String],
    started_at: &str,
    elapsed: std::time::Duration,
) -> Result<Option<String>> {
    if !inv.json.is_file() {
        eprintln!(
            "byo: the tester wrote no JSON report ({}); nothing was recorded",
            inv.json.display()
        );
        return Ok(None);
    }
    let mut rep = report::read(&inv.json)?;
    if rep.elapsed_ms == 0 {
        rep.elapsed_ms = elapsed.as_millis() as i64;
    }
    let mut conn = db::open(&paths.db())?;
    // Only real projects (those with a byo.toml) are registered; an ad-hoc
    // `byo shell --shell bash --stage 1` in some scratch directory is not one.
    let project_id = if project.root.join(crate::config::FILE).is_file() {
        Some(db::upsert_project(
            &conn,
            project.track,
            &project.root,
            &project.target,
            project.target_kind.as_str(),
        )?)
    } else {
        None
    };
    let kept = kept_report_path(paths, project.track, &inv.json)?;
    let run_id = db::ingest(
        &mut conn,
        project.track,
        project_id,
        &rep,
        &user.join(" "),
        kept.as_deref()
            .map(|p| p.to_string_lossy().into_owned())
            .as_deref(),
        started_at,
    )?;
    Ok(Some(format!(
        "byo: recorded run #{run_id} — {} passed, {} failed, {} skipped ({} track). \
         `byo status` for the map, `byo site` for the pretty one.",
        rep.passed, rep.failed, rep.skipped, project.track
    )))
}

/// Keep the raw report next to the database so `/progress` can link to a real file.
fn kept_report_path(paths: &Paths, track: Track, from: &Path) -> Result<Option<PathBuf>> {
    let dir = paths.home.join("reports");
    if std::fs::create_dir_all(&dir).is_err() {
        return Ok(None);
    }
    let dest = dir.join(format!("{}-latest.json", track.as_str()));
    match std::fs::copy(from, &dest) {
        Ok(_) => Ok(Some(dest)),
        Err(_) => Ok(None),
    }
}

/// The exit status as a code, mapping "killed by a signal" onto 128 + signal.
pub fn exit_code(status: &std::process::ExitStatus) -> i32 {
    if let Some(c) = status.code() {
        return c;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{self, TargetKind};

    fn paths_with(files: &[&str]) -> (tempfile::TempDir, Paths) {
        let d = tempfile::tempdir().unwrap();
        for f in files {
            let p = d.path().join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            if f.ends_with('/') {
                std::fs::create_dir_all(&p).unwrap();
            } else {
                std::fs::write(&p, "x").unwrap();
            }
        }
        let paths = Paths {
            home: d.path().to_path_buf(),
        };
        (d, paths)
    }

    fn project(text: &str) -> Project {
        config::parse(text, PathBuf::from("/proj")).unwrap()
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn shell_invocation_adds_target_and_data_paths() {
        let (_d, paths) = paths_with(&["tests/stages/01.yaml", "shells.yaml"]);
        let inv = plan(
            &paths,
            &project("track = \"shell\"\nshell = \"bash\"\n"),
            &[],
            PathBuf::from("/tmp/r.json"),
        );
        assert_eq!(inv.args[0], "--shell");
        assert_eq!(inv.args[1], "bash");
        assert!(inv
            .args
            .contains(&paths.tests_dir().to_string_lossy().into_owned()));
        assert!(inv
            .args
            .contains(&paths.shells_file().to_string_lossy().into_owned()));
        assert!(inv.args.contains(&"--all".to_string()), "{:?}", inv.args);
        assert_eq!(inv.json, PathBuf::from("/tmp/r.json"));
    }

    #[test]
    fn user_flags_are_passed_through_verbatim_and_last() {
        let (_d, paths) = paths_with(&["tests/stages/01.yaml", "shells.yaml"]);
        let user = s(&["--until", "12", "--verbose"]);
        let inv = plan(
            &paths,
            &project("track = \"shell\"\ncommand = \"./my_shell\"\n"),
            &user,
            PathBuf::from("/tmp/r.json"),
        );
        let tail = &inv.args[inv.args.len() - 3..];
        assert_eq!(tail, &s(&["--until", "12", "--verbose"])[..]);
        assert!(
            !inv.args.contains(&"--all".to_string()),
            "an explicit selector wins"
        );
        assert_eq!(inv.args[1], "./my_shell");
    }

    #[test]
    fn an_explicit_target_flag_is_not_duplicated() {
        let (_d, paths) = paths_with(&["shells.yaml"]);
        let inv = plan(
            &paths,
            &project("track = \"shell\"\nshell = \"bash\"\n"),
            &s(&["--shell", "zsh", "--all"]),
            PathBuf::from("/tmp/r.json"),
        );
        assert_eq!(inv.args.iter().filter(|a| *a == "--shell").count(), 1);
        assert!(inv.args.contains(&"zsh".to_string()));
    }

    #[test]
    fn a_user_json_path_is_reused_for_ingestion() {
        let (_d, paths) = paths_with(&["shells.yaml"]);
        let inv = plan(
            &paths,
            &project("track = \"shell\"\nshell = \"bash\"\n"),
            &s(&["--all", "--json", "mine.json"]),
            PathBuf::from("/tmp/r.json"),
        );
        assert_eq!(inv.json, PathBuf::from("mine.json"));
        assert_eq!(inv.args.iter().filter(|a| *a == "--json").count(), 1);
    }

    #[test]
    fn list_runs_produce_no_report() {
        let (_d, paths) = paths_with(&["shells.yaml"]);
        let inv = plan(
            &paths,
            &project("track = \"shell\"\nshell = \"bash\"\n"),
            &s(&["--list"]),
            PathBuf::from("/tmp/r.json"),
        );
        assert!(inv.no_report);
    }

    #[test]
    fn kafka_invocation_carries_port_and_log_dir() {
        let (_d, paths) = paths_with(&["brokers.yaml"]);
        let p =
            project("track = \"kafka\"\ncommand = \"./b\"\nport = 9099\nlog_dir = \"/tmp/kl\"\n");
        let inv = plan(
            &paths,
            &p,
            &s(&["--until", "2"]),
            PathBuf::from("/tmp/r.json"),
        );
        assert_eq!(inv.args[0], "--broker");
        assert!(inv.args.contains(&"--brokers-file".to_string()));
        assert!(inv
            .args
            .windows(2)
            .any(|w| w[0] == "--port" && w[1] == "9099"));
        assert!(inv
            .args
            .windows(2)
            .any(|w| w[0] == "--log-dir" && w[1] == "/tmp/kl"));
        assert_eq!(p.target_kind, TargetKind::Command);
    }

    #[test]
    fn missing_data_files_are_simply_not_passed() {
        let (_d, paths) = paths_with(&[]);
        let inv = plan(
            &paths,
            &project("track = \"shell\"\nshell = \"bash\"\n"),
            &[],
            PathBuf::from("/tmp/r.json"),
        );
        assert!(!inv.args.contains(&"--tests-dir".to_string()));
        assert!(!inv.args.contains(&"--shells-file".to_string()));
    }

    #[test]
    fn flag_detection_handles_equals_form() {
        assert!(has(&s(&["--json=x"]), "--json"));
        assert_eq!(value_of(&s(&["--json=x"]), "--json").as_deref(), Some("x"));
        assert_eq!(
            value_of(&s(&["--json", "y"]), "--json").as_deref(),
            Some("y")
        );
        assert_eq!(value_of(&s(&["--verbose"]), "--json"), None);
    }
}
