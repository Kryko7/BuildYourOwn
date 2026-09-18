# BuildYourOwn

Everything needed to build your own **POSIX shell** and your own **Kafka broker** from
scratch, with a tester telling you whether each stage is right and a map showing how far you
have come.

| Directory | What it is |
|---|---|
| [`shelltest/`](shelltest/README.md) | Black-box tester for a POSIX shell — 57 stages, 428 tests, validated against bash |
| [`kafkatest/`](kafkatest/README.md) | Black-box conformance tester for a Kafka broker — ~45 stages, validated against Apache Kafka 4.1 (KRaft) |
| [`site/`](site/README.md) | The journey site — a SvelteKit journey map for both tracks: what to build per stage, resources, playgrounds, live red/green |
| [`byo/`](byo/README.md) | The `byo` command: runs the right tester, keeps progress in SQLite, serves the site and its [JSON API](byo/API.md) |

You write the shell and the broker. Nothing in this repo is an implementation of either.

## Quick start

```sh
./install.sh
```

That builds the two testers, the `byo` command and the site, installs
`shelltest`, `kafkatest` and `byo` into `~/.local/bin`, and puts the test suites, broker
registry, stage catalogs and the built site into `~/.local/share/byo` together with a SQLite
progress database. It is idempotent — re-run it whenever you pull. If `~/.local/bin` is not on
your `PATH` it tells you what to add.

Then, in the repo where you are writing your shell:

```sh
cd my-shell
byo init shell --command ./your_program.sh    # or: byo init shell --shell bash, to try it out
byo test --stage 1
byo status
byo site                                      # opens the journey map in your browser
```

and for the Kafka track:

```sh
cd my-broker
byo init kafka --command ./your_program.sh
byo test --until 5
```

`byo test` passes every flag straight through to the tester (`--stage N`, `--until N`,
`--all`, `--only`, `--verbose`, `--keep-tmp`, …), streams its output unchanged, exits with its
exit code, and records the run so `byo status` and the site can show it.

To remove everything: `./uninstall.sh` (add `--purge` to delete your progress too).

## Requirements

Rust (cargo) for the testers and `byo`; Node/npm for the site (only needed to build it, or to
run `byo site --rebuild`); Java 17+ for `kafkatest`'s reference broker, which downloads Apache
Kafka into `~/.cache/kafkatest` on first use. `byo doctor` checks all of this.

## Plan

[`PLAN.md`](PLAN.md) is the master plan: what each tester covers, the site's design, the `byo`
CLI and database, and how the work was split.
