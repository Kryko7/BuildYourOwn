//! `byo` — one command for the BuildYourOwn tracks: run the testers, keep progress in
//! SQLite, and serve the journey site with its JSON API.

use anyhow::{bail, Context, Result};
use byo::config::{self, Project, TargetKind};
use byo::paths::{Paths, Track};
use byo::{db, doctor, runner, server, status};
use clap::{Args, Parser, Subcommand};
use std::io::Write;
use std::path::PathBuf;

/// Build Your Own — shell and Kafka tracks in one command.
#[derive(Parser, Debug)]
#[command(name = "byo", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Set up the current directory as a shell or Kafka project (writes byo.toml)
    Init(InitArgs),
    /// Run the tester for this project and record the result
    Test(TestArgs),
    /// Run the shell tester explicitly (alias for `byo test` on the shell track)
    Shell(TestArgs),
    /// Run the Kafka tester explicitly (alias for `byo test` on the kafka track)
    Kafka(TestArgs),
    /// Show progress: per-section bars, next stage, last run, streak
    Status,
    /// Mark a stage done
    Done(StageArgs),
    /// Mark a stage not done
    Undone(StageArgs),
    /// Attach a note to a stage
    Note(NoteArgs),
    /// Serve the journey site and its JSON API, and open a browser
    Site(SiteArgs),
    /// Inspect the progress database
    #[command(subcommand)]
    Db(DbCmd),
    /// Check that binaries, data, the database and the environment are in order
    Doctor,
}

#[derive(Args, Debug)]
struct InitArgs {
    /// `shell` or `kafka`
    track: String,
    /// Path to the program you are building, e.g. ./your_program.sh
    #[arg(long, value_name = "PATH")]
    command: Option<String>,
    /// A shell registered in shells.yaml (shell track), e.g. bash
    #[arg(long, value_name = "NAME")]
    shell: Option<String>,
    /// A broker registered in brokers.yaml (kafka track), e.g. my_broker
    #[arg(long, value_name = "NAME")]
    broker: Option<String>,
    /// Port the broker must listen on (kafka track)
    #[arg(long)]
    port: Option<u16>,
    /// Kafka log directory (kafka track)
    #[arg(long, value_name = "DIR")]
    log_dir: Option<String>,
    /// Overwrite an existing byo.toml
    #[arg(long)]
    force: bool,
}

#[derive(Args, Debug)]
struct TestArgs {
    /// Flags passed straight through to the tester (--stage N, --until N, --verbose, …)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
    args: Vec<String>,
}

#[derive(Args, Debug)]
struct StageArgs {
    /// Stage number
    stage: u32,
    /// Which track, when the current directory is not a project
    #[arg(long)]
    track: Option<String>,
}

#[derive(Args, Debug)]
struct NoteArgs {
    /// Stage number
    stage: u32,
    /// The note; pass an empty string to clear it
    text: String,
    /// Which track, when the current directory is not a project
    #[arg(long)]
    track: Option<String>,
}

#[derive(Args, Debug)]
struct SiteArgs {
    /// Port to listen on (the next free one is used if this is taken)
    #[arg(long)]
    port: Option<u16>,
    /// Do not open a browser
    #[arg(long)]
    no_open: bool,
    /// Rebuild the site from the repo sources and install it, then serve
    #[arg(long)]
    rebuild: bool,
}

#[derive(Subcommand, Debug)]
enum DbCmd {
    /// Print the path of the database file
    Path,
    /// Print the whole database as JSON
    Dump,
    /// Delete every project, run and stage (keeps the schema)
    Reset {
        /// Do not ask for confirmation
        #[arg(long)]
        yes: bool,
    },
    /// Record where the site sources live, for `byo site --rebuild`
    SetSiteSource {
        /// Path to the repo's site/ directory
        path: PathBuf,
    },
}

fn main() {
    let code = match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("byo: {e:#}");
            2
        }
    };
    std::process::exit(code);
}

fn run() -> Result<i32> {
    let cli = Cli::parse();
    let paths = Paths::resolve()?;
    match cli.command {
        Cmd::Init(a) => init(&paths, a),
        Cmd::Test(a) => test(&paths, None, &a.args),
        Cmd::Shell(a) => test(&paths, Some(Track::Shell), &a.args),
        Cmd::Kafka(a) => test(&paths, Some(Track::Kafka), &a.args),
        Cmd::Status => {
            status::print(&paths)?;
            Ok(0)
        }
        Cmd::Done(a) => mark(&paths, a, true),
        Cmd::Undone(a) => mark(&paths, a, false),
        Cmd::Note(a) => note(&paths, a),
        Cmd::Site(a) => site(&paths, a),
        Cmd::Db(c) => database(&paths, c),
        Cmd::Doctor => doctor::run(&paths),
    }
}

fn init(paths: &Paths, a: InitArgs) -> Result<i32> {
    let track = Track::parse(&a.track)?;
    let root = std::env::current_dir().context("cannot read the current directory")?;
    let existing = root.join(config::FILE);
    if existing.exists() && !a.force {
        bail!(
            "{} already exists — pass --force to overwrite it",
            existing.display()
        );
    }
    let registered = match track {
        Track::Shell => {
            if a.broker.is_some() {
                bail!("--broker belongs to the kafka track; use --shell or --command");
            }
            a.shell.clone()
        }
        Track::Kafka => {
            if a.shell.is_some() {
                bail!("--shell belongs to the shell track; use --broker or --command");
            }
            a.broker.clone()
        }
    };
    let (target, target_kind) = match (registered, a.command.clone()) {
        (Some(_), Some(_)) => bail!("pass either a registered name or --command, not both"),
        (Some(n), None) => (n, TargetKind::Registered),
        (None, Some(c)) => (c, TargetKind::Command),
        (None, None) => (
            match track {
                Track::Shell => "./your_program.sh".to_string(),
                Track::Kafka => "./your_program.sh".to_string(),
            },
            TargetKind::Command,
        ),
    };
    let project = Project {
        root: root.clone(),
        track,
        target,
        target_kind,
        port: a.port.or(match track {
            Track::Kafka => Some(9092),
            Track::Shell => None,
        }),
        log_dir: a.log_dir.or(match track {
            Track::Kafka => Some("/tmp/kraft-combined-logs".to_string()),
            Track::Shell => None,
        }),
    };
    let path = project.save()?;
    paths.ensure()?;
    let conn = db::open(&paths.db())?;
    db::upsert_project(
        &conn,
        track,
        &root,
        &project.target,
        project.target_kind.as_str(),
    )?;

    println!("byo: wrote {}", path.display());
    println!(
        "     track {track}, testing {} `{}`",
        project.target_kind.as_str(),
        project.target
    );
    if project.target_kind == TargetKind::Command && !PathBuf::from(&project.target).exists() {
        println!(
            "     note: {} does not exist yet — that is fine, write it and then run the tests",
            project.target
        );
    }
    println!("\nNext:  byo test --stage 1        run the first stage");
    println!("       byo status                where you are");
    println!("       byo site                  the journey map in your browser");
    Ok(0)
}

/// Find the project for a test run: `byo.toml`, or an ad-hoc one when the tester flags
/// already name the target (`byo shell --shell bash --stage 1` works anywhere).
fn project_for(track: Option<Track>, user: &[String]) -> Result<Project> {
    let cwd = std::env::current_dir().context("cannot read the current directory")?;
    match config::load_from(&cwd) {
        Ok(p) => {
            if let Some(t) = track {
                if p.track != t {
                    bail!(
                        "this is a {} project ({}), but `byo {t}` was asked for. Use `byo test`, \
                         or run `byo {t} {} <name>` from a {t} project.",
                        p.track,
                        cwd.join(config::FILE).display(),
                        t.target_flag()
                    );
                }
            }
            Ok(p)
        }
        Err(e) => {
            let track = track.unwrap_or(Track::Shell);
            let flag = track.target_flag();
            let inline = user
                .iter()
                .position(|a| a == flag)
                .and_then(|i| user.get(i + 1).cloned())
                .or_else(|| {
                    user.iter()
                        .find_map(|a| a.strip_prefix(&format!("{flag}=")).map(str::to_string))
                });
            match inline {
                Some(target) => Ok(Project {
                    root: cwd,
                    track,
                    target,
                    target_kind: TargetKind::Registered,
                    port: None,
                    log_dir: None,
                }),
                None => Err(e),
            }
        }
    }
}

fn test(paths: &Paths, track: Option<Track>, user: &[String]) -> Result<i32> {
    let project = project_for(track, user)?;
    runner::test(paths, &project, user)
}

fn resolve_track(explicit: &Option<String>) -> Result<Track> {
    if let Some(t) = explicit {
        return Track::parse(t);
    }
    let cwd = std::env::current_dir().context("cannot read the current directory")?;
    match config::load_from(&cwd) {
        Ok(p) => Ok(p.track),
        Err(e) => Err(e.context("pass --track shell or --track kafka to say which track you mean")),
    }
}

fn mark(paths: &Paths, a: StageArgs, done: bool) -> Result<i32> {
    let track = resolve_track(&a.track)?;
    paths.ensure()?;
    let conn = db::open(&paths.db())?;
    let row = db::set_state(&conn, track, a.stage, done)?;
    println!(
        "byo: {track} stage {:02} is now {}{}",
        row.stage,
        row.state,
        row.done_at
            .map(|d| format!(" (since {d})"))
            .unwrap_or_default()
    );
    Ok(0)
}

fn note(paths: &Paths, a: NoteArgs) -> Result<i32> {
    let track = resolve_track(&a.track)?;
    paths.ensure()?;
    let conn = db::open(&paths.db())?;
    let text = a.text.trim();
    let row = db::set_note(
        &conn,
        track,
        a.stage,
        if text.is_empty() { None } else { Some(text) },
    )?;
    match row.note {
        Some(n) => println!("byo: note on {track} stage {:02}: {n}", row.stage),
        None => println!("byo: cleared the note on {track} stage {:02}", row.stage),
    }
    Ok(0)
}

fn site(paths: &Paths, a: SiteArgs) -> Result<i32> {
    paths.ensure()?;
    if a.rebuild {
        server::rebuild(paths)?;
    }
    server::serve(paths, a.port, a.no_open)?;
    Ok(0)
}

fn database(paths: &Paths, cmd: DbCmd) -> Result<i32> {
    match cmd {
        DbCmd::Path => {
            println!("{}", paths.db().display());
            Ok(0)
        }
        DbCmd::Dump => {
            paths.ensure()?;
            let conn = db::open(&paths.db())?;
            println!("{}", serde_json::to_string_pretty(&db::dump(&conn)?)?);
            Ok(0)
        }
        DbCmd::Reset { yes } => {
            let path = paths.db();
            if !yes {
                print!(
                    "This deletes every project, run and stage in {}.\nType 'yes' to continue: ",
                    path.display()
                );
                std::io::stdout().flush().ok();
                let mut line = String::new();
                std::io::stdin()
                    .read_line(&mut line)
                    .context("cannot read your answer")?;
                if line.trim() != "yes" {
                    println!("byo: nothing was deleted.");
                    return Ok(1);
                }
            }
            paths.ensure()?;
            let mut conn = db::open(&path)?;
            db::reset(&mut conn)?;
            println!(
                "byo: {} is empty again (schema v{}).",
                path.display(),
                db::SCHEMA_VERSION
            );
            Ok(0)
        }
        DbCmd::SetSiteSource { path } => {
            let abs = std::fs::canonicalize(&path)
                .with_context(|| format!("cannot resolve {}", path.display()))?;
            if !abs.join("package.json").is_file() {
                bail!(
                    "{} does not look like the site sources (no package.json)",
                    abs.display()
                );
            }
            paths.ensure()?;
            let conn = db::open(&paths.db())?;
            db::set_meta(&conn, "site_source", &abs.to_string_lossy())?;
            println!("byo: `byo site --rebuild` will build {}", abs.display());
            Ok(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn tester_flags_after_test_are_not_eaten_by_clap() {
        let cli = Cli::try_parse_from(["byo", "test", "--stage", "5"]).unwrap();
        match cli.command {
            Cmd::Test(a) => assert_eq!(a.args, vec!["--stage", "5"]),
            other => panic!("{other:?}"),
        }
        let cli = Cli::try_parse_from(["byo", "test", "--until", "12", "--verbose"]).unwrap();
        match cli.command {
            Cmd::Test(a) => assert_eq!(a.args, vec!["--until", "12", "--verbose"]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn track_aliases_take_the_same_pass_through_args() {
        for (name, want) in [("shell", Track::Shell), ("kafka", Track::Kafka)] {
            let cli = Cli::try_parse_from(["byo", name, "--all", "--no-color"]).unwrap();
            let args = match cli.command {
                Cmd::Shell(a) | Cmd::Kafka(a) => a.args,
                other => panic!("{other:?}"),
            };
            assert_eq!(args, vec!["--all", "--no-color"]);
            assert_eq!(Track::parse(name).unwrap(), want);
        }
    }

    #[test]
    fn stage_and_note_commands_parse() {
        assert!(matches!(
            Cli::try_parse_from(["byo", "done", "3"]).unwrap().command,
            Cmd::Done(StageArgs {
                stage: 3,
                track: None
            })
        ));
        match Cli::try_parse_from(["byo", "note", "1", "hi", "--track", "kafka"])
            .unwrap()
            .command
        {
            Cmd::Note(a) => {
                assert_eq!(
                    (a.stage, a.text.as_str(), a.track.as_deref()),
                    (1, "hi", Some("kafka"))
                )
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn site_and_db_commands_parse() {
        match Cli::try_parse_from(["byo", "site", "--no-open", "--port", "4599"])
            .unwrap()
            .command
        {
            Cmd::Site(a) => assert_eq!((a.port, a.no_open, a.rebuild), (Some(4599), true, false)),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            Cli::try_parse_from(["byo", "db", "reset", "--yes"])
                .unwrap()
                .command,
            Cmd::Db(DbCmd::Reset { yes: true })
        ));
        assert!(Cli::try_parse_from(["byo", "db", "nope"]).is_err());
    }

    #[test]
    fn init_rejects_cross_track_flags() {
        let paths = Paths {
            home: std::env::temp_dir().join("byo-test-unused"),
        };
        let a = InitArgs {
            track: "shell".into(),
            command: None,
            shell: None,
            broker: Some("my_broker".into()),
            port: None,
            log_dir: None,
            force: false,
        };
        let e = init(&paths, a).unwrap_err();
        assert!(e.to_string().contains("kafka track"), "{e}");
    }
}
