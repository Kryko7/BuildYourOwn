//! The JSON API served by `byo site`, as a pure function of (request, database).
//!
//! Keeping the routing and the handlers free of `tiny_http` types means the whole API can
//! be exercised in-process by the unit tests at the bottom of this file, and the server in
//! `server.rs` only has to translate types.

use crate::db;
use crate::paths::{Paths, Track};
use anyhow::Result;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::HashMap;

/// Everything a handler needs besides the database.
pub struct Ctx {
    /// Where the data directory is (catalogs, DB path shown by `/api/health`).
    pub paths: Paths,
    /// The `byo` version string.
    pub version: String,
}

/// A request, already split into the parts the handlers care about.
#[derive(Debug, Clone)]
pub struct Request {
    /// `GET`, `POST`, …
    pub method: String,
    /// Path with the query string removed, percent-decoded.
    pub path: String,
    /// Parsed query string.
    pub query: HashMap<String, String>,
    /// Raw request body.
    pub body: String,
}

impl Request {
    /// Build a request from a method and a raw target such as `/api/runs?track=shell`.
    pub fn new(method: &str, target: &str, body: &str) -> Self {
        let (path, qs) = match target.split_once('?') {
            Some((p, q)) => (p, q),
            None => (target, ""),
        };
        Self {
            method: method.to_ascii_uppercase(),
            path: decode(path),
            query: parse_query(qs),
            body: body.to_string(),
        }
    }
}

/// What a handler produced.
#[derive(Debug, Clone)]
pub struct Response {
    /// HTTP status code.
    pub status: u16,
    /// Response body.
    pub body: String,
    /// `Content-Type` header value.
    pub content_type: &'static str,
}

impl Response {
    /// A `200 OK` JSON response.
    pub fn json(value: Value) -> Self {
        Self {
            status: 200,
            body: serde_json::to_string(&value).unwrap_or_else(|_| "{}".into()),
            content_type: "application/json; charset=utf-8",
        }
    }

    /// A JSON `{"error": ...}` response with a status code.
    pub fn error(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            body: json!({ "error": message.into() }).to_string(),
            content_type: "application/json; charset=utf-8",
        }
    }

    /// A response carrying a JSON document read verbatim from disk.
    pub fn raw_json(body: String) -> Self {
        Self {
            status: 200,
            body,
            content_type: "application/json; charset=utf-8",
        }
    }
}

/// Percent-decode a path, leaving invalid sequences alone.
pub fn decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .into_owned()
}

fn parse_query(qs: &str) -> HashMap<String, String> {
    qs.split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (decode(k), decode(&v.replace('+', " "))),
            None => (decode(pair), String::new()),
        })
        .collect()
}

/// Route and run one request. `None` means "not an API path" — the caller serves a file.
pub fn handle(conn: &Connection, ctx: &Ctx, req: &Request) -> Option<Response> {
    let rest = req.path.strip_prefix("/api/")?;
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let seg: Vec<&str> = rest.split('/').collect();
    Some(match dispatch(conn, ctx, req, &seg) {
        Ok(r) => r,
        Err(e) => Response::error(500, format!("{e:#}")),
    })
}

fn dispatch(conn: &Connection, ctx: &Ctx, req: &Request, seg: &[&str]) -> Result<Response> {
    let get = req.method == "GET";
    let post = req.method == "POST";
    match seg {
        ["health"] if get => health(conn, ctx),
        ["progress"] if get => progress(conn),
        ["runs"] if get => list_runs(conn, req),
        ["runs", "latest"] if get => latest_run(conn, req),
        ["runs", id] if get => one_run(conn, id),
        ["stages", track, n] if post => post_stage(conn, req, track, n),
        ["catalog", track] if get => catalog(ctx, track),
        ["health" | "progress" | "runs" | "stages" | "catalog", ..] => Ok(Response::error(
            405,
            format!("{} is not allowed on {}", req.method, req.path),
        )),
        _ => Ok(Response::error(
            404,
            format!("no such endpoint: {}", req.path),
        )),
    }
}

fn health(conn: &Connection, ctx: &Ctx) -> Result<Response> {
    let mut tracks = serde_json::Map::new();
    for t in Track::ALL {
        let project = db::latest_project(conn, t)?;
        tracks.insert(t.as_str().into(), json!({ "project": project }));
    }
    Ok(Response::json(json!({
        "ok": true,
        "version": ctx.version,
        "db": ctx.paths.db().to_string_lossy(),
        "schemaVersion": db::schema_version(conn)?,
        "dataDir": ctx.paths.home.to_string_lossy(),
        "tracks": tracks,
    })))
}

fn progress(conn: &Connection) -> Result<Response> {
    let mut out = serde_json::Map::new();
    for t in Track::ALL {
        let mut stages = serde_json::Map::new();
        for s in db::stages(conn, t)? {
            stages.insert(
                s.stage.to_string(),
                json!({
                    "state": s.state,
                    "doneAt": s.done_at,
                    "note": s.note,
                    "lastRunId": s.last_run_id,
                    "updatedAt": s.updated_at,
                }),
            );
        }
        out.insert(
            t.as_str().into(),
            json!({ "stages": stages, "latestRunId": db::latest_run_id(conn, t)? }),
        );
    }
    let score = db::score(conn)?;
    out.insert("streak".into(), json!(score.streak));
    out.insert("xp".into(), json!(score.xp));
    out.insert("level".into(), json!(score.level));
    Ok(Response::json(Value::Object(out)))
}

fn track_param(req: &Request) -> Result<Option<Track>, Response> {
    match req.query.get("track") {
        None => Ok(None),
        Some(t) if t.is_empty() => Ok(None),
        Some(t) => Track::parse(t)
            .map(Some)
            .map_err(|e| Response::error(400, e.to_string())),
    }
}

fn list_runs(conn: &Connection, req: &Request) -> Result<Response> {
    let track = match track_param(req) {
        Ok(t) => t,
        Err(r) => return Ok(r),
    };
    let limit = match req.query.get("limit") {
        None => 20,
        Some(l) => match l.parse::<i64>() {
            Ok(n) if (1..=1000).contains(&n) => n,
            _ => {
                return Ok(Response::error(
                    400,
                    "limit must be an integer between 1 and 1000",
                ))
            }
        },
    };
    Ok(Response::json(serde_json::to_value(db::runs(
        conn, track, limit,
    )?)?))
}

fn latest_run(conn: &Connection, req: &Request) -> Result<Response> {
    let track = match track_param(req) {
        Ok(t) => t,
        Err(r) => return Ok(r),
    };
    let id = match track {
        Some(t) => db::latest_run_id(conn, t)?,
        None => db::runs(conn, None, 1)?.first().map(|r| r.id),
    };
    let Some(id) = id else {
        return Ok(Response::error(
            404,
            "no runs recorded yet — run `byo test` first",
        ));
    };
    match db::run_report(conn, id)? {
        Some(rep) => Ok(Response::json(with_id(rep, id)?)),
        None => Ok(Response::error(
            404,
            format!("run {id} has no recorded tests"),
        )),
    }
}

fn one_run(conn: &Connection, id: &str) -> Result<Response> {
    let Ok(id) = id.parse::<i64>() else {
        return Ok(Response::error(
            400,
            format!("run id must be a number, got '{id}'"),
        ));
    };
    match db::run_report(conn, id)? {
        Some(rep) => Ok(Response::json(with_id(rep, id)?)),
        None => Ok(Response::error(404, format!("no run with id {id}"))),
    }
}

/// The testers' report shape, plus the `id` of the run it was rebuilt from.
fn with_id(rep: crate::report::Report, id: i64) -> Result<Value> {
    let mut v = serde_json::to_value(rep)?;
    if let Some(map) = v.as_object_mut() {
        map.insert("id".into(), json!(id));
    }
    Ok(v)
}

fn post_stage(conn: &Connection, req: &Request, track: &str, n: &str) -> Result<Response> {
    let track = match Track::parse(track) {
        Ok(t) => t,
        Err(e) => return Ok(Response::error(404, e.to_string())),
    };
    let Ok(stage) = n.parse::<u32>() else {
        return Ok(Response::error(
            400,
            format!("stage must be a number, got '{n}'"),
        ));
    };
    let body: Value = if req.body.trim().is_empty() {
        json!({})
    } else {
        match serde_json::from_str(&req.body) {
            Ok(v) => v,
            Err(e) => return Ok(Response::error(400, format!("invalid JSON body: {e}"))),
        }
    };
    let Some(obj) = body.as_object() else {
        return Ok(Response::error(400, "body must be a JSON object"));
    };
    if let Some(state) = obj.get("state") {
        match state.as_str() {
            Some("done") => {
                db::set_state(conn, track, stage, true)?;
            }
            Some("todo") => {
                db::set_state(conn, track, stage, false)?;
            }
            _ => return Ok(Response::error(400, "state must be \"done\" or \"todo\"")),
        }
    }
    if let Some(note) = obj.get("note") {
        match note {
            Value::String(s) if s.is_empty() => {
                db::set_note(conn, track, stage, None)?;
            }
            Value::String(s) => {
                db::set_note(conn, track, stage, Some(s))?;
            }
            Value::Null => {
                db::set_note(conn, track, stage, None)?;
            }
            _ => return Ok(Response::error(400, "note must be a string or null")),
        }
    }
    let row = db::stage(conn, track, stage)?;
    let row = match row {
        Some(r) => serde_json::to_value(r)?,
        None => json!({
            "stage": stage, "state": "todo", "doneAt": null, "note": null,
            "lastRunId": null, "updatedAt": db::now(),
        }),
    };
    let mut v = row;
    if let Some(map) = v.as_object_mut() {
        map.insert("track".into(), json!(track.as_str()));
    }
    Ok(Response::json(v))
}

fn catalog(ctx: &Ctx, track: &str) -> Result<Response> {
    let track = match Track::parse(track) {
        Ok(t) => t,
        Err(e) => return Ok(Response::error(404, e.to_string())),
    };
    let path = ctx.paths.catalog(track);
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Response::raw_json(text)),
        Err(_) => Ok(Response::error(
            404,
            format!(
                "no catalog for the {track} track at {} — reinstall with ./install.sh to copy it",
                path.display()
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report;
    use std::path::PathBuf;

    fn ctx(home: PathBuf) -> Ctx {
        Ctx {
            paths: Paths { home },
            version: "0.1.0-test".into(),
        }
    }

    fn seed(conn: &mut Connection) -> i64 {
        db::upsert_project(
            conn,
            Track::Shell,
            std::path::Path::new("/proj"),
            "bash",
            "registered",
        )
        .unwrap();
        let rep = report::Report {
            target: "bash".into(),
            validate: false,
            stages: vec![report::Stage {
                stage: 1,
                name: "Prompt".into(),
                file: "01_prompt.yaml".into(),
                passed: 1,
                failed: 0,
                skipped: 0,
                tests: vec![report::Test {
                    name: "prints a prompt".into(),
                    status: "pass".into(),
                    ext: false,
                    duration_ms: 4,
                    failures: vec![],
                    skip_reason: None,
                    actual: vec![("stdout".into(), "$ ".into())],
                    failure_kind: None,
                }],
            }],
            passed: 1,
            failed: 0,
            skipped: 0,
            elapsed_ms: 20,
        };
        db::ingest(
            conn,
            Track::Shell,
            None,
            &rep,
            "--stage 1",
            None,
            &db::now(),
        )
        .unwrap()
    }

    fn get(conn: &Connection, c: &Ctx, target: &str) -> Response {
        handle(conn, c, &Request::new("GET", target, "")).expect("api path")
    }

    fn body(r: &Response) -> Value {
        serde_json::from_str(&r.body).expect("json body")
    }

    #[test]
    fn non_api_paths_fall_through() {
        let conn = db::open_memory().unwrap();
        let c = ctx(PathBuf::from("/x"));
        assert!(handle(&conn, &c, &Request::new("GET", "/shell/", "")).is_none());
        assert!(handle(&conn, &c, &Request::new("GET", "/", "")).is_none());
    }

    #[test]
    fn health_reports_version_db_and_projects() {
        let mut conn = db::open_memory().unwrap();
        seed(&mut conn);
        let c = ctx(PathBuf::from("/data"));
        let r = get(&conn, &c, "/api/health");
        assert_eq!(r.status, 200);
        let v = body(&r);
        assert_eq!(v["ok"], true);
        assert_eq!(v["version"], "0.1.0-test");
        assert_eq!(v["db"], "/data/byo.db");
        assert_eq!(v["schemaVersion"], db::SCHEMA_VERSION);
        assert_eq!(v["tracks"]["shell"]["project"]["command"], "bash");
        assert!(v["tracks"]["kafka"]["project"].is_null());
    }

    #[test]
    fn progress_is_keyed_by_stage_number() {
        let mut conn = db::open_memory().unwrap();
        let run = seed(&mut conn);
        db::set_note(&conn, Track::Shell, 1, Some("hi")).unwrap();
        let v = body(&get(&conn, &ctx(PathBuf::from("/d")), "/api/progress"));
        assert_eq!(v["shell"]["stages"]["1"]["state"], "done");
        assert_eq!(v["shell"]["stages"]["1"]["note"], "hi");
        assert_eq!(v["shell"]["stages"]["1"]["lastRunId"], run);
        assert_eq!(v["kafka"]["stages"], json!({}));
        assert_eq!(v["xp"], 100);
        assert_eq!(v["streak"], 1);
    }

    #[test]
    fn runs_filter_and_limit() {
        let mut conn = db::open_memory().unwrap();
        seed(&mut conn);
        let c = ctx(PathBuf::from("/d"));
        let v = body(&get(&conn, &c, "/api/runs"));
        assert_eq!(v.as_array().unwrap().len(), 1);
        assert_eq!(v[0]["track"], "shell");
        assert_eq!(v[0]["elapsedMs"], 20);
        assert_eq!(v[0]["startedAt"].as_str().unwrap().len(), 20);
        assert_eq!(
            body(&get(&conn, &c, "/api/runs?track=kafka"))
                .as_array()
                .unwrap()
                .len(),
            0
        );
        assert_eq!(get(&conn, &c, "/api/runs?track=redis").status, 400);
        assert_eq!(get(&conn, &c, "/api/runs?limit=0").status, 400);
        assert_eq!(get(&conn, &c, "/api/runs?limit=abc").status, 400);
    }

    #[test]
    fn one_run_is_the_tester_report_shape() {
        let mut conn = db::open_memory().unwrap();
        let id = seed(&mut conn);
        let c = ctx(PathBuf::from("/d"));
        let v = body(&get(&conn, &c, &format!("/api/runs/{id}")));
        assert_eq!(v["id"], id);
        assert_eq!(v["target"], "bash");
        assert_eq!(v["stages"][0]["stage"], 1);
        assert_eq!(v["stages"][0]["tests"][0]["status"], "pass");
        assert_eq!(v["stages"][0]["tests"][0]["actual"][0][0], "stdout");
        assert_eq!(get(&conn, &c, "/api/runs/9999").status, 404);
        assert_eq!(get(&conn, &c, "/api/runs/abc").status, 400);
    }

    #[test]
    fn latest_run_per_track() {
        let mut conn = db::open_memory().unwrap();
        let id = seed(&mut conn);
        let c = ctx(PathBuf::from("/d"));
        let v = body(&get(&conn, &c, "/api/runs/latest?track=shell"));
        assert_eq!(v["id"], id);
        let r = get(&conn, &c, "/api/runs/latest?track=kafka");
        assert_eq!(r.status, 404);
        assert!(body(&r)["error"].as_str().unwrap().contains("byo test"));
    }

    #[test]
    fn post_stage_sets_state_and_note() {
        let conn = db::open_memory().unwrap();
        let c = ctx(PathBuf::from("/d"));
        let r = handle(
            &conn,
            &c,
            &Request::new(
                "POST",
                "/api/stages/kafka/7",
                r#"{"state":"done","note":"crc32c!"}"#,
            ),
        )
        .unwrap();
        assert_eq!(r.status, 200);
        let v = body(&r);
        assert_eq!(v["track"], "kafka");
        assert_eq!(v["stage"], 7);
        assert_eq!(v["state"], "done");
        assert_eq!(v["note"], "crc32c!");
        assert!(v["doneAt"].is_string());

        let v = body(
            &handle(
                &conn,
                &c,
                &Request::new("POST", "/api/stages/kafka/7", r#"{"state":"todo"}"#),
            )
            .unwrap(),
        );
        assert_eq!(v["state"], "todo");
        assert_eq!(v["note"], "crc32c!", "the note survives a state change");
        assert!(v["doneAt"].is_null());

        let v = body(
            &handle(
                &conn,
                &c,
                &Request::new("POST", "/api/stages/kafka/7", r#"{"note":""}"#),
            )
            .unwrap(),
        );
        assert!(v["note"].is_null(), "an empty note clears it");
    }

    #[test]
    fn post_stage_rejects_bad_input() {
        let conn = db::open_memory().unwrap();
        let c = ctx(PathBuf::from("/d"));
        let bad = |target: &str, body: &str| {
            handle(&conn, &c, &Request::new("POST", target, body))
                .unwrap()
                .status
        };
        assert_eq!(bad("/api/stages/redis/1", "{}"), 404);
        assert_eq!(bad("/api/stages/shell/x", "{}"), 400);
        assert_eq!(bad("/api/stages/shell/1", "not json"), 400);
        assert_eq!(bad("/api/stages/shell/1", "[1]"), 400);
        assert_eq!(bad("/api/stages/shell/1", r#"{"state":"maybe"}"#), 400);
        assert_eq!(bad("/api/stages/shell/1", r#"{"note":5}"#), 400);
        assert_eq!(
            bad("/api/stages/shell/1", ""),
            200,
            "an empty body is a no-op read"
        );
    }

    #[test]
    fn method_mismatch_is_405() {
        let conn = db::open_memory().unwrap();
        let c = ctx(PathBuf::from("/d"));
        assert_eq!(
            handle(&conn, &c, &Request::new("POST", "/api/progress", ""))
                .unwrap()
                .status,
            405
        );
        assert_eq!(
            handle(&conn, &c, &Request::new("GET", "/api/stages/shell/1", ""))
                .unwrap()
                .status,
            405
        );
        assert_eq!(
            handle(&conn, &c, &Request::new("GET", "/api/nope", ""))
                .unwrap()
                .status,
            404
        );
    }

    #[test]
    fn catalog_is_served_from_the_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("catalog.shell.json"),
            r#"{"track":"shell"}"#,
        )
        .unwrap();
        let conn = db::open_memory().unwrap();
        let c = ctx(dir.path().to_path_buf());
        let r = get(&conn, &c, "/api/catalog/shell");
        assert_eq!(r.status, 200);
        assert_eq!(body(&r)["track"], "shell");
        assert_eq!(get(&conn, &c, "/api/catalog/kafka").status, 404);
        assert_eq!(get(&conn, &c, "/api/catalog/redis").status, 404);
    }

    #[test]
    fn trailing_slashes_and_encoding_are_tolerated() {
        let mut conn = db::open_memory().unwrap();
        seed(&mut conn);
        let c = ctx(PathBuf::from("/d"));
        assert_eq!(get(&conn, &c, "/api/health/").status, 200);
        assert_eq!(get(&conn, &c, "/api/runs?track=shell&limit=5").status, 200);
        let q = Request::new("GET", "/api/runs?track=shell&x=a%20b", "");
        assert_eq!(q.query.get("x").map(String::as_str), Some("a b"));
    }
}
