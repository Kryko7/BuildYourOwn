/** Shared shapes for the catalogs, the testers' JSON reports and the resource library. */

export type TrackId = 'shell' | 'kafka';

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

export interface KafkaExampleField {
	offset: number;
	length: number;
	name: string;
	value: string;
	/** The generator saw this value change between captures (a topic id, a timestamp). */
	varies?: boolean;
}

/**
 * What kafkatest had set up when it captured the exchange: the topics the frames refer to
 * and the consumer group, so a reader can tell a fixture name from a protocol constant.
 */
export interface KafkaExampleTopic {
	key: string;
	name: string;
	id: string | null;
	partitions: number | null;
}

export interface KafkaExampleEnv {
	topics: KafkaExampleTopic[];
	group: string | null;
}

/** `wire` has bytes; `text`, `closed` and `silence` describe what does *not* go on the wire. */
export type KafkaExampleKind = 'wire' | 'text' | 'closed' | 'silence' | 'other';

export interface KafkaExampleSide {
	/** A one-line human summary ("ApiVersions v4 request, client_id kafka-cli"). */
	summary: string;
	/** Decoded wire bytes, or null for a stage with no frame to show. */
	bytes: Uint8Array | null;
	fields: KafkaExampleField[];
}

export interface KafkaExample {
	title: string;
	kind: KafkaExampleKind;
	note: string | null;
	env: KafkaExampleEnv | null;
	request: KafkaExampleSide;
	response: KafkaExampleSide;
}
