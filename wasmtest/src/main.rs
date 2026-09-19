//! `wasmtest` — a black-box conformance tester for WebAssembly runtimes.

use anyhow::{bail, Context, Result};
use clap::Parser;
use std::path::{Path, PathBuf};
use std::time::Instant;
use wasmtest::stages::Stage;
use wasmtest::{catalog, config, report, runner, stages};

/// stage-by-stage black-box tester for WebAssembly runtimes.
#[derive(Parser, Debug)]
#[command(name = "wasmtest", version, about)]
struct Cli {
    /// Runtime to test: a name from runtimes.yaml (wasmtime, my_runtime, ...) or a path
    #[arg(long)]
    runtime: Option<String>,
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
    /// Only run tests carrying this tag (e.g. ext, wasi, slow)
    #[arg(long)]
    tag: Option<String>,
    /// Hide tests tagged `ext` (stages beyond the core track)
    #[arg(long)]
    skip_ext: bool,
    /// Say what the harness is doing between invocations
    #[arg(long, short)]
    verbose: bool,
    /// Keep each test's temporary directory (the modules it wrote)
    #[arg(long)]
    keep_tmp: bool,
    /// Per-test timeout in milliseconds
    #[arg(long, value_name = "N", default_value_t = 10_000)]
    timeout_ms: u64,
    /// Run everything against the reference runtime and word failures as suite bugs
    #[arg(long)]
    validate: bool,
    /// List stages, test counts and PLAN.md tickbox state
    #[arg(long)]
    list: bool,
    /// Write a machine-readable report (with --list: the stage catalog; `-` means stdout)
    #[arg(long, value_name = "FILE", num_args = 0..=1, default_missing_value = "-")]
    json: Option<PathBuf>,
    /// Never colour the output
    #[arg(long)]
    no_color: bool,
    /// Path to runtimes.yaml
    #[arg(long, value_name = "FILE")]
    runtimes_file: Option<PathBuf>,
    /// Seed for every random choice, so a run can be reproduced
    #[arg(long, default_value_t = 0x5eed)]
    seed: u64,
}

fn main() {
    let code = match run() {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(e) => {
            eprintln!("error: {e:#}");
            2
        }
    };
    std::process::exit(code);
}

/// Look for a repo file next to the cwd, then next to the binary (`target/release/..`).
fn locate(explicit: Option<PathBuf>, relative: &str) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p);
    }
    let mut roots = vec![std::env::current_dir()?];
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(Path::to_path_buf);
        for _ in 0..4 {
            if let Some(d) = dir {
                roots.push(d.clone());
                dir = d.parent().map(Path::to_path_buf);
            }
        }
    }
    // In a cargo workspace the binary lives in the *workspace* target directory, so walking
    // up from it lands at the repo root rather than at this crate. Try the crate's own
    // subdirectory at each level too, which makes the command work from the repo root, from
    // inside the crate, and from wherever `install.sh` put it.
    let crate_name = env!("CARGO_PKG_NAME");
    roots
        .iter()
        .flat_map(|r| [r.join(relative), r.join(crate_name).join(relative)])
        .find(|p| p.exists())
        .ok_or_else(|| anyhow::anyhow!("cannot find {relative}; pass --runtimes-file"))
}

/// The stage range a command line asks for.
///
/// `--validate` only changes the runtime and the wording of a failure, never the selection:
/// `--validate --stage 5` runs stage 5. It does imply `--all` when nothing else was asked
/// for, so `--validate` on its own still means "prove the whole suite".
fn range(cli: &Cli) -> Result<(u32, u32)> {
    if let Some(n) = cli.stage {
        Ok((n, n))
    } else if cli.from.is_some() || cli.until.is_some() {
        Ok((cli.from.unwrap_or(0), cli.until.unwrap_or(u32::MAX)))
    } else if cli.all || cli.validate {
        Ok((0, u32::MAX))
    } else {
        bail!("select stages with --stage N, --until N, --from N, --all or --validate")
    }
}

fn select<'a>(cli: &Cli, all: &'a [Stage]) -> Result<Vec<&'a Stage>> {
    let (lo, hi) = range(cli)?;
    let picked: Vec<&Stage> = all
        .iter()
        .filter(|s| (lo..=hi).contains(&s.number))
        .collect();
    if picked.is_empty() {
        bail!("no implemented stages match the selection (see --list)");
    }
    Ok(picked)
}

fn run() -> Result<bool> {
    let cli = Cli::parse();
    report::set_color(!cli.no_color && std::env::var_os("NO_COLOR").is_none());
    let all_stages = stages::all();
    let plan = locate(None, "PLAN.md").unwrap_or_else(|_| PathBuf::from("PLAN.md"));

    if cli.list {
        match &cli.json {
            Some(p) if p == Path::new("-") => {
                let cat = catalog::build(&all_stages, catalog::now_iso8601());
                print!("{}", catalog::to_json(&cat)?);
            }
            Some(p) => {
                catalog::write(p, &all_stages)?;
                eprintln!("catalog written to {}", p.display());
            }
            None => catalog::list(&all_stages, &plan),
        }
        return Ok(true);
    }

    let runtimes_file = locate(cli.runtimes_file.clone(), "runtimes.yaml")?;
    let runtimes = config::load_runtimes(&runtimes_file)?;
    let spec = cli
        .runtime
        .as_deref()
        .context("--runtime <name|path> is required")?;
    let def = config::resolve_runtime(spec, &runtimes)?;
    if cli.validate && !runtimes.contains_key(spec) {
        bail!(
            "--validate needs a registered reference runtime (one of: {})",
            runtimes.keys().cloned().collect::<Vec<_>>().join(", ")
        );
    }

    let picked = select(&cli, &all_stages)?;
    let wanted = |t: &stages::Test| {
        cli.only
            .as_ref()
            .is_none_or(|s| t.name.contains(s.as_str()))
            && cli
                .tag
                .as_ref()
                .is_none_or(|tag| t.tags.contains(&tag.as_str()))
            && !(cli.skip_ext && t.is_ext())
    };

    let reporter = report::Reporter {
        validate: cli.validate,
        verbose: cli.verbose,
    };
    let opts = runner::RunOptions {
        keep_tmp: cli.keep_tmp,
        timeout_ms: cli.timeout_ms,
        seed: cli.seed,
        verbose: cli.verbose,
    };

    let mut runner = runner::Runner::new(&def, opts)?;
    println!(
        "wasmtest: testing '{}'{} with {} (seed {:#x})",
        def.name,
        runner
            .handle()
            .version()
            .map(|v| format!(" ({v})"))
            .unwrap_or_default(),
        runtimes_file.display(),
        cli.seed
    );

    let start = Instant::now();
    let mut all: Vec<(&Stage, Vec<runner::TestResult>)> = Vec::new();
    let mut index: u64 = 0;
    for st in &picked {
        let tests: Vec<&stages::Test> = st.tests.iter().filter(|t| wanted(t)).collect();
        if tests.is_empty() {
            continue;
        }
        reporter.stage_header(st);
        let mut results = Vec::new();
        for t in tests {
            index += 1;
            let r = runner.run_test(st, t, index);
            reporter.test_result(&r);
            results.push(r);
        }
        reporter.stage_summary(st, &results);
        all.push((st, results));
    }
    if all.is_empty() {
        bail!("no tests selected (check --only / --tag / --skip-ext)");
    }
    let ok = reporter.total(&all, start.elapsed(), &def.name);
    if let Some(p) = &cli.json {
        if p == Path::new("-") {
            bail!("--json needs a file when running tests");
        }
        report::write_json(p, &def.name, cli.validate, &all, start.elapsed())?;
        println!("JSON report written to {}", p.display());
    }
    if cli.keep_tmp {
        for d in runner.kept_dirs() {
            println!("kept {}", d.display());
        }
    }
    Ok(ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn cli(args: &[&str]) -> Cli {
        let mut v = vec!["wasmtest"];
        v.extend_from_slice(args);
        Cli::try_parse_from(v).expect("the test's own arguments must parse")
    }

    #[test]
    fn validate_changes_the_wording_not_the_selection() {
        assert_eq!(
            range(&cli(&["--validate", "--stage", "5"])).expect("range"),
            (5, 5),
            "--validate must not widen --stage"
        );
        assert_eq!(
            range(&cli(&["--validate", "--until", "5"])).expect("range"),
            (0, 5)
        );
        assert_eq!(
            range(&cli(&["--validate", "--from", "10"])).expect("range"),
            (10, u32::MAX)
        );
        assert_eq!(
            range(&cli(&["--validate", "--from", "10", "--until", "12"])).expect("range"),
            (10, 12)
        );
        assert_eq!(
            range(&cli(&["--validate"])).expect("range"),
            (0, u32::MAX),
            "--validate on its own still means --all"
        );
        assert_eq!(range(&cli(&["--all"])).expect("range"), (0, u32::MAX));
        assert!(
            range(&cli(&["--runtime", "wasmtime"])).is_err(),
            "no selection at all is a usage error"
        );
    }

    #[test]
    fn select_narrows_to_the_named_stage_under_validate() {
        let all = stages::all();
        let picked = select(&cli(&["--validate", "--stage", "5"]), &all).expect("select");
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].number, 5);
    }
}
