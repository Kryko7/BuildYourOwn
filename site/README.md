# The journey site

A garden trail through two build-your-own tracks: a POSIX **shell** and a **Kafka broker**.
Every stage shows what to build, **what to expect**, the tests it has to satisfy, the exact command
to run, curated reading, and live red/green from `byo`'s database.

SvelteKit 2 + Svelte 5 runes, TypeScript strict, `adapter-static` (fully prerendered), hand-written
CSS with design tokens. No component libraries, no external assets — every animal, flower and petal
on the page is inline SVG drawn here.

## Commands

```
npm install
npm run sync      # regenerate the catalogs from the testers (also runs before build)
npm run dev       # dev server, with the fallback report polling
npm run check     # svelte-check, 0 errors
npm run test      # vitest: parsers, API client, progress store, examples, component mounts
npm run build     # static site in build/
npm run preview   # serve build/
```

The site is meant to be served by **`byo site`**, which puts this build and the JSON API on one
origin (`byo/API.md`). `npm run dev` and `npm run preview` still work — they just have no database
behind them, and the page says so.

## Where progress comes from

**The database, not this browser** (PLAN.md §4.5). On load the site probes `GET /api/health`:

| | connected to `byo site` | no API (static hosting, `npm run dev`) |
|---|---|---|
| stage states | `GET /api/progress` — `todo` / `in_progress` / `failed` / `done` | localStorage |
| notes | `POST /api/stages/:track/:n` (debounced) | localStorage |
| mark done / undone | `POST /api/stages/:track/:n`, optimistic with rollback | localStorage |
| XP · level · streak | the API's numbers | computed locally |
| the colouring report | `GET /api/runs/latest?track=` | `/__reports/*.json` in dev |
| run history | `GET /api/runs`, `GET /api/runs/:id` | nothing to show |

While the tab is visible the site polls `/api/progress` every **2 s** and refetches the latest run
whenever its id changes — so a `byo test` in another terminal turns flowers green without a reload.
Polling stops while the tab is hidden.

Three states are spelled out to the visitor by `ConnectionNote`:

- **offline** — "Not connected to byo. Run `byo site` to see your real progress." The localStorage
  fallback is clearly labelled as a copy that `byo status` will never see.
- **reconnecting** — byo answered before and has stopped; the last data stays on screen rather than
  the page blanking.
- **no project** — byo is up but `byo init shell` / `byo init kafka` has never been run, with the
  command to fix it.

There is **no report upload**: a report reaches the site by being recorded with `byo test`, which is
the same thing that moves the map.

Code: `src/lib/api/client.ts` (typed client over an injectable `fetch`),
`src/lib/stores/journey.svelte.ts` (connection, polling, optimistic writes),
`src/lib/stores/progress.svelte.ts` (one façade over both sources).

## Where the stage data comes from

`scripts/sync-catalog.mjs` is the only thing that writes `src/lib/data/catalog.*.json`:

- **shell** — parses `../shelltest/PLAN.md` (stage number, name, `[ext]` flag, hint bullets, yaml
  file, test count) and every `../shelltest/tests/stages/*.yaml` (test names, tags, `skip_on`, mode,
  inputs and expectations). 57 stages, 428 tests.
- **kafka** — prefers `../kafkatest/catalog.json` and normalizes it
  (`scripts/normalize-kafka.mjs` accepts `stage`/`number` and `skip_on`/`skipOn`, upper-cases the
  section letters, and copies each stage's `examples` array through verbatim — the sync then
  splits those out into `src/lib/data/examples/kafka/`, see below). Until that file
  exists it generates a placeholder from `BuildYourOwn/PLAN.md` §1.5 (`scripts/kafka-placeholder.mjs`)
  and marks the catalog `pending: true`.

  **Gaps are normal.** `scripts/parse-kafka-plan.mjs` reads `../kafkatest/PLAN.md` and
  `mergePlannedStages` fills any holes with `planned: true` stages, so every `/kafka/N` prerenders
  and a planned stage says so instead of showing an empty test list. Shipped stages always win.

Resources live in `src/lib/data/resources.{shell,kafka}.json` (schema in PLAN.md §2.5), loaded
through a tolerant sanitizer.

## Worked examples ("What to expect", PLAN.md §4.2)

Every stage drawer and stage page shows two or three examples under *What to build*.

- **Shell** — derived from the catalog's own tests (`src/lib/examples/shell.ts`). No new data: the
  suite already records the stdin lines, the pty key steps and the expectations, so the shortest,
  most literal tests are rendered as a terminal transcript — `$ ` prompts, stdout, stderr in red,
  exit status. A loose expectation (`contains`, `regex`) is labelled as a pattern instead of being
  quoted as literal output, and a trailing `exit 0` is dropped as plumbing.
- **Kafka** — read from the `examples` array kafkatest writes into `catalog.json`
  (`src/lib/examples/kafka.ts`), rendered with the wire inspector's annotated byte view: request
  and response side by side, hover a field to light its bytes and a byte to find its field.

  **They are not in the bundle.** kafkatest's 91 examples are most of its 800 KB catalog, and a
  single annotated frame can be a kilobyte of hex. `npm run sync` splits them into
  `src/lib/data/examples/kafka/<n>.json`, one file per stage, and `catalog.kafka.json` keeps only
  `exampleCount` — which is all the UI needs to decide whether to draw the section. Each file is
  its own chunk (`import.meta.glob`), so opening one Kafka stage fetches that stage's frames and
  nothing else; the deep-linked page's `load` pulls it in during prerender, so `/kafka/13` is
  server-rendered with no flash, while the drawer fetches it on open. The shared client chunk went
  from 568 KB to 165 KB this way.

  A dump stops at **256 bytes** with a "show all *n* bytes" button (and opens on its own if you
  ask for a field further in) — nobody reads 800 bytes of hex in a drawer, and it is one DOM node
  per byte.

  Alongside the frames the generator sends three things the renderer uses: `kind`
  (`wire` · `text` · `closed` · `silence` — a stage where the lesson is that *nothing* goes on the
  wire says so instead of showing an empty dump), `env` (the topics and group the exchange was
  captured against, shown as a "set up with" line so a fixture name is never mistaken for a
  protocol constant), and `varies` on a field whose value differs every run — a topic id, a
  timestamp, a CRC — which is dotted-underlined and, when selected, says "varies per run — match
  the shape, not this value" rather than being presented as a number to copy.

  The reader is deliberately tolerant, because that array is generated by a crate that is still
  moving: a missing array means no examples, an unknown key is ignored, a missing `kind` is
  inferred from whether there are bytes, an empty `request_hex` means the side is prose rather
  than a frame, and a field range that runs past the end of the bytes is clamped.
  `src/lib/data/fixtures/kafka-examples.sample.json` is a hand-written example in that exact
  schema; a test checks its ApiVersions v4 request is byte-identical to the one this site's own
  codec builds, so the reader is exercised even for a shape the generator has not sent yet.

## The garden

Light-first pastel palette (pink, lavender, mint, butter, sky, peach on warm cream) with a plum
dusk dark mode; every text colour is AA against the surface it sits on. Sections of the trail are
meadows and glades, and a stage is a flower: a closed bud when it is waiting, a glowing bud when it
is the one you are up to, half open while it is in progress, a full bloom when it goes green, and a
red poppy when the tester is red on it (`src/lib/components/garden/Bloom.svelte`). The walked part
of the path is petal-strewn stepping stones; the rest is pale.

The animals are hand-drawn inline SVG with CSS animation — a fox for the shell, a bunny for Kafka,
a cat on the home hero, a bird that lands on the nav, butterflies and a bee drifting over the hero.
The track's mascot stands at the flower you are up to and walks when it moves; it hops when a stage
blooms, and the celebration is a burst of petals.

Rules every animation follows: **transform and opacity only**, nothing at all under
`prefers-reduced-motion`, and paused while the tab is hidden (`is-hidden-tab` on `<html>`, set by
`src/lib/motion.ts`). Scattered things — butterflies, grass tufts, fallen petals — are seeded, so
they land in the same places on every render.

## Layout

```
scripts/          sync-catalog.mjs + the pure parsers it uses, and their vitest suite
src/lib/api/      the byo JSON API client
src/lib/data/     generated catalogs, resource files, report and example fixtures
src/lib/data/examples/kafka/<n>.json   one lazy chunk of worked examples per Kafka stage
src/lib/examples/ shell transcript derivation, tolerant kafka example reader + lazy loader
src/lib/lab/      shell tokenizer, Kafka wire codec (varints, compact types, CRC32C),
                  RecordBatch v2 encoder, the four sample requests
src/lib/motion.ts reduced-motion, tab visibility, seeded randomness, the rAF loop
src/lib/stores/   journey (the API), progress (the façade), reports (fallback), theme
src/lib/components/garden/    Fox · Bunny · Cat · Bird · Flutterers · Bloom · Petals
src/lib/components/examples/  ShellTranscript · WireExample · StageExamples
src/routes/       / · /[track] · /[track]/[stage] · /resources · /lab · /progress (Runs)
```

## The trail map

The SVG viewBox is sized so that **one meadow row spans the viewport at zoom 1**, and that is the
default view, scrolled to the stage you are up to — not a fit-the-whole-trail zoom that renders 57
flowers at six pixels each. `fit` is a button next to `+`, `−` and `next`. Meadow labels and hover
labels are sized in screen pixels so they stay legible at any zoom, and every waypoint carries an
invisible 52-unit hit circle (≥ 40 css px across).

Keyboard: `Tab` puts you on one waypoint (a roving tabindex — the one you are up to, or the selected
one), `←` `→` `↑` `↓` walk the trail, `PageUp`/`PageDown` jump a row, `Home`/`End` go to the ends,
`+`/`−` zoom, and `Enter` or `Space` opens the stage drawer. The view pans to follow. A plain wheel
pans and hands the gesture back to the page at the ends; `Ctrl`+wheel zooms.

## Runs (`/progress`)

The history side of the database: the four `byo` commands with copy buttons, the latest run per
track as a per-stage bar chart, and a run history table. Opening a row fetches
`GET /api/runs/:id` and shows every stage, every test, and the terminal-style failure blocks for
the red ones. With no API the page says so and offers only the local state export/import.
