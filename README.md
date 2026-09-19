# BuildYourOwn

Six things worth building from scratch, each with a Rust tester that tells you whether you got
it right, and one site that shows you what to build next and how far you have come.

| Directory | You build | Checked against |
|---|---|---|
| [`shelltest/`](shelltest/README.md) | a POSIX **shell** — 70 stages, 592 tests | bash 5.3 |
| [`kafkatest/`](kafkatest/README.md) | a **Kafka broker** — 46 stages, 306 tests | Apache Kafka 4.1.2 (KRaft) |
| [`wasmtest/`](wasmtest/README.md) | a **WebAssembly runtime** — 48 stages, 381 tests | wasmtime 48.0.2 |
| [`tlstest/`](tlstest/README.md) | a **TLS 1.3 server** (RFC 8446) — 48 stages, 343 tests | `openssl s_server` 3.6 |
| [`linktest/`](linktest/README.md) | an **ELF64 linker** for x86-64 — 44 stages, 359 tests | GNU ld 2.47 |
| [`disttest/`](disttest/README.md) | a **distributed key/value store**, in four ladders — 88 stages, 830 tests | etcd 3.7.1 |
| [`site/`](site/README.md) | — | the journey site: a trail per track, what to build per stage, worked examples, resources, playgrounds |
| [`byo/`](byo/README.md) | — | the `byo` command: runs the right tester, keeps progress in SQLite, serves the site and its [JSON API](byo/API.md) |

**2 811 tests across six tracks.** You write the shell, the broker, the runtime, the server,
the linker and the cluster. Nothing in this repo is an implementation of any of them — every
tester is a black box harness, and every one of them is proved by pointing it at the real
thing and watching it come back all green.

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

Rust (cargo) for the testers and `byo`; Node/npm to build the site. Per track: Java 17+ for
kafka's reference broker, OpenSSL 3.x for tls, binutils for link's optional interop checks,
nothing extra for shell, wasm or dist. The wasm, kafka and dist references download themselves
into `~/.cache/<tester>` on first use. `byo doctor` checks all of it and says what is missing.

## Plan

[`PLAN.md`](PLAN.md) is the master plan: what each tester covers, the site's design, the `byo`
CLI and database, and how the work was split.
