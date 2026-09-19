# shelltest stage plan

Tick a stage when `shelltest --shell ./my_shell --stage N` is green. `shelltest --list` reads these boxes.
Stages marked **[ext]** go beyond the core track (`--skip-ext` hides them).

Run one stage: `shelltest --shell ./my_shell --stage 5` — everything so far: `--until 12` — the lot: `--all`.

## A. Basics

- [ ] **Stage 01** — Print the prompt and wait for input (`01_prompt.yaml`, 6 tests)
  - Write the prompt to stdout and flush before blocking on a read
  - Read one line at a time; treat EOF as 'exit'
- [ ] **Stage 02** — Invalid command reports an error and continues (`02_invalid_command.yaml`, 6 tests)
  - Split the line on whitespace; the first word is the command
  - Write `<cmd>: command not found` to stderr, then loop again
- [ ] **Stage 03** — REPL: prompt after every command, EOF exits (`03_repl.yaml`, 8 tests)
  - Read-eval-print loop; never exit on an error
  - EOF (read returns 0 bytes) → exit with the last status
- [ ] **Stage 04** — exit builtin (`04_exit.yaml`, 7 tests)
  - Parse the optional numeric argument; default to 0 (or the last status)
  - Flush stdout before exiting
- [ ] **Stage 05** — echo builtin (`05_echo.yaml`, 9 tests)
  - Join arguments with a single space; the tokenizer already collapsed whitespace
  - `-n` suppresses the newline
- [ ] **Stage 06** — type for builtins (`06_type_builtins.yaml`, 8 tests)
  - Keep a table of builtin names; `type` checks it first
  - Output format: `<name> is a shell builtin`
- [ ] **Stage 07** — type for executables in PATH (`07_type_executables.yaml`, 8 tests)
  - Split PATH on ':'; stat each `dir/name` and check the execute bit
  - Stop at the first match; format `<name> is <absolute path>`
  - Not found → `<name>: not found` on stderr
- [ ] **Stage 08** — Run external programs with arguments (`08_run_executables.yaml`, 9 tests)
  - fork/exec/waitpid (or your language's spawn); pass argv verbatim
  - Search PATH yourself or use execvp; propagate the child's status into `$?`
  - Inherit stdin/stdout/stderr so output flows straight through
- [ ] **Stage 09** — pwd builtin (`09_pwd.yaml`, 6 tests)
  - getcwd() and print it
  - Keep it a builtin: the process's own cwd is what matters
- [ ] **Stage 10** — cd with absolute paths (`10_cd_absolute.yaml`, 7 tests)
  - chdir(); on failure print `cd: <path>: No such file or directory`
  - Do not change anything else on failure
- [ ] **Stage 11** — cd with relative paths (`11_cd_relative.yaml`, 8 tests)
  - chdir() already understands `.`/`..`; no path arithmetic needed
  - Print paths from getcwd() so they are canonical
- [ ] **Stage 12** — cd ~ and bare cd go to HOME (`12_cd_home.yaml`, 6 tests)
  - Bare `cd` and `cd ~` both mean $HOME
  - Read HOME from the environment, not from the passwd file

## B. Quoting & parsing

- [ ] **Stage 13** — Single quotes (`13_single_quotes.yaml`, 8 tests)
  - Tokenizer state machine: NORMAL → IN_SINGLE on `'`
  - Inside single quotes every byte is literal; adjacent quotes glue to the current word
- [ ] **Stage 14** — Double quotes (`14_double_quotes.yaml`, 9 tests)
  - Add IN_DOUBLE state; only `\"`, `\\`, `\$` (and backtick, newline) are escapes
  - Whitespace inside double quotes never splits words
- [ ] **Stage 15** — Backslash outside quotes (`15_backslash_outside_quotes.yaml`, 9 tests)
  - Outside quotes a backslash quotes the next character, whatever it is
  - `\ ` keeps two words together; the backslash itself is dropped
- [ ] **Stage 16** — Backslash inside single quotes is literal (`16_backslash_in_single_quotes.yaml`, 7 tests)
  - No escape processing at all inside single quotes
  - A backslash right before the closing quote is still literal
- [ ] **Stage 17** — Backslash inside double quotes (`17_backslash_in_double_quotes.yaml`, 9 tests)
  - Only `\"`, `\\`, `\$`, backtick and backslash-newline are special
  - Any other `\X` stays as two characters
- [ ] **Stage 18** — Executables with spaces or quotes in their names (`18_quoted_executable.yaml`, 8 tests)
  - Tokenize first, then resolve the command word: quoting is invisible to PATH lookup
  - Spaces and quotes in file names are ordinary bytes to execve()
- [ ] **Stage 19** — Adjacent quoted and unquoted segments concatenate (`19_concatenation.yaml`, 8 tests)
  - Words are accumulated across quote transitions, split only on unquoted whitespace
  - Empty quotes contribute nothing but do not end the word
- [ ] **Stage 20** — Variable expansion **[ext]** (`20_variables.yaml`, 9 tests)
  - Expand `$NAME`, `${NAME}`, `$?` while scanning outside single quotes
  - Undefined → empty string; keep a variable table separate from the environment
- [ ] **Stage 21** — Variable assignment and export **[ext]** (`21_assignments.yaml`, 9 tests)
  - `NAME=value` with no command sets a shell variable
  - `export` copies into the environment passed to execve
  - Prefix assignments apply to one child only
- [ ] **Stage 22** — Tilde expansion in arguments **[ext]** (`22_tilde.yaml`, 8 tests)
  - Expand a leading `~` (or `~/`) to HOME before glob and quoting removal
  - Quoted tildes are literal; unknown `~user` is left alone
- [ ] **Stage 23** — Globbing **[ext]** (`23_globbing.yaml`, 9 tests)
  - fnmatch/glob over the directory entries of each unquoted word containing `*?[`
  - Sort matches (C locale); no matches → keep the literal word; skip dotfiles

## C. Redirection

- [ ] **Stage 24** — Redirect stdout with > and 1> (`24_redirect_stdout.yaml`, 9 tests)
  - Recognise `>` / `1>` tokens and the following filename during parsing
  - open(O_WRONLY|O_CREAT|O_TRUNC, 0644) then dup2 onto fd 1 in the child (or around the builtin)
- [ ] **Stage 25** — Redirect stderr with 2> (`25_redirect_stderr.yaml`, 6 tests)
  - Same as `>` but dup2 onto fd 2
  - Builtins must write their own errors to the redirected fd too
- [ ] **Stage 26** — Append stdout with >> and 1>> (`26_append_stdout.yaml`, 7 tests)
  - O_APPEND instead of O_TRUNC
  - `1>>` is the same token
- [ ] **Stage 27** — Append stderr with 2>> (`27_append_stderr.yaml`, 7 tests)
  - O_APPEND on fd 2
  - Test both `>>` and `2>>` on the same command
- [ ] **Stage 28** — Input redirection with < **[ext]** (`28_input_redirect.yaml`, 7 tests)
  - open(O_RDONLY) and dup2 onto fd 0
  - Missing file → error message, command not run, shell continues
- [ ] **Stage 29** — 2>&1, &>, redirection placement, truncation on failure **[ext]** (`29_redirect_combos.yaml`, 9 tests)
  - Process redirections left to right; `2>&1` is dup2(1, 2) at that moment
  - `&>` opens once and dups to both fds
  - Perform redirections before checking the command exists so `>` truncates on failure

## D. Tab completion (pty)

- [ ] **Stage 30** — Tab completion of builtins (`30_completion_builtin.yaml`, 7 tests)
  - Put the terminal in raw mode (termios: ~ICANON, ~ECHO) and read key by key
  - Echo typed characters yourself; on TAB find builtins starting with the buffer and insert the rest plus a space
- [ ] **Stage 31** — Completion followed by typing arguments (`31_completion_with_args.yaml`, 6 tests)
  - Completion only edits the buffer; the rest of the line is typed normally
  - Redraw only what changed (or reprint the line) so the terminal stays consistent
- [ ] **Stage 32** — Completion with no match rings the bell (`32_completion_no_match.yaml`, 6 tests)
  - No candidates → write `\x07` and leave the buffer untouched
  - Keep the bell out of the buffer itself
- [ ] **Stage 33** — Tab completion of executables in PATH (`33_completion_executables.yaml`, 6 tests)
  - Scan every PATH dir for executables starting with the prefix
  - Dedupe names across dirs; builtins are candidates too
- [ ] **Stage 34** — Completion with multiple matches (`34_completion_multiple.yaml`, 7 tests)
  - First TAB with >1 candidates: bell; second TAB: newline, candidates sorted and joined by two spaces, then reprint prompt + buffer
  - Remember the previous key so you know it is the second TAB
- [ ] **Stage 35** — Partial completion to the longest common prefix (`35_completion_partial.yaml`, 7 tests)
  - Compute the longest common prefix of all candidates and insert what is missing
  - If the prefix already equals the buffer, fall back to the bell/list behaviour

## E. Pipelines

- [ ] **Stage 36** — Pipeline of two external commands (`36_pipeline_two_externals.yaml`, 6 tests)
  - Split on unquoted `|`; pipe(2) between neighbours; fork every stage before waiting on any
  - Close unused pipe ends in parent and children or readers never see EOF
- [ ] **Stage 37** — Builtins in pipelines (`37_pipeline_builtins.yaml`, 8 tests)
  - Run builtins in a forked child when they are part of a pipeline
  - The parent's state (cwd, variables) must not change from inside a pipeline
- [ ] **Stage 38** — Pipelines with three or more commands, run concurrently (`38_pipeline_many.yaml`, 7 tests)
  - N stages need N-1 pipes; keep a list of child pids and waitpid all of them
  - Never run a stage to completion before starting the next
- [ ] **Stage 39** — Pipeline exit status is the last command's **[ext]** (`39_pipeline_status.yaml`, 7 tests)
  - `$?` is the status of the last stage only
  - Ignore statuses of earlier stages (unless you add pipefail)
- [ ] **Stage 40** — Pipelines combined with redirections **[ext]** (`40_pipeline_redirect.yaml`, 7 tests)
  - Apply per-stage redirections after connecting the pipe so they win
  - A `>` on a middle stage starves the next one; that is correct

## F. History

- [ ] **Stage 41** — history lists numbered entries (`41_history.yaml`, 6 tests)
  - Append every non-empty line to an in-memory list
  - Format `%5d  %s`; the `history` command itself is already in the list
- [ ] **Stage 42** — history N shows the last N entries (`42_history_n.yaml`, 6 tests)
  - `history N` prints the last N entries with their absolute numbers
  - N larger than the list → everything; 0 → nothing
- [ ] **Stage 43** — Up/Down arrows recall history (`43_history_arrows.yaml`, 7 tests)
  - In raw mode, ESC `[` `A`/`B` are Up/Down
  - Keep a cursor into the history list; erase the current line and redraw the recalled one
  - Down past the newest entry restores an empty line
- [ ] **Stage 44** — history -r loads entries from a file (`44_history_read.yaml`, 6 tests)
  - `history -r FILE` appends each line of FILE to the list
  - Missing or empty files add nothing
- [ ] **Stage 45** — history -w writes the history to a file (`45_history_write.yaml`, 6 tests)
  - `history -w FILE` truncates and writes every entry, one per line
  - Include the `history -w` line itself
- [ ] **Stage 46** — history -a appends only new entries (`46_history_append.yaml`, 6 tests)
  - Track the index of the last entry written/appended; `-a` writes only entries after it
  - Update that index after each `-a` or `-w`
- [ ] **Stage 47** — HISTFILE is loaded on startup and written on exit (`47_histfile.yaml`, 7 tests)
  - If HISTFILE is set and readable, load it before the first prompt
  - On exit (builtin or EOF) append entries added this session

## G. Beyond the base track [ext]

- [ ] **Stage 48** — $? semantics: 0, non-zero, 127 not found, 126 not executable **[ext]** (`48_exit_status.yaml`, 8 tests)
  - 127 for not found, 126 for exists-but-not-executable, 1 for failed builtins
  - Bare `exit` uses the last status
- [ ] **Stage 49** — Command lists: ; && || with short-circuit **[ext]** (`49_lists.yaml`, 9 tests)
  - Parse the line into a list of pipelines joined by `;`, `&&`, `||`
  - Evaluate left to right, skipping based on the previous status
- [ ] **Stage 50** — Ctrl-C clears the line or kills the foreground child **[ext]** (`50_ctrl_c.yaml`, 7 tests)
  - SIGINT handler in the shell: discard the buffer and print a new prompt
  - Children should get SIGINT: put the foreground job in its own process group or reset the handler in the child
  - Status 128+2 after an interrupted child
- [ ] **Stage 51** — Background jobs: &, jobs, wait **[ext]** (`51_jobs.yaml`, 7 tests)
  - Trailing `&` → do not waitpid; record the pid in a job table
  - `jobs` lists the table; `wait` waitpids everything; reap finished jobs with WNOHANG
- [ ] **Stage 52** — Comments and blank lines **[ext]** (`52_comments.yaml`, 7 tests)
  - Unquoted `#` at the start of a word begins a comment to end of line
  - Blank and whitespace-only lines are no-ops
- [ ] **Stage 53** — Subshells and command substitution **[ext]** (`53_subshells.yaml`, 10 tests)
  - `( ... )` forks and evaluates the inside in the child
  - `$( ... )` runs the inside with stdout captured into a pipe and substitutes the text minus trailing newlines
- [ ] **Stage 54** — alias and unalias **[ext]** (`54_alias.yaml`, 9 tests)
  - Alias table consulted for the first word before command lookup
  - Re-tokenize the alias body; quoted command words are not expanded
- [ ] **Stage 55** — Line editing keys: Ctrl-A/E/U/W/L and arrows **[ext]** (`55_line_editing.yaml`, 9 tests)
  - Cursor position in the buffer separate from its length
  - Ctrl-A/E move, Ctrl-U/K/W delete ranges, Left/Right move, Backspace deletes before cursor
  - Redraw: carriage return, prompt, buffer, then reposition the cursor
- [ ] **Stage 56** — Script mode: file argument, -c, shebang **[ext]** (`56_script_mode.yaml`, 9 tests)
  - Non-tty or script argument → no prompts, read from the file instead of stdin
  - `-c STRING` evaluates one string; positional parameters `$1..` come after the script path
  - Exit with the last command's status
- [ ] **Stage 57** — Robustness: long lines, continuation prompt, zombies, binary output **[ext]** (`57_robustness.yaml`, 9 tests)
  - Grow line buffers dynamically; never assume a maximum length
  - Unterminated quote → print `> ` and keep reading lines
  - waitpid every child (WNOHANG sweep for background ones); treat output as bytes, not text

## H. The shell as a language [ext]

Stages 1–57 make an interactive shell. These make it a language you can write programs in:
everything a real script uses and nothing the earlier sections already covered.

- [ ] **Stage 58** — if, elif, else and the status that drives them **[ext]** (`58_conditionals.yaml`, 12 tests)
  - The condition is a command list; its exit status is what decides, not a truthy value
  - Zero is true; the `if` takes the status of whichever branch ran, or 0 when none did
  - Redirection on the whole compound applies to every command inside it
- [ ] **Stage 59** — test and [ : the conditions themselves **[ext]** (`59_test_builtin.yaml`, 10 tests)
  - `[` is a builtin whose last argument must be `]`; a missing one is an error, not a false
  - String, numeric (`-eq` and friends) and file (`-e -f -d -s`) tests, and `!`
  - One bare argument is true when non-empty — which is why unquoted variables bite
- [ ] **Stage 60** — while, until, break and continue **[ext]** (`60_loops.yaml`, 12 tests)
  - `until` is `while` with the condition inverted, both testing before every pass
  - `break n` / `continue n` act on the nth enclosing loop
  - The loop's status is its last body command; a loop that never ran is 0
- [ ] **Stage 61** — for: iterating a word list **[ext]** (`61_for_loops.yaml`, 12 tests)
  - The list is expanded once, before the first pass, with splitting and globbing
  - `for x; do` with no list walks the positional parameters
  - The variable keeps the last word after the loop ends
- [ ] **Stage 62** — case: patterns, not equality **[ext]** (`62_case.yaml`, 12 tests)
  - Patterns are globs (`*`, `?`, `[a-z]`), alternatives joined by `|`, first match wins
  - The word is expanded but never split, so a value with a space still matches
  - Quoting a pattern makes its glob characters literal
- [ ] **Stage 63** — Functions: definition, arguments and return **[ext]** (`63_functions.yaml`, 13 tests)
  - A call swaps in new positional parameters and restores the old ones on return
  - `return` sets the status and leaves the body; `exit` still leaves the shell
  - Functions are looked up before external programs, and share the caller's variables
- [ ] **Stage 64** — Positional parameters, $@ and shift **[ext]** (`64_positional_parameters.yaml`, 14 tests)
  - `"$@"` is one word per parameter; `"$*"` is one word; unquoted, both just split
  - `"$@"` with nothing to expand produces no word at all, where `"$*"` produces one empty one
  - `shift [n]` renumbers and fails rather than emptying; `set --` replaces the list
- [ ] **Stage 65** — Parameter expansion: defaults, length and trimming **[ext]** (`65_parameter_expansion.yaml`, 14 tests)
  - `:-` `:=` `:+` `:?`, and what the colon changes about an empty-but-set value
  - `${#v}` for length; `#` `##` `%` `%%` trim shortest and longest glob matches
  - The replacement word is itself expanded
- [ ] **Stage 66** — Arithmetic expansion with $(( )) **[ext]** (`66_arithmetic.yaml`, 13 tests)
  - Integer arithmetic that truncates toward zero, with C precedence and a ternary
  - Names inside need no `$`; unset and empty both count as zero
  - Comparisons yield 1 and 0; division by zero is an error
- [ ] **Stage 67** — Here-documents **[ext]** (`67_heredocs.yaml`, 12 tests)
  - The body is read from the lines after the command, ending at the delimiter alone on a line
  - An unquoted delimiter expands the body like a double-quoted string; quoting it stops that
  - `<<-` strips leading tabs; two here-documents on one line are filled in order
- [ ] **Stage 68** — read, IFS and field splitting **[ext]** (`68_read_and_ifs.yaml`, 12 tests)
  - Fields split on `IFS`; the last variable keeps the remainder; read fails at end of input
  - Whitespace `IFS` collapses runs and trims; other separators make empty fields real
  - `-r` stops backslash from continuing lines and from being eaten
- [ ] **Stage 69** — trap, exec and numbered file descriptors **[ext]** (`69_traps_and_fds.yaml`, 14 tests)
  - `trap` on EXIT and on signals, `trap ''` to ignore, `trap -` to restore
  - `exec 3>file` opens a descriptor for the shell itself; `3>&-` closes it
  - Redirections apply left to right, which is what makes `2>&1 >f` differ from `>f 2>&1`
- [ ] **Stage 70** — set -e, -u, -x and the option flags **[ext]** (`70_set_options.yaml`, 14 tests)
  - `-e` exits on an untested failure, but a condition, `||`, `!` and a loop test are all tested
  - `-u` makes an unset name an error; `-f` disables globbing; `-x` traces after expansion
  - `$-` reports the flags; the same letters work on the command line

**70 stages, 592 tests.**
