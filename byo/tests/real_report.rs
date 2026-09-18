//! End-to-end-ish test over a real `shelltest --json` report (captured from
//! `byo test --until 3` against bash): ingest it, then read it back through the same API
//! handlers `byo site` serves.

use byo::api::{self, Request};
use byo::db;
use byo::paths::{Paths, Track};
use serde_json::Value;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/shelltest-until3.json"
);

struct Harness {
    _dir: tempfile::TempDir,
    conn: rusqlite::Connection,
    ctx: api::Ctx,
    run_id: i64,
}

fn harness() -> Harness {
    let dir = tempfile::tempdir().expect("temp dir");
    let paths = Paths {
        home: dir.path().to_path_buf(),
    };
    let mut conn = db::open(&paths.db()).expect("open db");
    let report = byo::report::read(std::path::Path::new(FIXTURE)).expect("read fixture");
    assert_eq!(report.target, "bash");
    assert_eq!(report.passed, 20);
    assert_eq!(report.failed, 0);
    let project_id =
        db::upsert_project(&conn, Track::Shell, dir.path(), "bash", "registered").expect("project");
    let run_id = db::ingest(
        &mut conn,
        Track::Shell,
        Some(project_id),
        &report,
        "--until 3",
        None,
        "2026-09-13T02:34:00Z",
    )
    .expect("ingest");
    let ctx = api::Ctx {
        paths,
        version: "0.1.0".into(),
    };
    Harness {
        _dir: dir,
        conn,
        ctx,
        run_id,
    }
}

fn get(h: &Harness, target: &str) -> (u16, Value) {
    let r = api::handle(&h.conn, &h.ctx, &Request::new("GET", target, "")).expect("api route");
    let v = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    (r.status, v)
}

#[test]
fn a_real_report_becomes_rows() {
    let h = harness();
    let tests: i64 = h
        .conn
        .query_row(
            "SELECT COUNT(*) FROM run_tests WHERE run_id = ?1",
            [h.run_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tests, 20, "every test in the report is a row");
    let stages: i64 = h
        .conn
        .query_row(
            "SELECT COUNT(DISTINCT stage) FROM run_tests WHERE run_id = ?1",
            [h.run_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stages, 3);
    let first: (u32, String, String, String) = h
        .conn
        .query_row(
            "SELECT stage, stage_name, test_name, status FROM run_tests
             WHERE run_id = ?1 ORDER BY seq LIMIT 1",
            [h.run_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(first.0, 1);
    assert_eq!(first.1, "Print the prompt and wait for input");
    assert_eq!(first.2, "prints the prompt on startup");
    assert_eq!(first.3, "pass");
}

#[test]
fn progress_is_derived_from_the_report() {
    let h = harness();
    let rows = db::stages(&h.conn, Track::Shell).unwrap();
    assert_eq!(rows.len(), 3);
    for r in &rows {
        assert_eq!(r.state, "done", "stage {} should be green", r.stage);
        assert_eq!(r.done_at.as_deref(), Some("2026-09-13T02:34:00Z"));
        assert_eq!(r.last_run_id, Some(h.run_id));
    }
    assert!(db::stages(&h.conn, Track::Kafka).unwrap().is_empty());
    assert_eq!(db::score(&h.conn).unwrap().xp, 300);
}

#[test]
fn the_run_round_trips_back_into_the_tester_shape() {
    let h = harness();
    let original: Value = serde_json::from_str(&std::fs::read_to_string(FIXTURE).unwrap()).unwrap();
    let (status, rebuilt) = get(&h, &format!("/api/runs/{}", h.run_id));
    assert_eq!(status, 200);
    assert_eq!(rebuilt["target"], original["shell"]);
    assert_eq!(rebuilt["passed"], original["passed"]);
    assert_eq!(rebuilt["failed"], original["failed"]);
    assert_eq!(rebuilt["skipped"], original["skipped"]);
    let ostages = original["stages"].as_array().unwrap();
    let rstages = rebuilt["stages"].as_array().unwrap();
    assert_eq!(ostages.len(), rstages.len());
    for (o, r) in ostages.iter().zip(rstages) {
        assert_eq!(o["stage"], r["stage"]);
        assert_eq!(o["name"], r["name"]);
        assert_eq!(o["file"], r["file"]);
        assert_eq!(o["passed"], r["passed"]);
        assert_eq!(o["failed"], r["failed"]);
        assert_eq!(o["skipped"], r["skipped"]);
        let ot = o["tests"].as_array().unwrap();
        let rt = r["tests"].as_array().unwrap();
        assert_eq!(ot.len(), rt.len());
        for (a, b) in ot.iter().zip(rt) {
            assert_eq!(a["name"], b["name"]);
            assert_eq!(a["status"], b["status"]);
            assert_eq!(a["ext"], b["ext"]);
            assert_eq!(a["failures"], b["failures"]);
            assert_eq!(a["actual"], b["actual"]);
        }
    }
}

#[test]
fn the_api_serves_the_ingested_run() {
    let h = harness();

    let (status, health) = get(&h, "/api/health");
    assert_eq!(status, 200);
    assert_eq!(health["ok"], true);
    assert_eq!(health["tracks"]["shell"]["project"]["command"], "bash");
    assert_eq!(health["schemaVersion"], db::SCHEMA_VERSION);

    let (status, progress) = get(&h, "/api/progress");
    assert_eq!(status, 200);
    assert_eq!(progress["shell"]["stages"]["1"]["state"], "done");
    assert_eq!(progress["shell"]["stages"]["3"]["state"], "done");
    assert_eq!(progress["shell"]["latestRunId"], h.run_id);
    assert_eq!(progress["xp"], 300);

    let (status, runs) = get(&h, "/api/runs?track=shell&limit=5");
    assert_eq!(status, 200);
    let runs = runs.as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["passed"], 20);
    assert_eq!(runs[0]["args"], "--until 3");

    let (status, latest) = get(&h, "/api/runs/latest?track=shell");
    assert_eq!(status, 200);
    assert_eq!(latest["id"], h.run_id);

    let (status, _) = get(&h, "/api/runs/latest?track=kafka");
    assert_eq!(status, 404);
}

#[test]
fn notes_and_manual_state_go_through_the_api() {
    let h = harness();
    let r = api::handle(
        &h.conn,
        &h.ctx,
        &Request::new(
            "POST",
            "/api/stages/shell/2",
            r#"{"state":"todo","note":"revisit quoting"}"#,
        ),
    )
    .unwrap();
    assert_eq!(r.status, 200);
    let v: Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["state"], "todo");
    assert_eq!(v["note"], "revisit quoting");
    assert_eq!(v["track"], "shell");

    let (_, progress) = get(&h, "/api/progress");
    assert_eq!(progress["shell"]["stages"]["2"]["note"], "revisit quoting");
    assert_eq!(progress["xp"], 200, "un-doing a stage costs its XP");
}
