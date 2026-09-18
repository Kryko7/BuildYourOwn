# `byo site` HTTP API

`byo site` serves two things on one port (default **4321**, the next free port if that is
taken — the actual URL is printed on startup):

| Prefix | What |
|---|---|
| `/api/*` | the JSON API below |
| everything else | the prerendered site from `$BYO_HOME/site` |

Same origin, so **no CORS headers are sent and none are needed**. Every response is
`application/json; charset=utf-8` with `Cache-Control: no-store`. Errors are
`{"error": "human readable sentence"}` with a non-2xx status.

The site should **probe `GET /api/health` on load**. If it answers, the API is the source of
truth for progress, notes and reports; if it does not (static hosting, `npm run dev`), fall
back to `localStorage` exactly as before.

Base URL in the examples: `http://127.0.0.1:4321`.

---

## `GET /api/health`

Liveness plus what the CLI knows about the user's projects.

```jsonc
{
  "ok": true,
  "version": "0.1.0",              // the `byo` binary's version
  "db": "/home/you/.local/share/byo/byo.db",
  "dataDir": "/home/you/.local/share/byo",
  "schemaVersion": 1,
  "tracks": {
    "shell": {
      "project": {                 // null when `byo init shell` has never been run
        "id": 1,
        "track": "shell",
        "path": "/home/you/code/my-shell",
        "command": "bash",         // the --shell / --broker value
        "targetKind": "registered", // "registered" (a name in shells.yaml) | "command" (a path)
        "createdAt": "2026-09-13T02:04:14Z"
      }
    },
    "kafka": { "project": null }
  }
}
```

**Status:** always `200` when the server is up.

---

## `GET /api/progress`

Everything the journey map needs to colour itself, for both tracks at once.

```jsonc
{
  "shell": {
    "latestRunId": 1,              // null when the track has no runs
    "stages": {                    // keyed by stage number, as a STRING
      "1": {
        "state": "done",           // "todo" | "in_progress" | "failed" | "done"
        "doneAt": "2026-09-13T02:04:19Z",  // null unless state is "done"
        "note": "hi",              // null when there is no note
        "lastRunId": 1,            // null when no run has touched the stage
        "updatedAt": "2026-09-13T02:06:01Z"
      }
    }
  },
  "kafka": { "latestRunId": null, "stages": {} },
  "streak": 1,                     // consecutive days a stage went green (today or yesterday counts)
  "xp": 300,
  "level": 1
}
```

Notes for the site:

- **Stages missing from `stages` are `todo`.** The object only holds stages the database has
  heard about, so a fresh install returns `{}` — render every catalog stage as `todo`.
- State meanings: `done` = the latest run passed every test it ran in that stage (or the user
  said `byo done N` / posted `state: "done"`); `in_progress` = the latest run had both passes
  and failures; `failed` = the latest run had no passes; `todo` = never run, or explicitly
  un-done.
- `xp` = 100 per `done` stage + 20 per `in_progress`/`failed` stage, across both tracks.
  `level` = `xp / 500 + 1`. `streak` counts distinct `doneAt` calendar days (UTC) backwards
  from today; a streak whose most recent day is yesterday still counts.

---

## `GET /api/runs?track=&limit=`

Run history, newest first. Both parameters are optional.

| Parameter | Values | Default |
|---|---|---|
| `track` | `shell` \| `kafka` | both tracks |
| `limit` | 1–1000 | 20 |

```jsonc
[
  {
    "id": 1,
    "track": "shell",
    "target": "bash",              // the shell/broker under test
    "startedAt": "2026-09-13T02:04:19Z",
    "elapsedMs": 3408,
    "passed": 20,
    "failed": 0,
    "skipped": 0,
    "args": "--until 3"            // the flags the user gave `byo test` ("" when none)
  }
]
```

**Status:** `200`, or `400` for an unknown `track` or an out-of-range `limit`.

> Deviation from the plan: the objects also carry `args`, which `/progress` can show as
> "how this run was produced".

---

## `GET /api/runs/:id`

One run, rebuilt from the database into **exactly the testers' `--json` shape** — the same
document `shelltest --json report.json` writes, so the site's existing report importer can
consume it unchanged.

```jsonc
{
  "id": 1,                         // ADDED by the API; not present in the tester's file
  "target": "bash",                // `shelltest` writes this as "shell"; the API always uses "target"
  "validate": false,
  "passed": 20, "failed": 0, "skipped": 0,
  "elapsed_ms": 3408,
  "stages": [
    {
      "stage": 1,
      "name": "Print the prompt and wait for input",
      "file": "01_prompt.yaml",
      "passed": 6, "failed": 0, "skipped": 0,
      "tests": [
        {
          "name": "prints the prompt on startup",
          "status": "pass",        // "pass" | "fail" | "skip"
          "ext": false,
          "duration_ms": 411,
          "failures": [],          // one string per failed check
          "skip_reason": null,
          "actual": [["terminal", "$ "], ["exit code", "0"]],   // [label, value] pairs
          "failure_kind": null     // kafkatest only
        }
      ]
    }
  ]
}
```

Field names inside this document stay `snake_case` (they are the testers'), unlike the rest
of the API which is `camelCase`.

**Status:** `200`; `400` when `:id` is not a number; `404` when there is no such run.

---

## `GET /api/runs/latest?track=`

The newest run, in the same shape as `/api/runs/:id`. `track` is optional (without it you get
the newest run of either track).

**Status:** `200`; `404` (`{"error": "no runs recorded yet — run `byo test` first"}`) when the
track has never been run; `400` for an unknown track.

---

## `POST /api/stages/:track/:n`

Mark a stage done/undone and/or set its note. `:track` is `shell` or `kafka`, `:n` is the
stage number.

Request body (`application/json`, both fields optional, an empty body is a plain read):

```jsonc
{ "state": "done", "note": "watch the CRC" }
```

- `state`: `"done"` or `"todo"` — anything else is `400`.
- `note`: a string, or `""`/`null` to clear it — anything else is `400`.
- Setting `state` never clears the note, and setting the note never changes the state.
- A later `byo test` run **re-derives** the state from its results, so a manual `done` on a
  stage that then fails becomes `failed`. The note survives.

Response — the stage row after the write:

```jsonc
{
  "track": "shell",
  "stage": 4,
  "state": "done",
  "doneAt": "2026-09-13T02:06:34Z",
  "note": "from the API",
  "lastRunId": null,
  "updatedAt": "2026-09-13T02:06:34Z"
}
```

**Status:** `200`; `400` for a bad stage number or body; `404` for an unknown track;
`405` when the same path is fetched with `GET`.

---

## `GET /api/catalog/:track`

The stage catalog from the data directory, served verbatim so the site can run against newer
testers than it was built with. `:track` is `shell` or `kafka`.

- shell → `$BYO_HOME/catalog.shell.json` (copied from `site/src/lib/data/catalog.shell.json`
  at install time)
- kafka → `$BYO_HOME/catalog.kafka.json` (copied from `kafkatest/catalog.json`)

Shape: whatever the generators write — `{ track, generatedAt, sections: [{id, title,
stages:[n]}], stages: [{number, slug, name, ext, file, hints, tests, …}] }`.

**Status:** `200`; `404` when the file is absent (a warning `byo doctor` also prints) or the
track is unknown. **Treat a 404 here as "use the catalog bundled into the build"** — it is not
an error worth showing the user.

---

## Everything else: the static site

Any path that does not start with `/api/` is served from `$BYO_HOME/site`:

- `/` → `index.html`
- `/shell` → `301` to `/shell/`
- `/shell/` → `shell/index.html`
- `/shell/12/` → `shell/12/index.html`
- `/about` → `about.html` when there is no `about/` directory
- anything unmatched → the nearest `index.html` at or above the path (SPA-safe), else `404`
- `..` never escapes the root
- `/_app/immutable/*` is sent with `Cache-Control: public, max-age=31536000, immutable`;
  everything else with `no-cache`

When `$BYO_HOME/site` has no build, **every** non-API path returns a placeholder page telling
the user to run `byo site --rebuild`, and the API keeps working.

---

## Quick recipe for the site

```ts
const api = {
  async detect(): Promise<boolean> {
    try {
      const r = await fetch('/api/health', { cache: 'no-store' });
      return r.ok && (await r.json()).ok === true;
    } catch { return false; }
  },
  progress: () => fetch('/api/progress').then(r => r.json()),
  runs: (track?: 'shell' | 'kafka', limit = 20) =>
    fetch(`/api/runs?${new URLSearchParams({ ...(track && { track }), limit: String(limit) })}`)
      .then(r => r.json()),
  latest: (track: 'shell' | 'kafka') =>
    fetch(`/api/runs/latest?track=${track}`).then(r => (r.ok ? r.json() : null)),
  setStage: (track: 'shell' | 'kafka', n: number, body: { state?: 'done' | 'todo'; note?: string }) =>
    fetch(`/api/stages/${track}/${n}`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
    }).then(r => r.json()),
  catalog: (track: 'shell' | 'kafka') =>
    fetch(`/api/catalog/${track}`).then(r => (r.ok ? r.json() : null)),
};
```

Poll `/api/progress` (and `/api/runs/latest?track=…`) every couple of seconds while the page is
visible to get the same live red/green the dev-mode `/__reports/*.json` plugin gives you — a
`byo test` in another terminal shows up within one poll.
