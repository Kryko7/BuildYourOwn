# `byo` — one command for the Build Your Own tracks

`byo` is the glue between the testers, a small SQLite progress database, and the journey
site. You run it **inside the repo where you are writing your own shell, broker, runtime,
TLS server, linker or distributed store**.

```
cd ~/code/my-shell
byo init shell --command ./your_program.sh
byo test --stage 1          # runs shelltest, streams its output, records the result
byo status                  # where you are
byo site                    # the journey map in a browser, live from the database
```

## The tracks

There is **one registry** — `src/track.rs` — and every other module reads it. No module
matches on a track name, so a new track is a `TrackDef` here plus a row in `install.sh`.

| Track | Title | Tester | Target flag | `byo.toml` key | Data files | Reference needs |
|---|---|---|---|---|---|---|
| `shell` | Build your own shell | `shelltest` | `--shell` | `shell` | `tests/stages/`, `shells.yaml` | — |
| `kafka` | Build your own Kafka broker | `kafkatest` | `--broker` | `broker` | `brokers.yaml` | `java` |
| `wasm` | Build your own WebAssembly runtime | `wasmtest` | `--runtime` | `runtime` | `runtimes.yaml` | a cached `wasmtime` |
| `tls` | Build your own TLS 1.3 server | `tlstest` | `--server` | `server` | `servers.yaml` | `openssl` |
| `link` | Build your own ELF linker | `linktest` | `--linker` | `linker` | `linkers.yaml` | — |
| `dist` | Build your own distributed system | `disttest` | `--target` | `target` | `targets.yaml` | a cached `etcd` |

A track whose tester has not been built yet is still registered: `byo tracks`, `byo status`,
`byo doctor` and `/api/*` all report it as **not installed** rather than pretending it does
not exist, and `install.sh` skips its directory with a "(not present, skipped)" line.

The registry also carries a one-line blurb, a CSS accent colour (the site's per-track
palette, served by `GET /api/tracks`), the terminal colour `byo status` paints the track
with, and any track-specific `byo.toml` keys — today only kafka's `port` and `log_dir`.

## Install

From the repo root:

```sh
./install.sh                 # builds every tester that is present, byo and the site
./install.sh --skip-site     # skip the slow npm build
./uninstall.sh [--purge]     # removes the binaries (and with --purge, the data directory)
```

| Environment variable | Default | What |
|---|---|---|
| `BYO_BIN_DIR` | `~/.local/bin` | where `byo` and the testers go |
| `BYO_HOME` | `$XDG_DATA_HOME/byo`, i.e. `~/.local/share/byo` | data directory (see below) |
| `PUBLIC_JOURNEY_OWNER` | unset | whose journey this is; `install.sh` sources it from the repo-root `.env`, and `byo` reads it at runtime (export it in your shell rc for `byo status`) |

`install.sh` **sources the repo-root `.env`** (the site build bakes `PUBLIC_JOURNEY_OWNER`
in) and never copies it into `$BYO_HOME` — it is personal configuration, not installable
data.

The data directory after an install:

```
$BYO_HOME/
  byo.db                 SQLite: projects, runs, run_tests, stage_progress, meta, migrations
  tests/stages/*.yaml    shelltest's suite          (--tests-dir)
  shells.yaml            shelltest's shell registry (--shells-file)
  brokers.yaml           kafkatest's brokers        (--brokers-file)
  runtimes.yaml          wasmtest's runtimes        (--runtimes-file)
  servers.yaml           tlstest's servers          (--servers-file)
  linkers.yaml           linktest's linkers         (--linkers-file)
  targets.yaml           disttest's targets         (--targets-file)
  catalog.<track>.json   one stage catalog per track → GET /api/catalog/<track>
  site/                  the prerendered SvelteKit build served by `byo site`
  reports/               the raw JSON report of the latest run per track
```

Because the data lives there, the testers work from **any** working directory — `byo` passes
each track's data-file flags for you, and silently leaves out the ones that are not installed.

## Commands

### `byo init <track>`

Writes `./byo.toml` and registers the directory as a project. `<track>` is any registered id;
an unknown one lists them all.

```sh
byo init shell --command ./your_program.sh     # a path: the tester runs it directly
byo init shell --shell bash                    # a name registered in shells.yaml
byo init wasm  --target wasmtime               # --target works for every track
byo init kafka --command ./your_program.sh --port 9092 --log-dir /tmp/kraft-combined-logs
byo init link  --command ./ld.sh --force       # overwrite an existing byo.toml
```

Sugar flags mirror each track's `byo.toml` key (`--shell`, `--broker`, `--runtime`,
`--server`, `--linker`); `--target/-t` is the generic spelling and works everywhere. Using
another track's flag is an error that names the track it belongs to. Track-specific keys come
from the registry: `--port` / `--log-dir`, or `--set key=value` for anything a later track
adds; a key the track does not declare is refused.

`byo.toml`:

```toml
track = "shell"           # any registered track id
shell = "bash"            # the track's registered-target key …
# command = "./my_shell"  # … or a path. Exactly one of the two.
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

- The command line is built from the registry: the track's target flag, then whichever of its
  data files exist in `$BYO_HOME`, then its extra keys — and your flags last, so they win.
- With no stage selector, `--all` is added (the testers require an explicit selection).
- If you pass your own `--json path`, that file is used for ingestion too.
- `--list` runs are not recorded.
- `byo <track> …` (e.g. `byo shell …`, `byo dist …`) is an explicit-track alias for
  `byo test`. It also works outside a project when the target is given inline:
  `byo shell --shell bash --stage 1`.

### `byo status`

Every track in registry order: per-section progress bars, the next stage, the last run, notes
and your streak. Sections come from the installed catalogs; a track with no catalog and no
history collapses to one line saying **not started** (its tester is installed, you have not
begun) or **not installed** (its tester has not been built yet).

### `byo tracks`

The registry as the terminal sees it: id, title, blurb, tester, target flag and whether the
binary is installed.

### `byo done N` / `byo undone N` / `byo note N "text"`

Manual progress. Outside a project, add `--track <id>`.

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

Generic checks (`byo` itself, the data directory, the database schema, the installed site,
`npm`, port 4321, `~/.local/bin` on `PATH`, the `byo.toml` here) plus **per-track rows**:

```
ok   shell tester           shelltest 0.1.0 (~/.local/bin/shelltest)
ok   shell data             tests/stages/ (57 files), shells.yaml
ok   shell catalog          57 stage(s), 7 section(s) — ~/.local/share/byo/catalog.shell.json
ok   kafka reference        openjdk version "17.0.20.1" — for `kafkatest --broker apache_kafka --validate`
warn dist                   not installed yet — ./install.sh builds disttest once disttest/ exists
ok   testers                3 of 6 track(s) installed
```

A missing track is always a **warning**, never a failure — the testers land one at a time and
`byo doctor` has to stay useful while that happens. The only fatal findings are a missing data
directory, a broken database, and *no* tester installed at all. The "reference" row is the
external tool a track's `--validate` self-check needs (`java`, `openssl`, a cached `wasmtime`,
a cached `etcd`) — never something your own program needs.

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

`track` has always been a **TEXT** column holding the registry id, so the four new tracks
needed **no migration** — `tests/registry.rs` asserts that a `wasm` run ingests and reads back
at the same `schema_version`. `run_tests` carries `stage_name`, `stage_file`, `ext`,
`skip_reason` and `failure_kind` beyond the plan's column list so `GET /api/runs/:id` can
rebuild the testers' exact report shape from rows alone.

Adding a schema v2 later means appending one `(version, name, sql)` entry to `MIGRATIONS` in
`src/db.rs` and bumping `SCHEMA_VERSION`; the `migrations` table records what has already run.

## Layout

```
byo/
  Cargo.toml
  README.md              this file
  API.md                 the HTTP API contract for the site
  src/
    main.rs              the clap CLI (`byo <track>` is an external subcommand)
    lib.rs               the library half (what the tests drive)
    track.rs             THE REGISTRY: TrackDef per track, Track as a handle into it
    paths.rs             $BYO_HOME, registry data files, finding the tester binaries
    config.rs            byo.toml, validated against the registry
    db.rs                schema, migrations, ingestion, queries, xp/streak
    report.rs            the testers' --json document
    runner.rs            building the tester command line from the registry, running, ingesting
    catalog.rs           the stage catalogs (for `byo status` section bars)
    status.rs            the terminal progress view
    api.rs               the JSON API as a pure (request, db) -> response function
    server.rs            tiny_http server, port picking, --rebuild, browser opening
    static_files.rs      URL -> file resolution, MIME types, the placeholder page
    doctor.rs            the environment checks, per track
  tests/
    real_report.rs       ingests a captured `shelltest --until 3` report and reads it back
    registry.rs          a fake tester per track, an API round trip for a non-shell track,
                         and `byo doctor` with a track missing
    fixtures/shelltest-until3.json
```

## Adding a track

1. One `TrackDef` in `src/track.rs` (id, title, blurb, tester, target flag, `byo.toml` key,
   data files, extra keys, external requirement, accent).
2. One row in the repo-root `install.sh` `TRACKS` table, and the binary name in
   `uninstall.sh`.

That is all: `byo init`, `byo test`, `byo status`, `byo doctor`, `/api/*` and the site's
track list follow. The test suite enforces it — `tests/registry.rs` drives a fake tester for
**every** registered track, and the unit tests iterate `Track::all()` rather than naming
tracks.

## Development

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

`cargo test` needs no network, no installed data directory and no testers: the unit tests use
in-memory databases and temporary directories, the integration tests replay a real `shelltest`
report and plant fake tester scripts in a temporary `PATH`.
