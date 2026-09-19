# shelltest

A self-contained, stage-by-stage tester for POSIX shells, written in Rust. It drives
**any** shell binary as a black box (stdin/stdout/stderr, or a real pseudo-terminal) and
compares what it sees with a declarative YAML suite: 70 stages, 592 tests, all validated
against bash 5.

```
cargo build --release
./target/release/shelltest --shell bash --all            # sanity-check the suite (all green)
./target/release/shelltest --shell ./my_shell --stage 1  # your first red/green
./target/release/shelltest --shell ./my_shell --until 12
./target/release/shelltest --list                        # stages, counts, PLAN.md tickboxes
```

No network, no Docker, no accounts. Linux and macOS.

## What a run looks like

```
Stage 05 echo builtin
  ✔ echo prints its arguments (4 ms)
  ✘ echo -n suppresses the trailing newline FAIL (4 ms)
      stdout: output differs
    ┌ input sent on stdin
    │ echo -n abc
    │ echo def
    │ exit 0
    ┌ expected
    │ stdout: "abcdef"
    ┌ actual (normalized)
    │ stdout: "-n abc\ndef"
    │ stderr: ""
    │ exit code: "0"
    ┌ diff stdout  (-expected  +actual)
    │ -abcdef
    │ +-n abc
    │ +def
Stage 05  echo builtin                       7/9 passed
```

Run `shelltest --shell examples/broken_shell.sh --until 5` to see a deliberately broken
shell fail. Add `--keep-tmp` to keep each test's sandbox directory, `--verbose` to see
input/output for passing tests too, `--json report.json` for a machine-readable report.

## CLI

```
shelltest --shell <name|path> [--stage N] [--until N] [--from N] [--all]
          [--only "substring"] [--tag ext] [--skip-ext]
          [--verbose] [--keep-tmp] [--timeout-ms N]
          [--validate] [--list] [--json report.json] [--no-color]
          [--tests-dir path] [--shells-file path]
```

- `--shell` takes a name from `shells.yaml` (`bash`, `zsh`, `dash`, `my_shell`) or a path.
  A path is used as the command in both pipe and pty mode.
- `--validate` runs everything against a *registered reference shell* and words failures
  as suite bugs. `shelltest --shell bash --validate` must be all green.
- `--skip-ext` hides tests tagged `ext` (stages beyond the core track);
  `--tag ext` runs only those.
- Exit code is 0 only when every selected test passed (2 on a harness/usage error).
- `shells.yaml` and `tests/stages` are found relative to the current directory or to the
  binary (`target/release/..`); override with `--shells-file` / `--tests-dir`.
- Colors honour `NO_COLOR` and `--no-color`.

## Conventions your shell must follow

The suite encodes the "build your own shell" conventions. Where bash differs
the expectation is loosened (`contains`/`regex`) so both pass; where it cannot be, the test
is `skip_on: [bash]` with a reason. Your shell needs to:

| Behaviour | Convention |
|---|---|
| Prompt | Print `$ ` (dollar, space) to **stdout** before reading each line, also when stdin is a pipe. The harness strips `$ ` at line starts, so printing it costs nothing. |
| Unknown command | `<cmd>: command not found` on **stderr**, keep running, `$?` = 127. |
| EOF on stdin | Exit with the last command's status (0 after a successful command). Tests only assert 0 after a success. |
| `exit [N]` | Exit with N; bare `exit` = 0 or the last status (both accepted; stage 48 requires last status). |
| `type` | `X is a shell builtin` / `X is /abs/path/X` on stdout; `X: not found` on stderr. Check the execute bit; the first PATH match wins; builtins beat executables. |
| `cd` errors | `cd: <path>: No such file or directory` on stderr; cwd unchanged. |
| `echo -n` | Suppress the newline. |
| `history` | `%5d  %s` per entry (five-wide number, two spaces), 1-based, including the `history` command itself. `-r/-w/-a FILE` as in bash. HISTFILE is loaded at startup and new entries appended at exit. |
| Completion (pty) | TAB completes builtins and PATH executables and appends one space. No match → bell (`\x07`), line unchanged. Several matches → first TAB bell, second TAB prints matches sorted and separated by two spaces on a new line, then the prompt and partial input again. Shared prefix → complete to the longest common prefix. |
| Line editing (pty) | Up/Down recall history; Ctrl-C at the prompt prints a new prompt and discards the line; Ctrl-C during a foreground command kills it (status 130) and the shell survives. |
| Quoting | POSIX: single quotes literal; in double quotes only `\"`, `\\`, `\$`, `` \` `` and backslash-newline are special; outside quotes a backslash escapes any character; adjacent segments concatenate. |
| Redirections | `>`, `1>`, `>>`, `1>>`, `2>`, `2>>` anywhere in the command; `>` truncates even if the command fails or is not found. `[ext]`: `<`, `2>&1`, `&>`. |
| Pipelines | All stages started before any is waited on; builtins allowed at either end; status of the pipeline is the last stage's `[ext]`. |

Sandbox: every test runs in a fresh temp dir with `cwd={TMP}`, `PATH={TMP}/bin[:host PATH]`,
`HOME={TMP}/home`, `HISTFILE={TMP}/.history`, `TERM=dumb`, `LANG=LC_ALL=C` and nothing else
in the environment. Fixture executables are `#!/bin/sh` scripts. Host tools used by tests
are limited to `sh cat ls wc sleep true false head`; anything that inspects PATH uses
`path_isolated: true`, which removes the host PATH entirely.

## Normalization (what "exact" means)

Before comparing, the harness normalizes captured text. Pipe-mode stdout/stderr:

1. `{TMP}`, `{BIN}`, `{HOME}`, `{HISTFILE}`, `{SHELL}` placeholders are substituted in
   inputs, fixtures and expectations (`${HOME}` with a `$` in front is left alone).
2. The prompt (`$ `, configurable per shell) is stripped wherever a line starts with it,
   repeatedly, and at the end of the output.
3. Trailing whitespace on each line and trailing blank lines are removed. Expected
   strings get the same trimming, so YAML block scalars with a final newline are fine.
4. ANSI escapes and `\r` are stripped only if `normalize: { strip_ansi: true }` is set.

Pty-mode `terminal` is the merged stream: ANSI/VT escape sequences and `\r` are removed,
trailing newlines dropped; the prompt and spaces are **kept** (tests assert on them).
Bells (`\x07`), backspaces etc. survive and are shown as `^G`, `^H` in reports. The
`files:` expectations compare raw file contents with no normalization at all.

Per-test overrides: `normalize: { strip_prompt: false, strip_ansi: true, strip_cr: true, trim_lines: false }`.

## Test file format

One file per stage in `tests/stages/NN_name.yaml`:

```yaml
stage: 5
name: "echo builtin"
tests:
  - name: "echo prints its arguments"
    mode: pipe                      # default; or pty
    fixtures:
      files: { "in.txt": "hello\n", "{HOME}/.rc": "" }
      dirs: [sub/dir]
      executables:                  # placed in {BIN}; "#!/bin/sh" prepended if missing
        greet: 'echo "hi $1"'
        alt/prog: "echo x"          # a path puts it under {TMP}/alt instead
      path_isolated: true           # PATH = {BIN} only
      env: { FOO: "bar" }
      histfile: "echo old\n"        # initial HISTFILE contents
      cwd: sub/dir                  # start directory, relative to {TMP}
    input: ["greet bob", "exit 0"]  # pipe mode: lines written to stdin, then EOF
    shell_args: ["-c", "echo hi"]   # extra argv for the shell (script mode tests)
    timeout_ms: 5000
    tags: [ext]
    skip_on: [zsh]                  # requires a reason
    reason: "zsh words the message differently"
    expect:
      stdout: "hi bob"              # plain string = exact after normalization
      stderr: { contains: "x" }     # or regex | not_contains | lines_unordered | lines_ordered_subset
      exit_code: 0
      files: { "{TMP}/out.txt": "hi bob\n" }
      file_absent: ["{TMP}/nope"]

  - name: "tab completes a builtin"
    mode: pty
    keys: ["ech\t", "hello\r", "exit\r"]   # sugar: wait for prompt, send each item;
                                           # after items ending in \r wait for the next prompt,
                                           # after others pause key_delay_ms (default 100)
    expect:
      terminal: { contains: "$ echo hello\nhello\n" }

  - name: "ctrl-c kills a foreground child"
    mode: pty
    steps:                          # explicit form when the prompt is not what you wait for
      - wait: "$ "
      - send: "sleep 5\r"
      - sleep_ms: 300
      - send: "\x03"
      - wait: "$ "
      - send: "exit\r"
    expect:
      terminal: { contains: "$ " }
      exit_code: 0
```

Use YAML double quotes for control keys: `"\t"`, `"\r"`, `"\x1b[A"` (Up), `"\x1b[B"`
(Down), `"\x03"` (Ctrl-C), `"\x04"` (Ctrl-D), `"\x15"` (Ctrl-U), `"\x7f"` (Backspace).
Use single quotes for shell lines full of backslashes (`'echo a\\b'`).

The schema is validated at load time (unknown keys, wrong modes, `skip_on` without a
reason, an empty `expect`) and the error names the file, the test and the key.

Pty mode: the shell runs as session leader with the pty as its controlling terminal
(120x24, `TERM=dumb`). If the shell is still running after the last step it is stopped
by the harness and the exit code shows `(still running; stopped by harness)`; send
`exit\r` when you want to assert on the exit code. A `wait` that never matches fails the
test as a timeout with the partial terminal shown.

Timeouts: 5 s by default (`--timeout-ms`, or `timeout_ms` per test). A hung shell gets
SIGHUP, SIGTERM, then SIGKILL and the test is reported as a timeout with partial output.

## Registering a shell

`shells.yaml`:

```yaml
shells:
  my_shell:
    pipe_command: ["./my_shell"]         # relative paths resolve against your cwd
    pty_command:  ["./my_shell"]
    prompt: "$ "
    env: { MY_SHELL_NO_COLOR: "1" }      # optional extras
    init_script: |                       # optional; written to {INIT} before every test
      echo "sourced if your shell reads $MY_SHELL_RC"
```

The bash entry launches `bash --norc --noprofile` with `BASH_ENV` pointing at an init
script that disables command hashing (so `type` prints paths), enables alias expansion
and turns on history so non-interactive bash behaves like the interactive shell the
tests describe. zsh runs with `BSD_ECHO` and without `PROMPT_SP`/`PROMPT_CR`. dash is
declared but not installed on every system.

## Reference shells and `skip_on`

- **bash 5.x**: `shelltest --shell bash --validate` passes 427/427 (one skip). The only
  `skip_on: [bash]` is stage 7 "a non-executable file in PATH is not reported": bash's
  `type` falls back to non-executable files.
- **zsh 5.9**: 47 tests are `skip_on: [zsh]` because zsh words errors differently
  (`command not found: x`, `cd: no such file or directory: x`, `x not found`) and its
  `history` builtin (`fc -l`) is unavailable non-interactively. With those skipped zsh
  passes ~350 of the remaining tests; the rest are genuine divergences left unskipped on
  purpose: zsh's completion menus (stages 30–35), `~unknownuser` and dotfile globbing
  errors, HISTFILE format (stage 47) and `%` partial-line handling in stage 57.
- **dash**: no completion, history or line editing, so stages 30–35, 41–47, 50, 55 cannot
  pass; the basics do. No `skip_on: [dash]` entries are shipped because dash was not
  available to validate them.

## Adding tests

Drop a test into the right `tests/stages/NN_*.yaml` (or a new file with a new stage
number), then run `shelltest --shell bash --validate --stage NN` to prove bash agrees
with your expectation before running it against your own shell. Prefer fixture
executables over host tools, use `path_isolated: true` for anything touching PATH, and
never rely on timing except through explicit `wait`/`sleep_ms` steps.

## Development

```
cargo test          # unit tests per module + tests/selftest.rs (runs the suite on bash)
cargo clippy
cargo build --release
```

Layout:

```
src/main.rs          CLI (clap), stage selection, --list
src/config.rs        shells.yaml
src/loader.rs        YAML stages + schema validation
src/fixtures.rs      sandbox dir, fake executables, environment, placeholders
src/runner/mod.rs    one test: sandbox → launch → capture → normalize → assert
src/runner/pipe.rs   piped child, poll-based non-blocking I/O, timeout/kill
src/runner/pty.rs    openpty, setsid, TIOCSCTTY, send/wait/sleep steps (the only unsafe)
src/normalize.rs     prompt/ANSI stripping, placeholders
src/matchers.rs      exact/regex/contains/not_contains/lines_*
src/report.rs        colored output, unified diffs, JSON
tests/selftest.rs    runs the YAML suite against bash
tests/stages/        57 stage files
examples/broken_shell.sh
PLAN.md              tickboxes per stage with implementation hints
```
