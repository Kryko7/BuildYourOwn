# linktest

A self-contained, stage-by-stage conformance tester for **static ELF64 x86-64 linkers**,
written in Rust. It drives **any** linker binary as a black box — real relocatable objects,
real archives, real machine code — and then does the only thing that settles the argument:
it **runs the binary your linker produced**. 44 stages, 359 tests, all validated against
GNU ld 2.47. Every stage also carries worked examples — 87 of them — each a real `.o`,
annotated field by field, next to the linker command line and the exact stdout and exit
status the linked program must produce.

> Commands below are run from the **repo root**: this is a cargo workspace, so every
> binary and example lands in the one `target/` directory at the top.
```
cargo build --release -p linktest
target/release/linktest --linker gnu_ld --validate --all    # suite self-check, all green
target/release/linktest --linker ./your_program.sh --stage 1  # your first red/green
target/release/linktest --linker my_linker --until 12
target/release/linktest --list                              # stages, counts, PLAN.md tickboxes
target/release/linktest --list --json catalog.json          # stage catalog for the site
```

Nothing is downloaded and nothing is installed. The suite **writes its own object files** —
its own ELF64 writer, its own archive writer, its own hand-assembled x86-64 — so it runs on a
machine with no `as` and no `ld`. GNU ld is needed only for `--validate`, and `readelf`,
`nm` and `objdump` only for the handful of `[ext]` interop tests, which skip themselves with
a written reason when the tools are absent.

## What a run looks like

```
Stage 01 Link one object and run it
  ✔ a one-object program links (4 ms)
  ✘ the linked program runs and prints FAIL (6 ms)
      program.stdout: expected "hello, linker\n", got "linker\n\0\0\0\0"
    ┌ context
    │ while checking what the linked program printed and exited with
    ┌ expected
    │ program.stdout: "hello, linker\n"
    ┌ actual
    │ program.stdout: "linker\n\0\0\0\0"
    ┌ linker command
    │ ./your_program.sh -o /tmp/linktest-9182/s01/t002/l01/a.out /tmp/.../a.o
    ┌ output program headers
    │   Type       Offset     VirtAddr   FileSiz    MemSiz     Flg Align
    │   LOAD       0x00001000 0x00401000 0x00000024 0x00000024 R X 0x1000
    │   LOAD       0x00002000 0x00402000 0x0000000e 0x0000000e R   0x1000
  ✔ the output is an ET_EXEC x86-64 executable (3 ms)
Stage 01  Link one object and run it                   5/8 passed
```

That failure is the classic one: the relocation addend was dropped, so `S + A - P` came out
as `S - P` and the string pointer landed four bytes late. Every failure names a **field
path** (`output.e_entry`, `program.stdout`, `.rela.text[0].r_addend`), shows the exact linker
command line that produced it, and dumps whatever explains it — the linker's own stderr, the
output's program and section header tables, a hex dump with the relocated bytes marked.

A linker that dies, hangs, or produces a binary the kernel refuses is its own failure kind:

```
  ✘ a call across objects resolves FAIL [linker crash] (91 ms)
      the linker was killed by a signal while linking two objects: signal 11 (SIGSEGV)
  ✘ the linked program runs and prints FAIL [program crash] (12 ms)
      the linked program died with signal 11 (SIGSEGV)
  ✘ an undefined symbol is refused FAIL [link accepted] (4 ms)
      the linker exited 0 while linking an object with an undefined symbol; this must be an error
```

To see the red path without writing a linker:

```
cargo build --release -p linktest --example broken_linker
target/release/linktest --linker target/release/examples/broken_linker --until 5
```

## The contract your linker must follow

```
./your_program.sh -o <out> [-e <entry>] [-L <dir>] [-l <name>] [--entry=<sym>] <input.o|input.a>...
```

| Thing | Convention |
|---|---|
| Invocation | The GNU ld flag subset above, and nothing else. The harness never passes `-shared`, `-pie`, `--dynamic-linker`, `-T` or a linker script. |
| Input paths | **Absolute**. The harness writes the inputs into a fresh temporary directory and names them by absolute path, so your program may keep any working directory it likes (`cwd:` in `linkers.yaml`). |
| Output | A **statically linked, non-PIE ELF64 executable for x86-64 Linux** (`ET_EXEC`, `EM_X86_64`) that the kernel runs directly. No `PT_INTERP`, no `PT_DYNAMIC`, no dynamic symbol table, no libc. |
| Permissions | The output must be **chmod 0755** (any execute bit). ELF alone is not enough; the kernel refuses to `execve` a file with no `x`. |
| Entry | `_start` by default; `-e <sym>` and `--entry=<sym>` override it. `e_entry` must be the address of that symbol. |
| Symbol table | The output must carry a `.symtab` naming the defined globals. Every debugger wants it, and the suite reads a symbol's *address* out of it to check a relocation without caring where you put things. |
| Segments | At least one `PT_LOAD`; `p_filesz <= p_memsz`; no two `PT_LOAD`s overlapping; `p_offset ≡ p_vaddr (mod 4096)`; `.text` in an executable segment, `.data` in a writable one, and **no segment both writable and executable**. |
| `.bss` | Memory but no file bytes: `SHT_NOBITS`, `p_memsz > p_filesz`, and **zeroed** when the program starts. |
| Relocations | `R_X86_64_64` (`S + A`), `PC32` and `PLT32` (`S + A - P`), `32` and `32S` (`S + A`, with the range check), addends included — positive, negative and into the middle of a symbol. `GOTPCREL`/`GOTPCRELX` may be relaxed or given a real GOT entry; the suite accepts either. |
| Archives | System V/GNU `.a`: the `/` symbol index, the `//` long-name table, members pulled in **only** when they resolve a pending undefined symbol, and command-line order that means what it says. |
| Errors | Undefined symbol, duplicate definition, relocation overflow, unreadable input: **exit non-zero with a message on stderr**, and leave no runnable output file behind. The suite never checks your wording — only the exit status and that the message **names the offending symbol or file**. |
| Layout | Entirely yours. The suite asserts *invariants* — the displacement implied by the addresses you chose, the permissions of the segment a section landed in — never addresses. |

## The three-legged verification model

Every stage uses at least two of these, and the whole point of the suite is that the third
one is not optional theatre:

1. **Run it.** The produced binary is executed in a sandboxed temporary directory, with a
   timeout, never with elevated privileges, and its stdout and exit status are compared with
   what the fixture's own machine code says they must be. Fixtures are freestanding: they
   make raw `write(2)` and `exit(2)` syscalls, so there is no libc between your linker's
   mistake and the failure message.
2. **Re-parse it.** The output is read back with the suite's own ELF reader
   (`src/elf/read.rs`) and asserted on structurally: header fields, program headers, segment
   permissions and alignment, `e_entry` pointing at the right symbol's address, no
   overlapping segments, `p_filesz <= p_memsz`, `.bss` occupying no file space.
3. **Compare against the reference.** For the properties that must match GNU ld — the
   relocated value, the relationship between two symbols' addresses, which segments exist —
   the suite asserts the **invariant**, not byte equality. `S + A - P` is checked against the
   addresses *your* linker chose. GNU ld's layout is never a requirement.

There is an optional fourth leg in stage 42: `readelf`, `nm` and `objdump` are asked about
the same file and must agree with the suite's own parse. Those tests skip themselves, with a
reason, when the tools are not installed.

## CLI

```
linktest --linker <name|path> [--stage N] [--until N] [--from N] [--all]
         [--only "substring"] [--tag ext] [--skip-ext]
         [--verbose] [--keep-tmp] [--timeout-ms N] [--seed N]
         [--validate] [--list] [--json report.json] [--no-color]
         [--linkers-file path]
```

| Flag | Meaning |
|---|---|
| `--linker` | A name from `linkers.yaml` (`gnu_ld`, `my_linker`, `broken`) **or a path**. A path is run as `<path> -o out ... inputs` with no working directory of its own. |
| `--stage N` / `--until N` / `--from N` / `--all` | Stage selection. Gaps are fine: `--until 20` runs the implemented stages up to 20. |
| `--only SUBSTR` | Only tests whose name contains the substring. |
| `--tag T` | Only tests carrying a tag. `--tag slow` is the long ones (stages 38-40); `--tag ext` everything past the core track. |
| `--skip-ext` | Hide `ext` tests. Every test of an `ext` stage carries the tag, so `--skip-ext` on such a stage leaves nothing to run — which is a usage error, on purpose. |
| `--validate` | Run against the registered **reference** linker and word failures as suite bugs. `linktest --linker gnu_ld --validate --all` must be all green; that is the suite's own regression test. |
| `--verbose` | Print each test's temporary directory as it runs. |
| `--keep-tmp` | Keep every test's temporary directory — the input objects, the archives, the output binary, exactly as the linker saw them — and print the paths. This is the debugging flag: `--keep-tmp --only "..."`, then poke at the files with `readelf` yourself. |
| `--timeout-ms N` | Per-test deadline, default 10 000. It bounds every link and every linked program; a stage that is inherently slow raises its own floor (`--tag slow`). |
| `--seed N` | Seeds every random choice: the fuzz mutations, generated symbol names, the order things are shuffled in. The same seed reproduces the same run, and the seed is printed at the top of every run. |
| `--json FILE` | Machine-readable report (schema below). With `--list`, writes the stage catalog instead; `--list --json` with no value prints the catalog on stdout. |
| `--list` | Stages, source files, test counts, example counts and the `PLAN.md` tickbox state. |
| `--no-color` | No ANSI. `NO_COLOR` in the environment does the same. |
| `--linkers-file` | Path to `linkers.yaml`; found next to the cwd or the binary by default. |

Exit code is **0** only when every selected test passed, **1** when something failed, **2** on
a usage or harness error (unknown linker, no stage selected, the linker cannot be run).

## Registering a linker

`linkers.yaml`:

```yaml
linkers:
  gnu_ld:                            # the reference, and the --validate target
    kind: reference                  # resolves $LD, then /usr/bin/ld; records its version
  my_linker:
    command: ["./your_program.sh"]   # the harness appends -o <out>, the flags and the inputs
    cwd: "."                         # where your program runs; {TMP} is the test's directory
    env: { RUST_LOG: "debug" }       # optional
  broken:
    command: ["target/release/examples/broken_linker"]
    cwd: "."
```

`kind: reference` takes no `command`: it resolves `$LD` if set, then `/usr/bin/ld`,
`/usr/local/bin/ld`, `/bin/ld`, then `ld` on `PATH`, and records `ld --version` so the report
says what the suite was validated against.

## The ELF a learner needs to know

The whole track is three file layouts and one equation.

**A relocatable object (`.o`, `ET_REL`)** is a 64-byte header, a section header table
(`e_shoff`, `e_shnum` entries of `e_shentsize` bytes each), and the sections' contents
somewhere in between. `e_shstrndx` names the section that holds the section *names*. The
sections that matter:

| Section | What it is |
|---|---|
| `.text`, `.rodata`, `.data` | `SHT_PROGBITS` with `SHF_ALLOC`; bytes that end up in the program |
| `.bss` | `SHT_NOBITS`: a size, no bytes. Zeroed at run time |
| `.symtab` | An array of 24-byte `Elf64_Sym`: `st_name`, `st_info` (binding + type), `st_other` (visibility), `st_shndx`, `st_value`, `st_size`. Locals come first, and `sh_info` is the index of the first non-local |
| `.strtab` | The NUL-terminated names `st_name` indexes into |
| `.rela.text` | An array of 24-byte `Elf64_Rela`: `r_offset` (where to patch, inside the section named by `sh_info`), `r_info` (symbol index in the high 32 bits, type in the low 32), `r_addend` |

A symbol's `st_shndx` is the section it is defined in, or `SHN_UNDEF` (a reference to
resolve elsewhere), `SHN_ABS` (a value that must never be relocated) or `SHN_COMMON` (a
tentative definition: `st_size` bytes at `st_value` alignment, allocated once, largest wins).
Its binding is `STB_LOCAL` (invisible outside this object), `STB_GLOBAL`, or `STB_WEAK` (a
strong definition beats it, and an *undefined* weak symbol is simply 0).

**An executable (`ET_EXEC`)** is a 64-byte header, a **program** header table (`e_phoff`,
`e_phnum` entries of 56 bytes), and the segments. The kernel reads only the program headers:
for each `PT_LOAD` it maps `p_filesz` bytes from `p_offset` at `p_vaddr`, zero-fills up to
`p_memsz`, and applies `p_flags` (`PF_R`/`PF_W`/`PF_X`). It requires
`p_offset ≡ p_vaddr (mod 4096)`, because a mapping starts at a page boundary in both the file
and memory. Then it jumps to `e_entry`. Section headers are for tools, not for the kernel.

**An archive (`.a`)** is `!<arch>\n` and then members, each with a 60-byte header of
space-padded decimal ASCII, each padded to an even offset. Two members are special and come
first: `/` is the symbol index (a big-endian count, that many big-endian member-header
offsets, then that many NUL-terminated names), and `//` is the long-name table for members
whose names do not fit 15 characters.

**The equation.** For every relocation entry, let `S` be the final address of the symbol, `A`
the addend from the entry, and `P` the final address of the field being patched. Then:

| Type | Value written | Width |
|---|---|---|
| `R_X86_64_64` | `S + A` | 8 bytes |
| `R_X86_64_PC32` | `S + A - P` | 4 bytes, signed range checked |
| `R_X86_64_PLT32` | `S + A - P` — identical to `PC32` in a static link with no PLT | 4 |
| `R_X86_64_32` | `S + A`, must fit **unsigned** 32 bits | 4 |
| `R_X86_64_32S` | `S + A`, must fit **signed** 32 bits | 4 |
| `R_X86_64_GOTPCREL` | `GOT + G + A - P`, or the relaxed instruction | 4 |

The addend for a RIP-relative reference is `-4` (or `-4 + offset`) because the instruction's
displacement is measured from the *next* instruction, four bytes past the field. Dropping it
is the single most common first-linker bug, and it is what `examples/broken_linker.rs` does
on purpose.

## JSON report

`--json report.json` writes the schema every tester in this repo writes, with `target`
naming the linker, so one parser reads all the tracks:

```jsonc
{
  "target": "gnu_ld",
  "validate": true,
  "passed": 341, "failed": 0, "skipped": 0, "elapsed_ms": 3412,
  "stages": [
    {
      "stage": 1, "name": "Link one object and run it",
      "file": "src/stages/s01_link_one_object.rs",
      "passed": 8, "failed": 0, "skipped": 0,
      "tests": [
        {
          "name": "the linked program runs and prints",
          "status": "pass",              // "pass" | "fail" | "skip"
          "ext": false,
          "duration_ms": 6,
          "failures": [],                // one line per failed check
          "skip_reason": null,
          "actual": [],                  // [field path, value] pairs that were seen
          "notes": [],                   // ctx.note(..) lines, pass or fail
          "failure_kind": null           // assertion | timeout | link_failed | link_accepted |
                                         // linker_crash | malformed_output | program_crash | harness
        }
      ]
    }
  ]
}
```

`catalog.json` (`--list --json`) is the file the site consumes:

```jsonc
{
  "track": "link",
  "generatedAt": "2026-09-18T20:13:31Z",
  "sections": [{ "id": "a", "title": "Reading relocatable objects", "stages": [1,2,3,4,5,6,7] }],
  "stages": [{
    "number": 1, "slug": "link_one_object", "name": "Link one object and run it",
    "ext": false, "file": "src/stages/s01_link_one_object.rs",
    "hints": ["Read the 64-byte ELF header, then the section header table it points at, ...", "..."],
    "tests": [{ "name": "a one-object program links" }],   // ext/tags/skipOn when set
    "examples": [{ /* see below */ }]
  }]
}
```

`sections` lists **every planned stage** whether or not it is implemented, so the site can
draw the whole journey; `stages` holds only the implemented ones. `catalog.json` is committed
and `cargo test` fails if it is stale.

## Examples

Every stage carries one to three **worked examples**: a real relocatable object — the same
bytes the tests feed the linker — annotated field by field, next to the linker command line
and the exact stdout and exit status the linked program must produce.

There is nothing to capture from a reference implementation: an input object is
deterministic, so `--list --json` builds the whole catalog offline and `catalog.json` is
reproducible on any machine.

```rust
fn stage_examples() -> Vec<ExampleSpec> {
    vec![ExampleSpec::object(
        "One object, one syscall pair",
        "ld -o prog hello.o",
        || print_and_exit("hello, linker\n", 0).map_err(|f| f.messages.join("; ")),
    )
    .request("hello.o: a .text with write(1, message, 14) then exit(0), a .rodata holding \
              the string, and one R_X86_64_PC32 relocation for the lea that finds it")
    .response("An ET_EXEC ELF64 file with at least one PT_LOAD covering .text, e_entry equal \
               to the address the linker gave _start, and the lea's displacement patched to \
               S + A - P")
    .note("The whole program is two syscalls. If it prints nothing, the displacement is wrong.")
    .runs("hello, linker\n", 0)]
}
```

The example's builder is the same code the tests use, so an example can never drift away
from the suite, and the annotator (`src/examples/annotate.rs`) walks the object it produced
and labels every field automatically — the ELF header, every section header, every symbol and
every relocation. A stage author writes the prose, never the offsets.

### The JSON the site reads

```jsonc
"examples": [{
  "title": "One object, one syscall pair",
  "kind": "object",                    // object | archive | error | text
  "command": "ld -o prog hello.o",
  "request": "hello.o: a .text with write(1, message, 14) then exit(0), ...",
  "request_hex": "7f454c4602010100000000000000000001003e0001000000...",
  "response": "An ET_EXEC ELF64 file with at least one PT_LOAD covering .text, ...",
  "response_hex": "",                  // always empty — see below
  "note": "The whole program is two syscalls. ...",
  "request_fields": [
    { "offset": 0,  "length": 4, "field": "header.e_ident.magic", "value": "7f 45 4c 46 — \\x7fELF" },
    { "offset": 16, "length": 2, "field": "header.e_type",        "value": "1 (ET_REL, a relocatable object)" },
    { "offset": 40, "length": 8, "field": "header.e_shoff",       "value": "376 (the section header table)" },
    { "offset": 176,"length": 8, "field": ".rela.text[0].r_offset","value": "0xd — the field to patch, inside '.text'" },
    { "offset": 184,"length": 8, "field": ".rela.text[0].r_info", "value": "symbol 3 ('message'), type R_X86_64_PC32" },
    { "offset": 192,"length": 8, "field": ".rela.text[0].r_addend","value": "-4 (the A in S + A - P)" }
  ],
  "response_fields": [],
  "stdout": "hello, linker\n",
  "exit_status": 0
}]
```

- Offsets are byte offsets **into the file**, which is the same as into `request_hex`'s bytes.
- `field` is a dotted path: `header.*`, `sections[i].*`, `symtab[i].*`, `.rela.text[i].*`,
  `<section>.contents`. Tables annotate their first seven entries individually and summarise
  the rest, so an object with two hundred symbols does not produce two hundred rows.
- **`response_hex` is always empty, on purpose.** A linker chooses its own layout, and the
  suite never asserts an address; what the output must *be* is in `response`, and what the
  program must *do* is pinned exactly in `stdout` and `exit_status`.
- `kind: "error"` marks an example the linker has to refuse; it has no `stdout`/`exit_status`.

## Adding a stage

One file per stage, `src/stages/sNN_slug.rs`, so several people can work at once without
touching the same file. The whole API is five things: `Stage`, `Test`, `Ctx`, `Link` and
`Check`.

```rust
//! Stage 17 — An undefined symbol is an error.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::link::Link;
use crate::link_test;
use crate::stages::helpers::*;
use crate::stages::{Stage, Test};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 17,
        slug: "undefined_symbol",        // the file must be src/stages/s17_undefined_symbol.rs
        name: "An undefined symbol is an error",
        ext: false,                      // true for everything past the core track
        hints: &[                        // 2-4 lines; they go into PLAN.md and catalog.json
            "Every R_X86_64_* entry names a symbol; a symbol that is still SHN_UNDEF when \
             every input has been read is an error, not a zero",
            "Name the symbol in the message — that is the only part of the wording anyone \
             depends on",
        ],
        examples: stage_examples,        // 1-3 worked examples
        tests: vec![
            Test::new("an undefined symbol is refused", refused),
            Test::new("the diagnostic names the symbol", names_it)
                .ext()                   // --skip-ext hides it
                .tag("slow")             // --tag slow selects it
                .min_timeout_ms(60_000)  // a floor under --timeout-ms, for slow tests
                .timeout_ms(120_000)     // ... or an outright override
                .skip_on("gnu_ld", "GNU ld only warns here; see the note in the test"),
        ],
    }
}

link_test!(refused, |ctx| {
    let run = ctx.link_fails(
        &Link::new().object("a.o", caller("hi\n", "nowhere")?),
        "linking an object that references a symbol nothing defines",
    )?;
    let mut c = Check::new("the diagnostic for an unresolved reference");
    c.mentions("linker.stderr", "nowhere", &run.output.stderr);
    c.finish()
});
```

Then add two lines to `src/stages/mod.rs` (`mod s17_undefined_symbol;` and
`s17_undefined_symbol::stage(),`), and prove it against the real thing:

```
cargo test                                             # registry invariants + catalog freshness
target/release/linktest --linker gnu_ld --validate --stage 17
target/release/linktest --list --json catalog.json   # catalog.json must be regenerated
```

`PLAN.md` carries a tickbox line per stage naming its source file and test count
(``- [ ] **Stage NN** — Title (`src/stages/sNN_slug.rs`, N tests)``) followed by its hints,
and a `cargo test` checks that it matches the registry.

### What `Ctx` gives a test

| | |
|---|---|
| `ctx.link(&link)?` | Run one link and hand back whatever happened, with no expectation at all: the exit status, both output streams, and the output file's bytes if one appeared. |
| `ctx.link_ok(&link)?` | The link must exit 0, produce a file, and produce a file that parses. Returns a `Linked`. |
| `ctx.link_fails(&link, "what was being linked")?` | The link must exit non-zero **and** say something on stderr. Returns the `LinkRun` so the test can assert what the message names. |
| `ctx.reference_link(&link)?` | Link the same inputs with GNU ld, for the comparison leg. `Ok(None)` when there is no reference on the machine, or when the linker under test *is* the reference. |
| `ctx.run(&linked)?` / `ctx.run_with(&linked, &["arg"], b"stdin")?` | Execute the produced binary with a deadline. A signal or a timeout is its own failure kind. |
| `ctx.expect_output(&linked, "hello\n", 0)?` | Leg one: run it and assert stdout and exit status exactly. |
| `ctx.note("the linker chose a real GOT entry")` | An informational line shown under the test **whatever the outcome**, and carried in the JSON report's `notes`. This is where a documented divergence goes. |
| `ctx.skip("readelf is not installed")` | Decide at run time that the test does not apply. Always with a reason. |
| `ctx.tool("readelf")` / `ctx.run_tool("nm", &["-n", path])?` | The optional interop leg. |
| `ctx.rng`, `ctx.seed` | The seeded RNG. Every random choice must come from it, or `--seed` stops meaning anything. |
| `ctx.unique("prefix")`, `ctx.dir`, `ctx.timeout`, `ctx.linker_name` | The rest of the environment. |

### What `Link` gives a test

The order of the calls is the order of the argv, because in a linker command line order is
semantics:

```rust
Link::new()
    .object("a.o", bytes)             // writes the file AND passes it as an input
    .archive("libx.a", bytes)
    .write_file("libs/libx.a", bytes) // writes it WITHOUT passing it, for -L/-l to find
    .entry("main")                    // -e main       .entry_long("main")  // --entry=main
    .lib_dir("libs").lib("x")         // -L libs -l x
    .arg("--start-group")
    .out("prog")                      // default a.out
    .label("the second link")         // names this link in a failure message
```

### What `Check` gives a test

`eq` · `addr_eq` (hex on both sides) · `ne` · `at_least` · `at_most` ·
`that(path, expected, ok, actual)` · `bytes_eq(path, expected, actual)` ·
`mentions(path, needle, haystack)` · `observe(path, value)` · `note(line)` ·
`block(title, body)` · `hex(title, bytes, marks)` · `finish() -> Result<(), Failure>`.

And the shared assertions in `src/stages/helpers.rs`: `assert_runnable_layout(&linked)` (every
structural invariant a correct static executable satisfies), `assert_entry_is(&linked,
"_start")`, `assert_pc32(&mut c, &exe, path, site, target, addend)` and `assert_abs64(..)`,
which are how leg three is done honestly — the value the addresses *the linker itself chose*
imply.

### The fixtures

`src/stages/helpers.rs` builds the freestanding objects nearly every stage needs —
`print_and_exit`, `exit_only`, `caller`/`callee_returning`, `weak_callee_returning`,
`bss_prober`, `pointer_program` — and `src/elf/write.rs` builds anything else, including
objects that are deliberately wrong (`HeaderOverrides`, `truncate_to`, `offset_override`,
`size_override`, `rela_before_target`, `symtab_first`). `src/asm/mod.rs` is the
hand-assembled x86-64: every builder documents the instruction it encodes and records the
relocation its reference needs, at the right offset, so a fixture never counts bytes by hand.

## Deviations, divergences and skips

- **Nothing here is async.** A link is a subprocess and a linked program is a subprocess, so
  the other testers' `tokio` runtime would buy nothing. A test body is a plain function, and
  the deadline is enforced where a linker can actually hang: on every subprocess, and again
  between them. A test body that looped forever in Rust would not be caught — but that would
  be a bug in the suite, and `cargo test` is where those are caught.
- **A missing entry symbol is only *diagnosed*, not necessarily refused.** GNU ld prints
  `warning: cannot find entry symbol _start; defaulting to 0000000000401000` and exits **0**.
  Stage 23 therefore accepts either a non-zero exit **or** a diagnostic that names the entry
  symbol. A linker that treats it as an error passes too.
- **`GOTPCREL` relaxation is a choice, and both roads pass.** GNU ld 2.47 relaxes
  `R_X86_64_REX_GOTPCRELX` into `mov $imm32, %rax` and builds no GOT at all. Stage 31 asserts
  that the program obtains the right address at run time and notes which road the linker
  took.
- **`W^X` is asserted, section merging is not.** A linker that puts `.rodata` in the same
  read-execute segment as `.text` passes; one that puts anything writable in an executable
  segment fails, and the message says so in those words.
- **Input paths are made absolute** before they reach the linker, so a linker with its own
  working directory works unchanged. A test that wants to exercise relative-path handling
  cannot, and that is deliberate: the learner's program should never have to guess a cwd.
- **`response_hex` in the catalog is always empty** — see "Examples" above.
- **Skips.** Under `--validate` against GNU ld the suite runs everything and skips nothing on
  this machine: **341/341 passed, 0 failed, 0 skipped**. The only tests that can skip
  themselves are stage 42's interop tests, one per tool, when `readelf`, `nm` or `objdump` is
  not on `PATH`; each prints the missing tool as its reason.

### Where GNU ld does something a hand-written linker need not copy

Every expectation below was measured against GNU ld 2.47 on a real link before it was
written, and each one is loosened to the invariant *both* a correct hand-written linker and
GNU ld satisfy. Each carries a `ctx.note(..)` saying so, so the reasoning shows up in the run
and in the JSON report's `notes` rather than only here.

| Situation | What GNU ld does | What the suite requires |
|---|---|---|
| A missing `_start`, or `-e nosuch` (23) | warns, exits **0**, defaults the entry to the start of `.text` | a non-zero exit **or** a diagnostic naming the entry symbol |
| Two weak definitions (20) | takes the first on the command line | the result is one of the two; which one is noted |
| `R_X86_64_GOTPCREL` and friends (31) | relaxes to `lea` or `mov $imm32`, builds no GOT at all | the program obtains the right address at run time, whichever road was taken |
| An `ET_DYN` input (02) | accepts it as a shared library and writes an unrunnable dynamic image | a refusal, **or** an output that is not a static executable — never a silent success that looks right |
| A corrupt `e_shstrndx` (05) | warns, drops the input's sections, exits 0 | a refusal, a working binary, or at minimum a diagnostic — but never exit 0 with a broken binary and no word about it |
| A zero-length input (06) | reads it as an empty linker script and exits 0 | that length alone only has to leave nothing runnable; every length ≥ 1 must be refused |
| An object with `e_shnum == 0` (07) | refuses it | not required of anyone; the stage tests the case toolchains actually emit instead |
| An archive with **no** symbol index (36) | links anyway, scanning each member's own symtab | the same: a missing index is a missing cache, not a broken archive |
| An archive with a **stale** index (36) | follows it blindly and reports an undefined reference | either outcome; verifying the member after the lookup is the better behaviour and also passes |
| `-L` written *after* the `-l` that needs it (35) | still finds the library — every `-L` is collected before any `-l` is resolved | the successful link, with the two-pass requirement spelled out in the stage's hints |
| `STV_HIDDEN` in the output (22) | demotes the symbol to `STB_LOCAL` | only that it resolved and the program runs |
| Fuzzed metadata (40) | accepts about half of 400 mutations, because half the corrupted fields a static linker never reads | never a signal, never a hang, a diagnostic on every refusal, a parsable ELF64 on every acceptance |
| Section order, segment count, addresses (09, 12, 14) | its own conventions | only non-overlap, coverage, the page congruence, and command-line order for section contributions |

## Development

```
cargo build --release -p linktest
cargo test                                  # 61 unit + 30 integration tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
target/release/linktest --linker gnu_ld --validate --all   # 341/341, ~3.5 s
```

The whole suite runs in under four seconds, which is the point: a linker is a batch program,
so there is no server to boot and nothing to wait for. `--tag slow` selects the three stages
that do real work (200 objects, a 5 MB section, 400 fuzz mutations) and they still finish in
about two seconds between them.

Layout:

```
src/main.rs              CLI (clap), stage selection, --list
src/lib.rs               everything else, so tests/ can use it
src/config.rs            linkers.yaml, placeholder substitution
src/linker/mod.rs        resolving and invoking the linker under test
src/linker/reference.rs  finding GNU ld and recording its version
src/exec.rs              subprocess with a deadline, process-group kill, output draining
src/elf/mod.rs           the constants, the hex dump
src/elf/write.rs         the ELF64 relocatable-object writer (including malformed fixtures)
src/elf/read.rs          the ELF64 reader: objects and executables, bounds-checked throughout
src/elf/archive.rs       the System V/GNU archive writer and a reader for it
src/asm/mod.rs           hand-assembled x86-64, each builder documenting its encoding
src/link.rs              Link / LinkRun / Linked: one link, its inputs and its result
src/stages/mod.rs        the registry, Stage/Test/Ctx, the link_test! macro
src/stages/helpers.rs    the standard fixtures and the shared assertions
src/stages/sNN_*.rs      one file per stage
src/assert.rs            Check, Failure, FailureKind
src/report.rs            terminal output + JSON
src/catalog.rs           --list and catalog.json
src/examples/mod.rs      ExampleSpec: the worked examples a stage declares
src/examples/annotate.rs the byte walk that labels every field of an input object
tests/                   ELF round-trip, archive writer, instruction bytes, catalog, CLI
examples/broken_linker.rs  a linker that forgets the addend, to show the red path
PLAN.md                  tickboxes and hints for every stage
catalog.json             committed; a test fails if it is stale
```
