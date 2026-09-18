//! `disttest` — a black-box conformance tester for a distributed key-value system.

use anyhow::{bail, Context, Result};
use clap::Parser;
use disttest::examples::CapturedFile;
use disttest::stages::{Ladder, Stage};
use disttest::{catalog, config, examples, report, runner, stages};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// stage-by-stage black-box tester for a distributed key-value system.
#[derive(Parser, Debug)]
#[command(name = "disttest", version, about)]
struct Cli {
    /// Program to test: a name from targets.yaml (my_node, etcd, ...) or a path
    #[arg(long)]
    target: Option<String>,
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
    /// Only run tests carrying this tag: a ladder (primitives, node, cluster), ext, or any
    /// tag a stage added
    #[arg(long)]
    tag: Option<String>,
    /// Hide tests tagged `ext` (everything beyond a sensible core)
    #[arg(long)]
    skip_ext: bool,
    /// Say what the harness is doing between tests
    #[arg(long, short)]
    verbose: bool,
    /// Keep each test's temporary directory
    #[arg(long)]
    keep_tmp: bool,
    /// Per-test timeout in milliseconds (starting a node is not counted)
    #[arg(long, value_name = "N", default_value_t = 20_000)]
    timeout_ms: u64,
    /// Run every stage against its ladder's reference and word failures as suite bugs
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
    /// Path to targets.yaml
    #[arg(long, value_name = "FILE")]
    targets_file: Option<PathBuf>,
    /// Seed for every random choice, so a run can be reproduced
    #[arg(long, default_value_t = 0x5eed)]
    seed: u64,
    /// Run every stage's examples against the reference and record the answers into this file
    #[arg(long, value_name = "FILE")]
    capture_examples: Option<PathBuf>,
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
        .ok_or_else(|| anyhow::anyhow!("cannot find {relative}; pass --targets-file"))
}

/// The stage range a command line asks for.
///
/// `--validate` only changes which reference each ladder runs against and the wording of a
/// failure, never the selection: `--validate --stage 5` runs stage 5. It does imply `--all`
/// when nothing else was asked for, so `--validate` alone still means "prove the suite".
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
        .filter(|s| {
            // A ladder named with --tag selects whole stages, which keeps the run header
            // honest about what was skipped.
            Ladder::parse(cli.tag.as_deref().unwrap_or_default())
                .is_none_or(|l| s.ladder == l)
        })
        .collect();
    if picked.is_empty() {
        bail!("no stages match the selection (see --list)");
    }
    Ok(picked)
}

fn run() -> Result<bool> {
    let cli = Cli::parse();
    report::set_color(!cli.no_color && std::env::var_os("NO_COLOR").is_none());
    let all_stages = stages::all();
    let plan = locate(None, "PLAN.md").unwrap_or_else(|_| PathBuf::from("PLAN.md"));

    if cli.list {
        let captured_path = locate(None, catalog::CAPTURED_EXAMPLES)
            .unwrap_or_else(|_| PathBuf::from(catalog::CAPTURED_EXAMPLES));
        let captured = CapturedFile::load_or_empty(&captured_path);
        match &cli.json {
            Some(p) if p == Path::new("-") => {
                let cat = catalog::build(&all_stages, catalog::now_iso8601(), &captured);
                print!("{}", catalog::to_json(&cat)?);
            }
            Some(p) => {
                catalog::write(p, &all_stages, &captured)?;
                eprintln!(
                    "catalog written to {} (examples from {})",
                    p.display(),
                    captured_path.display()
                );
            }
            None => catalog::list(&all_stages, &plan),
        }
        return Ok(true);
    }

    let targets_file = locate(cli.targets_file.clone(), "targets.yaml")?;
    let targets = config::load_targets(&targets_file)?;
    let spec = cli
        .target
        .as_deref()
        .context("--target <name|path> is required")?;
    let def = config::resolve_target(spec, &targets)?;
    if cli.validate && !def.is_reference() {
        bail!(
            "--validate needs a reference target (one of: {})",
            targets
                .values()
                .filter(|d| d.is_reference())
                .map(|d| d.name.clone())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let opts = runner::RunOptions {
        keep_tmp: cli.keep_tmp,
        timeout_ms: cli.timeout_ms,
        seed: cli.seed,
        verbose: cli.verbose,
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("cannot start the tokio runtime")?;

    if let Some(out) = cli.capture_examples.clone() {
        println!(
            "disttest: capturing examples into {} (seed {:#x})",
            out.display(),
            cli.seed
        );
        return rt
            .block_on(examples::capture::run(&targets, &all_stages, &out, &opts))
            .map(|()| true);
    }

    let picked = select(&cli, &all_stages)?;
    let reporter = report::Reporter {
        validate: cli.validate,
        verbose: cli.verbose,
    };

    rt.block_on(async move {
        let mut runner = runner::Runner::new(targets.clone(), def.clone(), cli.validate, opts)?;
        println!(
            "disttest: testing '{}' with {} (seed {:#x})",
            def.name,
            targets_file.display(),
            cli.seed
        );
        if cli.validate {
            for (ladder, target) in runner.routing() {
                println!("  {:<11} validated against {target}", ladder.as_str());
            }
        }
        let start = Instant::now();
        let mut all: Vec<(&Stage, Vec<runner::TestResult>)> = Vec::new();
        let mut index: u64 = 0;
        for st in &picked {
            let wanted = |t: &stages::Test| {
                cli.only
                    .as_ref()
                    .is_none_or(|s| t.name.contains(s.as_str()))
                    && cli.tag.as_ref().is_none_or(|tag| {
                        t.all_tags(st.ladder).iter().any(|x| x == tag)
                    })
                    && !(cli.skip_ext && t.is_ext())
            };
            let tests: Vec<&stages::Test> = st.tests.iter().filter(|t| wanted(t)).collect();
            if tests.is_empty() {
                continue;
            }
            reporter.stage_header(st);
            runner.begin_stage(st).await;
            let mut results = Vec::new();
            for t in tests {
                index += 1;
                let r = runner.run_test(st, t, index).await;
                let failed = r.status == runner::Status::Fail;
                reporter.test_result(&r);
                if failed {
                    if let Some(out) = runner.output() {
                        reporter.node_output(&out);
                    }
                }
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn cli(args: &[&str]) -> Cli {
        let mut v = vec!["disttest"];
        v.extend_from_slice(args);
        Cli::try_parse_from(v).expect("the test's own arguments must parse")
    }

    #[test]
    fn validate_changes_the_reference_not_the_selection() {
        assert_eq!(
            range(&cli(&["--validate", "--stage", "5"])).expect("range"),
            (5, 5)
        );
        assert_eq!(
            range(&cli(&["--validate", "--until", "5"])).expect("range"),
            (0, 5)
        );
        assert_eq!(
            range(&cli(&["--validate", "--from", "36"])).expect("range"),
            (36, u32::MAX)
        );
        assert_eq!(
            range(&cli(&["--validate"])).expect("range"),
            (0, u32::MAX),
            "--validate on its own still means --all"
        );
        assert!(
            range(&cli(&["--target", "etcd"])).is_err(),
            "no selection at all is a usage error"
        );
    }

    #[test]
    fn a_ladder_tag_selects_whole_stages() {
        let all = stages::all();
        let picked = select(&cli(&["--all", "--tag", "primitives"]), &all).expect("select");
        assert!(picked.iter().all(|s| s.ladder == Ladder::Primitives));
        assert_eq!(picked.len(), 20);
        let picked = select(&cli(&["--all", "--tag", "cluster"]), &all).expect("select");
        assert!(picked.iter().all(|s| s.ladder == Ladder::Cluster));
        // A tag that is not a ladder leaves the stage selection alone.
        let picked = select(&cli(&["--all", "--tag", "ext"]), &all).expect("select");
        assert_eq!(picked.len(), all.len());
    }

    #[test]
    fn select_narrows_to_the_named_stage_under_validate() {
        let all = stages::all();
        let picked = select(&cli(&["--validate", "--stage", "22"]), &all).expect("select");
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].number, 22);
    }
}
