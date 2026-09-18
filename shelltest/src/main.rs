mod config;
mod fixtures;
mod loader;
mod matchers;
mod normalize;
mod report;
mod runner;

use anyhow::{bail, Context, Result};
use clap::Parser;
use loader::Stage;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// stage-by-stage black-box tester for POSIX shells.
#[derive(Parser, Debug)]
#[command(name = "shelltest", version, about)]
struct Cli {
    /// Shell to test: a name from shells.yaml (bash, zsh, ...) or a path to a binary
    #[arg(long)]
    shell: Option<String>,
    /// Run a single stage
    #[arg(long, value_name = "N")]
    stage: Option<u32>,
    /// Run stages 1..=N (or --from..=N)
    #[arg(long, value_name = "N")]
    until: Option<u32>,
    /// Run stages N.. (or N..=--until)
    #[arg(long, value_name = "N")]
    from: Option<u32>,
    /// Run every stage
    #[arg(long)]
    all: bool,
    /// Only run tests whose name contains this substring
    #[arg(long, value_name = "SUBSTRING")]
    only: Option<String>,
    /// Only run tests carrying this tag (e.g. ext)
    #[arg(long)]
    tag: Option<String>,
    /// Hide tests tagged `ext` (beyond the core track)
    #[arg(long)]
    skip_ext: bool,
    /// Show input and captured output for passing tests too
    #[arg(long, short)]
    verbose: bool,
    /// Keep each test's sandbox directory
    #[arg(long)]
    keep_tmp: bool,
    /// Default per-test timeout (tests may override with timeout_ms)
    #[arg(long, value_name = "N", default_value_t = 5000)]
    timeout_ms: u64,
    /// Run the whole suite against a reference shell and report failures as suite bugs
    #[arg(long)]
    validate: bool,
    /// List stages, test counts and PLAN.md tickbox state
    #[arg(long)]
    list: bool,
    /// Write a machine-readable report
    #[arg(long, value_name = "FILE")]
    json: Option<PathBuf>,
    #[arg(long)]
    no_color: bool,
    #[arg(long, value_name = "DIR")]
    tests_dir: Option<PathBuf>,
    #[arg(long, value_name = "FILE")]
    shells_file: Option<PathBuf>,
}

fn main() {
    let code = match run() {
        Ok(ok) => i32::from(!ok),
        Err(e) => {
            eprintln!("error: {e:#}");
            2
        }
    };
    std::process::exit(code);
}

/// Look for a repo file next to the cwd, then next to the binary (target/release/..).
fn locate(explicit: Option<PathBuf>, relative: &str) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p);
    }
    let mut roots = vec![std::env::current_dir()?];
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(Path::to_path_buf);
        for _ in 0..3 {
            if let Some(d) = dir {
                roots.push(d.clone());
                dir = d.parent().map(Path::to_path_buf);
            }
        }
    }
    roots
        .iter()
        .map(|r| r.join(relative))
        .find(|p| p.exists())
        .ok_or_else(|| anyhow::anyhow!("cannot find {relative}; pass --tests-dir/--shells-file"))
}

fn plan_ticks(tests_dir: &Path) -> std::collections::HashMap<u32, bool> {
    let candidates = [tests_dir.join("../../PLAN.md"), PathBuf::from("PLAN.md")];
    let re = regex::Regex::new(r"^- \[([ xX])\] \*\*Stage (\d+)").unwrap();
    let mut ticks = std::collections::HashMap::new();
    for c in candidates {
        if let Ok(text) = std::fs::read_to_string(&c) {
            for line in text.lines() {
                if let Some(m) = re.captures(line) {
                    ticks.insert(m[2].parse().unwrap_or(0), &m[1] != " ");
                }
            }
            break;
        }
    }
    ticks
}

fn list_stages(stages: &[Stage], tests_dir: &Path) {
    let ticks = plan_ticks(tests_dir);
    for st in stages {
        let ext = st.tests.iter().filter(|t| t.is_ext()).count();
        let tick = match ticks.get(&st.stage) {
            Some(true) => "[x]",
            Some(false) => "[ ]",
            None => "[?]",
        };
        let ext_note = if ext > 0 { format!(", {ext} ext") } else { String::new() };
        println!("{tick} Stage {:02}  {:<40} {:<24} {} tests{ext_note}", st.stage, st.name, st.file_name(), st.tests.len());
    }
    println!("\n{} stages, {} tests", stages.len(), stages.iter().map(|s| s.tests.len()).sum::<usize>());
}

fn select<'a>(cli: &Cli, stages: &'a [Stage]) -> Result<Vec<&'a Stage>> {
    let (lo, hi) = if cli.all || cli.validate {
        (0, u32::MAX)
    } else if let Some(n) = cli.stage {
        (n, n)
    } else if cli.from.is_some() || cli.until.is_some() {
        (cli.from.unwrap_or(0), cli.until.unwrap_or(u32::MAX))
    } else {
        bail!("select stages with --stage N, --until N, --from N, --all or --validate");
    };
    let picked: Vec<&Stage> = stages.iter().filter(|s| (lo..=hi).contains(&s.stage)).collect();
    if picked.is_empty() {
        bail!("no stages match the selection");
    }
    Ok(picked)
}

fn run() -> Result<bool> {
    let cli = Cli::parse();
    report::set_color(!cli.no_color && std::env::var_os("NO_COLOR").is_none());
    let tests_dir = locate(cli.tests_dir.clone(), "tests/stages")?;
    let stages = loader::load_dir(&tests_dir)?;
    if cli.list {
        list_stages(&stages, &tests_dir);
        return Ok(true);
    }
    let shells_file = locate(cli.shells_file.clone(), "shells.yaml")?;
    let shells = config::load_shells(&shells_file)?;
    let spec = cli.shell.as_deref().context("--shell <name|path> is required")?;
    let shell = config::resolve_shell(spec, &shells)?;
    if cli.validate && !shells.contains_key(spec) {
        bail!("--validate needs a registered reference shell (one of: {})", shells.keys().cloned().collect::<Vec<_>>().join(", "));
    }
    let picked = select(&cli, &stages)?;
    let wanted = |t: &loader::TestCase| {
        cli.only.as_ref().is_none_or(|s| t.name.contains(s.as_str()))
            && cli.tag.as_ref().is_none_or(|tag| t.tags.contains(tag))
            && !(cli.skip_ext && t.is_ext())
    };

    let reporter = report::Reporter { validate: cli.validate, verbose: cli.verbose };
    let opts = runner::RunOptions { keep_tmp: cli.keep_tmp, default_timeout_ms: cli.timeout_ms };
    let start = Instant::now();
    let mut all: Vec<(&Stage, Vec<runner::TestResult>)> = Vec::new();
    println!("shelltest: testing '{}' with {}", shell.name, shells_file.display());
    for st in picked {
        let tests: Vec<&loader::TestCase> = st.tests.iter().filter(|t| wanted(t)).collect();
        if tests.is_empty() {
            continue;
        }
        reporter.stage_header(st);
        let mut results = Vec::new();
        for tc in tests {
            let r = runner::run_test(tc, &shell, &opts);
            reporter.test_result(&r);
            results.push(r);
        }
        reporter.stage_summary(st, &results);
        all.push((st, results));
    }
    if all.is_empty() {
        bail!("no tests selected (check --only / --tag / --skip-ext)");
    }
    let ok = reporter.total(&all, start.elapsed(), &shell.name);
    if let Some(p) = &cli.json {
        report::write_json(p, &shell.name, cli.validate, &all, start.elapsed())?;
        println!("JSON report written to {}", p.display());
    }
    Ok(ok)
}
