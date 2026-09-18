import type { TrackId } from './types';

export interface Convention {
	id: string;
	topic: string;
	rule: string;
	stages: number[];
	source: string;
}

/** Excerpted from shelltest/README.md "Conventions your shell must follow". */
const shell: Convention[] = [
	{
		id: 'prompt',
		topic: 'Prompt',
		rule: 'Print `$ ` (dollar, space) to stdout before reading each line, also when stdin is a pipe. The harness strips `$ ` at line starts, so printing it costs nothing.',
		stages: [1, 2, 3, 57],
		source: 'shelltest/README.md'
	},
	{
		id: 'not-found',
		topic: 'Unknown command',
		rule: '`<cmd>: command not found` on stderr, keep running, `$?` = 127.',
		stages: [2, 3, 48],
		source: 'shelltest/README.md'
	},
	{
		id: 'eof',
		topic: 'EOF on stdin',
		rule: "Exit with the last command's status (0 after a successful command).",
		stages: [3, 4, 47, 56],
		source: 'shelltest/README.md'
	},
	{
		id: 'exit',
		topic: '`exit [N]`',
		rule: 'Exit with N; bare `exit` = 0 or the last status (both accepted; stage 48 requires the last status).',
		stages: [4, 48],
		source: 'shelltest/README.md'
	},
	{
		id: 'type',
		topic: '`type`',
		rule: '`X is a shell builtin` / `X is /abs/path/X` on stdout; `X: not found` on stderr. Check the execute bit; first PATH match wins; builtins beat executables.',
		stages: [6, 7, 33],
		source: 'shelltest/README.md'
	},
	{
		id: 'cd',
		topic: '`cd` errors',
		rule: '`cd: <path>: No such file or directory` on stderr; cwd unchanged.',
		stages: [10, 11, 12],
		source: 'shelltest/README.md'
	},
	{
		id: 'echo-n',
		topic: '`echo -n`',
		rule: 'Suppress the trailing newline.',
		stages: [5],
		source: 'shelltest/README.md'
	},
	{
		id: 'quoting',
		topic: 'Quoting',
		rule: 'POSIX: single quotes literal; in double quotes only `\\"`, `\\\\`, `\\$`, backtick and backslash-newline are special; outside quotes a backslash escapes any character; adjacent segments concatenate.',
		stages: [13, 14, 15, 16, 17, 18, 19],
		source: 'shelltest/README.md'
	},
	{
		id: 'redirection',
		topic: 'Redirections',
		rule: '`>`, `1>`, `>>`, `1>>`, `2>`, `2>>` anywhere in the command; `>` truncates even if the command fails or is not found. [ext]: `<`, `2>&1`, `&>`.',
		stages: [24, 25, 26, 27, 28, 29, 40],
		source: 'shelltest/README.md'
	},
	{
		id: 'completion',
		topic: 'Completion (pty)',
		rule: 'TAB completes builtins and PATH executables and appends one space. No match → bell (`\\x07`), line unchanged. Several matches → first TAB bell, second TAB prints matches sorted and separated by two spaces on a new line, then prompt and partial input again. Shared prefix → complete to the longest common prefix.',
		stages: [30, 31, 32, 33, 34, 35],
		source: 'shelltest/README.md'
	},
	{
		id: 'history',
		topic: '`history`',
		rule: '`%5d  %s` per entry (five-wide number, two spaces), 1-based, including the `history` command itself. `-r/-w/-a FILE` as in bash. HISTFILE is loaded at startup and new entries appended at exit.',
		stages: [41, 42, 44, 45, 46, 47],
		source: 'shelltest/README.md'
	},
	{
		id: 'line-editing',
		topic: 'Line editing (pty)',
		rule: 'Up/Down recall history; Ctrl-C at the prompt prints a new prompt and discards the line; Ctrl-C during a foreground command kills it (status 130) and the shell survives.',
		stages: [43, 50, 55],
		source: 'shelltest/README.md'
	},
	{
		id: 'pipelines',
		topic: 'Pipelines',
		rule: 'All stages started before any is waited on; builtins allowed at either end; the status of the pipeline is the last stage’s [ext].',
		stages: [36, 37, 38, 39, 40],
		source: 'shelltest/README.md'
	},
	{
		id: 'sandbox',
		topic: 'Sandbox',
		rule: 'Every test runs in a fresh temp dir with `cwd={TMP}`, `PATH={TMP}/bin[:host PATH]`, `HOME={TMP}/home`, `HISTFILE={TMP}/.history`, `TERM=dumb`, `LANG=LC_ALL=C` and nothing else in the environment.',
		stages: [],
		source: 'shelltest/README.md'
	},
	{
		id: 'normalization',
		topic: 'Normalization',
		rule: 'Placeholders are substituted, the prompt is stripped wherever a line starts with it, trailing whitespace and blank lines are removed. Pty `terminal` keeps the prompt and spaces but drops ANSI escapes and `\\r`.',
		stages: [],
		source: 'shelltest/README.md'
	}
];

/** Excerpted from BuildYourOwn/PLAN.md §1.2. */
const kafka: Convention[] = [
	{
		id: 'startup',
		topic: 'Startup',
		rule: 'Started as `./your_program.sh /tmp/server.properties` (argv[1] = properties file, written by the harness). Listens on `0.0.0.0:9092`; `brokers.yaml` can override with `{PORT}`.',
		stages: [1],
		source: 'PLAN.md §1.2'
	},
	{
		id: 'logdir',
		topic: 'Log directory',
		rule: '`/tmp/kraft-combined-logs` (overridable via `{LOGDIR}`), Kafka on-disk format: `__cluster_metadata-0/00000000000000000000.log`, `<topic>-<n>/00000000000000000000.log`, `partition.metadata`, `meta.properties`. The harness writes these fixtures before start.',
		stages: [12, 13, 23, 24, 34, 35],
		source: 'PLAN.md §1.2'
	},
	{
		id: 'framing',
		topic: 'Framing',
		rule: '4-byte big-endian size prefix + header v2 (flexible) or v1, as the API dictates.',
		stages: [2, 3, 6, 8, 9],
		source: 'PLAN.md §1.2'
	},
	{
		id: 'errors',
		topic: 'Error codes',
		rule: '`UNSUPPORTED_VERSION` = 35, `UNKNOWN_TOPIC_OR_PARTITION` = 3, `UNKNOWN_TOPIC_ID` = 100. Everything else follows the official protocol guide.',
		stages: [4, 11, 21, 30],
		source: 'PLAN.md §1.2'
	},
	{
		id: 'divergence',
		topic: 'Where a hand-written broker and real Kafka differ',
		rule: 'The expectation is loosened so both pass, or the test is `skip_on: [apache_kafka]` with a written reason — exactly as shelltest does.',
		stages: [],
		source: 'PLAN.md §1.2'
	},
	{
		id: 'flexible',
		topic: 'Flexible versions',
		rule: 'Compact arrays and strings are length+1 as an unsigned varint; every flexible struct ends with a tagged-field buffer (`0x00` when empty).',
		stages: [3, 5, 10, 11, 16, 19, 29],
		source: 'KIP-482'
	}
];

/** Excerpted from BuildYourOwn/PLAN.md §5.3 — the program contract wasmtest drives. */
const wasm: Convention[] = [
	{
		id: 'invoke',
		topic: 'How your runtime is called',
		rule: '`./your_program.sh run --invoke <export> <module.wasm> [args...]` prints one result per line; `./your_program.sh run <module.wasm> [--] [args...]` runs `_start` as a WASI command.',
		stages: [1, 7],
		source: 'PLAN.md §5.3'
	},
	{
		id: 'traps',
		topic: 'Traps',
		rule: 'A trap exits non-zero with `wasm trap: <canonical reason>` on stderr, using the spec’s own strings — "integer divide by zero", "out of bounds memory access", "unreachable".',
		stages: [3, 4, 5],
		source: 'PLAN.md §5.3'
	},
	{
		id: 'validate-first',
		topic: 'Decode and validate before you run',
		rule: 'A module that fails to decode or validate exits non-zero **before executing anything** — no partial effects, no output from a function that should never have started.',
		stages: [1, 2],
		source: 'PLAN.md §5.3'
	},
	{
		id: 'real-bytes',
		topic: 'The modules are real binaries',
		rule: 'The tester builds every module with its own encoder (no wabt, no .wat), so every test is exact bytes and a failure can print the module’s hex.',
		stages: [],
		source: 'PLAN.md §5.3'
	},
	{
		id: 'reference',
		topic: 'The reference',
		rule: '`wasmtest --runtime wasmtime --validate` must be all green: whatever the suite expects, the real wasmtime does.',
		stages: [],
		source: 'PLAN.md §5.1'
	}
];

/** Excerpted from BuildYourOwn/PLAN.md §5.4 — the `openssl s_server` flag subset. */
const tls: Convention[] = [
	{
		id: 'accept',
		topic: 'How your server is called',
		rule: '`./your_program.sh -accept <port> -cert <cert.pem> -key <key.pem> -rev [-naccept <n>]`. Certificates and keys (RSA-2048, ECDSA P-256, Ed25519) are generated per run by the harness.',
		stages: [1],
		source: 'PLAN.md §5.4'
	},
	{
		id: 'rev',
		topic: '`-rev` is the application layer',
		rule: 'Every line received is echoed back reversed. That is the whole data path, and it is what makes application data deterministic to test.',
		stages: [5],
		source: 'PLAN.md §5.4'
	},
	{
		id: 'legacy',
		topic: 'The fields that lie',
		rule: 'A TLS 1.3 record still says `legacy_version` 0x0303, the ServerHello still echoes a `legacy_session_id`, and a `change_cipher_spec` byte is still sent. The real version lives in the `supported_versions` extension.',
		stages: [1, 2],
		source: 'RFC 8446 §4.1.2, §5.1'
	},
	{
		id: 'schedule',
		topic: 'The key schedule',
		rule: 'HKDF-Extract and Derive-Secret exactly as RFC 8446 §7.1 lays them out, with `HkdfLabel` byte-identical ("tls13 " + the label) and the right transcript boundary for each secret.',
		stages: [3],
		source: 'RFC 8446 §7.1'
	},
	{
		id: 'client',
		topic: 'What the tester is',
		rule: 'A raw TLS 1.3 client written for this suite — X25519, HKDF, transcript hashes, AEAD and record framing all explicit — so a failure can show the exact bytes, the secrets’ labels and the transcript it hashed.',
		stages: [],
		source: 'PLAN.md §5.4'
	}
];

/** Excerpted from BuildYourOwn/PLAN.md §5.5 — the GNU ld flag subset. */
const link: Convention[] = [
	{
		id: 'cli',
		topic: 'How your linker is called',
		rule: '`./your_program.sh -o <out> [-e <entry>] [-L <dir>] [-l <name>] <input.o|input.a>...` — the GNU ld flag subset, so `/usr/bin/ld` is the reference.',
		stages: [1, 2],
		source: 'PLAN.md §5.5'
	},
	{
		id: 'runs',
		topic: 'The output has to run',
		rule: 'The tester links your output, **executes it**, and checks its stdout and exit status — then re-parses the file with its own ELF reader. An executable that only looks right is not right.',
		stages: [2],
		source: 'PLAN.md §5.5'
	},
	{
		id: 'inputs',
		topic: 'The inputs are written by the harness',
		rule: 'Every `.o` and `.a` comes from the tester’s own ELF64 writer, so there is no dependency on binutils at test time and the bytes are exactly what the test meant.',
		stages: [],
		source: 'PLAN.md §5.5'
	},
	{
		id: 'target',
		topic: 'The target',
		rule: 'Static ELF64 for x86-64: `ET_REL` in, `ET_EXEC` out, little-endian, with the x86-64 psABI relocation types.',
		stages: [1, 4],
		source: 'PLAN.md §5.5'
	},
	{
		id: 'overflow',
		topic: 'Relocation overflow',
		rule: 'A relocation whose computed value does not fit its field is an error to report, not a truncation to write — that is the difference between a linker and a memcpy.',
		stages: [4],
		source: 'x86-64 psABI'
	}
];

/** The distributed-systems track's contract (PLAN.md Part 5, the `dist` ladders). */
const dist: Convention[] = [
	{
		id: 'ladders',
		topic: 'Three ladders',
		rule: 'The trail climbs in three: the **primitives** (clocks, quorums, probabilistic structures, CRDTs, failure detection), then a **durable node**, then a **cluster**. Each rung assumes the one below it works.',
		stages: [],
		source: 'the dist track'
	},
	{
		id: 'linearizable',
		topic: 'What you are building',
		rule: 'A replicated, linearizable key-value store — not eventually-consistent, not best-effort. The last rung checks a recorded history against a linearizability model rather than a golden file.',
		stages: [],
		source: 'the dist track'
	},
	{
		id: 'determinism',
		topic: 'Faults are injected, and seeded',
		rule: 'Partitions, pauses and crashes are injected mid-run from a seed, so a failure replays exactly. A test that only fails sometimes is a test that has not been written yet.',
		stages: [],
		source: 'PLAN.md §5.2'
	},
	{
		id: 'durability',
		topic: 'Durability means after `kill -9`',
		rule: 'Everything acknowledged must still be there after the process dies without warning — not after a clean shutdown.',
		stages: [],
		source: 'the dist track'
	}
];

export const conventions: Record<TrackId, Convention[]> = { shell, kafka, wasm, tls, link, dist };

export function conventionsForStage(track: TrackId, stage: number): Convention[] {
	return conventions[track].filter((c) => c.stages.includes(stage));
}
