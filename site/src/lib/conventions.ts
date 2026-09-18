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

export const conventions: Record<TrackId, Convention[]> = { shell, kafka };

export function conventionsForStage(track: TrackId, stage: number): Convention[] {
	return conventions[track].filter((c) => c.stages.includes(stage));
}
