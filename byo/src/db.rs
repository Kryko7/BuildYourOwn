//! The progress database: schema, migrations, ingestion and the queries the CLI and the
//! HTTP API read.
//!
//! One SQLite file (`$BYO_HOME/byo.db`) in WAL mode. Every write goes through a
//! transaction. `migrations` records what has been applied so future schema versions can
//! be added without guessing.

use crate::paths::Track;
use crate::report::Report;
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;

/// The schema version this build writes and expects.
pub const SCHEMA_VERSION: i64 = 1;

/// Ordered list of migrations; index 0 is version 1.
const MIGRATIONS: &[(i64, &str, &str)] = &[(1, "initial schema", SCHEMA_V1)];

const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS projects (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    track       TEXT NOT NULL,
    path        TEXT NOT NULL,
    command     TEXT NOT NULL,
    target_kind TEXT NOT NULL DEFAULT 'command',
    created_at  TEXT NOT NULL,
    UNIQUE (track, path)
);

CREATE TABLE IF NOT EXISTS runs (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER REFERENCES projects(id) ON DELETE SET NULL,
    track      TEXT NOT NULL,
    target     TEXT NOT NULL,
    started_at TEXT NOT NULL,
    elapsed_ms INTEGER NOT NULL,
    passed     INTEGER NOT NULL,
    failed     INTEGER NOT NULL,
    skipped    INTEGER NOT NULL,
    args       TEXT NOT NULL,
    json_path  TEXT
);
CREATE INDEX IF NOT EXISTS runs_track_started ON runs(track, started_at DESC);

CREATE TABLE IF NOT EXISTS run_tests (
    run_id        INTEGER NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    seq           INTEGER NOT NULL,
    stage         INTEGER NOT NULL,
    stage_name    TEXT NOT NULL DEFAULT '',
    stage_file    TEXT NOT NULL DEFAULT '',
    test_name     TEXT NOT NULL,
    status        TEXT NOT NULL,
    ext           INTEGER NOT NULL DEFAULT 0,
    duration_ms   INTEGER NOT NULL DEFAULT 0,
    failures_json TEXT NOT NULL DEFAULT '[]',
    actual_json   TEXT NOT NULL DEFAULT '[]',
    skip_reason   TEXT,
    failure_kind  TEXT,
    PRIMARY KEY (run_id, seq)
);
CREATE INDEX IF NOT EXISTS run_tests_stage ON run_tests(run_id, stage);

CREATE TABLE IF NOT EXISTS stage_progress (
    track       TEXT NOT NULL,
    stage       INTEGER NOT NULL,
    state       TEXT NOT NULL,
    done_at     TEXT,
    last_run_id INTEGER REFERENCES runs(id) ON DELETE SET NULL,
    note        TEXT,
    updated_at  TEXT NOT NULL,
    PRIMARY KEY (track, stage)
);
"#;

/// Open (creating if needed) the database and bring it up to date.
pub fn open(path: &Path) -> Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let conn = Connection::open(path)
        .with_context(|| format!("cannot open the database at {}", path.display()))?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .context("cannot enable WAL mode")?;
    conn.pragma_update(None, "foreign_keys", "ON").ok();
    conn.busy_timeout(std::time::Duration::from_secs(5)).ok();
    migrate(&conn)?;
    Ok(conn)
}

/// Open an in-memory database (used by the tests).
#[cfg(test)]
pub fn open_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    migrate(&conn)?;
    Ok(conn)
}

/// Apply any migrations this build knows about that the file does not have yet.
pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS migrations (
             version    INTEGER PRIMARY KEY,
             name       TEXT NOT NULL,
             applied_at TEXT NOT NULL
         );",
    )
    .context("cannot create the migrations table")?;
    let applied: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM migrations",
            [],
            |r| r.get(0),
        )
        .context("cannot read the migrations table")?;
    for (version, name, sql) in MIGRATIONS {
        if *version <= applied {
            continue;
        }
        conn.execute_batch(sql)
            .with_context(|| format!("migration {version} ({name}) failed"))?;
        conn.execute(
            "INSERT INTO migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
            params![version, name, now()],
        )
        .with_context(|| format!("cannot record migration {version}"))?;
    }
    set_meta(conn, "schema_version", &SCHEMA_VERSION.to_string())?;
    if get_meta(conn, "installed_at")?.is_none() {
        set_meta(conn, "installed_at", &now())?;
    }
    Ok(())
}

/// The schema version recorded in the file.
pub fn schema_version(conn: &Connection) -> Result<i64> {
    Ok(get_meta(conn, "schema_version")?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0))
}

/// Read one `meta` key.
pub fn get_meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
            r.get(0)
        })
        .optional()?)
}

/// Write one `meta` key.
pub fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// An RFC 3339 timestamp in UTC, second precision.
pub fn now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Register (or refresh) a project. Returns its id.
pub fn upsert_project(
    conn: &Connection,
    track: Track,
    path: &Path,
    command: &str,
    target_kind: &str,
) -> Result<i64> {
    let path = path.to_string_lossy().to_string();
    conn.execute(
        "INSERT INTO projects (track, path, command, target_kind, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(track, path) DO UPDATE SET command = excluded.command,
                                                target_kind = excluded.target_kind",
        params![track.as_str(), path, command, target_kind, now()],
    )
    .context("cannot register the project")?;
    Ok(conn.query_row(
        "SELECT id FROM projects WHERE track = ?1 AND path = ?2",
        params![track.as_str(), path],
        |r| r.get(0),
    )?)
}

/// A registered project, as reported by `/api/health`.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectRow {
    /// Row id.
    pub id: i64,
    /// `shell` or `kafka`.
    pub track: String,
    /// Absolute path of the project directory.
    pub path: String,
    /// The `--shell` / `--broker` value.
    pub command: String,
    /// `registered` or `command`.
    #[serde(rename = "targetKind")]
    pub target_kind: String,
    /// When the project was first registered.
    #[serde(rename = "createdAt")]
    pub created_at: String,
}

/// The most recently registered project for a track.
pub fn latest_project(conn: &Connection, track: Track) -> Result<Option<ProjectRow>> {
    Ok(conn
        .query_row(
            "SELECT id, track, path, command, target_kind, created_at FROM projects
             WHERE track = ?1 ORDER BY id DESC LIMIT 1",
            params![track.as_str()],
            |r| {
                Ok(ProjectRow {
                    id: r.get(0)?,
                    track: r.get(1)?,
                    path: r.get(2)?,
                    command: r.get(3)?,
                    target_kind: r.get(4)?,
                    created_at: r.get(5)?,
                })
            },
        )
        .optional()?)
}

/// A row of the `runs` table, as returned by `/api/runs`.
#[derive(Debug, Clone, Serialize)]
pub struct RunRow {
    /// Row id.
    pub id: i64,
    /// `shell` or `kafka`.
    pub track: String,
    /// The shell/broker that was tested.
    pub target: String,
    /// When the run started (RFC 3339, UTC).
    #[serde(rename = "startedAt")]
    pub started_at: String,
    /// Wall-clock duration in milliseconds.
    #[serde(rename = "elapsedMs")]
    pub elapsed_ms: i64,
    /// Tests passed.
    pub passed: i64,
    /// Tests failed.
    pub failed: i64,
    /// Tests skipped.
    pub skipped: i64,
    /// The tester arguments this run used.
    pub args: String,
}

/// Ingest a tester report as a new run, updating `stage_progress`. Returns the run id.
pub fn ingest(
    conn: &mut Connection,
    track: Track,
    project_id: Option<i64>,
    report: &Report,
    args: &str,
    json_path: Option<&str>,
    started_at: &str,
) -> Result<i64> {
    let tx = conn
        .transaction()
        .context("cannot start the ingest transaction")?;
    tx.execute(
        "INSERT INTO runs (project_id, track, target, started_at, elapsed_ms, passed, failed,
                           skipped, args, json_path)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            project_id,
            track.as_str(),
            report.target,
            started_at,
            report.elapsed_ms,
            report.passed,
            report.failed,
            report.skipped,
            args,
            json_path,
        ],
    )
    .context("cannot record the run")?;
    let run_id = tx.last_insert_rowid();

    let mut seq: i64 = 0;
    for st in &report.stages {
        for t in &st.tests {
            tx.execute(
                "INSERT INTO run_tests (run_id, seq, stage, stage_name, stage_file, test_name,
                                        status, ext, duration_ms, failures_json, actual_json,
                                        skip_reason, failure_kind)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    run_id,
                    seq,
                    st.stage,
                    st.name,
                    st.file,
                    t.name,
                    t.status,
                    t.ext as i64,
                    t.duration_ms,
                    serde_json::to_string(&t.failures)?,
                    serde_json::to_string(&t.actual)?,
                    t.skip_reason,
                    t.failure_kind,
                ],
            )
            .context("cannot record a test result")?;
            seq += 1;
        }

        // Derived progress: a stage whose latest run passed every test it ran is done.
        let ran = st.passed + st.failed;
        let state = if ran == 0 {
            None
        } else if st.failed == 0 {
            Some("done")
        } else if st.passed > 0 {
            Some("in_progress")
        } else {
            Some("failed")
        };
        let Some(state) = state else { continue };
        let done = state == "done";
        tx.execute(
            "INSERT INTO stage_progress (track, stage, state, done_at, last_run_id, note, updated_at)
             VALUES (?1, ?2, ?3, CASE WHEN ?4 THEN ?5 ELSE NULL END, ?6, NULL, ?5)
             ON CONFLICT(track, stage) DO UPDATE SET
                 state       = excluded.state,
                 done_at     = CASE WHEN ?4 THEN COALESCE(stage_progress.done_at, ?5) ELSE NULL END,
                 last_run_id = excluded.last_run_id,
                 updated_at  = ?5",
            params![track.as_str(), st.stage, state, done, started_at, run_id],
        )
        .context("cannot update stage progress")?;
    }
    tx.commit()
        .context("cannot commit the ingest transaction")?;
    Ok(run_id)
}

/// A row of `stage_progress`.
#[derive(Debug, Clone, Serialize)]
pub struct StageRow {
    /// Stage number.
    pub stage: u32,
    /// `todo`, `in_progress`, `failed` or `done`.
    pub state: String,
    /// When the stage first went green (RFC 3339), if it has.
    #[serde(rename = "doneAt")]
    pub done_at: Option<String>,
    /// The note attached with `byo note`.
    pub note: Option<String>,
    /// The run that last touched this stage.
    #[serde(rename = "lastRunId")]
    pub last_run_id: Option<i64>,
    /// When the row was last written.
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

/// Every recorded stage of a track, lowest first.
pub fn stages(conn: &Connection, track: Track) -> Result<Vec<StageRow>> {
    let mut stmt = conn.prepare(
        "SELECT stage, state, done_at, note, last_run_id, updated_at FROM stage_progress
         WHERE track = ?1 ORDER BY stage",
    )?;
    let rows = stmt
        .query_map(params![track.as_str()], |r| {
            Ok(StageRow {
                stage: r.get(0)?,
                state: r.get(1)?,
                done_at: r.get(2)?,
                note: r.get(3)?,
                last_run_id: r.get(4)?,
                updated_at: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// One stage row, if it exists.
pub fn stage(conn: &Connection, track: Track, stage: u32) -> Result<Option<StageRow>> {
    Ok(stages(conn, track)?.into_iter().find(|s| s.stage == stage))
}

/// Mark a stage done or not done. Returns the updated row.
pub fn set_state(conn: &Connection, track: Track, stage: u32, done: bool) -> Result<StageRow> {
    let ts = now();
    conn.execute(
        "INSERT INTO stage_progress (track, stage, state, done_at, last_run_id, note, updated_at)
         VALUES (?1, ?2, ?3, CASE WHEN ?4 THEN ?5 ELSE NULL END, NULL, NULL, ?5)
         ON CONFLICT(track, stage) DO UPDATE SET
             state      = excluded.state,
             done_at    = CASE WHEN ?4 THEN COALESCE(stage_progress.done_at, ?5) ELSE NULL END,
             updated_at = ?5",
        params![
            track.as_str(),
            stage,
            if done { "done" } else { "todo" },
            done,
            ts
        ],
    )
    .context("cannot update the stage")?;
    stage_or_default(conn, track, stage)
}

/// Attach a note to a stage. Returns the updated row.
pub fn set_note(
    conn: &Connection,
    track: Track,
    stage: u32,
    note: Option<&str>,
) -> Result<StageRow> {
    let ts = now();
    conn.execute(
        "INSERT INTO stage_progress (track, stage, state, done_at, last_run_id, note, updated_at)
         VALUES (?1, ?2, 'todo', NULL, NULL, ?3, ?4)
         ON CONFLICT(track, stage) DO UPDATE SET note = ?3, updated_at = ?4",
        params![track.as_str(), stage, note, ts],
    )
    .context("cannot write the note")?;
    stage_or_default(conn, track, stage)
}

fn stage_or_default(conn: &Connection, track: Track, n: u32) -> Result<StageRow> {
    Ok(stage(conn, track, n)?.unwrap_or(StageRow {
        stage: n,
        state: "todo".into(),
        done_at: None,
        note: None,
        last_run_id: None,
        updated_at: now(),
    }))
}

/// Recent runs, newest first.
pub fn runs(conn: &Connection, track: Option<Track>, limit: i64) -> Result<Vec<RunRow>> {
    let map = |r: &rusqlite::Row<'_>| {
        Ok(RunRow {
            id: r.get(0)?,
            track: r.get(1)?,
            target: r.get(2)?,
            started_at: r.get(3)?,
            elapsed_ms: r.get(4)?,
            passed: r.get(5)?,
            failed: r.get(6)?,
            skipped: r.get(7)?,
            args: r.get(8)?,
        })
    };
    const COLS: &str = "id, track, target, started_at, elapsed_ms, passed, failed, skipped, args";
    let rows = match track {
        Some(t) => {
            let sql = format!("SELECT {COLS} FROM runs WHERE track = ?1 ORDER BY id DESC LIMIT ?2");
            let mut stmt = conn.prepare(&sql)?;
            let it = stmt.query_map(params![t.as_str(), limit], map)?;
            it.collect::<rusqlite::Result<Vec<_>>>()?
        }
        None => {
            let sql = format!("SELECT {COLS} FROM runs ORDER BY id DESC LIMIT ?1");
            let mut stmt = conn.prepare(&sql)?;
            let it = stmt.query_map(params![limit], map)?;
            it.collect::<rusqlite::Result<Vec<_>>>()?
        }
    };
    Ok(rows)
}

/// The id of the newest run of a track.
pub fn latest_run_id(conn: &Connection, track: Track) -> Result<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM runs WHERE track = ?1 ORDER BY id DESC LIMIT 1",
            params![track.as_str()],
            |r| r.get(0),
        )
        .optional()?)
}

/// Rebuild a run into the testers' `JsonReport` shape, straight from `run_tests`.
pub fn run_report(conn: &Connection, run_id: i64) -> Result<Option<Report>> {
    let row: Option<(String, String, i64, i64, i64, i64)> = conn
        .query_row(
            "SELECT track, target, elapsed_ms, passed, failed, skipped FROM runs WHERE id = ?1",
            params![run_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .optional()?;
    let Some((_track, target, elapsed_ms, passed, failed, skipped)) = row else {
        return Ok(None);
    };
    let mut stmt = conn.prepare(
        "SELECT stage, stage_name, stage_file, test_name, status, ext, duration_ms,
                failures_json, actual_json, skip_reason, failure_kind
         FROM run_tests WHERE run_id = ?1 ORDER BY seq",
    )?;
    let mut stages: Vec<crate::report::Stage> = Vec::new();
    let rows = stmt.query_map(params![run_id], |r| {
        Ok((
            r.get::<_, u32>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            crate::report::Test {
                name: r.get(3)?,
                status: r.get(4)?,
                ext: r.get::<_, i64>(5)? != 0,
                duration_ms: r.get(6)?,
                failures: serde_json::from_str(&r.get::<_, String>(7)?).unwrap_or_default(),
                actual: serde_json::from_str(&r.get::<_, String>(8)?).unwrap_or_default(),
                skip_reason: r.get(9)?,
                failure_kind: r.get(10)?,
            },
        ))
    })?;
    for row in rows {
        let (n, name, file, test) = row?;
        if stages.last().map(|s| s.stage) != Some(n) {
            stages.push(crate::report::Stage {
                stage: n,
                name,
                file,
                passed: 0,
                failed: 0,
                skipped: 0,
                tests: Vec::new(),
            });
        }
        let st = stages.last_mut().expect("just pushed");
        match test.status.as_str() {
            "pass" => st.passed += 1,
            "fail" => st.failed += 1,
            _ => st.skipped += 1,
        }
        st.tests.push(test);
    }
    Ok(Some(Report {
        target,
        validate: false,
        stages,
        passed,
        failed,
        skipped,
        elapsed_ms,
    }))
}

/// XP and streak, as shown on the home page and by `byo status`.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Score {
    /// 100 per done stage, 20 per stage in progress or red.
    pub xp: i64,
    /// Level, one per 500 XP, starting at 1.
    pub level: i64,
    /// Consecutive days (ending today or yesterday) on which a stage went green.
    pub streak: i64,
}

/// Compute XP, level and the completion streak across both tracks.
pub fn score(conn: &Connection) -> Result<Score> {
    let mut xp = 0i64;
    for t in Track::ALL {
        for s in stages(conn, t)? {
            xp += match s.state.as_str() {
                "done" => 100,
                "in_progress" | "failed" => 20,
                _ => 0,
            };
        }
    }
    let mut stmt = conn.prepare(
        "SELECT DISTINCT substr(done_at, 1, 10) FROM stage_progress
         WHERE done_at IS NOT NULL ORDER BY 1 DESC",
    )?;
    let days: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Score {
        xp,
        level: xp / 500 + 1,
        streak: streak_from_days(&days, &today()),
    })
}

/// Today's date in UTC, `YYYY-MM-DD`.
pub fn today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

/// Count consecutive days back from today (a streak may end yesterday and still count).
pub fn streak_from_days(days_desc: &[String], today: &str) -> i64 {
    use chrono::NaiveDate;
    let parse = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok();
    let Some(today) = parse(today) else { return 0 };
    let mut dates: Vec<NaiveDate> = days_desc.iter().filter_map(|d| parse(d)).collect();
    dates.sort_unstable();
    dates.dedup();
    dates.reverse();
    let Some(&first) = dates.first() else {
        return 0;
    };
    let gap = (today - first).num_days();
    if !(0..=1).contains(&gap) {
        return 0;
    }
    let mut streak = 1;
    let mut prev = first;
    for &d in dates.iter().skip(1) {
        if (prev - d).num_days() == 1 {
            streak += 1;
            prev = d;
        } else {
            break;
        }
    }
    streak
}

/// The whole database as one JSON document (`byo db dump`).
pub fn dump(conn: &Connection) -> Result<serde_json::Value> {
    let mut meta = serde_json::Map::new();
    let mut stmt = conn.prepare("SELECT key, value FROM meta ORDER BY key")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (k, v) = row?;
        meta.insert(k, serde_json::Value::String(v));
    }
    let mut projects = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT id, track, path, command, target_kind, created_at FROM projects ORDER BY id",
    )?;
    for row in stmt.query_map([], |r| {
        Ok(ProjectRow {
            id: r.get(0)?,
            track: r.get(1)?,
            path: r.get(2)?,
            command: r.get(3)?,
            target_kind: r.get(4)?,
            created_at: r.get(5)?,
        })
    })? {
        projects.push(row?);
    }
    let mut progress = serde_json::Map::new();
    for t in Track::ALL {
        progress.insert(t.as_str().into(), serde_json::to_value(stages(conn, t)?)?);
    }
    let runs = runs(conn, None, 10_000)?;
    Ok(serde_json::json!({
        "schemaVersion": schema_version(conn)?,
        "meta": meta,
        "projects": projects,
        "progress": progress,
        "runs": runs,
        "score": score(conn)?,
    }))
}

/// Drop every row (keeping the schema and `installed_at`).
pub fn reset(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch(
        "DELETE FROM run_tests;
         DELETE FROM runs;
         DELETE FROM stage_progress;
         DELETE FROM projects;
         DELETE FROM sqlite_sequence WHERE name IN ('runs', 'projects');",
    )
    .context("cannot clear the database")?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report;

    fn sample(target: &str, stages: Vec<report::Stage>) -> Report {
        let passed = stages.iter().map(|s| s.passed).sum();
        let failed = stages.iter().map(|s| s.failed).sum();
        let skipped = stages.iter().map(|s| s.skipped).sum();
        Report {
            target: target.into(),
            validate: false,
            stages,
            passed,
            failed,
            skipped,
            elapsed_ms: 7,
        }
    }

    fn st(n: u32, tests: &[(&str, &str)]) -> report::Stage {
        let tests: Vec<report::Test> = tests
            .iter()
            .map(|(name, status)| report::Test {
                name: (*name).into(),
                status: (*status).into(),
                ext: false,
                duration_ms: 1,
                failures: if *status == "fail" {
                    vec!["boom".into()]
                } else {
                    vec![]
                },
                skip_reason: None,
                actual: vec![("stdout".into(), "x".into())],
                failure_kind: None,
            })
            .collect();
        let c = |s: &str| tests.iter().filter(|t| t.status == s).count() as i64;
        report::Stage {
            stage: n,
            name: format!("stage {n}"),
            file: format!("{n:02}_x.yaml"),
            passed: c("pass"),
            failed: c("fail"),
            skipped: c("skip"),
            tests,
        }
    }

    #[test]
    fn migrations_are_idempotent_and_record_versions() {
        let conn = open_memory().unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), SCHEMA_VERSION);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, MIGRATIONS.len() as i64);
        assert!(get_meta(&conn, "installed_at").unwrap().is_some());
    }

    #[test]
    fn ingest_writes_rows_and_derives_progress() {
        let mut conn = open_memory().unwrap();
        let pid = upsert_project(
            &conn,
            Track::Shell,
            Path::new("/proj"),
            "bash",
            "registered",
        )
        .unwrap();
        let rep = sample(
            "bash",
            vec![
                st(1, &[("a", "pass"), ("b", "pass")]),
                st(2, &[("c", "pass"), ("d", "fail")]),
                st(3, &[("e", "fail")]),
                st(4, &[("f", "skip")]),
            ],
        );
        let id = ingest(
            &mut conn,
            Track::Shell,
            Some(pid),
            &rep,
            "--until 4",
            None,
            "2026-09-13T10:00:00Z",
        )
        .unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM run_tests WHERE run_id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 6);
        let rows = stages(&conn, Track::Shell).unwrap();
        let state = |s: u32| rows.iter().find(|r| r.stage == s).map(|r| r.state.clone());
        assert_eq!(state(1).as_deref(), Some("done"));
        assert_eq!(state(2).as_deref(), Some("in_progress"));
        assert_eq!(state(3).as_deref(), Some("failed"));
        assert_eq!(
            state(4),
            None,
            "a stage with only skips leaves progress alone"
        );
        assert_eq!(rows[0].done_at.as_deref(), Some("2026-09-13T10:00:00Z"));
        assert_eq!(rows[0].last_run_id, Some(id));
    }

    #[test]
    fn a_later_red_run_clears_done() {
        let mut conn = open_memory().unwrap();
        let green = sample("bash", vec![st(1, &[("a", "pass")])]);
        ingest(
            &mut conn,
            Track::Shell,
            None,
            &green,
            "",
            None,
            "2026-09-12T10:00:00Z",
        )
        .unwrap();
        let red = sample("bash", vec![st(1, &[("a", "fail")])]);
        ingest(
            &mut conn,
            Track::Shell,
            None,
            &red,
            "",
            None,
            "2026-09-13T10:00:00Z",
        )
        .unwrap();
        let row = stage(&conn, Track::Shell, 1).unwrap().unwrap();
        assert_eq!(row.state, "failed");
        assert!(row.done_at.is_none());
    }

    #[test]
    fn notes_survive_runs_and_state_changes() {
        let mut conn = open_memory().unwrap();
        set_note(&conn, Track::Kafka, 5, Some("watch the CRC")).unwrap();
        let rep = sample("broken", vec![st(5, &[("a", "pass")])]);
        ingest(
            &mut conn,
            Track::Kafka,
            None,
            &rep,
            "",
            None,
            "2026-09-13T10:00:00Z",
        )
        .unwrap();
        let row = stage(&conn, Track::Kafka, 5).unwrap().unwrap();
        assert_eq!(row.note.as_deref(), Some("watch the CRC"));
        assert_eq!(row.state, "done");
        let row = set_state(&conn, Track::Kafka, 5, false).unwrap();
        assert_eq!(row.state, "todo");
        assert_eq!(row.note.as_deref(), Some("watch the CRC"));
    }

    #[test]
    fn rebuilds_the_report_shape() {
        let mut conn = open_memory().unwrap();
        let rep = sample(
            "bash",
            vec![
                st(1, &[("a", "pass"), ("b", "fail")]),
                st(2, &[("c", "skip")]),
            ],
        );
        let id = ingest(
            &mut conn,
            Track::Shell,
            None,
            &rep,
            "--all",
            None,
            "2026-09-13T10:00:00Z",
        )
        .unwrap();
        let back = run_report(&conn, id).unwrap().unwrap();
        assert_eq!(back.target, "bash");
        assert_eq!(back.stages.len(), 2);
        assert_eq!(back.stages[0].passed, 1);
        assert_eq!(back.stages[0].failed, 1);
        assert_eq!(back.stages[0].tests[1].failures, vec!["boom".to_string()]);
        assert_eq!(back.stages[1].skipped, 1);
        assert_eq!(back.passed, rep.passed);
        assert!(run_report(&conn, 999).unwrap().is_none());
    }

    #[test]
    fn runs_are_listed_newest_first_and_filtered_by_track() {
        let mut conn = open_memory().unwrap();
        ingest(
            &mut conn,
            Track::Shell,
            None,
            &sample("bash", vec![]),
            "a",
            None,
            "2026-09-13T10:00:00Z",
        )
        .unwrap();
        ingest(
            &mut conn,
            Track::Kafka,
            None,
            &sample("broken", vec![]),
            "b",
            None,
            "2026-09-13T11:00:00Z",
        )
        .unwrap();
        let all = runs(&conn, None, 10).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].track, "kafka");
        assert_eq!(runs(&conn, Some(Track::Shell), 10).unwrap().len(), 1);
        assert_eq!(latest_run_id(&conn, Track::Kafka).unwrap(), Some(all[0].id));
    }

    #[test]
    fn streaks_count_back_from_today() {
        let d = |s: &str| s.to_string();
        assert_eq!(streak_from_days(&[], "2026-09-13"), 0);
        assert_eq!(streak_from_days(&[d("2026-09-13")], "2026-09-13"), 1);
        assert_eq!(streak_from_days(&[d("2026-09-12")], "2026-09-13"), 1);
        assert_eq!(streak_from_days(&[d("2026-09-11")], "2026-09-13"), 0);
        assert_eq!(
            streak_from_days(
                &[d("2026-09-13"), d("2026-09-12"), d("2026-09-10")],
                "2026-09-13"
            ),
            2
        );
    }

    #[test]
    fn score_counts_stages() {
        let mut conn = open_memory().unwrap();
        let rep = sample(
            "bash",
            vec![st(1, &[("a", "pass")]), st(2, &[("b", "fail")])],
        );
        ingest(&mut conn, Track::Shell, None, &rep, "", None, &now()).unwrap();
        let s = score(&conn).unwrap();
        assert_eq!(s.xp, 120);
        assert_eq!(s.level, 1);
        assert_eq!(s.streak, 1);
    }

    #[test]
    fn reset_empties_everything() {
        let mut conn = open_memory().unwrap();
        upsert_project(&conn, Track::Shell, Path::new("/p"), "bash", "registered").unwrap();
        ingest(
            &mut conn,
            Track::Shell,
            None,
            &sample("bash", vec![st(1, &[("a", "pass")])]),
            "",
            None,
            &now(),
        )
        .unwrap();
        reset(&mut conn).unwrap();
        assert!(runs(&conn, None, 10).unwrap().is_empty());
        assert!(stages(&conn, Track::Shell).unwrap().is_empty());
        assert!(latest_project(&conn, Track::Shell).unwrap().is_none());
        assert_eq!(schema_version(&conn).unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn dump_has_every_section() {
        let conn = open_memory().unwrap();
        let v = dump(&conn).unwrap();
        for k in [
            "schemaVersion",
            "meta",
            "projects",
            "progress",
            "runs",
            "score",
        ] {
            assert!(v.get(k).is_some(), "missing {k}");
        }
    }
}
