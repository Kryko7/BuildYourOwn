# BuildYourOwn — what exists, and how to check it

This file used to be a 40 KB forward-looking build plan. Almost all of that work is done, so
the plan itself now lives in [`docs/build-log.md`](docs/build-log.md) as a record of what was
intended and why. What is left here is the part that is still true.

**Nothing below should be taken on trust.** Every number is produced by a command you can
run, and every command runs in [CI](.github/workflows/ci.yml) on each push.

## The six tracks

| Track | Stages / tests | Reference | Reproduce | Wall clock |
|---|---|---|---|---|
| `shell` | 70 / 592 | bash 5.3 | `target/release/shelltest --shell bash --validate --all` | 23 s, 1 documented skip |
| `kafka` | 47 / 315 | Apache Kafka 4.1.2 (KRaft) | `target/release/kafkatest --broker apache_kafka --validate --all` | 215 s |
| `wasm` | 48 / 381 | wasmtime 48.0.2 | `target/release/wasmtest --runtime wasmtime --validate --all` | 11 s |
| `tls` | 48 / 343 | `openssl s_server` 3.6 | `target/release/tlstest --server openssl --validate --all` | 74 s, 1 documented skip |
| `link` | 44 / 359 | GNU ld 2.47 | `target/release/linktest --linker gnu_ld --validate --all` | 4 s |
| `dist` primitives | 20 / 183 | this repo's `reference_primitives` | `target/release/disttest --target reference_primitives --validate --until 20` | 6 s |
| `dist` algorithms | 33 / 355 | this repo's `reference_algorithms` | `target/release/disttest --target reference_algorithms --validate --from 56` | 1 s |
| `dist` node | 15 / 128 | etcd 3.7.1 | `target/release/disttest --target etcd --validate --from 21 --until 35` | 109 s |
| `dist` cluster | 20 / 164 | etcd 3.7.1 | `target/release/disttest --target etcd --validate --from 36 --until 55` | 864 s |

**2 820 tests.** Two skips, both with a printed reason. The two `dist` ladders whose
reference is a program in this repo are explained in the [README](README.md#) — they are the
only place the repo ships an implementation of something it asks you to build.

## The check that makes the rest mean anything

A suite that passes everything proves nothing. Each track ships a program that is wrong on
purpose, and a test asserting the suite goes red **at the stages that bug touches** and stays
green everywhere else:

```
cd <track> && cargo test --release --test discrimination
```

The interesting rows are the partial ones. `broken_shell.sh` half-implements `echo` and
`exit`, and stages 04 and 05 come back mixed rather than red. `broken_node` is a correct
key/value store that acknowledges writes before they are durable: stages 22 and 23 are
*entirely green* and stage 35 is entirely red. `broken_linker` emits a valid, runnable ELF
and ignores relocation addends, so the structural tests pass and the ones that check where a
relocation landed fail.

## Building it

One cargo workspace, seven crates, one lockfile. `rust-toolchain.toml` pins the toolchain:
the dependency tree uses edition 2024, which needs Rust 1.85 to parse, and `time` raises the
floor to **1.88**. That number is measured, not claimed — CI builds every crate on exactly
1.88, which is how it got corrected from 1.87.

```
cargo build --release          # all seven binaries into ./target/release
./install.sh                   # and into ~/.local/bin, with the site and a progress database
```

## Known gaps

- The seven crates still carry their own copies of `assert.rs` and `report.rs`, and those
  copies have drifted: 13 of 17 public functions in `assert.rs` are common, 6 in `report.rs`.
  The workspace makes that visible and fixable; it does not fix it. `config.rs` is *not* in
  this list — target registries genuinely differ per track.
- No `.eh_frame` / `--eh-frame-hdr` stage for `link`. The header bytes are verified but
  hand-encoding DWARF CFI confidently enough to assert against every case is not something to
  guess at.
- Two reading-list entries are missing for want of a citable copy: the ABD paper and the
  Bayou session-guarantees paper. No URL goes in the library that has not been seen to
  return 200 with the right content.

Two process-hygiene notes for whoever runs these next: the cluster ladder leaks etcd children
if its harness is killed mid-test rather than left to finish, so kill the tester and then sweep
`pgrep -f cache/disttest/etcd` and `/tmp/disttest-*`; and a cluster `--validate` takes ~15
minutes, which is long enough to look stalled when it is not.
