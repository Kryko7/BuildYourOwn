//! `byo` — one command for the BuildYourOwn tracks: run the testers, keep progress in
//! SQLite, and serve the journey site with its JSON API.
//!
//! Every track-shaped decision here (which flags `byo init` accepts, what `byo <track>`
//! means, what error message lists the tracks) is answered by the
//! [registry][byo::track::TRACKS], so `byo` grows a sixth track without growing a `match`.

use anyhow::{bail, Context, Result};
use byo::config::{self, Project, TargetKind};
use byo::paths::Paths;
use byo::track::{self, Track};
use byo::{db, doctor, runner, server, status};
use clap::{Args, Parser, Subcommand};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

/// Build Your Own — six tracks, one command.
#[derive(Parser, Debug)]
#[command(name = "byo", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Set up the current directory as a project for one of the tracks (writes byo.toml)
    Init(InitArgs),
    /// Run the tester for this project and record the result
    Test(TestArgs),
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
    /// List the registered tracks and whether their testers are installed
    Tracks,
    /// `byo <track> [tester flags…]` — run one track's tester explicitly
    #[command(external_subcommand)]
    Track(Vec<String>),
}

#[derive(Args, Debug)]
struct InitArgs {
    /// Which track: shell, kafka, wasm, tls, link or dist
    track: String,
    /// Path to the program you are building, e.g. ./your_program.sh
    #[arg(long, value_name = "PATH")]
    command: Option<String>,
    /// A target already registered for this track (any track), e.g. bash
    #[arg(long, short = 't', value_name = "NAME")]
    target: Option<String>,
    /// A shell registered in shells.yaml (shell track), e.g. bash
    #[arg(long, value_name = "NAME")]
    shell: Option<String>,
    /// A broker registered in brokers.yaml (kafka track), e.g. my_broker
    #[arg(long, value_name = "NAME")]
    broker: Option<String>,
    /// A runtime registered in runtimes.yaml (wasm track), e.g. wasmtime
    #[arg(long, value_name = "NAME")]
    runtime: Option<String>,
    /// A server registered in servers.yaml (tls track), e.g. openssl
    #[arg(long, value_name = "NAME")]
    server: Option<String>,
    /// A linker registered in linkers.yaml (link track), e.g. gnu_ld
    #[arg(long, value_name = "NAME")]
    linker: Option<String>,
    /// Port the program must listen on (tracks that have a `port` key, e.g. kafka)
    #[arg(long)]
    port: Option<u16>,
    /// Log directory (tracks that have a `log_dir` key, e.g. kafka)
    #[arg(long, value_name = "DIR")]
    log_dir: Option<String>,
    /// Any other track-specific key: --set key=value (repeatable)
    #[arg(long = "set", value_name = "KEY=VALUE")]
    set: Vec<String>,
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
        Cmd::Init(a) => {
            let root = std::env::current_dir().context("cannot read the current directory")?;
            init(&paths, root, a)
        }
        Cmd::Test(a) => test(&paths, None, &a.args),
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
        Cmd::Tracks => {
            tracks();
            Ok(0)
        }
        Cmd::Track(argv) => track_alias(&paths, argv),
    }
}

/// `byo shell …`, `byo kafka …`, `byo wasm …` — an explicit-track alias for `byo test`.
///
/// Any subcommand clap does not know lands here; if it is a registered track id it runs
/// that track's tester, and if it is not, this is where the "did you mean" message is.
fn track_alias(paths: &Paths, argv: Vec<String>) -> Result<i32> {
    let (name, rest) = argv.split_first().context("empty command")?;
    match Track::find(name) {
        Some(t) => test(paths, Some(t), rest),
        None => bail!(
            "unknown command or track '{name}'.\n\
             Commands: init, test, status, done, undone, note, site, db, doctor, tracks.\n\
             Tracks:   {} (used as `byo {name} --stage 1`).",
            track::names()
        ),
    }
}

/// `byo tracks` — the registry, as the terminal sees it.
fn tracks() {
    println!();
    for t in Track::all() {
        let def = t.def();
        let state = match byo::paths::locate_tester(def.tester) {
            Some(p) => format!("installed ({})", p.display()),
            None => format!("not installed — build {}/", def.dir),
        };
        println!("{:<6} {}", def.id, def.title);
        println!("       {}", def.blurb);
        println!("       {} {} · {state}", def.tester, def.target_flag);
    }
    println!("\nStart one with: byo init <track> --command ./your_program.sh");
}

/// `byo init <track>` in `root` (the current directory, except in tests).
fn init(paths: &Paths, root: PathBuf, a: InitArgs) -> Result<i32> {
    let track = Track::parse(&a.track)?;
    let def = track.def();
    let existing = root.join(config::FILE);
    if existing.exists() && !a.force {
        bail!(
            "{} already exists — pass --force to overwrite it",
            existing.display()
        );
    }

    // The per-track sugar flags (--shell, --broker, …) are one registry key each: a flag
    // that is not this track's key is the user asking for the wrong track.
    let named: [(&str, &Option<String>); 5] = [
        ("shell", &a.shell),
        ("broker", &a.broker),
        ("runtime", &a.runtime),
        ("server", &a.server),
        ("linker", &a.linker),
    ];
    let mut registered = a.target.clone();
    for (key, value) in named {
        let Some(v) = value else { continue };
        if key != def.target_key {
            let owner = Track::by_target_key(key).map(|t| t.as_str()).unwrap_or(key);
            bail!(
                "--{key} belongs to the {owner} track; a {} project uses --{} or --command",
                def.id,
                def.target_key
            );
        }
        if registered.is_some() {
            bail!("name the target once: --target or --{key}, not both");
        }
        registered = Some(v.clone());
    }

    // Extra keys: the two sugar flags, plus --set key=value for anything a later track adds.
    let mut extras: BTreeMap<String, String> = BTreeMap::new();
    if let Some(p) = a.port {
        extras.insert("port".into(), p.to_string());
    }
    if let Some(d) = &a.log_dir {
        extras.insert("log_dir".into(), d.clone());
    }
    for pair in &a.set {
        let (k, v) = pair
            .split_once('=')
            .with_context(|| format!("--set wants key=value, got '{pair}'"))?;
        extras.insert(k.trim().to_string(), v.to_string());
    }
    if let Some(key) = extras.keys().find(|k| track.extra_key(k).is_none()) {
        let owner = Track::all().find(|t| t.extra_key(key).is_some());
        match owner {
            Some(t) => bail!(
                "`{key}` is a {t}-track key; the {} track has no such key",
                def.id
            ),
            None => bail!(
                "`{key}` is not a key of the {} track ({})",
                def.id,
                if def.extra_keys.is_empty() {
                    "it has no extra keys".to_string()
                } else {
                    format!(
                        "it has: {}",
                        def.extra_keys
                            .iter()
                            .map(|k| k.name)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            ),
        }
    }
    // Registry defaults fill in whatever the user did not give.
    for key in def.extra_keys {
        if let (None, Some(d)) = (extras.get(key.name), key.default) {
            extras.insert(key.name.to_string(), d.to_string());
        }
    }

    let command = match (&registered, a.command.clone()) {
        (Some(_), Some(_)) => bail!("pass either a registered name or --command, not both"),
        (Some(_), None) => None,
        (None, c) => Some(c.unwrap_or_else(|| def.default_command.to_string())),
    };
    let project = config::build(root.clone(), track, registered, command, extras)?;
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
        "     {} — testing {} `{}`",
        def.title,
        project.target_kind.as_str(),
        project.target
    );
    if project.target_kind == TargetKind::Command && !PathBuf::from(&project.target).exists() {
        println!(
            "     note: {} does not exist yet — that is fine, write it and then run the tests",
            project.target
        );
    }
    if !byo::paths::tester_installed(track) {
        println!(
            "     warning: {} is not installed yet — run ./install.sh from the BuildYourOwn repo",
            def.tester
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
            // No byo.toml: the run is still possible when the flags name the target
            // themselves. Which flag that is comes from the registry, so
            // `byo test --runtime wasmtime --stage 1` works in a scratch directory
            // exactly like `byo shell --shell bash --stage 1` does.
            let candidates: Vec<(Track, String)> = match track {
                Some(t) => inline_target(t, user)
                    .map(|v| vec![(t, v)])
                    .unwrap_or_default(),
                None => Track::all()
                    .filter_map(|t| inline_target(t, user).map(|v| (t, v)))
                    .collect(),
            };
            match candidates.as_slice() {
                [(track, target)] => Ok(Project {
                    root: cwd,
                    track: *track,
                    target: target.clone(),
                    target_kind: TargetKind::Registered,
                    extras: BTreeMap::new(),
                }),
                _ => Err(e),
            }
        }
    }
}

/// The value the user gave a track's target flag, as `--flag value` or `--flag=value`.
fn inline_target(track: Track, user: &[String]) -> Option<String> {
    let flag = track.target_flag();
    user.iter()
        .position(|a| a == flag)
        .and_then(|i| user.get(i + 1).cloned())
        .or_else(|| {
            user.iter()
                .find_map(|a| a.strip_prefix(&format!("{flag}=")).map(str::to_string))
        })
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
        Err(e) => Err(e.context(format!(
            "pass --track <{}> to say which track you mean",
            track::names()
        ))),
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
    use byo::track::TRACKS;
    use clap::CommandFactory;

    fn init_args(track: &str) -> InitArgs {
        InitArgs {
            track: track.into(),
            command: None,
            target: None,
            shell: None,
            broker: None,
            runtime: None,
            server: None,
            linker: None,
            port: None,
            log_dir: None,
            set: vec![],
            force: false,
        }
    }

    /// A temporary project directory plus its own `$BYO_HOME`.
    fn sandbox() -> (tempfile::TempDir, PathBuf, Paths) {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("project");
        std::fs::create_dir_all(&root).expect("project dir");
        let paths = Paths {
            home: dir.path().join("byo-home"),
        };
        (dir, root, paths)
    }

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
    fn every_registered_track_is_an_alias_subcommand() {
        for t in Track::all() {
            let cli =
                Cli::try_parse_from(["byo", t.as_str(), "--all", "--no-color"]).expect("parses");
            match cli.command {
                Cmd::Track(argv) => {
                    assert_eq!(argv[0], t.as_str());
                    assert_eq!(&argv[1..], &["--all", "--no-color"]);
                    assert_eq!(Track::find(&argv[0]), Some(t));
                }
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn an_unknown_alias_lists_commands_and_tracks() {
        let paths = Paths {
            home: std::env::temp_dir().join("byo-test-unused"),
        };
        let e = track_alias(&paths, vec!["redis".into()])
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown command or track 'redis'"), "{e}");
        for t in TRACKS {
            assert!(e.contains(t.id), "{e} should list {}", t.id);
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
        match Cli::try_parse_from(["byo", "note", "1", "hi", "--track", "wasm"])
            .unwrap()
            .command
        {
            Cmd::Note(a) => {
                assert_eq!(
                    (a.stage, a.text.as_str(), a.track.as_deref()),
                    (1, "hi", Some("wasm"))
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
    fn init_writes_a_project_for_every_registered_track() {
        let (_d, root, paths) = sandbox();
        for t in Track::all() {
            let mut a = init_args(t.as_str());
            a.force = true;
            assert_eq!(init(&paths, root.clone(), a).unwrap(), 0);
            let p = config::load_from(&root).expect("config round-trips");
            assert_eq!(p.track, t);
            assert_eq!(p.target, t.def().default_command);
            assert_eq!(p.target_kind, TargetKind::Command);
            for key in t.def().extra_keys {
                if let Some(d) = key.default {
                    assert_eq!(p.extra(key.name), Some(d), "{t} {}", key.name);
                }
            }
            let conn = db::open(&paths.db()).unwrap();
            assert!(db::latest_project(&conn, t).unwrap().is_some());
        }
    }

    #[test]
    fn init_refuses_to_overwrite_without_force() {
        let (_d, root, paths) = sandbox();
        init(&paths, root.clone(), init_args("shell")).unwrap();
        let e = init(&paths, root.clone(), init_args("kafka")).unwrap_err();
        assert!(e.to_string().contains("--force"), "{e}");
        let mut a = init_args("kafka");
        a.force = true;
        init(&paths, root.clone(), a).unwrap();
        assert_eq!(config::load_from(&root).unwrap().track, Track::KAFKA);
    }

    #[test]
    fn init_accepts_the_per_track_target_flags() {
        let (_d, root, paths) = sandbox();
        let mut a = init_args("shell");
        a.shell = Some("bash".into());
        init(&paths, root.clone(), a).unwrap();
        let p = config::load_from(&root).unwrap();
        assert_eq!(
            (p.target.as_str(), p.target_kind),
            ("bash", TargetKind::Registered)
        );

        let mut a = init_args("wasm");
        a.target = Some("wasmtime".into());
        a.force = true;
        init(&paths, root.clone(), a).unwrap();
        let p = config::load_from(&root).unwrap();
        assert_eq!(p.track, Track::WASM);
        assert_eq!(p.target, "wasmtime");
    }

    #[test]
    fn init_rejects_cross_track_flags_and_keys() {
        let (_d, root, paths) = sandbox();
        let mut a = init_args("shell");
        a.broker = Some("my_broker".into());
        let e = init(&paths, root.clone(), a).unwrap_err();
        assert!(e.to_string().contains("kafka track"), "{e}");

        let mut a = init_args("wasm");
        a.port = Some(9092);
        let e = init(&paths, root.clone(), a).unwrap_err();
        assert!(e.to_string().contains("kafka-track key"), "{e}");

        let e = init(&paths, root.clone(), init_args("redis")).unwrap_err();
        assert!(e.to_string().contains("unknown track 'redis'"), "{e}");
        assert!(!root.join(config::FILE).exists(), "nothing was written");
    }

    #[test]
    fn an_inline_target_flag_picks_its_track() {
        for t in Track::all() {
            let args = vec![t.target_flag().to_string(), "reference".to_string()];
            assert_eq!(inline_target(t, &args).as_deref(), Some("reference"));
            let equals = vec![format!("{}=reference", t.target_flag())];
            assert_eq!(inline_target(t, &equals).as_deref(), Some("reference"));
            for other in Track::all().filter(|o| *o != t) {
                assert_eq!(inline_target(other, &args), None, "{t} vs {other}");
            }
        }
        assert_eq!(
            inline_target(Track::SHELL, &["--verbose".to_string()]),
            None
        );
    }

    #[test]
    fn init_rejects_an_unregistered_set_key() {
        let (_d, root, paths) = sandbox();
        let mut a = init_args("link");
        a.set = vec!["colour=blue".into()];
        let e = init(&paths, root, a).unwrap_err();
        assert!(e.to_string().contains("not a key of the link track"), "{e}");
    }
}
