/** Shared shapes for the catalogs, the testers' JSON reports and the resource library. */

// The set of tracks is the registry's business, not this file's.
export type { TrackId } from './tracks';
import type { TrackId } from './tracks';

export type ExpectKind =
	| 'exact'
	| 'contains'
	| 'regex'
	| 'not_contains'
	| 'lines_unordered'
	| 'lines_ordered_subset'
	| 'absent';

export interface ExpectRow {
	label: string;
	kind: ExpectKind;
	value: string;
}

export interface TestStep {
	op: string;
	value: string;
}

export interface TestSpec {
	name: string;
	ext?: boolean;
	tags?: string[];
	skipOn?: string[];
	reason?: string;
	mode?: 'pty' | 'pipe';
	input?: string[];
	keys?: string[];
	steps?: TestStep[];
	expect?: ExpectRow[];
}

export interface StageSpec {
	number: number;
	slug: string;
	name: string;
	ext: boolean;
	section: string;
	file: string;
	planDone: boolean;
	/** True when the stage is in the tester's PLAN.md but not yet in its catalog.json. */
	planned?: boolean;
	/** The rung of the track's ladder this stage sits on (dist: primitives|node|cluster). */
	ladder?: string;
	hints: string[];
	tests: TestSpec[];
	/**
	 * Worked examples emitted by kafkatest's catalog (snake_case, straight from the Rust
	 * harness). Shape-checked at read time by `src/lib/examples/kafka.ts`, never here: the
	 * field may be absent, empty, or carry keys this build has never heard of.
	 *
	 * The generated catalog does **not** carry them inline — `npm run sync` splits them into
	 * `src/lib/data/examples/kafka/<n>.json` so they stay out of the shared bundle — but the
	 * field is still honoured when something (a test, a fixture) supplies them directly.
	 */
	examples?: unknown[];
	/** How many worked examples the split file holds; written by `npm run sync`. */
	exampleCount?: number;
}

export interface SectionSpec {
	id: string;
	title: string;
	stages: number[];
}

export interface Catalog {
	track: TrackId;
	generatedAt: string;
	pending?: boolean;
	source?: string;
	sections: SectionSpec[];
	stages: StageSpec[];
	totals: { stages: number; tests: number; ext: number };
}

/* ---------- tester reports (shelltest/src/report.rs :: JsonReport) ---------- */

export type TestStatus = 'pass' | 'fail' | 'skip';

export interface RawJsonTest {
	name: string;
	status: TestStatus;
	ext: boolean;
	duration_ms: number;
	failures: string[];
	skip_reason: string | null;
	actual: [string, string][];
}

export interface RawJsonStage {
	stage: number;
	name: string;
	file: string;
	passed: number;
	failed: number;
	skipped: number;
	tests: RawJsonTest[];
}

export interface RawJsonReport {
	/** shelltest writes `shell`, kafkatest writes `target`. */
	shell?: string;
	target?: string;
	validate: boolean;
	stages: RawJsonStage[];
	passed: number;
	failed: number;
	skipped: number;
	elapsed_ms: number;
}

/** A report after import: same data, normalized names and indexed for lookup. */
export interface ReportTest {
	name: string;
	status: TestStatus;
	ext: boolean;
	durationMs: number;
	failures: string[];
	skipReason: string | null;
	actual: [string, string][];
}

export interface ReportStage {
	stage: number;
	name: string;
	file: string;
	passed: number;
	failed: number;
	skipped: number;
	tests: ReportTest[];
}

export interface Report {
	track: TrackId;
	target: string;
	validate: boolean;
	passed: number;
	failed: number;
	skipped: number;
	elapsedMs: number;
	stages: ReportStage[];
	importedAt: string;
	/** Where the report came from: the byo database, the dev-server poll, or local storage. */
	origin: 'api' | 'live' | 'file' | 'fixture';
}

/* ---------- resources (BuildYourOwn/PLAN.md §2.5) ---------- */

export type ResourceType =
	| 'spec'
	| 'doc'
	| 'book'
	| 'article'
	| 'video'
	| 'course'
	| 'repo'
	| 'tool'
	| 'paper';
export type ResourceLevel = 'intro' | 'core' | 'deep';

export interface Resource {
	id: string;
	title: string;
	url: string;
	type: ResourceType;
	level: ResourceLevel;
	track: TrackId | 'both';
	stages: number[];
	concepts: string[];
	why: string;
	minutes?: number;
	free: boolean;
	checkedAt?: string;
}

/* ---------- derived UI state ---------- */

export type StageState = 'locked' | 'next' | 'in-progress' | 'done' | 'failing' | 'available';

/* ---------- worked examples (PLAN.md §4.2) ---------- */

/** One line of a shell transcript, derived from a catalog test. */
export interface ShellExampleLine {
	kind: 'input' | 'key' | 'stdout' | 'stderr' | 'terminal' | 'exit' | 'note';
	text: string;
}

export interface ShellExample {
	/** The test the example was derived from. */
	title: string;
	mode: 'pipe' | 'pty';
	lines: ShellExampleLine[];
	/** True when every expectation was an exact match, i.e. the transcript is literal. */
	exact: boolean;
}

/* ---------- worked examples that are bytes (PLAN.md §5.2) ----------

   Every tester past the shell emits the same shape: a title, a note, one or more named
   blocks of bytes with per-field annotations, and optionally a transcript of what running
   the thing prints. A Kafka exchange is `request` + `response`; a wasm example is a module
   plus the stdout of an invocation; a TLS example is one or more handshake messages; a link
   example is the ELF structures plus the linked program's output. They all render with the
   same component — a new track needs no new one. */

export interface ExampleField {
	offset: number;
	length: number;
	name: string;
	value: string;
	/** The generator saw this value change between captures (a topic id, a timestamp). */
	varies?: boolean;
}

/**
 * What the tester had set up when it captured the example: the topics an exchange refers
 * to and the consumer group, so a reader can tell a fixture name from a protocol constant.
 */
export interface ExampleFixture {
	key: string;
	name: string;
	id: string | null;
	partitions: number | null;
}

export interface ExampleEnv {
	topics: ExampleFixture[];
	group: string | null;
}

/**
 * `wire` and `module` have bytes; `text`, `closed` and `silence` describe what does *not*
 * go on the wire. Anything a tester invents that this build has not heard of is `other`,
 * which renders as an ordinary example rather than as an error.
 */
export type ExampleKindTag =
	| 'wire'
	| 'module'
	| 'object'
	| 'archive'
	| 'error'
	| 'text'
	| 'closed'
	| 'silence'
	| 'other';

/** One named run of bytes: a request, a response, a module, a handshake message, a header. */
export interface ExampleBlock {
	/** Stable key from the generator (`request`, `module`, `client_hello`, `rela_text`). */
	key: string;
	/** What to print above it (`request →`, `module bytes`). */
	label: string;
	/** A one-line human summary ("ApiVersions v4 request, client_id kafka-cli"). */
	summary: string;
	/** Decoded bytes, or null when the block is prose rather than a frame. */
	bytes: Uint8Array | null;
	fields: ExampleField[];
}

/** A line of "and this is what it prints" — reuses the shell transcript's line kinds. */
export type TranscriptLine = ShellExampleLine;

export interface ByteExample {
	title: string;
	kind: ExampleKindTag;
	note: string | null;
	env: ExampleEnv | null;
	blocks: ExampleBlock[];
	/** What running it produced: stdout, stderr, the exit status. Often empty. */
	transcript: TranscriptLine[];
}
