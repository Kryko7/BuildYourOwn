# BuildYourOwn — master plan

Three deliverables in this repo (`~/Personal/BuildYourOwn`):

| Dir | What | Status |
|---|---|---|
| `shelltest/` | Rust black-box tester for a POSIX shell, 70 stages / 592 tests, validated on bash | done (2026-09-12; section H added 2026-09-19) |
| `kafkatest/` | Rust black-box tester for a Kafka broker, ~45 stages, validated on real Apache Kafka 4.1 (KRaft) | **to build** |
| `site/` | the journey site — SvelteKit site: interactive journey map for BOTH tracks, what to build per stage, learning resources, live red/green from the testers' JSON reports | **to build** |

You write the shell and the broker yourself. **Never write shell or broker
implementation code.** Everything here is harness, suite, docs and site.

Repo is not a git repository; do not `git init` unless asked. Node 26, cargo 1.97, Java 17 are
installed. Docker is NOT available (daemon down) — the reference Kafka must be the Apache
tarball run directly with Java. No accounts, no cloud.

---

## Part 1 — kafkatest

### 1.1 Goals

stage-by-stage tester with the same UX as `shelltest` (read `shelltest/README.md`,
`shelltest/src/report.rs`, `shelltest/src/main.rs` first and mirror them), but for the Kafka
wire protocol, and much deeper than the classic track: it must be a production-grade
conformance suite that a real broker (Apache Kafka 4.1.x) passes 100 % via `--validate`.

```
kafkatest --broker my_broker --stage 4
kafkatest --broker my_broker --until 12 --json report.json
kafkatest --broker apache_kafka --validate      # must be all green — suite self-check
kafkatest --list                                 # stages, counts, PLAN.md tickboxes
kafkatest --list --json > catalog.json           # stage catalog consumed by the site
```

### 1.2 Conventions the user's broker must follow ("build your own Kafka")

- Started as `./your_program.sh /tmp/server.properties` (argv[1] = properties file; the
  harness writes one). Listens on `0.0.0.0:9092` (`brokers.yaml` can override with `{PORT}`).
- Log directory `/tmp/kraft-combined-logs` (overridable, `{LOGDIR}`), Kafka on-disk format:
  `__cluster_metadata-0/00000000000000000000.log`, `<topic>-<n>/00000000000000000000.log`,
  `partition.metadata`, `meta.properties`. The harness writes these fixtures before start.
- Request/response framing: 4-byte big-endian size + header v2 (flexible) / v1 as per API.
- `UNSUPPORTED_VERSION` = 35, `UNKNOWN_TOPIC_OR_PARTITION` = 3, `UNKNOWN_TOPIC_ID` = 100.
- Everything else follows the official protocol guide; when real Kafka and a hand-written broker differ
  (e.g. hand-written brokers ignore throttle) the expectation is loosened so both pass, or
  the test is `skip_on: [apache_kafka]` with a written reason, exactly as shelltest does.

### 1.3 Architecture (crate `kafkatest`, binary `kafkatest`)

```
kafkatest/
  Cargo.toml            tokio, bytes, kafka-protocol (0.18, verify API coverage), crc32c/crc-fast,
                        clap, serde{,_json,_yaml}, anyhow, owo-colors, similar, tempfile, regex,
                        rand (seeded), flate2 + snap + lz4_flex + zstd, uuid, indexmap
  brokers.yaml          reference + user broker definitions (see 1.4)
  PLAN.md               tickbox stage plan (same shape as shelltest/PLAN.md; `--list` reads it)
  README.md             UX, conventions table, normalization/report semantics, test-authoring guide
  catalog.json          `kafkatest --list --json` output, committed; a test asserts it is current
  src/
    main.rs             CLI (clap) — flag parity with shelltest: --broker --stage --until --from
                        --all --only --tag --skip-ext --verbose --keep-tmp --timeout-ms --validate
                        --list --json --no-color --brokers-file  + --port --log-dir --seed
    config.rs           brokers.yaml model, placeholder substitution {PORT} {LOGDIR} {PROPS} {TMP}
    broker/mod.rs       BrokerHandle: spawn, wait-for-port, stdout/stderr capture, kill, crash
                        detection, restart policy (per_test | per_stage)
    broker/reference.rs Apache Kafka download (archive.apache.org, kafka_2.13-4.1.x.tgz, sha512
                        checked) into ~/.cache/kafkatest with a lock file; `kafka-storage.sh
                        format` + generated server.properties; KRaft combined mode; random free
                        ports; JVM flags for fast boot (small heap, -XX:TieredStopAtLevel=1)
    proto/mod.rs        thin client: connect, send framed request, read framed response, decode
                        via kafka-protocol; raw-bytes mode for malformed-frame tests; pipelining;
                        hex-dump + annotated decode for reports
    proto/records.rs    RecordBatch v2 writer/reader (CRC32C, varints, headers, compression,
                        control batches); MUST round-trip and be accepted by
                        `kafka-dump-log.sh` from the tarball (unit test + validate step)
    fixtures/mod.rs     abstract fixtures: Topic{name, partitions, records, id}, Group{..}
    fixtures/files.rs   strategy `files`: write __cluster_metadata + topic logs on disk
                        (FeatureLevelRecord, TopicRecord, PartitionRecord, bootstrap records)
    fixtures/api.rs     strategy `api`: create the same state through CreateTopics/Produce on a
                        running broker; topic ids discovered via Metadata. Tests never assume
                        a fixed topic id — they read it from the FixtureHandle.
    stages/mod.rs       registry `pub fn all() -> Vec<Stage>`; Stage{number,name,ext,hints,tests}
                        Test{name, tags, skip_on, restart, run: async fn(&mut Ctx) -> Result<(),Failure>}
    stages/sNN_*.rs     one file per stage (see 1.5) — parallel agents can work without conflicts
    assert.rs           structured Failure: expected vs actual with decoded field paths, hex diff,
                        the raw request/response bytes; `similar` diffs for text
    report.rs           terminal reporter identical in feel to shelltest (✔/✘, ┌ blocks, per-stage
                        totals) + JSON report with the SAME top-level schema as shelltest's
                        JsonReport (rename `shell` → `target`, keep passed/failed/skipped/stages/
                        tests/failures/actual/duration_ms/ext/skip_reason) so the site reads both
  tests/                cargo integration tests: records round-trip, fixture writer, catalog
                        freshness, CLI smoke with a fake broker (a tiny tokio TCP stub inside tests)
```

Rules: every test runs in a fresh `{TMP}`; the broker's stdout/stderr are captured and shown on
failure; a broker crash mid-test is a distinct failure kind ("broker exited with status N");
timeouts are per test (default 10 s, reference broker boot excluded). `--keep-tmp` keeps log
dirs. Deterministic: `--seed` drives every random choice (fuzz stage, payloads, topic names).
No `unwrap()` on network/decoding paths; `clippy -D warnings` clean; `cargo fmt`.

### 1.4 brokers.yaml

```yaml
brokers:
  apache_kafka:            # reference — `--validate` target
    kind: reference        # auto-download & KRaft format; version pinned
    version: "4.1.2"
    fixtures: api
    restart: per_stage     # boot is ~4 s; tests use unique topic names
  my_broker:
    command: ["./your_program.sh", "{PROPS}"]
    port: 9092
    log_dir: /tmp/kraft-combined-logs
    fixtures: files
    restart: per_test
    cwd: "."
```

### 1.5 Stages (target ≈ 45 stages / ≥ 350 tests; **[ext]** = beyond the core track)

**A. Bootstrap & framing**
01 Bind to port 9092 (TCP accept; nothing sent on connect)
02 Respond with the correlation id (any request → 4-byte size + correlation id echoed)
03 Parse request header (api_key, api_version, client_id, tagged fields)
04 UNSUPPORTED_VERSION (35) for ApiVersions with a bad version
05 ApiVersions v4 response body (error 0, ApiVersions(18) with min≤4≤max, throttle, tagged fields)
06 Sequential requests on one connection
07 Concurrent connections (N clients, interleaved, correct correlation ids each)
08 Pipelined requests (many frames written at once, responses in order) **[ext]**
09 Framing robustness: partial frames, oversized size field, zero-length, garbage, half-close;
   broker must not crash and must keep serving other connections **[ext]**

**B. Metadata & topics**
10 ApiVersions advertises DescribeTopicPartitions(75) v0
11 DescribeTopicPartitions: unknown topic → error 3, id zero-UUID, cursor null
12 Single partition topic
13 Multiple partitions (ordering, leader, replicas, ISR arrays)
14 Multiple topics in one request (response ordered by name)
15 Response-partition-limit + cursor pagination **[ext]**
16 Metadata (3) v12: brokers, controller id, topics with ids **[ext]**
17 CreateTopics (19): create, duplicate → 36 TOPIC_ALREADY_EXISTS, invalid names **[ext]**
18 DeleteTopics (20) and subsequent describe → unknown **[ext]**

**C. Fetch**
19 ApiVersions advertises Fetch(1) v16
20 Fetch with no topics
21 Fetch unknown topic id → 100 UNKNOWN_TOPIC_ID
22 Fetch empty topic → records empty, high watermark 0
23 Fetch a single record from disk (bytes identical to the log file batch)
24 Fetch multiple batches / partitions, offsets and high watermark advance
25 Fetch from an offset in the middle; OFFSET_OUT_OF_RANGE (1) **[ext]**
26 max_bytes / partition_max_bytes truncation, at least one batch always returned **[ext]**
27 Compressed batches: gzip, snappy, lz4, zstd returned intact **[ext]**
28 Long poll: max_wait_ms / min_bytes semantics; fetch returns when data arrives **[ext]**

**D. Produce**
29 ApiVersions advertises Produce(0) v11
30 Produce to unknown topic → error 3
31 Produce one record: base offset 0, log_append_time -1, error 0
32 Produce multiple records / multiple partitions / multiple topics in one request
33 acks = 0 (no response), 1, -1 **[ext]**
34 Persisted on disk: the segment file contains a valid RecordBatch (CRC, offsets) **[ext]**
35 Produce → Fetch round trip, offsets continue across restarts **[ext]**
36 Idempotent producer: InitProducerId (22), sequence numbers, duplicate detection
   (DUPLICATE_SEQUENCE_NUMBER / OUT_OF_ORDER_SEQUENCE_NUMBER) **[ext]**
37 Record validation: bad CRC → CORRUPT_MESSAGE (2), oversized → MESSAGE_TOO_LARGE (10) **[ext]**

**E. Offsets & consumer groups** (all **[ext]**)
38 ListOffsets (2): earliest, latest, by timestamp
39 FindCoordinator (10) for a group
40 JoinGroup (11) / SyncGroup (14) / Heartbeat (12) / LeaveGroup (13) — classic protocol,
   leader assignment, generation ids, rebalance on second member
41 OffsetCommit (8) / OffsetFetch (9) — commit, fetch, unknown group → -1
42 Group errors: UNKNOWN_MEMBER_ID (25), REBALANCE_IN_PROGRESS (27), ILLEGAL_GENERATION (22)

**F. Interop, robustness, performance** (all **[ext]**)
43 Real client interop: Kafka's own `kafka-console-producer.sh` / `kafka-console-consumer.sh` /
   `kafka-topics.sh --describe` from the downloaded tarball talk to the broker successfully
44 Fuzz: 500 seeded mutated frames (bit flips, truncation, huge lengths, negative array sizes)
   — broker never crashes, never hangs, still answers ApiVersions afterwards
45 Soak: 10 000 records produced and fetched back byte-identical; 1 000 connect/close cycles;
   p99 request latency reported (informational, no hard limit except "finishes within 60 s")

Each stage carries 2–4 `hints:` lines (what to implement, which struct/field, where the trap is)
which go into PLAN.md and catalog.json and are rendered by the site.

### 1.6 Acceptance

- `cargo build --release`, `cargo test`, `cargo clippy --all-targets -- -D warnings` clean.
- `kafkatest --broker apache_kafka --validate --all` all green (skips with reasons allowed
  only where documented in README).
- `kafkatest --broker examples/broken_broker --until 5` shows red output with hex dumps
  (`examples/broken_broker.py` or a small Rust example binary that answers wrong ids).
- `kafkatest --list` and `catalog.json` consistent with PLAN.md; `tests/` checks it.
- README documents every convention, all flags, JSON schema, how the reference works, how to
  add a stage.

---

## Part 2 — the journey site

### 2.1 Stack

SvelteKit 2 + Svelte 5 (runes), TypeScript strict, `@sveltejs/adapter-static` (prerendered,
`npm run build` → `build/`), Vite, vitest, svelte-check. Hand-written CSS with design tokens
(no Tailwind, no component library). No backend, no accounts. Works from `file://`-less static
hosting and from `npm run dev`. Node 26 / npm only (no pnpm/bun).

### 2.2 Data pipeline (single source of truth = the testers)

`site/scripts/sync-catalog.mjs` (run by `npm run sync`, and by `npm run build`):
- shell track: parse `../shelltest/PLAN.md` (stage number, name, ext flag, hint bullets,
  yaml file, test count) + `../shelltest/tests/stages/*.yaml` (test names, mode, tags,
  skip_on) → `src/lib/data/catalog.shell.json`.
- kafka track: read `../kafkatest/catalog.json` (until it exists, generate a placeholder from
  the stage list in section 1.5 so the site is complete from day one; a `pending: true`
  flag marks it) → `src/lib/data/catalog.kafka.json`.
- Sections/regions per track (shell: A Basics … as PLAN.md headings; kafka: A–F above).

Catalog schema (both tracks):
```ts
type Catalog = { track: 'shell'|'kafka'; generatedAt: string; pending?: boolean;
  sections: { id: string; title: string; stages: number[] }[];
  stages: { number: number; slug: string; name: string; ext: boolean; file: string;
            hints: string[]; tests: { name: string; ext?: boolean; tags?: string[];
            skipOn?: string[] }[]; }[] }
```

Report import: both testers' `--json` output share the schema in `shelltest/src/report.rs`
(`JsonReport`; kafkatest uses `target` instead of `shell`). A report colours the map per test.

Resources: `src/lib/data/resources.shell.json`, `resources.kafka.json` (schema in 2.5).

### 2.3 Pages

- `/` Home: hero with the journey title, two track cards with progress rings, overall XP/level,
  streak (days with a stage completed), "continue where you left off" CTA, recent activity.
- `/shell`, `/kafka` Track page: an SVG **journey map** — a winding path (metro-map style)
  through sections, a node per stage (locked/next/in-progress/done/red-from-report), zoom &
  pan, keyboard navigation (←/→/Enter), animated progress along the path. Clicking a node
  opens a **stage drawer**: goal, "what to build" (hints), test list with statuses, the exact
  command to run (copy button), conventions excerpt relevant to the stage, resources for the
  stage, notes (persisted), mark done / reset.
- `/shell/[stage]`, `/kafka/[stage]` Deep-linkable stage pages with the same content and
  prev/next, prerendered from the catalog.
- `/resources` Library: searchable, filterable by track / type / level / concept, grouped;
  "why read it" blurb; marks resources tied to your current stage.
- `/lab` Interactive playgrounds (each also embedded in the relevant stage drawer):
  - Shell tokenizer: type a command line; tokens colour by quote state (normal/single/double/
    escape); shows word boundaries, expansion markers, and the redirection/pipeline graph.
  - Kafka wire inspector: pick a sample request (ApiVersions v4, DescribeTopicPartitions v0,
    Fetch v16, Produce v11) or paste hex; annotated byte view (each field highlighted, varints
    & compact arrays explained, CRC32C computed live).
  - RecordBatch anatomy: build a batch from records and watch the bytes; toggle compression.
  - Journey replay: after importing a report, step through a failing test's actual request/
    response bytes (kafka) or stdin/stdout/terminal (shell) with the diff.
- `/progress` Import/export: drag-and-drop `report.json`, shows run summary, per-stage
  results, failure details rendered like the terminal (monospace ┌ blocks); export/import
  of all local state; clear.

### 2.4 Interactivity & polish requirements

- Svelte 5 runes, `$state` stores persisted to localStorage (guarded try/catch, SSR-safe).
- Live mode in `npm run dev`: a Vite plugin serves `../shelltest/report.json` and
  `../kafkatest/report.json` at `/__reports/{shell,kafka}.json`; the site polls every 2 s and
  updates the map — run the tester with `--json report.json` and watch nodes turn green.
- Confetti/celebration on stage completion (CSS/canvas, no library), sound optional and off
  by default, respects `prefers-reduced-motion`.
- Light/dark theme (system default, toggle), fully keyboard-accessible, responsive down to
  360 px, `aria-*` on the map nodes, focus rings.
- Command palette (Ctrl-K): jump to stage/resource/page.
- Design: distinctive, not generic — journey/expedition motif (map, waypoints, camp/badges
  per section); typography with a display face + mono for code; Fable-grade attention to
  spacing and motion. The app title is `"<owner>'s Journey"`, where the owner comes from
  `PUBLIC_JOURNEY_OWNER` in the git-ignored repo-root `.env` (`.env.example` is the template);
  with nothing set it reads "The Journey".
- `npm run check`, `npm run test`, `npm run build` clean; Lighthouse-ish sanity: no console
  errors, no layout shift on load.

### 2.5 Resources (curated, verified)

Schema:
```ts
type Resource = { id: string; title: string; url: string; type: 'spec'|'doc'|'book'|'article'|
  'video'|'course'|'repo'|'tool'|'paper'; level: 'intro'|'core'|'deep'; track: 'shell'|'kafka'|
  'both'; stages: number[]; concepts: string[]; why: string; minutes?: number; free: boolean }
```
Every URL must be fetched and return 200 at curation time (record `checkedAt`). Aim for
≥ 40 resources per track covering every section: shell → POSIX.1-2024 Shell Command Language,
bash manual chapters, APUE, "The Linux Programming Interface" chapters, termios/pty articles,
readline/line-editing, job control; kafka → Kafka protocol guide, KIP-482 (flexible versions),
KIP-500/595/631 (KRaft, metadata log), record batch format docs, log segment/index docs,
consumer group protocol (KIP-848 and classic), idempotent producer (KIP-98), Kafka: The
Definitive Guide chapters, tokio & bytes docs, kafka-protocol crate docs.

---

## Part 3 — Execution (agents, all on Opus 5)

Phase 1 (parallel):
- **K1** kafkatest framework: crate, CLI, brokers.yaml, reference broker download/boot,
  proto client, records codec, fixtures (both strategies), report, stages 01–09 + 10–14 +
  19–24 + 29–32 (the core stages) green on apache_kafka; PLAN.md, README, catalog.json.
- **F1** site: scaffold, design system, data pipeline, all pages, shell track complete, kafka
  track from the plan's placeholder catalog, playgrounds, import/live mode, tests.
- **R1** resources: both JSON files, verified URLs, blurbs, stage/concept tags.

Phase 2 (parallel, after K1; each owns its own `stages/` files only): K2 B-15..18,
K3 C-25..28, K4 D-33..37, K5 E-38..42, K6 F-43..45. Each keeps `--validate` green and
updates PLAN.md/catalog.json for its own stages only (append-only edits).
- **F2** (after K-phase 2 and R1): resync catalog, wire resources into stages, final polish,
  browser walkthrough.

Phase 3: **V1** verification & review: full `--validate --all`, `cargo test`, clippy, site
build/check/test, browser screenshots of every page, fix findings, write the delivery report.

Coordination rules for agents: work only inside your directory/files; do not touch
`shelltest/` except F1's read-only parsing; do not start anything on port 9092 (the reference
broker uses random ports); the Kafka tarball cache is `~/.cache/kafkatest` (lock file, reuse);
leave the repo buildable at every step; never write broker/shell implementation code.

---

## Part 4 — Install, `byo` command, progress database, examples, pastel UI (added 2026-09-13)

User requirements added after Phase 1:
1. Installable: one terminal command, usable from inside the repo where the user builds their own shell or Kafka.
2. A lightweight database, created at install time, that stores progress and run history.
3. "What to build" shows examples of what to expect (sample input → expected output / request → response).
4. UI revamp: pastel colour theme, beautiful animations with cute animals and flowers.
5. The Lab tab must work (currently a Svelte `each_key_duplicate` during hydration breaks it).
6. The frontend can be opened from the terminal (`byo site`) and opens a browser tab.

### 4.1 `byo` CLI (Rust crate `byo/`, binary `byo`)

```
install.sh                 # builds shelltest, kafkatest, byo (release) and the site; installs to
                           # ~/.local/bin/{byo,shelltest,kafkatest}; copies data to ~/.local/share/byo/
                           # (shelltest/tests, shells.yaml, brokers.yaml, site/build); creates the SQLite DB
                           # at ~/.local/share/byo/byo.db (schema below); prints PATH advice. Idempotent.
byo init shell|kafka [--command "./your_program.sh"]
                           # run inside the user's project repo: writes ./byo.toml {track, command, port,
                           # log_dir} and registers the project in the DB (projects table).
byo test [--stage N | --until N | --all | any tester flag...]
                           # inside a project: reads byo.toml, runs the right tester with --json into a temp
                           # file, streams the tester's coloured output to the terminal unchanged, then
                           # ingests the JSON into the DB (runs, run_tests) and updates stage_progress.
                           # `byo shell ...` / `byo kafka ...` are explicit-track aliases.
byo status                 # terminal progress summary: per-section bars, next stage, last run, streak.
byo done N | byo undone N  # mark a stage done/undone (also derived automatically: a stage whose latest
                           # run passed every test is done).
byo note N "text"          # attach a note to a stage.
byo site [--port 4321] [--no-open]
                           # serves the built site from ~/.local/share/byo/site AND the JSON API below on
                           # one port, opens the default browser (xdg-open / open), keeps running until Ctrl-C.
byo db path|dump|reset     # where the DB is, dump as JSON, reset (asks for confirmation).
byo doctor                 # checks binaries, data dir, DB schema version, java for kafka, node absent ok.
```

Implementation: clap, rusqlite (bundled), serde/serde_json, axum or tiny_http for the server, `open` crate
for the browser, directories via `dirs`. Testers are found next to the `byo` binary or on PATH; data via
`$BYO_HOME` (default `~/.local/share/byo`). Pass `--tests-dir`/`--shells-file`/`--brokers-file` so the
testers work from any cwd. Exit code of `byo test` = the tester's exit code.

SQLite schema (v1): `projects(id, track, path, command, created_at)`, `runs(id, project_id, track, target,
started_at, elapsed_ms, passed, failed, skipped, args, json_path)`, `run_tests(run_id, stage, test_name,
status, duration_ms, failures_json, actual_json)`, `stage_progress(track, stage, state, done_at,
last_run_id, note, updated_at)`, `meta(key, value)` (schema_version, installed_at). Every write in a
transaction; WAL mode.

JSON API served by `byo site` (also the contract the site consumes; CORS not needed, same origin):
```
GET  /api/health                      → {ok, version, db, tracks:{shell:{project?}, kafka:{project?}}}
GET  /api/progress                    → {shell:{stages:{[n]:{state, doneAt, note, lastRunId}}}, kafka:{...}, streak, xp}
GET  /api/runs?track=&limit=          → [{id, track, target, startedAt, elapsedMs, passed, failed, skipped}]
GET  /api/runs/:id                    → the run in the testers' JsonReport shape (rebuilt from run_tests)
GET  /api/runs/latest?track=          → same shape (404 if none)
POST /api/stages/:track/:n            body {state?: 'done'|'todo', note?: string} → updated stage row
GET  /api/catalog/:track              → catalog JSON (from the data dir) so the site can be served with
                                        newer testers than it was built with
```
The site detects the API by probing `/api/health` on load; when present, progress/notes/report come from
the API (localStorage stays the fallback for static hosting), and `/progress` shows run history from the DB.

### 4.2 Examples in "what to build"

- Shell: every stage page/drawer shows 2–3 examples rendered as a terminal transcript, derived from the
  catalog's tests (input lines prefixed with `$ `, expected stdout/stderr/exit code below, pty tests show
  key steps). No new data needed; pick the shortest exact-match tests.
- Kafka: `kafkatest` stages get `examples: &[Example]` (`title`, `request` summary, `request_hex`,
  `response` summary, `response_hex`, `note`) generated by the harness itself at catalog time (encode the
  request with the real codec; the response bytes are what Apache Kafka answered during `--validate` or a
  hand-built expected frame), emitted in `catalog.json`; the site renders them with the wire inspector's
  annotated byte view.

### 4.3 UI revamp — pastel garden edition

- Palette: pastel (soft pinks, lavenders, mint, butter yellow, sky blue, peach) on a warm cream base; dark
  mode is a dusk variant (deep plum/indigo base with the same pastel accents). Contrast stays AA.
- Motif: a garden trail. Sections are meadows/glades; stage nodes are flower buds that bloom when done
  (SVG petal animation), wilt-free red poppy when failing; the walked path is a stone/petal path.
- Animals: a small animal mascot per track (e.g. a fox for the shell, a bunny for Kafka, plus a cat on the
  home page) as hand-drawn inline SVG with CSS/JS animations: idle breathing/blink, walking along the trail
  to the current stage, a little hop + confetti-of-petals on completion; butterflies/bees drifting across
  the hero; birds on the nav. All animations honour `prefers-reduced-motion`; no external assets or
  libraries (no Lottie), 60 fps, GPU-friendly transforms only.
- Typography: rounded friendly display face + clean body + mono for code (Google Fonts with fallbacks).
- Keep every feature from Part 2 (map, drawer, palette, playgrounds, import, live mode) and add the API
  integration from 4.1.

### 4.4 Execution

- **C1** (Opus): `byo/` crate, `install.sh`, DB, API server, browser opening, docs; end-to-end test with a
  fake project (bash as the "shell", `--broker` example as the "kafka").
- **F3** (Opus, after F2): pastel revamp, animals/flowers, examples, API integration, Lab fix.
- **K7** (Opus, after K-merge): examples in kafkatest stages + catalog, full `--validate`.
- **V1**: install from scratch in a temp HOME, run `byo` end-to-end, browser walkthrough, delivery report.

### 4.5 Progress source of truth = the database (user clarification, 2026-09-13)

Progress flows through the DB only: `byo test` records every run; `byo site` serves the site and the
API; the site reads progress, notes, run history and failure details from `/api/*` and writes
done/undone/notes through `POST /api/stages/:track/:n`. Remove the drag-and-drop / "choose a file"
report upload and the "load sample" buttons from `/progress`; replace with run history from the DB
(latest run per track, per-run stage/test results, failure blocks). The dev-mode `/__reports` polling
and localStorage remain only as a fallback when no API is present (static hosting), shown with a clear
"not connected to byo — run `byo site`" banner; they are not the primary path. Live updates: the site
polls `/api/runs/latest?track=` (or `/api/progress`) every 2 s while a run is in progress, so the map
turns green as `byo test` finishes.

---

## Part 5 — Four more tracks (added 2026-09-18)

Requirements added by the user:

1. No third-party course branding anywhere in the repo (done: the vocabulary is now
   "core track" for the base stages and `[ext]` for everything past them).
2. More challenges — and deliberately **not** the usual ones. No Redis, no Git, no HTTP
   server, no DNS, no SQLite reader, no BitTorrent, no toy interpreter. Each new track has
   to be something a learner cannot get as a polished guided course today.
3. Every tester stays **Rust**, black-box, and comprehensive: real formats, real protocols,
   real reference implementations — never a simplified re-imagining of the thing.
4. The repo is a git repository; work is committed in steps.
5. The journey owner's name lives in the git-ignored `.env`, never in the source.

### 5.1 The four tracks

| Dir | Track id | You build | Reference for `--validate` |
|---|---|---|---|
| `wasmtest/` | `wasm` | a WebAssembly runtime (binary decoder + validator + interpreter + WASI preview1 + fixed-width SIMD) | `wasmtime` 48.0.2, downloaded and cached in `~/.cache/wasmtest` |
| `tlstest/` | `tls` | a TLS 1.3 server (RFC 8446), client authentication included | `openssl s_server` from the system OpenSSL 3.x |
| `linktest/` | `link` | a static ELF64 linker for x86-64 | `/usr/bin/ld` (GNU ld) |
| `disttest/` | `dist` | a distributed system, in four ladders: primitives → algorithms → a durable node → a replicated cluster | `etcd` 3.7.1, cached in `~/.cache/disttest` (node, cluster) and this crate's own `reference_primitives` / `reference_algorithms` examples (primitives, algorithms) |

Why these four: each is a real, specified artefact with a production reference that is either
installed or pinned-downloadable, and they cover different muscles — **decode + execute**
(wasm), **crypto + state machine** (tls), **emit a binary that the kernel actually runs**
(link), **agree under partition** (dist). None of them is a tutorial-shaped toy.

The suite self-check is the same contract as the two existing testers: pointing the tester
at the real implementation must be **all green**. `wasmtest --runtime wasmtime --validate`,
`tlstest --server openssl --validate`, `linktest --linker gnu_ld --validate`,
`disttest --target etcd --validate`.

### 5.2 Shared contracts (every tester, old and new)

- **CLI parity**: `--stage N --until N --from N --all --only SUBSTR --tag T --skip-ext
  --verbose --keep-tmp --timeout-ms MS --validate --list [--json] --no-color --seed N`
  plus one target flag per track (`--shell`, `--broker`, `--runtime`, `--server`, `--linker`).
- **`--list --json`** emits the catalog schema kafkatest already emits (`track`, `generatedAt`,
  `sections[]`, `stages[]{number,slug,name,ext,file,hints[],tests[],examples[]}`), committed as
  `<tester>/catalog.json` with a cargo test asserting it is current.
- **`--json report.json`** emits the `JsonReport` schema (`target`, `validate`, `stages[]`,
  `passed/failed/skipped/elapsed_ms`, per-test `status/ext/duration_ms/failures/actual/
  failure_kind/notes`) — byo ingests it unchanged.
- **Examples**: every stage carries worked examples (`title`, `note`, and either a byte-level
  request/response pair with per-field annotations, or a transcript) so the site can show
  "what to expect" without the learner running anything.
- Deterministic (`--seed`), fresh tmp dir per test, target stdout/stderr captured and shown on
  failure, crash-of-target is its own failure kind, `clippy -D warnings` and `cargo fmt` clean,
  no `unwrap()` on I/O or decode paths.

### 5.3 wasmtest — build your own WebAssembly runtime

Program contract (a subset of wasmtime's CLI, so the reference is literally wasmtime):

```
./your_program.sh run --invoke <export> <module.wasm> [args...]   # prints one result per line
./your_program.sh run <module.wasm> [--] [args...]                # runs _start (WASI command)
```
Traps exit non-zero with `wasm trap: <canonical reason>` on stderr; a module that fails to
decode or validate exits non-zero before executing anything. Canonical reasons are the spec's
own strings ("integer divide by zero", "out of bounds memory access", "unreachable", …).

The tester **builds every module itself** with its own encoder (no wabt, no .wat files), so
tests are real binaries with exact bytes, and failures can print the module's hex.

Sections (~45 stages): A binary format & decoding · B validation & type checking ·
C numeric instructions & traps · D control flow · E memory & bulk operations ·
F tables, globals, indirect calls · G WASI preview1 · H robustness (fuzz, soak, interop).

### 5.4 tlstest — build your own TLS 1.3 server

Program contract (the `openssl s_server` flag subset, so the reference is the real thing):

```
./your_program.sh -accept <port> -cert <cert.pem> -key <key.pem> -rev [-naccept <n>]
```
`-rev` is the application layer: every line received is echoed back reversed, which makes the
data path deterministic and testable. Certificates and keys are generated per test run
(RSA-2048, ECDSA P-256, Ed25519) by the harness.

The tester is a **raw TLS 1.3 client written for this suite** — X25519, HKDF, transcript
hashes, AEAD and record framing done explicitly — so every failure can show the exact
handshake message bytes, the derived secrets' labels and the transcript it hashed.

Sections (~45 stages): A TCP & record layer · B ClientHello, extensions, negotiation ·
C key schedule & handshake encryption · D authentication (Certificate, CertificateVerify,
Finished) · E application data & AEAD · F alerts, errors, robustness · G advanced
(HelloRetryRequest, KeyUpdate, tickets/resumption, early-data rejection, real-client interop).

### 5.5 linktest — build your own ELF linker

Program contract (the GNU ld flag subset, so the reference is `/usr/bin/ld`):

```
./your_program.sh -o <out> [-e <entry>] [-L <dir>] [-l <name>] <input.o|input.a>...
```
The tester **writes the input objects itself** with its own ELF64 writer (no dependency on
binutils at test time), links them with the program under test, then **runs the output** and
checks its stdout and exit status, and re-parses the produced ELF with its own reader.

Sections (~42 stages): A reading relocatable objects · B emitting a runnable executable ·
C symbol resolution (weak, common, COMDAT, visibility) · D relocations and overflow ·
E archives and link order · F real multi-object programs, fuzz, soak.

### 5.6 disttest — build your own distributed system

The one track with **levels**, because the user asked for whole implementations, algorithms and
small integrations in the same place. Every stage is tagged with its ladder:

- **Ladder A — primitives** (`primitives`): a line-oriented CLI answering one JSON object per
  command. Lamport and vector clocks, version vectors, hybrid logical clocks, consistent and
  rendezvous hashing, quorum math, Bloom filters, HyperLogLog, Merkle diff, five CRDTs checked
  by replaying every permutation of the delivery order, Chandy–Lamport snapshots, causal
  broadcast, token/leaky buckets, backoff jitter distributions, phi-accrual and SWIM.
  The oracle is the tester's own brute-force specification, never a second implementation.
- **Ladder B — algorithms and integration patterns** (`algorithms`): the same line-oriented CLI,
  one topic per classic algorithm. Not another server: these are the named algorithms as
  exercises. Raft (election, replication, the Figure 8 commitment rule, snapshots, membership
  change), single-decree and Multi-Paxos, two- and three-phase commit with coordinator-crash
  recovery, orchestrated and choreographed sagas with compensations, the transactional outbox,
  idempotent consumers and idempotency keys, distributed locks with fencing tokens, leader
  leases under clock skew, read repair, hinted handoff, gossip, circuit breakers, hedged
  requests, bulkheads and load shedding. Each stage's oracle is the tester's own model of the
  published rules, written out beside the tests that use it.
- **Ladder C — a durable node** (`node`): a server speaking a subset of the **etcd v3 HTTP/JSON
  API** — `kv/put`, `kv/range`, `kv/deleterange`, `kv/txn`, `kv/compaction`, leases, watches,
  `maintenance/status`. MVCC revisions, compare-and-swap transactions, lease expiry, watch
  delivery, and durability proved by `SIGKILL` mid-workload followed by restart.
- **Ladder D — a cluster** (`cluster`): 3- and 5-node clusters with a userspace TCP proxy in
  front of every peer link, so partitions, delays, drops, duplicates and reordering are injected
  without privileges. Leader election, replication, linearizable reads, minority-partition write
  refusal, failover with no acknowledged write lost, snapshot catch-up, membership change,
  whole-cluster restart — and a Wing-and-Gong-style **linearizability checker** run over
  randomized concurrent workloads with a seeded fault schedule, printing the minimal offending
  sub-history when it fails.

The etcd subset is what makes `--validate` possible: real etcd is the reference for the node and
cluster ladders, so the suite self-checks against a production consensus implementation. The two
CLI ladders are checked against this crate's own `reference_primitives` and
`reference_algorithms` examples, which exist only so the suite can prove itself — reading either
spoils its ladder.

The algorithms ladder is numbered 56–77 rather than 21–42: stages 1–55 were already cited by the
resource library and by work in flight, so it was appended rather than inserted. `--tag` and the
`ladder` field say where a stage belongs; its number never does.

### 5.7 Everything else becomes track-agnostic

- `byo`: `Track` stops being a two-value enum and becomes a registry entry (id, tester binary,
  target flag, data files, blurb, colour). `byo init <track>`, `byo test`, `byo status`,
  `byo doctor` and the API work for any registered track with no per-track branches left.
- `install.sh`: builds and installs all six testers plus `byo`, copies every track's data
  files and catalogs, sources the repo-root `.env`.
- `site/`: the track list comes from the catalogs on disk; `params/track.ts`, the home page,
  the nav, the map, resources, conventions and the lab all iterate the registry. Each track
  gets its own accent palette, garden mascot and conventions list.
- Resources: `resources.wasm.json`, `resources.tls.json`, `resources.link.json`,
  `resources.dist.json`, same schema, ≥ 30 verified links each (spec sections, RFCs, psABI, papers, reference implementations).

### 5.8 Execution

Phase 1 (parallel, one agent each): **W1** wasmtest · **T1** tlstest · **L1** linktest ·
**D1** disttest ·
**B2** byo track registry + install.sh · **S1** site N-track generalization on placeholder
catalogs. Phase 2: **R2** resources + conventions for the new tracks · per-track lab tools ·
catalog resync. Phase 3: verification — every `--validate` green, `cargo test`, clippy, site
build/check/test, a real browser walkthrough against `byo site`, delivery report.

Agent rules: work only inside your own directory; never edit the root `PLAN.md`; never run
`git`; never write runtime/server/linker implementation code for the learner; leave the repo
buildable at every step.

### 5.9 Delivered (2026-09-19)

| Track | Stages / tests | `--validate` against | Wall clock |
|---|---|---|---|
| `wasm` | 48 / 381 | wasmtime 48.0.2 | 20 s |
| `tls` | 48 / 343 | `openssl s_server` 3.6.4 | 75 s (1 documented skip) |
| `link` | 44 / 359 | GNU ld 2.47 | 5 s |
| `dist` primitives | 20 / 183 | `examples/reference_primitives` | 5 s |
| `dist` algorithms | 33 / 355 | `examples/reference_algorithms` | 0.7 s |
| `dist` node | 15 / 128 | etcd 3.7.1 | 108 s |
| `dist` cluster | 20 / 164 | etcd 3.7.1 | 859 s |

With shell (70/592) and kafka (47/315) that is **2 820 tests over six tracks**, every one of
them green against the real implementation. `byo` carries a six-entry registry (98 tests), the
site renders six trails from those catalogs (355 stage pages prerendered), and `install.sh`
builds and installs all of it.

Verified after the build, in a sandbox `BYO_HOME`: `install.sh --skip-site` → `byo tracks`
(six installed) → `byo init wasm --runtime wasmtime` → `byo test --until 3` (24/24, recorded as
run #1) → `byo status` (owner read from `.env`) → `byo site` + `/api/tracks` and `/api/runs`.

Two process-hygiene notes for whoever runs these next: the cluster ladder leaks etcd children
if its harness is killed mid-test rather than left to finish, so kill the tester and then sweep
`pgrep -f cache/disttest/etcd` and `/tmp/disttest-*`; and a cluster `--validate` takes ~15
minutes, which is long enough to look stalled when it is not.
