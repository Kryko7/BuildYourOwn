import type {
	RawJsonReport,
	Report,
	ReportStage,
	ReportTest,
	TestStatus,
	TrackId
} from './types';

const STATUSES: TestStatus[] = ['pass', 'fail', 'skip'];

function isStatus(v: unknown): v is TestStatus {
	return typeof v === 'string' && (STATUSES as string[]).includes(v);
}

export class ReportParseError extends Error {}

/**
 * Both testers emit the same JSON (shelltest/src/report.rs :: JsonReport); the only
 * difference is `shell` vs `target` for the thing under test. Parsing is defensive:
 * a dropped file is user input and may be anything at all.
 */
export function parseReport(
	input: unknown,
	opts: { track?: TrackId; origin?: Report['origin'] } = {}
): Report {
	if (typeof input === 'string') {
		try {
			input = JSON.parse(input);
		} catch {
			throw new ReportParseError('That file is not JSON.');
		}
	}
	if (!input || typeof input !== 'object') throw new ReportParseError('Expected a JSON object.');
	const raw = input as Partial<RawJsonReport>;
	if (!Array.isArray(raw.stages)) {
		throw new ReportParseError('No "stages" array — is this a tester --json report?');
	}

	const target = String(raw.target ?? raw.shell ?? 'unknown');
	const track: TrackId = opts.track ?? (raw.target !== undefined && raw.shell === undefined ? 'kafka' : 'shell');

	const stages: ReportStage[] = raw.stages.map((s) => {
		const tests: ReportTest[] = Array.isArray(s?.tests)
			? s.tests.map((t) => ({
					name: String(t?.name ?? 'unnamed'),
					status: isStatus(t?.status) ? t.status : 'fail',
					ext: Boolean(t?.ext),
					durationMs: Number(t?.duration_ms ?? 0),
					failures: Array.isArray(t?.failures) ? t.failures.map(String) : [],
					skipReason: t?.skip_reason == null ? null : String(t.skip_reason),
					actual: Array.isArray(t?.actual)
						? t.actual
								.filter((pair) => Array.isArray(pair) && pair.length >= 2)
								.map((pair) => [String(pair[0]), String(pair[1])] as [string, string])
						: []
				}))
			: [];
		const count = (st: TestStatus) => tests.filter((t) => t.status === st).length;
		return {
			stage: Number(s?.stage ?? 0),
			name: String(s?.name ?? ''),
			file: String(s?.file ?? ''),
			passed: Number.isFinite(s?.passed) ? Number(s.passed) : count('pass'),
			failed: Number.isFinite(s?.failed) ? Number(s.failed) : count('fail'),
			skipped: Number.isFinite(s?.skipped) ? Number(s.skipped) : count('skip'),
			tests
		};
	});

	const sum = (k: 'passed' | 'failed' | 'skipped') => stages.reduce((n, s) => n + s[k], 0);

	return {
		track,
		target,
		validate: Boolean(raw.validate),
		passed: Number.isFinite(raw.passed) ? Number(raw.passed) : sum('passed'),
		failed: Number.isFinite(raw.failed) ? Number(raw.failed) : sum('failed'),
		skipped: Number.isFinite(raw.skipped) ? Number(raw.skipped) : sum('skipped'),
		elapsedMs: Number(raw.elapsed_ms ?? 0),
		stages,
		importedAt: new Date().toISOString(),
		origin: opts.origin ?? 'file'
	};
}

/** Per-test status lookup: `statusIndex(report)[stageNumber]?.[testName]`. */
export function statusIndex(report: Report | null): Record<number, Record<string, ReportTest>> {
	const index: Record<number, Record<string, ReportTest>> = {};
	if (!report) return index;
	for (const stage of report.stages) {
		const byName: Record<string, ReportTest> = {};
		for (const t of stage.tests) byName[t.name] = t;
		index[stage.stage] = byName;
	}
	return index;
}

export type StageOutcome = 'green' | 'red' | 'partial' | 'untested';

export function stageOutcome(report: Report | null, stage: number): StageOutcome {
	const s = report?.stages.find((x) => x.stage === stage);
	if (!s) return 'untested';
	if (s.failed > 0) return s.passed > 0 ? 'partial' : 'red';
	if (s.passed > 0) return 'green';
	return 'untested';
}

/** Every failing test in a report, flattened, for the replay picker. */
export function failingTests(report: Report | null) {
	if (!report) return [];
	return report.stages.flatMap((s) =>
		s.tests.filter((t) => t.status === 'fail').map((t) => ({ stage: s.stage, stageName: s.name, test: t }))
	);
}

export function formatDuration(ms: number): string {
	if (ms < 1000) return `${Math.round(ms)} ms`;
	if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
	const m = Math.floor(ms / 60_000);
	return `${m}m ${Math.round((ms % 60_000) / 1000)}s`;
}

/** Show control characters the way shelltest's reporter does: ^G, ^H, ^[. */
export function caretNotation(s: string): string {
	let out = '';
	for (const ch of s) {
		const code = ch.codePointAt(0) ?? 0;
		if (ch === '\n' || ch === '\t') out += ch;
		else if (code < 0x20) out += '^' + String.fromCharCode(code + 0x40);
		else if (code === 0x7f) out += '^?';
		else out += ch;
	}
	return out;
}
