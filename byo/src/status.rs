//! `byo status` — the terminal progress summary.

use crate::catalog;
use crate::db;
use crate::paths::{self, Paths};
use crate::track::Track;
use anyhow::Result;
use rusqlite::Connection;
use std::collections::BTreeMap;

/// ANSI colouring, off when `NO_COLOR` is set or stdout is not a terminal.
fn color_enabled() -> bool {
    std::env::var_os("NO_COLOR").is_none() && std::env::var_os("BYO_NO_COLOR").is_none()
}

fn paint(s: &str, code: &str) -> String {
    if code != "0" && color_enabled() {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// A progress bar of `width` cells.
pub fn bar(done: usize, total: usize, width: usize) -> String {
    if total == 0 {
        return "░".repeat(width);
    }
    let filled = (done * width).div_ceil(total).min(width);
    let filled = if done == 0 { 0 } else { filled.max(1) };
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

/// Print the whole status view.
pub fn print(paths: &Paths) -> Result<()> {
    let conn = db::open(&paths.db())?;
    let score = db::score(&conn)?;
    println!(
        "\n{}  —  level {}, {} XP, {}",
        paint(&crate::branding::title(), "1;35"),
        score.level,
        score.xp,
        match score.streak {
            0 => "no streak yet".to_string(),
            1 => "1-day streak 🌱".to_string(),
            n => format!("{n}-day streak 🌸"),
        }
    );
    for track in Track::all() {
        println!();
        print_track(&conn, paths, track)?;
    }
    println!();
    Ok(())
}

fn print_track(conn: &Connection, paths: &Paths, track: Track) -> Result<()> {
    let def = track.def();
    let cat = catalog::load(&paths.catalog(track)).filter(|c| {
        let matches = c.track.is_empty() || c.track == def.id;
        if !matches {
            eprintln!(
                "byo: {} is a '{}' catalog, not '{track}' — ignoring it",
                paths.catalog(track).display(),
                c.track
            );
        }
        matches
    });
    let rows = db::stages(conn, track)?;
    let states: BTreeMap<u32, &db::StageRow> = rows.iter().map(|r| (r.stage, r)).collect();
    let project = db::latest_project(conn, track)?;
    let installed = paths::tester_installed(track);

    // A track with no catalog has nothing to draw bars from. Say which kind of nothing it
    // is — "not installed" is `./install.sh`'s problem, "not started" is the learner's.
    if cat.is_none() && states.is_empty() && project.is_none() {
        let head = paint(&format!("{:<6}", def.id), def.ansi);
        let tail = if installed {
            paint(
                &format!(
                    "not started — `byo init {track} --command {}`",
                    def.default_command
                ),
                "2",
            )
        } else {
            paint(
                &format!("not installed — {}/ has not been built yet", def.dir),
                "2",
            )
        };
        println!("{head}  {}  ·  {tail}", paint(def.blurb, "2"));
        return Ok(());
    }

    let known: Vec<u32> = match &cat {
        Some(c) if !c.stages.is_empty() => c.numbers(),
        _ => states.keys().copied().collect(),
    };
    let done = known
        .iter()
        .filter(|n| states.get(n).is_some_and(|r| r.state == "done"))
        .count();

    let head = match &project {
        Some(p) => format!("{} ({})", p.command, p.target_kind),
        None => paint("no project yet", "2"),
    };
    println!(
        "{}  {head}  ·  {}",
        paint(&format!("{:<6}", def.id), def.ansi),
        paint(
            &format!("{done}/{} stages done", known.len()),
            if done == known.len() && done > 0 {
                "32"
            } else {
                "0"
            }
        )
    );
    if let Some(p) = &project {
        println!("        {}", paint(&p.path, "2"));
    } else {
        println!(
            "        {}",
            paint(
                &format!(
                    "run `byo init {track} --command {}` in your project",
                    def.default_command
                ),
                "2"
            )
        );
    }
    if !installed {
        println!(
            "        {}",
            paint(
                &format!("{} is not installed — run ./install.sh", def.tester),
                "33"
            )
        );
    }

    if let Some(c) = &cat {
        for sec in &c.sections {
            let total = sec.stages.len();
            let d = sec
                .stages
                .iter()
                .filter(|n| states.get(n).is_some_and(|r| r.state == "done"))
                .count();
            let red = sec
                .stages
                .iter()
                .filter(|n| {
                    states
                        .get(n)
                        .is_some_and(|r| r.state == "failed" || r.state == "in_progress")
                })
                .count();
            let tint = if d == total {
                "32"
            } else if red > 0 {
                "33"
            } else {
                "0"
            };
            println!(
                "  {:<2} {:<26} {}  {}",
                sec.id,
                truncate(&sec.title, 26),
                paint(&bar(d, total, 20), tint),
                paint(&format!("{d}/{total}"), tint)
            );
        }
    } else if !known.is_empty() {
        println!("  {}  {done}/{}", bar(done, known.len(), 20), known.len());
    }

    if let Some(c) = &cat {
        let ext: Vec<u32> = c
            .stages
            .iter()
            .filter(|s| s.ext)
            .map(|s| s.number)
            .collect();
        if !ext.is_empty() {
            let d = ext
                .iter()
                .filter(|n| states.get(n).is_some_and(|r| r.state == "done"))
                .count();
            println!(
                "  {}",
                paint(
                    &format!("{d}/{} stages beyond the core track", ext.len()),
                    "2"
                )
            );
        }
    }

    // Next stage: the lowest known stage that is not done.
    if let Some(next) = known
        .iter()
        .find(|n| !states.get(n).is_some_and(|r| r.state == "done"))
    {
        let name = cat.as_ref().and_then(|c| c.name_of(*next)).unwrap_or("");
        let state = states.get(next).map(|r| r.state.as_str()).unwrap_or("todo");
        let mark = match state {
            "failed" | "in_progress" => paint("red", "31"),
            _ => paint("todo", "2"),
        };
        println!("  next: Stage {next:02} {name}  [{mark}]   run: byo test --stage {next}");
    } else if !known.is_empty() {
        println!("  {}", paint("every stage is green — 🎉", "32"));
    }

    match db::runs(conn, Some(track), 1)?.first() {
        Some(r) => println!(
            "  last run: #{} {} passed, {} failed, {} skipped  ({}, {:.1}s{})",
            r.id,
            paint(&r.passed.to_string(), "32"),
            paint(&r.failed.to_string(), if r.failed > 0 { "31" } else { "2" }),
            r.skipped,
            r.started_at,
            r.elapsed_ms as f64 / 1000.0,
            if r.args.is_empty() {
                String::new()
            } else {
                format!(", `{}`", r.args)
            }
        ),
        None => println!("  {}", paint("no runs yet — `byo test --stage 1`", "2")),
    }

    let notes: Vec<&db::StageRow> = rows.iter().filter(|r| r.note.is_some()).collect();
    if !notes.is_empty() {
        println!("  notes:");
        for n in notes {
            println!(
                "    Stage {:02}  {}",
                n.stage,
                n.note.as_deref().unwrap_or("")
            );
        }
    }
    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_fill_proportionally() {
        assert_eq!(bar(0, 10, 10), "░░░░░░░░░░");
        assert_eq!(bar(10, 10, 10), "██████████");
        assert_eq!(bar(5, 10, 10), "█████░░░░░");
        assert_eq!(
            bar(1, 100, 10),
            "█░░░░░░░░░",
            "any progress shows at least one cell"
        );
        assert_eq!(bar(3, 0, 4), "░░░░");
    }

    #[test]
    fn truncation_keeps_width() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(
            truncate("a very long section title", 10).chars().count(),
            10
        );
    }

    #[test]
    fn printing_works_on_an_empty_database() {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths {
            home: d.path().to_path_buf(),
        };
        print(&paths).unwrap();
    }

    #[test]
    fn a_track_with_a_catalog_is_drawn_and_the_others_are_not() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("catalog.link.json"),
            r#"{"track":"link","sections":[{"id":"A","title":"Objects","stages":[1,2]}],
                "stages":[{"number":1,"name":"Read an object","ext":false},
                          {"number":2,"name":"Emit a header","ext":true}]}"#,
        )
        .unwrap();
        let paths = Paths {
            home: d.path().to_path_buf(),
        };
        let conn = db::open(&paths.db()).unwrap();
        db::set_state(&conn, Track::LINK, 1, true).unwrap();
        // Every track is visited, the catalogued one in full; nothing panics on the rest.
        print(&paths).unwrap();
        print_track(&conn, &paths, Track::LINK).unwrap();
        print_track(&conn, &paths, Track::WASM).unwrap();
    }
}
