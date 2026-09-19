# BuildYourOwn

Six things worth building from scratch, each with a Rust tester that tells you whether you got
it right, and one site that shows you what to build next and how far you have come.

| Directory | You build | Checked against |
|---|---|---|
| [`shelltest/`](shelltest/README.md) | a POSIX **shell** — 70 stages, 592 tests | bash 5.3 |
| [`kafkatest/`](kafkatest/README.md) | a **Kafka broker** — 47 stages, 315 tests | Apache Kafka 4.1.2 (KRaft) |
| [`wasmtest/`](wasmtest/README.md) | a **WebAssembly runtime** — 48 stages, 381 tests | wasmtime 48.0.2 |
| [`tlstest/`](tlstest/README.md) | a **TLS 1.3 server** (RFC 8446) — 48 stages, 343 tests | `openssl s_server` 3.6 |
| [`linktest/`](linktest/README.md) | an **ELF64 linker** for x86-64 — 44 stages, 359 tests | GNU ld 2.47 |
| [`disttest/`](disttest/README.md) | a **distributed key/value store**, in four ladders — 88 stages, 830 tests | etcd 3.7.1 for the node and cluster ladders (35 stages); this repo's own reference CLI for the primitives and algorithms ladders (53 stages) — see below |
| [`site/`](site/README.md) | — | the journey site: a trail per track, what to build per stage, worked examples, resources, playgrounds |
| [`byo/`](byo/README.md) | — | the `byo` command: runs the right tester, keeps progress in SQLite, serves the site and its [JSON API](byo/API.md) |

**2 820 tests across six tracks.** You write the shell, the broker, the runtime, the server,
the linker and the cluster. Every tester is a black-box harness that drives your program
from the outside, and each is proved by pointing it at a reference and watching it come back
all green — in public, on every push, in [CI](.github/workflows/ci.yml).

**What "reference" means, exactly.** For five of the six tracks it is the real thing, which
this repo does not contain: bash, Apache Kafka, wasmtime, OpenSSL, GNU ld. The dist track is
split. Its node and cluster ladders (stages 21–55) run against real etcd. Its primitives and
algorithms ladders (stages 1–20 and 56–88) are line-oriented CLI exercises with no external
program to compare against, so this repo ships two of its own — `disttest/examples/
reference_primitives.rs` and `reference_algorithms.rs` — purely so those ladders can be
self-checked the same way. They are implementations, they are about 8 500 lines of the
answer, and reading either one spoils its ladder. Nothing else in the repo implements anything you are
asked to build.

**And a suite that passes everything proves nothing.** Each track also ships a program that
is wrong on purpose, and a test that asserts the suite goes red *at the stages that bug
touches* and stays green everywhere else — see any `tests/discrimination.rs`. That is the
check that separates 2 820 tests from 2 820 tests that discriminate.

## Quick start

```sh
cp .env.example .env     # optional: put your name in PUBLIC_JOURNEY_OWNER
./install.sh
```

That builds the six testers, the `byo` command and the site, installs the binaries into
`~/.local/bin`, and puts the suites, target registries, stage catalogs and the built site into
`~/.local/share/byo` with a SQLite progress database. It is idempotent — re-run it whenever you
pull. If `~/.local/bin` is not on your `PATH` it tells you what to add.

Then, in the repo where you are writing your own:

```sh
cd my-shell
byo init shell --command ./your_program.sh   # or: byo init shell --shell bash, to try it out
byo test --stage 1
byo status
byo site                                     # opens the journey map in your browser
```

and the same shape for every other track:

```sh
byo init kafka --command ./your_program.sh    # --broker  for the reference
byo init wasm  --command ./your_program.sh    # --runtime
byo init tls   --command ./your_program.sh    # --server
byo init link  --command ./your_program.sh    # --linker
byo init dist  --command ./your_program.sh    # --target
```

`byo tracks` lists them. `byo test` passes every flag straight through to the tester
(`--stage N`, `--until N`, `--all`, `--only`, `--tag`, `--verbose`, `--keep-tmp`, …), streams
its output unchanged, exits with its exit code, and records the run so `byo status` and the
site can show it.

To remove everything: `./uninstall.sh` (add `--purge` to delete your progress too).

## Seeing a tester prove itself

Every track's suite is green against the real implementation, which is what makes a red mark
on your own program mean something:

```sh
shelltest --shell bash            --validate --all
kafkatest --broker apache_kafka   --validate --all
wasmtest  --runtime wasmtime      --validate --all
tlstest   --server openssl        --validate --all
linktest  --linker gnu_ld         --validate --all
disttest  --target etcd           --validate --all
```

## Requirements

**Rust 1.88 or newer.** The dependency tree uses edition 2024, which needs 1.85 just to
parse, and `time` raises the floor to 1.88 — so a distribution cargo (Ubuntu 24.04 ships
1.75) fails to resolve the lockfile with an error that points at a dependency rather than at
your toolchain. `rust-toolchain.toml` pins it, so `rustup` installs the right one for you.
The floor is checked in CI rather than asserted here.

Node/npm to build the site. Per track: Java 17+ for kafka's reference broker, OpenSSL 3.x for
tls, binutils for link's optional interop checks, nothing extra for shell, wasm or dist. The
wasm, kafka and dist references download themselves into `~/.cache/<tester>` on first use.
`byo doctor` checks all of it and says what is missing.

## Plan

[`PLAN.md`](PLAN.md) is short and current: every track's numbers with the command that
reproduces each one, the discrimination check, the toolchain floor and the known gaps. The
original 40 KB build plan is kept as [`docs/build-log.md`](docs/build-log.md) — it is the
record of what was intended, not a description of what is there.
