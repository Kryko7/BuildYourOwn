# `byo` — one command for the Build Your Own tracks

`byo` is the glue between the two testers (`shelltest`, `kafkatest`), a small SQLite
progress database, and the journey site. You run it **inside the repo where you
are writing your own shell or your own Kafka broker**.

```
cd ~/code/my-shell
byo init shell --command ./your_program.sh
byo test --stage 1          # runs shelltest, streams its output, records the result
byo status                  # where you are
byo site                    # the journey map in a browser, live from the database
```

## Install

From the repo root:

```sh
./install.sh                 # builds shelltest, kafkatest, byo and the site, then installs
./uninstall.sh [--purge]     # removes the binaries (and with --purge, the data directory)
```

| Environment variable | Default | What |
|---|---|---|
| `BYO_BIN_DIR` | `~/.local/bin` | where the three binaries go |
| `BYO_HOME` | `$XDG_DATA_HOME/byo`, i.e. `~/.local/share/byo` | data directory (see below) |

The data directory after an install:

```
$BYO_HOME/
  byo.db                 SQLite: projects, runs, run_tests, stage_progress, meta, migrations
  tests/stages/*.yaml    shelltest's suite (passed as --tests-dir)
  shells.yaml            shelltest's shell registry (--shells-file)
  brokers.yaml           kafkatest's broker registry (--brokers-file)
  catalog.shell.json     stage catalog for the shell track  → GET /api/catalog/shell
  catalog.kafka.json     stage catalog for the kafka track  → GET /api/catalog/kafka
  site/                  the prerendered SvelteKit build served by `byo site`
  reports/               the raw JSON report of the latest run per track
```

Because the data lives there, the testers work from **any** working directory — `byo` passes
`--tests-dir`/`--shells-file`/`--brokers-file` for you.

## Commands

### `byo init shell|kafka`

Writes `./byo.toml` and registers the directory as a project.

```sh
byo init shell --command ./your_program.sh    # a path: shelltest runs it directly
byo init shell --shell bash                   # a name registered in shells.yaml
byo init kafka --command ./your_program.sh --port 9092 --log-dir /tmp/kraft-combined-logs
byo init kafka --broker my_broker             # a name registered in brokers.yaml
byo init shell --command ./x --force          # overwrite an existing byo.toml
```

`byo.toml`:

```toml
track = "shell"           # "shell" | "kafka"
shell = "bash"            # a registered name (shell track) …
# broker = "my_broker"    # … or (kafka track) …
# command = "./my_shell"  # … or a path. Exactly one of the three.
# port = 9092             # kafka only, passed as --port
# log_dir = "/tmp/kraft-combined-logs"   # kafka only, passed as --log-dir
```

Every command that needs a project walks **up** from the current directory looking for
`byo.toml`, so subdirectories of your project work too.

### `byo test [tester flags…]`

Runs the right tester for this project:

- streams the tester's coloured output to your terminal **unchanged** (stdout/stderr are
  inherited);
- adds `--json <tempfile>` and ingests the result into the database afterwards;
- **passes every flag you give it through verbatim** — `byo test --stage 5`,
  `byo test --until 12 --verbose`, `byo test --only quoting --keep-tmp`;
- exits with the **tester's** exit code (0 green, 1 red, 2 harness error).

Details:

- With no stage selector, `--all` is added (the testers require an explicit selection).
- If you pass your own `--json path`, that file is used for ingestion too.
- `--list` runs are not recorded.
- `byo shell …` and `byo kafka …` are explicit-track aliases. They also work outside a
  project when the target is given inline: `byo shell --shell bash --stage 1`.

### `byo status`

Per-section progress bars for both tracks, the next stage, the last run, notes and your
streak. Sections come from the installed catalogs.

### `byo done N` / `byo undone N` / `byo note N "text"`

Manual progress. Outside a project, add `--track shell|kafka`.

Progress is also **derived**: after every run, a stage whose latest run passed every test it
ran becomes `done`; a stage with some failures becomes `in_progress`; one with only failures
becomes `failed`. Notes always survive. `byo note N ""` clears a note.

### `byo site [--port N] [--no-open] [--rebuild]`

Serves `$BYO_HOME/site` **and** the JSON API (documented in [`API.md`](API.md)) on one port
(default 4321; the next free port is used if it is taken, and the real URL is printed), then
opens your browser unless `--no-open`. `--rebuild` runs `npm run build` in the repo's `site/`
first and installs the result into `$BYO_HOME/site`.

If there is no build yet, every page is a placeholder telling you to run `byo site --rebuild`;
the API still works.

### `byo db path|dump|reset|set-site-source`

```sh
byo db path                          # where the database is
byo db dump                          # the whole database as JSON
byo db reset [--yes]                 # delete every project, run and stage (asks first)
byo db set-site-source ~/…/site      # where `byo site --rebuild` should build from
```

### `byo doctor`

Checks the binaries, the data directory, every data file, the database schema version, the
installed site, `java` (for `kafkatest`'s reference broker), `npm` (for `--rebuild`), whether
port 4321 is free, whether `~/.local/bin` is on `PATH`, and whether the `byo.toml` here is
valid. Exits non-zero when something is actually broken; warnings do not fail it.

## The database

One SQLite file in WAL mode; every write is a transaction. Schema **v1**:

| Table | Columns |
|---|---|
| `meta` | `key`, `value` — `schema_version`, `installed_at`, `site_source` |
| `migrations` | `version`, `name`, `applied_at` |
| `projects` | `id`, `track`, `path`, `command`, `target_kind`, `created_at` — unique on (track, path) |
| `runs` | `id`, `project_id`, `track`, `target`, `started_at`, `elapsed_ms`, `passed`, `failed`, `skipped`, `args`, `json_path` |
| `run_tests` | `run_id`, `seq`, `stage`, `stage_name`, `stage_file`, `test_name`, `status`, `ext`, `duration_ms`, `failures_json`, `actual_json`, `skip_reason`, `failure_kind` |
| `stage_progress` | `track`, `stage`, `state`, `done_at`, `last_run_id`, `note`, `updated_at` — primary key (track, stage) |

`run_tests` carries `stage_name`, `stage_file`, `ext`, `skip_reason` and `failure_kind` beyond
the plan's column list so `GET /api/runs/:id` can rebuild the testers' exact report shape from
rows alone.

Adding a schema v2 later means appending one `(version, name, sql)` entry to `MIGRATIONS` in
`src/db.rs`; the `migrations` table records what has already run.

## Layout

```
byo/
  Cargo.toml
  README.md              this file
  API.md                 the HTTP API contract for the site
  src/
    main.rs              the clap CLI
    lib.rs               the library half (what the tests drive)
    paths.rs             $BYO_HOME, finding the tester binaries, the Track enum
    config.rs            byo.toml
    db.rs                schema, migrations, ingestion, queries, xp/streak
    report.rs            the testers' --json document (shelltest's `shell` / kafkatest's `target`)
    runner.rs            building the tester command line, running it, ingesting
    catalog.rs           the stage catalogs (for `byo status` section bars)
    status.rs            the terminal progress view
    api.rs               the JSON API as a pure (request, db) -> response function
    server.rs            tiny_http server, port picking, --rebuild, browser opening
    static_files.rs      URL -> file resolution, MIME types, the placeholder page
    doctor.rs            the environment checks
  tests/
    real_report.rs       ingests a captured `shelltest --until 3` report and reads it back
    fixtures/shelltest-until3.json
```

## Development

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

`cargo test` needs no network, no installed data directory and no testers: the unit tests use
in-memory databases and temporary directories, and the integration test replays a real
`shelltest` report from `tests/fixtures/`.
