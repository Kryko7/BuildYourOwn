# wasmtest

A self-contained, stage-by-stage conformance tester for **WebAssembly runtimes**, written in
Rust. It drives any runtime binary as a black box through a small subset of `wasmtime`'s
command line — real modules, real bytes, real traps — and compares what comes back with a
suite written in Rust: 48 stages, 381 tests, all validated against `wasmtime` 48.0.2.
Every module the suite runs is **built by this crate's own encoder**, so a failure can print
the exact bytes it ran, annotated section by section.

```
cargo build --release
./target/release/wasmtest --runtime wasmtime --validate --all    # suite self-check, all green
./target/release/wasmtest --runtime ./your_program.sh --stage 1   # your first red/green
./target/release/wasmtest --runtime my_runtime --until 12
./target/release/wasmtest --list                                  # stages, counts, PLAN.md tickboxes
./target/release/wasmtest --list --json catalog.json              # stage catalog for the site
```

The reference runtime is downloaded once (~11 MB) into `~/.cache/wasmtest` and run straight
from there. No Docker, no accounts, no network except that one download. Linux x86-64.

## What a run looks like

```
Stage 15 Division, remainder and the two integer traps
  ✘ INT_MIN / -1 traps with integer overflow FAIL (6 ms)
      stderr: expected the canonical trap reason 'integer overflow' somewhere on stderr, got ""
    ┌ context
    │ while checking that module 'div-overflow' traps with 'integer overflow'
    │ the case is 'i32::MIN / -1'
    ┌ expected
    │ exit: a non-zero exit status, because a trap aborts the program
    ┌ actual
    │ exit: "exit status 0"
    ┌ command
    │ /home/you/runtime/your_program.sh run --invoke f3 /tmp/wasmtest-91/s15-t0003/001-div.wasm
    ┌ outcome
    │ exit status 0, 4 ms
    ┌ stdout (2 bytes)
    │ 0
    ┌ stderr
    │ (empty)
    ┌ module 'div' (51 bytes)
    │ 0000  00 61 73 6d                            header.magic = \0asm
    │ 0004  01 00 00 00                            header.version = 1 (little-endian)
    │ 0008  01                                     section[1].id = type section
    │ 0009  05                                     section[1].size = 5 bytes
    │ 000b  60 00 01 7f                            type[0] = () -> (i32)
    │ 000f  03                                     section[3].id = function section
    │ ...
    │ 0021  0a                                     section[10].id = code section
    │ 0024  09                                     code[0].body = i32.const -2147483648, i32.const -1, i32.div_s
    │ 002d  07                                     section[7].id = export section
Stage 15  Division, remainder and the two integer traps    7/8 passed
```

Every failure names what was expected, shows the **exact command line**, both streams kept
apart, and the module as an annotated listing — offset, bytes, field path, decoded value —
so there is never any doubt about which bytes the runtime was given. A runtime that dies or
hangs is its own failure kind:

```
  ✘ a truncated code section is refused FAIL [runtime crash] (12 ms)
      the runtime was killed by signal 11 (SIGSEGV) — a module is input, never a reason to die
  ✘ 1 000 functions decode and run FAIL [timeout] (10004 ms)
      the runtime did not finish within 10000 ms and was killed
```

To see the red path without writing a runtime:

```
cargo build --release --example broken_runtime
./target/release/wasmtest --runtime target/release/examples/broken_runtime --until 5
```

## The contract your runtime must follow

Your program is invoked as a `wasmtime`-compatible subset. That is the whole interface:
there is no protocol, no daemon, no port.

```
./your_program.sh run --invoke <export> <module.wasm> [args...]
./your_program.sh run <module.wasm> [args...]
```

| Thing | Convention |
|---|---|
| `--invoke <export>` | Call that exported function with the given arguments and print **each returned value on its own line** to stdout. Arguments are decimal integers or float literals, in the order the function takes them. |
| `run <module.wasm>` | A WASI command: instantiate the module and call its `_start` export. The arguments after the path are the module's own `argv[1..]`; `argv[0]` is the module file's **basename**. |
| stdout vs stderr | Results go to **stdout**. Diagnostics, warnings and trap messages go to **stderr**. The suite never folds them together — `wasmtime` prints an experimental-feature warning on stderr for every `--invoke` call, and a suite that merged the streams would be comparing against noise. |
| A trap | Exit **non-zero** with `wasm trap: <reason>` somewhere on stderr. The reasons are the spec's own strings — `unreachable`, `integer divide by zero`, `integer overflow`, `invalid conversion to integer`, `out of bounds memory access`, `out of bounds table access`, `undefined element`, `uninitialized element`, `indirect call type mismatch`, `call stack exhausted`. They are matched as **case-insensitive substrings, anywhere on stderr**, so the sentence around them is yours. |
| A module that does not decode or does not validate | Exit **non-zero** having executed **nothing**: stdout must be completely empty, and stderr must say something. The suite never compares that message — the wording is yours — it only records it as a note. |
| Instantiation failures | A data or element segment whose offset is out of range, or a `start` function that traps, fails *after* validation: non-zero exit, nothing on stdout, and for the segment case the canonical `out of bounds memory access` reason. |
| Exit status | `proc_exit(n)` makes the process exit with status `n`. Otherwise 0 when the program completed and non-zero when it did not. |
| Nothing else | No files outside the directory the harness gives you, no ports, no network. Every invocation is a fresh process with a fresh temporary directory. |

Where the spec permits more than one behaviour, the suite accepts all of them and says so in
the failure's `context` block — the `expect_trap_one_of` helper exists for exactly that, and
an out-of-range `call_indirect` index is the standing example: the spec calls it *undefined
element*, `wasmtime` writes *undefined element: out of bounds table access*, and either
wording passes.

## CLI

```
wasmtest --runtime <name|path> [--stage N] [--until N] [--from N] [--all]
         [--only "substring"] [--tag ext|wasi|slow] [--skip-ext]
         [--verbose] [--keep-tmp] [--timeout-ms N] [--seed N]
         [--validate] [--list] [--json report.json] [--no-color]
         [--runtimes-file path]
```

| Flag | Meaning |
|---|---|
| `--runtime` | A name from `runtimes.yaml` (`wasmtime`, `my_runtime`, `broken`) **or a path**. A path is run as `<path> run ...` from the current directory. |
| `--stage N` / `--until N` / `--from N` / `--all` | Stage selection. `--until 20` runs stages 1–20; `--from 13 --until 20` runs that window. |
| `--only SUBSTR` | Only tests whose name contains the substring. |
| `--tag T` | Only tests carrying a tag. `ext` is everything beyond the core track, `wasi` the whole WASI section, `slow` the fuzz and soak stages. |
| `--skip-ext` | Hide `ext` tests. An empty selection is a usage error, so `--stage 43 --skip-ext` tells you the stage is entirely `ext`. |
| `--validate` | Run against a *registered* reference runtime and word failures as suite bugs. `wasmtest --runtime wasmtime --validate --all` must be all green. It never widens the selection: `--validate --stage 5` runs stage 5, and `--validate` on its own means `--all`. |
| `--verbose` | Print every module the harness writes and the export it invokes. |
| `--keep-tmp` | Keep each test's temporary directory — every `.wasm` it built, numbered in the order it ran them — and print the paths. |
| `--timeout-ms N` | Per-test deadline, default 10 000. A test may raise the floor for itself (the fuzz and soak stages do); an explicit `--timeout-ms` larger than that floor still wins. |
| `--seed N` | Seeds every random choice: the fuzz stage's mutations, the values it picks, the order it picks them in. The same seed reproduces the same run, and a fuzz failure prints the seed and the mutation index to replay. |
| `--json FILE` | Machine-readable report (schema below). With `--list`, writes the stage catalog instead; `--list --json` with no value prints the catalog on stdout. |
| `--list` | Stages, source files, test counts, example counts and the `PLAN.md` tickbox state. |
| `--no-color` | No ANSI. `NO_COLOR` in the environment does the same. |
| `--runtimes-file` | Path to `runtimes.yaml`; found next to the cwd or the binary by default. |

Exit code is **0** only when every selected test passed, **1** when something failed, **2** on
a usage or harness error (unknown runtime, no stage selected, the runtime will not start).

## The reference runtime

`--runtime wasmtime` is the Bytecode Alliance's `wasmtime`, and it is what `--validate`
proves the suite against. The harness:

1. Downloads `wasmtime-v48.0.2-x86_64-linux.tar.xz` from the GitHub release into
   `~/.cache/wasmtest` (override with `WASMTEST_CACHE`), behind an `flock`ed lock file so
   parallel runs share one copy.
2. Checks it against a **SHA-256 pinned in `src/runtime/reference.rs`**. GitHub publishes no
   checksum file next to the asset, so pinning the digest in the source is both the integrity
   check and the version lock: a tarball that does not hash to that value is not the build
   this suite was validated against, and the harness refuses it rather than running it.
3. Unpacks it with `tar xJf` into `~/.cache/wasmtest/wasmtime-v48.0.2-x86_64-linux/` and runs
   the binary from there. Nothing is installed system-wide.

The download uses `curl` or `wget` rather than an HTTP crate: it happens once, and it keeps
`reqwest` and a TLS stack out of the dependency tree.

## Building modules

There is no `wat2wasm`, no `.wat` file and no encoder crate. `src/wasm/` is the suite's own
encoder, and it exists for three reasons:

* the malformed-module stages need bytes no well-behaved encoder would emit — an overlong
  LEB128, a section size that lies, two type sections, a body that ends early;
* every module carries **annotations** (`{offset, length, field, value}`) produced while it is
  encoded, which is what the failure block prints and what the catalog's examples render;
* the worked examples are therefore known offline: nothing has to be captured from a running
  runtime, because a module is input, not a conversation.

```rust
let mut b = ModuleBuilder::new("add");
let ty  = b.add_type(ftype(&[ValType::I32, ValType::I32], &[ValType::I32]));
let idx = b.add_func(ty, Func::new(
    Expr::new().local_get(0).local_get(1).op(op::I32_ADD)));
let m = b.export_func("add", idx)
         .memory(Limits::range(1, 2))
         .export_memory()
         .global(global_i32(7, /* mutable */ true))
         .data_active(0, b"hello")
         .build();
```

`Expr` records both the bytes and the mnemonics, so a function body annotates itself:

```
0024  01 02 7f 41 80 02 41 00 …   code[1].body = locals 2×i32; i32.const 256, i32.const 0,
                                  i32.store align=4 offset=0, …, call 0, drop
```

For anything a well-formed encoder will not write, drop to `Enc`:

```rust
let mut e = Enc::with_header();                              // magic + version, annotated
e.section(section::TYPE, &body);                             // id, uLEB size, body
e.section_with_size(section::TYPE, &uleb(99), &body,
                    "99, but only 3 bytes follow");          // the size that lies
let m = e.finish("liar");
```

`tests/encoder.rs` pins the encoder against a module written out by hand from the spec's
grammar, byte for byte, so a change that silently moves a byte fails there rather than in
forty stages at once.

## JSON report

`--json report.json` writes the same schema `shelltest` and `kafkatest` write, so one parser
reads every track:

```jsonc
{
  "target": "wasmtime",
  "validate": true,
  "passed": 347, "failed": 0, "skipped": 0, "elapsed_ms": 13182,
  "stages": [
    {
      "stage": 15, "name": "Division, remainder and the two integer traps",
      "file": "src/stages/s15_division_traps.rs",
      "passed": 8, "failed": 0, "skipped": 0,
      "tests": [
        {
          "name": "INT_MIN / -1 traps with integer overflow",
          "status": "pass",              // "pass" | "fail" | "skip"
          "ext": false,
          "duration_ms": 41,
          "failures": [],                // one line per failed check
          "skip_reason": null,
          "actual": [],                  // [path, value] pairs that were seen
          "notes": [],                   // ctx.note(..) lines, pass or fail
          "failure_kind": null           // assertion | timeout | runtime_crash |
                                         // output | harness
        }
      ]
    }
  ]
}
```

`catalog.json` (`--list --json`) is the file the site consumes:

```jsonc
{
  "track": "wasm",
  "generatedAt": "2026-09-18T20:13:31Z",
  "sections": [{ "id": "a", "title": "Binary format & decoding", "stages": [1,2,3,4,5,6] }],
  "stages": [{
    "number": 1, "slug": "magic_and_version", "name": "The magic number and the version",
    "ext": false, "file": "src/stages/s01_magic_and_version.rs",
    "hints": ["A module begins with the four bytes 00 61 73 6d …", "…"],
    "tests": [{ "name": "the eight-byte header is accepted" }],   // ext/tags/skipOn when set
    "examples": [{ /* see below */ }]
  }]
}
```

`sections` lists **all 45 stages** whether or not they are implemented, so the site can draw
the whole journey; `stages` holds the implemented ones. `catalog.json` is committed and
`cargo test` fails if it is stale.

## Examples

Every stage carries two or three **worked examples** — 92 in all: a module this crate builds, its bytes,
an annotation per section and per field, and the output a correct runtime prints — so a
learner can see what a stage is about before writing a line of code.

```rust
fn examples() -> Vec<ExampleSpec> {
    vec![ExampleSpec::module("The smallest module that returns a number", seven)
        .summary("the eight-byte header, a type section for `() -> i32`, a function \
                  section, an export named `f`, and a body of `i32.const 7`")
        .command("run --invoke f mod.wasm")
        .output("7")
        .note("Read the first eight bytes before anything else: 00 61 73 6d 01 00 00 00.")]
}
```

The JSON keys are the ones `kafkatest` already emits, so the site renders both tracks with
one component. For this track the mapping is:

| key | what it holds here |
|---|---|
| `kind` | always `"module"` |
| `request` | the summary and the command line: `"…  —  \`run --invoke f mod.wasm\`"` |
| `request_hex` | the exact module bytes, lower-case hex |
| `request_fields` | `{offset, length, field, value}` per annotated region, ascending |
| `response` | what a correct runtime prints, written as it appears in a terminal |
| `response_hex`, `response_fields` | always empty — a runtime answers in text, not in bytes |

Offsets are byte offsets into the module, so offset 0 is the first byte of the magic. `field`
is a dotted path: `header.magic`, `section[1].id`, `type[0]`, `function[0]`, `export[2]`,
`code[1].body`, `data[0]`. Regions the builder did not annotate print as `(unannotated)` in
the terminal listing, so the listing always adds up to the whole module.

Because the bytes are produced offline, editing an example needs no runtime — and every
example is also a test somewhere in its stage, so `--validate` is what proves the `output`
line is really what `wasmtime` prints. `src/examples/mod.rs` has unit tests that every stage
declares 1–3 complete examples, that each one encodes to a real module, that its annotations
lie inside it, and that rendering is deterministic.

## Registering a runtime

`runtimes.yaml`:

```yaml
runtimes:
  wasmtime:                # the reference, and the --validate target
    kind: reference
    version: "48.0.2"
  my_runtime:
    command: ["./your_program.sh"]
    cwd: "."
    env: { RUST_LOG: "debug" }
  broken:
    command: ["target/release/examples/broken_runtime"]
    cwd: "."
```

`command` is only the **prefix**: the harness appends `run [--invoke <export>]
<module.wasm> [args...]` itself. `{TMP}` is substituted with the test's temporary directory
in `command`, `cwd` and `env`.

## Adding a stage

One file per stage, `src/stages/sNN_slug.rs`, so several people can work at once without
touching the same file. The whole API is four things: `Stage`, `Test`, `Ctx` and `Check`.

```rust
//! Stage 15 — Division, remainder and the two integer traps.

use crate::examples::ExampleSpec;
use crate::stages::{case_i32, case_trap, run_cases, trap, Ctx, Stage, Test};
use crate::wasm::{op, Expr};
use crate::wasm_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 15,
        slug: "division_traps",          // the file must be src/stages/s15_division_traps.rs
        name: "Division, remainder and the two integer traps",
        ext: false,                      // true when the whole stage is beyond the core track
        hints: &[                        // 2-4 lines; they go into PLAN.md and catalog.json
            "div_s truncates towards zero, so -7 / 2 is -3 and not -4",
            "rem_s takes its sign from the dividend: -7 % 2 is -1",
            "i32::MIN / -1 traps with integer overflow, but i32::MIN rem_s -1 is 0",
        ],
        examples: examples,              // 1-3 worked examples
        tests: vec![
            Test::new("div_s truncates towards zero", truncation),
            Test::new("a huge module still decodes", big)
                .ext()                   // --skip-ext hides it
                .tag("slow")             // --tag slow selects it
                .min_timeout_ms(60_000)  // a floor under --timeout-ms
                .timeout_ms(120_000)     // ... or an outright override
                .skip_on("wasmtime", "wasmtime folds this at compile time"),
        ],
    }
}

wasm_test!(truncation, |ctx| {
    run_cases(ctx, "div-s", vec![
        case_i32("-7 / 2 is -3, not -4",
                 Expr::new().i32_const(-7).i32_const(2).op(op::I32_DIV_S), -3),
        case_trap("1 / 0 traps",
                  Expr::new().i32_const(1).i32_const(0).op(op::I32_DIV_S),
                  &[trap::DIVIDE_BY_ZERO]),
    ])
});
```

Then add two lines to `src/stages/mod.rs` (`mod s15_division_traps;` and
`s15_division_traps::stage(),`), and:

```
cargo test                                                # registry invariants + catalog
./target/release/wasmtest --runtime wasmtime --validate --stage 15
./target/release/wasmtest --list --json catalog.json      # catalog.json must be regenerated
```

`PLAN.md` is generated from the registry, so its tickbox line — the source file, the test
count, the `**[ext]**` marker and every hint — has to match; `tests/catalog_is_current.rs`
checks all four.

### What `Ctx` gives a test

| | |
|---|---|
| `ctx.invoke(&m, "f", &["3"])?` | `run --invoke f <module> 3`, returning a `Run`. |
| `ctx.start(&m, &args)?` | `run <module> args…` — the WASI command path. |
| `ctx.start_with_stdin(&m, &args, b"…")?` | The same, with bytes on standard input. |
| `ctx.write_module(&m)?` | Write a module into the test's temp directory and get its path. |
| `ctx.rng`, `ctx.seed` | The seeded RNG. Every random choice must come from it, or `--seed` stops meaning anything. |
| `ctx.note("…")` | An informational line shown under the test **whatever the outcome**, and carried in the JSON report's `notes`. For counts, timings, and "the reference answered X here". |
| `ctx.tmp`, `ctx.timeout`, `ctx.deadline`, `ctx.runtime_name` | The rest of the environment. |

A `Run` carries `stdout`, `stderr`, `raw_stdout`, `exit`, `timed_out`, `duration`, plus
`lines()`, `first_line()`, `stderr_has(needle)`, `command_line()` and `stderr_tail(n)`. The
harness turns a signal into a `runtime crash` failure and a deadline into a `timeout` failure
before a test ever sees them, so a stage body never has to check for either.

### The checks

`expect` · `expect_line` · `expect_lines` · `expect_stdout` · `expect_trap` ·
`expect_trap_one_of` · `expect_start_trap` · `expect_rejected` · `expect_start_rejected` ·
`expect_accepted` — and `run_cases` / `run_cases_with`, which build one module holding many
named cases and invoke each in its own process, which is how the numeric and control-flow
stages stay readable.

For anything they do not cover, `Check::new(what, &run)` then `eq` · `ne` · `at_least` ·
`at_most` · `that(path, expected, ok, actual)` · `bytes_eq` · `note` · `observe` ·
`module(&m)` · `finish() -> Result<(), Failure>`. Calling `c.module(&m)` is what attaches the
annotated listing to the failure; every canned check does it already.

Stage tests must never `unwrap()` on anything that can fail.

## Where the suite is deliberately loose, and why

Every one of these is a place where a first draft of the suite was wrong about the reference,
or about the spec, and the expectation was corrected rather than the test deleted. They are
the interesting half of the suite: each one is a rule a from-scratch runtime gets wrong.

**About traps and error reporting**

- **Trap reasons are matched as substrings, case-insensitively, anywhere on stderr.** The
  spec fixes the reason, not the sentence. `wasmtime` writes ``wasm trap: wasm `unreachable`
  instruction executed``; `unreachable` is what is asserted.
- **An out-of-range `call_indirect` index has two accepted wordings**: *undefined element*
  (the spec's) and *out of bounds table access*. `wasmtime` writes both.
  `trap::TABLE_INDEX_OUT_OF_RANGE` accepts either.
- **An infinity is `integer overflow`, not `invalid conversion to integer`.** Only a NaN
  gives *invalid conversion*; `±inf` and every out-of-range finite value give *integer
  overflow*, and the spec's own `conversions.wast` agrees. Stage 20 asserts the split.
- **Exit statuses are never compared to a number** except where `proc_exit` fixes one. A trap
  exits 134 under `wasmtime` because of how it aborts; all that is required is *non-zero*.
- **A decode or validation failure's message is never compared** — only the exit status, the
  empty stdout, and that stderr is not empty. The message is recorded as a note so a reader
  can see what the reference said. (`wasmtime` does not even report a bad magic as a binary
  decode error: it falls back to parsing the file as WAT text and complains about a stray
  character. That is exactly why the wording is not part of the contract.)
- **Instantiation failures are told apart from validation failures by their side effects, not
  by their wording.** A module whose active data or element segment is out of range validates
  fine and then traps at instantiation — exit 134, `out of bounds memory access` — so the
  suite asserts the start function produced nothing, which is the observable part.

**About floating point**

- **A NaN result is only checked for being a NaN**, unless the test reinterprets it. Every
  NaN prints as the bare word `NaN`, so the tests that care about payload or sign go through
  `i32.reinterpret_f32` / `i64.reinterpret_f64` and compare integers; they assert the
  exponent bits and the quiet bit and leave the payload to the runtime. `wasmtime` in fact
  *preserves* an operand's payload and only forces the quiet bit on.
- **`0.0 / 0.0` gives a *negative* canonical NaN** under `wasmtime` (`0xffc00000`), which is
  x86's default. "Canonical" pins the payload but not the sign, so those cases mask the sign
  bit off and compare the remaining bits.
- **Float results are compared as the text a runtime prints.** `wasmtime` formats them
  exactly the way Rust's `Display` does, exponent-free: `1.5`, `-0`, `inf`, `-inf`, `NaN`,
  and `f32::MAX` as 39 digits. `show_f32` / `show_f64` produce the same strings.
- **`0.1 + 0.2 == 0.3` is *true* at f32 width.** The famous inequality is an f64 fact, and
  stage 18 keeps both in one sweep precisely because it is the thing a learner would "fix".

**About validation**

- **The polymorphic stack supplies values of any type; it does not absorb a concrete one.**
  `unreachable; i32.const 1` in a `() -> i64` function is *rejected* — the `i32` is a real
  type that still has to match the block's result at its `end`. Every accepted case in stage
  09 is therefore junk whose *result* still fits. This is the single most misleading thing in
  the area, and the first draft of the suite had it backwards.
- **`end` resets reachability**, so `block unreachable end` in a `() -> i32` function is a
  validation error rather than the trap it looks like. Three of stage 24's trap cases carry
  an `i32.const 0` that the run never reaches, for that reason alone.
- **`global.get` of a *module-defined* global in a constant expression is accepted** by
  `wasmtime` 48, although the 1.0 format only allowed naming an *imported* one. Since
  `wasmtime run` supplies no host globals, that is the only non-literal constant expression
  reachable against the reference at all. A mutable global there is still refused, and is
  tested. The stage doc-comments say so, so a learner whose 1.0-era validator refuses the
  first form knows why.
- **`ref.func` may only name a function declared outside a function body** — an export, the
  start section, a global initialiser or an element segment. Stage 39 tests the rejection;
  stage 25's typed-`select` cases rely on the fact that a sweep exports every case function.
- **Truncating a module at a section boundary can leave a perfectly valid smaller module.**
  Stage 05 does not assert "every prefix is refused"; it asserts the accepted set is exactly
  the three prefixes that really are complete modules and that the other 41 offsets are
  refused, and reports the counts.

**About runtime behaviour**

- **`fd_write` may write fewer bytes than it was asked to.** Given two iovecs, `wasmtime`
  writes the first and reports its length in `*nwritten` — what POSIX `writev` is allowed to
  do, and what preview1 permits. Stage 40 asserts the bytes that came out are a **prefix** of
  the concatenation and that `*nwritten` says how many there were. The same applies to
  `fd_read` in stage 42. A runtime that moves the lot in one call passes the same tests.
- **`data.drop` empties a segment rather than setting a flag**, so a `memory.init` of length
  **zero** from a dropped segment is still legal while one byte traps. A runtime keeping a
  "dropped" boolean and trapping on any `memory.init` naming the segment fails stage 33.
- **"A trapping store wrote nothing" is not observable from inside the instance** — a trap
  ends the invocation and the next process gets a fresh page. Stages 31 and 34 pin down the
  boundary instead: the largest access that fits works *and reads back correctly*, one byte
  more traps.
- **`wasmtime run <module>` with no `_start` exits 0 and does nothing.** It does not complain
  about a missing command, so stage 06 tells a decoded module from a broken one by whether
  `--invoke` names the missing export in its error, not by whether `run` fails.
- **The `min_timeout_ms` floors on stages 43–45 are three orders of magnitude above what
  `wasmtime` needs.** They are sized for a tree-walking interpreter, which is what a learner
  has by then; the `ctx.note` timings show the real numbers.
- **Never pass `--` between the module path and the module's arguments.** With `--invoke`,
  `wasmtime` treats `--` as an argument to the function and fails to parse it as a number.
  The harness never emits one.

There are **no skipped tests** in a `--validate` run — 347 pass, 0 fail, 0 skip. If one ever
appears it carries a reason, which is printed in the summary and belongs in this list.

## Development

```
cargo build --release
cargo test                                  # unit + integration tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
./target/release/wasmtest --runtime wasmtime --validate --all
```

Layout:

```
src/main.rs                CLI (clap), stage selection, --list
src/lib.rs                 everything else, so tests/ can use it
src/config.rs              runtimes.yaml, placeholder substitution
src/runtime/mod.rs         spawn, process-group kill, deadline, captured streams
src/runtime/reference.rs   wasmtime download, SHA-256, unpack, cache lock
src/wasm/mod.rs            the encoder: why it exists
src/wasm/encode.rs         LEB128, section framing, ModuleBuilder, Enc, annotations
src/wasm/instr.rs          Expr and the opcode tables
src/wasm/hex.rs            hex dumps with marked bytes
src/stages/mod.rs          the registry, Stage/Test/Ctx, the wasm_test! macro
src/stages/helpers.rs      trap reasons, module shorthands, the checks, case sweeps
src/stages/wasi.rs         WASI preview1 imports and the glue the WASI stages share
src/stages/sNN_*.rs        one file per stage
src/assert.rs              Check, Failure, FailureKind
src/report.rs              terminal output + JSON
src/catalog.rs             --list and catalog.json
src/examples/mod.rs        ExampleSpec: the worked examples a stage declares
tests/encoder.rs           the encoder against hand-written bytes, examples vs the reference
tests/catalog_is_current.rs  catalog.json and PLAN.md freshness
tests/cli_smoke.rs         selection, exit codes, the red path via examples/broken_runtime.rs
examples/broken_runtime.rs a runtime that checks the header and nothing else
PLAN.md                    tickboxes and hints for all 45 stages (generated from the registry)
catalog.json               committed; a test fails if it is stale
runtimes.yaml              the reference, your runtime, the broken example
```
