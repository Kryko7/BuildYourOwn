import { describe, it, expect } from 'vitest';
import sample from './data/fixtures/report.shell.sample.json';
import broken from './data/fixtures/report.shell.broken.json';
import { parseReport, statusIndex, stageOutcome, failingTests, ReportParseError, caretNotation } from './report';
import { lineDiff, hasDifference } from './diff';
import { stageState } from './stage-state';
import { levelFor, xpForStage, rankName } from './stores/progress.svelte';
import { getCatalog, commandFor, neighbours, stageOf, badgeFor } from './catalog';
import { conventionsForStage } from './conventions';

describe('parseReport on a real shelltest --json report', () => {
	// Produced by: shelltest --shell bash --until 3 --json report.shell.sample.json
	const report = parseReport(sample, { track: 'shell', origin: 'fixture' });

	it('reads the tester, totals and stage list', () => {
		expect(report.target).toBe('bash');
		expect(report.track).toBe('shell');
		expect(report.passed).toBe(20);
		expect(report.failed).toBe(0);
		expect(report.stages.map((s) => s.stage)).toEqual([1, 2, 3]);
	});

	it('maps every test to a status', () => {
		const index = statusIndex(report);
		expect(Object.keys(index)).toEqual(['1', '2', '3']);
		expect(index[1]['prints the prompt on startup'].status).toBe('pass');
		const all = report.stages.flatMap((s) => s.tests);
		expect(all).toHaveLength(20);
		expect(all.every((t) => t.status === 'pass')).toBe(true);
	});

	it('lines report test names up with the catalog test names', () => {
		const catalog = getCatalog('shell');
		for (const stage of report.stages) {
			const spec = catalog.stages.find((s) => s.number === stage.stage);
			expect(spec, `catalog has stage ${stage.stage}`).toBeDefined();
			const specNames = spec!.tests.map((t) => t.name);
			for (const t of stage.tests) expect(specNames).toContain(t.name);
		}
	});

	it('reports a green outcome per stage', () => {
		expect(stageOutcome(report, 1)).toBe('green');
		expect(stageOutcome(report, 3)).toBe('green');
		expect(stageOutcome(report, 9)).toBe('untested');
		expect(failingTests(report)).toEqual([]);
	});
});

describe('parseReport on a failing run', () => {
	// Produced by: shelltest --shell examples/broken_shell.sh --until 5 --json ...
	const report = parseReport(broken, { track: 'shell' });

	it('keeps failure messages and the captured actual streams', () => {
		expect(report.failed).toBeGreaterThan(0);
		const failures = failingTests(report);
		expect(failures.length).toBe(report.failed);
		const first = failures[0].test;
		expect(first.failures.length).toBeGreaterThan(0);
		expect(first.actual.length).toBeGreaterThan(0);
		expect(first.actual[0]).toHaveLength(2);
	});

	it('marks partly-failing stages as partial and fully-failing ones as red', () => {
		const outcomes = report.stages.map((s) => stageOutcome(report, s.stage));
		expect(outcomes).toContain('red');
		expect(new Set(outcomes).size).toBeGreaterThan(1);
	});
});

describe('parseReport error handling', () => {
	it('accepts a JSON string as well as an object', () => {
		expect(parseReport(JSON.stringify(sample)).passed).toBe(20);
	});

	it('detects a kafkatest report by its `target` key', () => {
		const r = parseReport({ target: 'apache_kafka', validate: true, stages: [], passed: 0, failed: 0, skipped: 0, elapsed_ms: 1 });
		expect(r.track).toBe('kafka');
		expect(r.target).toBe('apache_kafka');
	});

	// The two testers emit the same schema; only the name of the thing under test differs.
	// Both spellings have to import, and both have to colour the map.
	const kafkaReport = {
		target: 'my_broker',
		validate: false,
		passed: 2,
		failed: 1,
		skipped: 0,
		elapsed_ms: 412,
		stages: [
			{
				stage: 1,
				name: 'Bind to the broker port',
				file: 'src/stages/s01_bind.rs',
				passed: 2,
				failed: 0,
				skipped: 0,
				tests: [
					{ name: 'accepts a TCP connection', status: 'pass', ext: false, duration_ms: 3, failures: [], skip_reason: null, actual: [] },
					{ name: 'nothing is sent first', status: 'pass', ext: false, duration_ms: 2, failures: [], skip_reason: null, actual: [] }
				]
			},
			{
				stage: 2,
				name: 'Respond with the correlation id',
				file: 'src/stages/s02_correlation_id.rs',
				passed: 0,
				failed: 1,
				skipped: 0,
				tests: [
					{
						name: 'the correlation id comes back unchanged',
						status: 'fail',
						ext: false,
						duration_ms: 5,
						failures: ['response: correlation id differs'],
						skip_reason: null,
						actual: [['response', '00 00 00 04 00 00 00 00']]
					}
				]
			}
		]
	};

	it('imports a kafkatest report (`target`) as fully as a shelltest one (`shell`)', () => {
		const r = parseReport(kafkaReport, { origin: 'file' });
		expect(r.track).toBe('kafka');
		expect(r.target).toBe('my_broker');
		expect(r.stages.map((s) => s.stage)).toEqual([1, 2]);
		expect(stageOutcome(r, 1)).toBe('green');
		expect(stageOutcome(r, 2)).toBe('red');
		expect(failingTests(r)).toHaveLength(1);
		expect(statusIndex(r)[2]['the correlation id comes back unchanged'].failures).toEqual([
			'response: correlation id differs'
		]);
		expect(r.stages[1].tests[0].actual).toEqual([['response', '00 00 00 04 00 00 00 00']]);
	});

	it('imports the same run written either way to the same thing', () => {
		const { target, ...rest } = kafkaReport;
		const asShell = parseReport({ shell: target, ...rest }, { track: 'kafka' });
		const asKafka = parseReport(kafkaReport, { track: 'kafka' });
		expect({ ...asShell, importedAt: '' }).toEqual({ ...asKafka, importedAt: '' });
	});

	it('rejects nonsense with a readable message', () => {
		expect(() => parseReport('not json')).toThrow(ReportParseError);
		expect(() => parseReport({ hello: 'world' })).toThrow(/stages/);
	});

	it('recomputes totals when a report omits them', () => {
		const r = parseReport({
			shell: 'my_shell',
			validate: false,
			stages: [{ stage: 1, name: 'x', file: 'a.yaml', tests: [{ name: 't', status: 'fail', failures: ['boom'] }] }]
		});
		expect(r.failed).toBe(1);
		expect(r.stages[0].tests[0].failures).toEqual(['boom']);
	});
});

describe('stageState', () => {
	const none = { done: false };
	it('shows a red stage as failing even when it was ticked off', () => {
		const report = parseReport(broken, { track: 'shell' });
		expect(stageState({ track: 'shell', stage: 5, report, progress: { done: true }, nextStage: 9 })).toBe('failing');
	});
	it('locks stages past the next one and marks the next one', () => {
		expect(stageState({ track: 'shell', stage: 4, report: null, progress: none, nextStage: 4 })).toBe('next');
		expect(stageState({ track: 'shell', stage: 9, report: null, progress: none, nextStage: 4 })).toBe('locked');
		expect(stageState({ track: 'shell', stage: 2, report: null, progress: { done: true }, nextStage: 4 })).toBe('done');
		expect(stageState({ track: 'shell', stage: 2, report: null, progress: { done: false, notes: 'hm' }, nextStage: 4 })).toBe('in-progress');
	});
	it('turns a green tester result into in-progress until it is ticked', () => {
		const report = parseReport(sample, { track: 'shell' });
		expect(stageState({ track: 'shell', stage: 1, report, progress: none, nextStage: 1 })).toBe('in-progress');
	});
});

describe('xp and levels', () => {
	it('gives ext stages more xp', () => {
		expect(xpForStage('shell', 1)).toBe(10);
		expect(xpForStage('shell', 20)).toBe(15);
	});
	it('walks up the level thresholds', () => {
		expect(levelFor(0).level).toBe(1);
		expect(levelFor(40).level).toBe(2);
		expect(levelFor(10_000).level).toBe(11);
		expect(levelFor(45).into).toBe(5);
		expect(rankName(1)).toBe('Seedling');
		expect(rankName(11)).toBe('Head Gardener');
	});
});

describe('catalog helpers', () => {
	it('builds the exact command for a stage', () => {
		expect(commandFor('shell', 5)).toBe('./target/release/shelltest --shell ./my_shell --stage 5');
		expect(commandFor('kafka', 12, 'my_broker')).toBe('./target/release/kafkatest --broker my_broker --stage 12');
	});
	it('knows a stage and its neighbours', () => {
		expect(stageOf('shell', 1)?.name).toBe('Print the prompt and wait for input');
		expect(neighbours('shell', 1).prev).toBeUndefined();
		expect(neighbours('shell', 1).next?.number).toBe(2);
		expect(neighbours('shell', 57).next).toBeUndefined();
	});
	it('names a badge for every section of both tracks', () => {
		for (const track of ['shell', 'kafka'] as const) {
			for (const s of getCatalog(track).sections) {
				expect(badgeFor(track, s.id).badge).not.toMatch(/^Section/);
			}
		}
	});
	it('has conventions attached to the stages they govern', () => {
		expect(conventionsForStage('shell', 2).map((c) => c.id)).toContain('not-found');
		expect(conventionsForStage('kafka', 5).map((c) => c.id)).toContain('flexible');
	});
});

describe('diff and terminal rendering', () => {
	it('marks removed and added lines like the tester does', () => {
		const d = lineDiff('a\nb', 'a\nc');
		expect(hasDifference(d)).toBe(true);
		expect(d.filter((l) => l.sign === '-').map((l) => l.text)).toEqual(['b']);
		expect(d.filter((l) => l.sign === '+').map((l) => l.text)).toEqual(['c']);
	});
	it('is empty of changes for identical text', () => {
		expect(hasDifference(lineDiff('same\nlines', 'same\nlines'))).toBe(false);
	});
	it('shows control characters in caret notation', () => {
		expect(caretNotation('a\x07b\x1b[A\n')).toBe('a^Gb^[[A\n');
	});
});
