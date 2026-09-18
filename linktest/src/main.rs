//! `linktest` — a black-box conformance tester for static ELF64 x86-64 linkers.

use anyhow::{bail, Context, Result};
use clap::Parser;
use linktest::stages::Stage;
use linktest::{catalog, config, report, runner, stages};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// stage-by-stage black-box tester for static ELF64 linkers.
#[derive(Parser, Debug)]
#[command(name = "linktest", version, about)]
struct Cli {
    /// Linker to test: a name from linkers.yaml (gnu_ld, my_linker, ...) or a path
    #[arg(long)]
    linker: Option<String>,
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
    /// Hide tests tagged `ext` (stages beyond the core track)
    #[arg(long)]
    skip_ext: bool,
    /// Say what the harness is doing between tests
    #[arg(long, short)]
    verbose: bool,
    /// Keep each test's temporary directory (inputs, output, the linker's own files)
    #[arg(long)]
    keep_tmp: bool,
    /// Per-test deadline in milliseconds; every link and every linked program runs under it
    #[arg(long, value_name = "N", default_value_t = 10_000)]
    timeout_ms: u64,
    /// Run everything against the reference linker and word failures as suite bugs
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
    /// Path to linkers.yaml
    #[arg(long, value_name = "FILE")]
    linkers_file: Option<PathBuf>,
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
    roots
        .iter()
        .map(|r| r.join(relative))
        .find(|p| p.exists())
        .ok_or_else(|| anyhow::anyhow!("cannot find {relative}; pass --linkers-file"))
}

/// The stage range a command line asks for.
///
/// `--validate` only changes the linker and the wording of a failure, never the selection:
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

    let linkers_file = locate(cli.linkers_file.clone(), "linkers.yaml")?;
    let linkers = config::load_linkers(&linkers_file)?;
    let spec = cli
        .linker
        .as_deref()
        .context("--linker <name|path> is required")?;
    let def = config::resolve_linker(spec, &linkers)?;
    if cli.validate && !linkers.contains_key(spec) {
        bail!(
            "--validate needs a registered reference linker (one of: {})",
            linkers.keys().cloned().collect::<Vec<_>>().join(", ")
        );
    }
    if cli.validate && def.kind != config::LinkerKind::Reference {
        bail!(
            "--validate is the suite's self-check: point it at a `kind: reference` linker \
             (gnu_ld), not at '{spec}'"
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

    let mut runner = runner::Runner::new(def.clone(), opts)?;
    println!(
        "linktest: testing '{}' [{}] with {} (seed {:#x})",
        def.name,
        runner.handle().describe(),
        linkers_file.display(),
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
    runner.end_run();
    Ok(ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn cli(args: &[&str]) -> Cli {
        let mut v = vec!["linktest"];
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
            (0, 5),
            "--validate must not widen --until"
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
        assert_eq!(range(&cli(&["--stage", "5"])).expect("range"), (5, 5));
        assert!(
            range(&cli(&["--linker", "gnu_ld"])).is_err(),
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
