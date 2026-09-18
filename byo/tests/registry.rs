//! The track registry, end to end: a fake tester per track shape, an ingest + API round
//! trip for a track that is neither shell nor kafka, and `byo doctor` with a track missing.
//!
//! The fake testers are tiny shell scripts named after each track's real tester. They record
//! the argv `byo` built for them and write a `--json` report in the testers' shape, which is
//! enough to prove that `byo test` speaks every track's command line and that the database
//! and the API carry a track id they have never seen before.

use byo::config::{self, Project};
use byo::db;
use byo::doctor::{self, Level};
use byo::paths::Paths;
use byo::track::Track;
use byo::{api, runner};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// `PATH` is process-wide; the tests that plant fake binaries take turns.
static PATH_LOCK: Mutex<()> = Mutex::new(());

/// A report in the testers' `--json` shape: stage 1 green, stage 2 half red.
const REPORT: &str = r#"{
  "target": "TARGET", "validate": false,
  "stages": [
    {"stage": 1, "name": "First", "file": "01_first.yaml", "passed": 2, "failed": 0, "skipped": 0,
     "tests": [
       {"name": "decodes a header", "status": "pass", "ext": false, "duration_ms": 3,
        "failures": [], "actual": [["bytes", "00 61 73 6d"]]},
       {"name": "rejects a short module", "status": "pass", "ext": false, "duration_ms": 1,
        "failures": [], "actual": []}
     ]},
    {"stage": 2, "name": "Second", "file": "02_second.yaml", "passed": 1, "failed": 1, "skipped": 0,
     "tests": [
       {"name": "runs the start function", "status": "pass", "ext": false, "duration_ms": 9,
        "failures": [], "actual": []},
       {"name": "traps on divide by zero", "status": "fail", "ext": false, "duration_ms": 4,
        "failures": ["expected `integer divide by zero`, got `oops`"],
        "actual": [["stderr", "oops"]], "failure_kind": "wrong_output"}
     ]}
  ],
  "passed": 3, "failed": 1, "skipped": 0, "elapsed_ms": 42
}"#;

/// Write a fake tester that records its argv next to itself and writes the report.
fn plant_tester(bin_dir: &Path, name: &str) -> PathBuf {
    std::fs::create_dir_all(bin_dir).expect("bin dir");
    let path = bin_dir.join(name);
    let script = format!(
        r#"#!/bin/sh
# `--version` is what `byo doctor` and /api/tracks probe with; answer without recording,
# so a probe from another test never clobbers the argv this one is about to read.
if [ "$1" = "--version" ]; then echo "fake {name} 0.0.0"; exit 0; fi
printf '%s\n' "$@" > "$0.argv"
json=""
prev=""
for a in "$@"; do
  case "$a" in
    --json=*) json="${{a#--json=}}" ;;
  esac
  if [ "$prev" = "--json" ]; then json="$a"; fi
  prev="$a"
done
if [ -n "$json" ]; then
  cat > "$json" <<'BYO_EOF'
{REPORT}
BYO_EOF
fi
echo "fake {name} ran"
exit 0
"#
    );
    std::fs::write(&path, script).expect("write fake tester");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    path
}

/// The argv the fake tester was called with.
fn argv(tester: &Path) -> Vec<String> {
    let recorded = tester.with_file_name(format!(
        "{}.argv",
        tester.file_name().unwrap().to_string_lossy()
    ));
    std::fs::read_to_string(recorded)
        .expect("the fake tester recorded no argv")
        .lines()
        .map(str::to_string)
        .collect()
}

/// Install every data file a track declares, so `byo test` has something to pass.
fn install_data(paths: &Paths, track: Track) {
    for f in track.def().data_files {
        let p = paths.data_file(f);
        if f.dir {
            std::fs::create_dir_all(&p).expect("data dir");
            std::fs::write(p.join("01_first.yaml"), "stage: 1\n").expect("stage file");
        } else {
            std::fs::create_dir_all(p.parent().unwrap()).expect("data parent");
            std::fs::write(&p, "targets: {}\n").expect("data file");
        }
    }
}

fn project_for(root: &Path, track: Track) -> Project {
    let p = config::build(
        root.to_path_buf(),
        track,
        None,
        Some(track.def().default_command.to_string()),
        track
            .def()
            .extra_keys
            .iter()
            .filter_map(|k| k.default.map(|d| (k.name.to_string(), d.to_string())))
            .collect::<BTreeMap<_, _>>(),
    )
    .expect("a project for every track");
    p.save().expect("write byo.toml");
    p
}

#[test]
fn byo_test_drives_a_tester_for_every_registered_track() {
    let _guard = PATH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("temp dir");
    let bin = dir.path().join("bin");
    let original_path = std::env::var_os("PATH");
    let mut search = vec![bin.clone()];
    if let Some(p) = &original_path {
        search.extend(std::env::split_paths(p));
    }
    std::env::set_var("PATH", std::env::join_paths(search).expect("PATH"));

    for track in Track::all() {
        let def = track.def();
        let home = dir.path().join(format!("home-{}", def.id));
        let paths = Paths { home };
        paths.ensure().expect("data dir");
        install_data(&paths, track);
        let root = dir.path().join(format!("proj-{}", def.id));
        std::fs::create_dir_all(&root).expect("project dir");
        let project = project_for(&root, track);
        let tester = plant_tester(&bin, def.tester);

        let code = runner::test(&paths, &project, &["--stage".into(), "1".into()])
            .unwrap_or_else(|e| panic!("{}: {e:#}", def.id));
        assert_eq!(code, 0, "{} exit code", def.id);

        // The command line came from the registry: target flag, data files, extra keys.
        let args = argv(&tester);
        let pos = |flag: &str| args.iter().position(|a| a == flag);
        assert_eq!(
            args.get(pos(def.target_flag).map(|i| i + 1).unwrap_or(0)),
            Some(&def.default_command.to_string()),
            "{} target: {args:?}",
            def.id
        );
        for f in def.data_files {
            let i = pos(f.flag).unwrap_or_else(|| panic!("{} lacks {}: {args:?}", def.id, f.flag));
            assert_eq!(
                args[i + 1],
                paths.data_file(f).to_string_lossy(),
                "{} {}",
                def.id,
                f.flag
            );
        }
        for k in def.extra_keys {
            let i = pos(k.flag).unwrap_or_else(|| panic!("{} lacks {}: {args:?}", def.id, k.flag));
            assert_eq!(args[i + 1], k.default.unwrap_or_default(), "{}", def.id);
        }
        assert!(args.contains(&"--stage".to_string()) && args.contains(&"1".to_string()));

        // …and the run landed in the database under this track's id.
        let conn = db::open(&paths.db()).expect("db");
        let runs = db::runs(&conn, Some(track), 10).expect("runs");
        assert_eq!(runs.len(), 1, "{}", def.id);
        assert_eq!(runs[0].track, def.id);
        assert_eq!((runs[0].passed, runs[0].failed), (3, 1), "{}", def.id);
        let stages = db::stages(&conn, track).expect("stages");
        assert_eq!(stages.len(), 2, "{}", def.id);
        assert_eq!(stages[0].state, "done", "{} stage 1", def.id);
        assert_eq!(stages[1].state, "in_progress", "{} stage 2", def.id);
        for other in Track::all().filter(|t| *t != track) {
            assert!(
                db::stages(&conn, other).expect("stages").is_empty(),
                "{} leaked into {other}",
                def.id
            );
        }
    }

    match original_path {
        Some(p) => std::env::set_var("PATH", p),
        None => std::env::remove_var("PATH"),
    }
}

#[test]
fn a_new_track_round_trips_through_the_database_and_the_api() {
    // wasm is neither of the two tracks the schema was written for; no migration needed,
    // because `track` has always been a string column.
    let track = Track::WASM;
    let dir = tempfile::tempdir().expect("temp dir");
    let paths = Paths {
        home: dir.path().to_path_buf(),
    };
    std::fs::write(
        paths.catalog(track),
        r#"{"track":"wasm","sections":[{"id":"A","title":"Binary format","stages":[1,2]}],
            "stages":[{"number":1,"name":"First","ext":false},
                      {"number":2,"name":"Second","ext":false}]}"#,
    )
    .expect("catalog");

    let report_path = dir.path().join("report.json");
    std::fs::write(&report_path, REPORT.replace("TARGET", "./your_program.sh")).expect("report");
    let report = byo::report::read(&report_path).expect("parse report");

    let mut conn = db::open(&paths.db()).expect("db");
    assert_eq!(
        db::schema_version(&conn).expect("schema"),
        db::SCHEMA_VERSION,
        "a new track needs no migration"
    );
    let project_id = db::upsert_project(&conn, track, dir.path(), "./your_program.sh", "command")
        .expect("project");
    let run_id = db::ingest(
        &mut conn,
        track,
        Some(project_id),
        &report,
        "--until 2",
        None,
        "2026-09-18T10:00:00Z",
    )
    .expect("ingest");

    let ctx = api::Ctx {
        paths,
        version: "0.1.0-test".into(),
    };
    let get = |target: &str| {
        let r = api::handle(&conn, &ctx, &api::Request::new("GET", target, "")).expect("api route");
        (
            r.status,
            serde_json::from_str::<Value>(&r.body).unwrap_or(Value::Null),
        )
    };

    let (status, health) = get("/api/health");
    assert_eq!(status, 200);
    assert_eq!(
        health["tracks"]["wasm"]["project"]["command"],
        "./your_program.sh"
    );
    assert_eq!(health["tracks"]["wasm"]["catalog"], true);
    assert_eq!(health["tracks"]["shell"]["catalog"], false);

    let (status, tracks) = get("/api/tracks");
    assert_eq!(status, 200);
    let wasm = tracks
        .as_array()
        .expect("array")
        .iter()
        .find(|t| t["id"] == "wasm")
        .expect("wasm is registered");
    assert_eq!(wasm["title"], Track::WASM.title());
    assert_eq!(wasm["accent"], Track::WASM.def().accent);
    assert_eq!(wasm["catalog"], true);

    let (status, progress) = get("/api/progress");
    assert_eq!(status, 200);
    assert_eq!(progress["wasm"]["stages"]["1"]["state"], "done");
    assert_eq!(progress["wasm"]["stages"]["2"]["state"], "in_progress");
    assert_eq!(progress["wasm"]["latestRunId"], run_id);
    assert_eq!(progress["xp"], 120);

    let (status, runs) = get("/api/runs?track=wasm&limit=5");
    assert_eq!(status, 200);
    assert_eq!(runs.as_array().expect("array").len(), 1);
    assert_eq!(runs[0]["track"], "wasm");
    assert_eq!(runs[0]["args"], "--until 2");

    let (status, latest) = get("/api/runs/latest?track=wasm");
    assert_eq!(status, 200);
    assert_eq!(latest["id"], run_id);
    assert_eq!(latest["stages"][1]["tests"][1]["status"], "fail");
    assert_eq!(
        latest["stages"][1]["tests"][1]["failures"][0],
        "expected `integer divide by zero`, got `oops`"
    );
    assert_eq!(
        latest["stages"][1]["tests"][1]["failure_kind"],
        "wrong_output"
    );

    let (status, catalog) = get("/api/catalog/wasm");
    assert_eq!(status, 200);
    assert_eq!(catalog["track"], "wasm");

    let r = api::handle(
        &conn,
        &ctx,
        &api::Request::new(
            "POST",
            "/api/stages/wasm/2",
            r#"{"state":"done","note":"traps!"}"#,
        ),
    )
    .expect("api route");
    assert_eq!(r.status, 200);
    let v: Value = serde_json::from_str(&r.body).expect("json");
    assert_eq!(
        (v["track"].as_str(), v["state"].as_str()),
        (Some("wasm"), Some("done"))
    );
    assert_eq!(v["note"], "traps!");
}

#[test]
fn doctor_reports_a_missing_track_without_failing() {
    let dir = tempfile::tempdir().expect("temp dir");
    let paths = Paths {
        home: dir.path().to_path_buf(),
    };
    // A data directory that knows about the shell track only.
    install_data(&paths, Track::SHELL);
    std::fs::write(
        paths.catalog(Track::SHELL),
        r#"{"track":"shell","sections":[],"stages":[{"number":1,"name":"Prompt","ext":false}]}"#,
    )
    .expect("catalog");

    let checks = doctor::collect(&paths);
    let named = |n: &str| checks.iter().find(|c| c.name == n).map(|c| c.level);
    assert_eq!(named("data dir"), Some(Level::Ok));
    assert_eq!(named("shell data"), Some(Level::Ok));
    assert_eq!(named("shell catalog"), Some(Level::Ok));

    for absent in [Track::TLS, Track::LINK] {
        let rows = doctor::track_checks(&paths, absent);
        assert!(
            rows.iter().all(|c| c.level != Level::Bad),
            "{absent} must never be fatal: {rows:#?}"
        );
        assert!(
            rows.iter().any(|c| c.detail.contains("not installed")
                || c.detail.contains("not found")
                || c.detail.contains("missing")),
            "{absent} should say what is missing: {rows:#?}"
        );
    }
    // The only thing that may be fatal here is "no tester at all on this machine".
    for c in checks.iter().filter(|c| c.level == Level::Bad) {
        assert_eq!(c.name, "testers", "unexpected failure: {c:?}");
    }
}
