# The journey site

A garden trail through six build-your-own tracks: a POSIX **shell**, a **Kafka broker**, a
**WebAssembly runtime**, a **TLS 1.3 server**, an **ELF linker** and a **distributed store**.
Every stage shows what to build, **what to expect**, the tests it has to satisfy, the exact command
to run, curated reading, and live red/green from `byo`'s database.

**There is one list of tracks** (`src/lib/tracks.ts`) and everything reads it: the route matcher,
the home page, the nav, the command palette, the map, the conventions, `/resources`, `/progress`,
the lab and `npm run sync`. Adding a seventh track is an entry in that file plus its accent tokens
in `app.css` and a mascot in `src/lib/components/garden/` — no page needs editing.

SvelteKit 2 + Svelte 5 runes, TypeScript strict, `adapter-static` (fully prerendered), hand-written
CSS with design tokens. No component libraries, no external assets — every animal, flower and petal
on the page is inline SVG drawn here.

## Commands

```
npm install
npm run sync      # regenerate the catalogs from the testers (also runs before build)
npm run dev       # dev server, with the fallback report polling
npm run check     # svelte-check, 0 errors
npm run test      # vitest: registry, parsers, API client, progress store, examples,
                  # the lab decoders, component mounts
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
- **no project** — byo is up but `byo init <track>` has never been run here, with the command to
  fix it.

There is **no report upload**: a report reaches the site by being recorded with `byo test`, which is
the same thing that moves the map.

**A byo older than this site is fine.** `/api/progress` and `/api/health` are read per registered
track and a track the server has never heard of simply comes back empty, so a two-track `byo` still
drives a six-track page. `GET /api/tracks` (id, title, blurb, accent, installed, testerVersion) is
consumed when the server offers it and is `null` otherwise — it is not in `byo/API.md` yet, so
nothing on the site depends on it; the built-in registry is the fallback and the display source.

Code: `src/lib/api/client.ts` (typed client over an injectable `fetch`),
`src/lib/stores/journey.svelte.ts` (connection, polling, optimistic writes),
`src/lib/stores/progress.svelte.ts` (one façade over both sources).

## The track registry

`src/lib/tracks.ts` carries, per track: the title and blurb, what you are building and what
`--validate` is checked against, the accent (`--wasm` and friends, AA in both themes — there is a
test), the garden mascot and the section badges, the tester's name, binary, target flag and target
file, which example renderer its catalog wants, and — for `dist` — its **ladders**.

It is deliberately import-free plain data, because `scripts/sync-catalog.mjs` imports it directly
(Node 26 strips the types), so the site and the sync script cannot disagree about what a track is.
Mascot names are resolved to components in `src/lib/components/garden/mascots.ts`.

**Ladders.** A track may group its sections into rungs; `dist` climbs `primitives` → `node` →
`cluster`. The track page draws a rung per ladder with its own progress, each camp says which rung
it is on, and every stage carries `ladder` in the catalog. A track without ladders renders none of
this and nothing changes for it.

## Where the stage data comes from

`scripts/sync-catalog.mjs` is the only thing that writes `src/lib/data/catalog.*.json`. It loops
the registry; for each track it takes the first of these that exists:

1. **`../<tester>/catalog.json`** — the real catalog, normalized by `scripts/normalize-catalog.mjs`
   (accepts `stage`/`number`, `skip_on`/`skipOn`, `ladder`/`tier`, upper-cases the section letters,
   and copies each stage's `examples` array through verbatim — the sync then splits those out into
   `src/lib/data/examples/<track>/`, see below). Any holes are filled from the tester's PLAN.md as
   `planned: true` stages, so every `/<track>/N` prerenders and a planned stage says so instead of
   showing an empty test list. Shipped stages always win.
2. **`../<tester>/PLAN.md`** — the real stage list before the tester emits a catalog, every stage
   `planned: true` and the catalog `pending: true`.
3. **the built-in placeholder** (`scripts/placeholders.mjs`) — one waypoint per section, named after
   the section, carrying what that section will cover. Nothing invents a stage list: a tester that
   does not exist yet gets a trail that is honest about being a placeholder, and a map that is not
   empty. `npm run sync` throws all of it away the moment the tester lands.

The shell track is the exception only in where its data lives: `../shelltest/PLAN.md` plus every
`../shelltest/tests/stages/*.yaml`, which carry the tests, their inputs and their expectations.

Resources live in `src/lib/data/resources.<track>.json` (schema in PLAN.md §2.5), loaded through a
tolerant sanitizer. **An empty file is normal** — a track whose reading list has not been curated
yet simply has no resources, `/resources` says which ones those are, and the stage drawer omits the
section rather than drawing an empty box.

## Worked examples ("What to expect", PLAN.md §4.2, §5.2)

Every stage drawer and stage page shows two or three examples under *What to build*. Which of the
two renderers a track uses is one field in the registry (`examples: 'transcript' | 'bytes'`).

- **Shell** — derived from the catalog's own tests (`src/lib/examples/shell.ts`). No new data: the
  suite already records the stdin lines, the pty key steps and the expectations, so the shortest,
  most literal tests are rendered as a terminal transcript — `$ ` prompts, stdout, stderr in red,
  exit status. A loose expectation (`contains`, `regex`) is labelled as a pattern instead of being
  quoted as literal output, and a trailing `exit 0` is dropped as plumbing.
- **Everything else** — read from the `examples` array the tester writes into `catalog.json`
  (`src/lib/examples/bytes.ts`) and drawn by one component (`ByteExample.svelte`): **blocks** of
  annotated bytes, plus an optional transcript of what running it prints. Hover a field to light its
  bytes and a byte to find its field.

  **A new track needs no new component.** The reader is generic: any `<name>_hex` key becomes a
  labelled block, with `<name>` as its summary and `<name>_fields` as its annotations. So Kafka's
  `request`/`response` pair, wasm's module and its stdout, a TLS handshake message and an ELF
  structure are all the same shape. An explicit `blocks: [{key, label, hex, fields}]` array works
  too, `<name>_label` overrides a label, and `kind` retitles the conventional names — wasmtest sends
  `kind: "module"`, so its blocks read "the module" and "it prints" rather than "request" and
  "response". A transcript comes either from `transcript` or from plain
  `command`/`stdout`/`stderr`/`exit_code` keys.

  **They are not in the bundle.** kafkatest's 91 examples are most of its 800 KB catalog, and a
  single annotated frame can be a kilobyte of hex. `npm run sync` splits them into
  `src/lib/data/examples/<track>/<n>.json`, one file per stage, and the catalog keeps only
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
an owl for WebAssembly, a hedgehog for TLS, a squirrel for the linker, a duckling for the
distributed store, a cat on the home hero, a bird that lands on the nav, butterflies and a bee
drifting over the hero. Each one takes the same props (`size`, `mood`, `flip`, `label`), which is
what lets the map, the home cards and the runs page pick one from the registry.
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
src/lib/data/examples/<track>/<n>.json one lazy chunk of worked examples per stage
src/lib/tracks.ts the track registry — every page and the sync script read it
src/lib/examples/ shell transcript derivation, the tolerant byte-example reader + lazy loader
src/lib/lab/      shell tokenizer; Kafka wire codec (varints, compact types, CRC32C) and
                  RecordBatch v2 encoder; a wasm decoder + disassembler (LEB128, sections,
                  opcodes); a TLS record/handshake parser + the key schedule; an ELF64
                  reader — each with its own sample bytes, encoded by the same file
src/lib/motion.ts reduced-motion, tab visibility, seeded randomness, the rAF loop
src/lib/stores/   journey (the API), progress (the façade), reports (fallback), theme
src/lib/components/garden/    Fox · Bunny · Owl · Hedgehog · Squirrel · Duckling · Cat ·
                              Bird · Flutterers · Bloom · Petals · mascots.ts
src/lib/components/examples/  Transcript · ShellTranscript · ByteExample · StageExamples
src/lib/components/lab/       HexFields (the shared hex+fields instrument) · the seven
                              playgrounds · tools.ts (which one belongs to which stage)
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

## The lab

Seven instruments, listed once in `src/lib/components/lab/tools.ts` and read by both `/lab` (the tab
strip) and the stage drawer's *Try it* box — `toolForStage(track, stage)` picks one from the
**section**, not the stage number, so a tester renumbering its stages cannot silently change which
instrument appears.

| Instrument | What it does |
|---|---|
| Shell tokenizer | the quote state machine, character by character, with the pipeline graph |
| Kafka wire inspector | four real requests, byte by byte, varints and compact types spelled out |
| RecordBatch anatomy | build a v2 batch and watch the bytes, with a live CRC32C |
| Wasm module inspector | section tree, every LEB128 decoded, bodies disassembled to instruction names |
| TLS record inspector | record framing, handshake messages, extensions by name, and the RFC 8446 key schedule with the exact `HkdfLabel` bytes |
| ELF viewer | header, sections, symbols and relocations with the formula each one applies |
| Journey replay | step through a failing test from the latest run |

All four new ones share `HexFields.svelte` — a hex dump wired both ways to a field list — and all of
them decode in the page: no network, nothing installed, and every sample's bytes are built by the
same module that reads them, so the decoders are exercised against bytes the site can also print.

## Runs (`/progress`)

The history side of the database: the four `byo` commands with copy buttons, the latest run per
track as a per-stage bar chart, and a run history table. Opening a row fetches
`GET /api/runs/:id` and shows every stage, every test, and the terminal-style failure blocks for
the red ones. With no API the page says so and offers only the local state export/import.
