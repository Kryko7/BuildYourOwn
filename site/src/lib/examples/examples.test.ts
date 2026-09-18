import { describe, it, expect } from 'vitest';
import { shellExamples } from './shell';
import {
	kafkaExampleCount,
	kafkaExamples,
	loadKafkaExampleData,
	loadKafkaExamples,
	normalizeKafkaExample,
	readHex
} from './kafka';
import { exampleCount } from './index';
import { getCatalog } from '../catalog';
import { samples } from '../lab/samples';
import { toHex } from '../lab/kafka';
import fixture from '../data/fixtures/kafka-examples.sample.json';
import type { KafkaExample, StageSpec } from '../types';

function stage(partial: Partial<StageSpec>): StageSpec {
	return {
		number: 1,
		slug: 's',
		name: 'A stage',
		ext: false,
		section: 'A',
		file: '01.yaml',
		planDone: true,
		hints: [],
		tests: [],
		...partial
	};
}

describe('shellExamples', () => {
	it('renders a pipe test as a transcript and drops the trailing `exit 0`', () => {
		const [ex] = shellExamples(
			stage({
				tests: [
					{
						name: 'echo prints its arguments',
						input: ['echo hello world', 'exit 0'],
						expect: [
							{ label: 'stdout', kind: 'exact', value: 'hello world' },
							{ label: 'exit code', kind: 'exact', value: '0' }
						]
					}
				]
			})
		);
		expect(ex.title).toBe('echo prints its arguments');
		expect(ex.mode).toBe('pipe');
		expect(ex.exact).toBe(true);
		expect(ex.lines).toEqual([
			{ kind: 'input', text: 'echo hello world' },
			{ kind: 'stdout', text: 'hello world' },
			{ kind: 'exit', text: '0' }
		]);
	});

	it('splits multi-line expected output into one line each', () => {
		const [ex] = shellExamples(
			stage({
				tests: [
					{
						name: 'two lines',
						input: ['echo a', 'echo b', 'exit 0'],
						expect: [{ label: 'stdout', kind: 'exact', value: 'a\nb' }]
					}
				]
			})
		);
		expect(ex.lines.filter((l) => l.kind === 'stdout').map((l) => l.text)).toEqual(['a', 'b']);
	});

	it('says so when the suite matches loosely instead of quoting a literal', () => {
		const [ex] = shellExamples(
			stage({
				tests: [
					{
						name: 'loose',
						input: ['nope_cmd', 'exit 0'],
						expect: [{ label: 'stderr', kind: 'contains', value: 'nope_cmd: command not found' }]
					}
				]
			})
		);
		expect(ex.exact).toBe(false);
		expect(ex.lines.at(-1)).toEqual({
			kind: 'stderr',
			text: '(contains) nope_cmd: command not found'
		});
	});

	it('turns a pty test into key steps and a terminal expectation', () => {
		const [ex] = shellExamples(
			stage({
				tests: [
					{
						name: 'prompt',
						mode: 'pty',
						steps: [
							{ op: 'wait', value: '$ ' },
							{ op: 'sleep_ms', value: '300' }
						],
						keys: ['\r'],
						expect: [{ label: 'terminal', kind: 'exact', value: '$ ' }]
					}
				]
			})
		);
		expect(ex.mode).toBe('pty');
		// the sleep is harness plumbing, not something the reader should copy
		expect(ex.lines.map((l) => l.kind)).toEqual(['key', 'key', 'terminal']);
		expect(ex.lines[1].text).toBe('press "^M"');
	});

	it('keeps the suite’s own teaching order among equally readable tests', () => {
		const examples = shellExamples(
			stage({
				tests: [
					{
						name: 'the canonical one, which happens to be longer',
						input: ['echo hello world', 'exit 0'],
						expect: [{ label: 'stdout', kind: 'exact', value: 'hello world' }]
					},
					{
						name: 'a shorter corner case',
						input: ['echo\ta\t\tb', 'exit 0'],
						expect: [{ label: 'stdout', kind: 'exact', value: 'a b' }]
					}
				]
			})
		);
		expect(examples.map((e) => e.title)).toEqual([
			'the canonical one, which happens to be longer',
			'a shorter corner case'
		]);
	});

	it('demotes a test full of harness placeholders below a plain one', () => {
		const examples = shellExamples(
			stage({
				tests: [
					{
						name: 'uses {TMP}',
						input: ['history -r {TMP}/h.txt', 'exit 0'],
						expect: [{ label: 'stdout', kind: 'exact', value: 'x' }]
					},
					{
						name: 'plain',
						input: ['history', 'exit 0'],
						expect: [{ label: 'stdout', kind: 'exact', value: '1  history' }]
					}
				]
			})
		);
		expect(examples[0].title).toBe('plain');
	});

	it('prefers literal tests over pattern ones, and caps at the limit', () => {
		const examples = shellExamples(
			stage({
				tests: [
					{
						name: 'long regex',
						input: ['a'.repeat(200)],
						expect: [{ label: 'stdout', kind: 'regex', value: '(?s)x.*y' }]
					},
					{
						name: 'short and literal',
						input: ['echo hi', 'exit 0'],
						expect: [{ label: 'stdout', kind: 'exact', value: 'hi' }]
					},
					{
						name: 'also literal',
						input: ['echo ho', 'exit 0'],
						expect: [{ label: 'stdout', kind: 'exact', value: 'ho' }]
					}
				]
			}),
			2
		);
		expect(examples).toHaveLength(2);
		expect(examples[0].title).toBe('short and literal');
	});

	it('skips tests with nothing recorded, and returns nothing for an empty stage', () => {
		expect(shellExamples(stage({ tests: [{ name: 'bare' }] }))).toEqual([]);
		expect(shellExamples(stage({ tests: [] }))).toEqual([]);
	});

	it('produces examples for real catalog stages', () => {
		const catalog = getCatalog('shell');
		const withTests = catalog.stages.filter((s) => s.tests.length > 0);
		expect(withTests.length).toBeGreaterThan(40);
		const covered = withTests.filter((s) => shellExamples(s).length > 0);
		// Every shipped shell stage records inputs and expectations, so every one can teach.
		expect(covered.length).toBe(withTests.length);
		const five = catalog.stages.find((s) => s.number === 5);
		expect(five && shellExamples(five)[0].lines[0].kind).toBe('input');
	});
});

describe('readHex', () => {
	it('accepts spaced, bare and 0x-prefixed hex', () => {
		expect([...(readHex('00 01 ff') ?? [])]).toEqual([0, 1, 255]);
		expect([...(readHex('0001ff') ?? [])]).toEqual([0, 1, 255]);
		expect([...(readHex('0x00,0x01') ?? [])]).toEqual([0, 1]);
	});

	it('rejects odd lengths and rubbish instead of guessing', () => {
		expect(readHex('000')).toBeNull();
		expect(readHex('zz')).toBeNull();
		expect(readHex('')).toBeNull();
		expect(readHex(undefined)).toBeNull();
	});
});

describe('kafkaExamples (tolerant reader)', () => {
	it('returns nothing when the catalog has no examples at all', () => {
		expect(kafkaExamples(stage({}))).toEqual([]);
		expect(kafkaExamples(stage({ examples: [] }))).toEqual([]);
		expect(kafkaExamples(stage({ examples: 'nope' as unknown as unknown[] }))).toEqual([]);
		expect(kafkaExamples(null)).toEqual([]);
	});

	it('ignores fields it has never heard of and keeps the ones it knows', () => {
		const [ex] = kafkaExamples(
			stage({
				examples: [
					{
						title: 'Hello',
						request: 'a request',
						request_hex: '00 01',
						response: 'a response',
						response_hex: '',
						future_field: { nested: true },
						request_fields: [{ offset: 0, length: 2, field: 'size', value: '1', extra: 9 }]
					}
				]
			})
		);
		expect(ex.title).toBe('Hello');
		expect([...(ex.request.bytes ?? [])]).toEqual([0, 1]);
		// an empty hex string means "this side is prose, not a frame"
		expect(ex.response.bytes).toBeNull();
		expect(ex.response.summary).toBe('a response');
		expect(ex.request.fields).toEqual([{ offset: 0, length: 2, name: 'size', value: '1' }]);
	});

	it('clamps a field range that runs off the end of the frame', () => {
		const [ex] = kafkaExamples(
			stage({
				examples: [
					{
						request: 'x',
						request_hex: '00 01 02 03',
						request_fields: [
							{ offset: 2, length: 40, field: 'body', value: '…' },
							{ offset: 99, length: 4, field: 'past the end', value: '' },
							{ offset: -3, length: 2, field: 'negative', value: '' },
							{ length: 2, value: 'no name' }
						]
					}
				]
			})
		);
		expect(ex.request.fields).toEqual([
			{ offset: 2, length: 2, name: 'body', value: '…' },
			{ offset: 4, length: 0, name: 'past the end', value: '' }
		]);
	});

	it('names an untitled example and drops an entirely empty one', () => {
		expect(normalizeKafkaExample({ request: 'only a summary' }, 2)?.title).toBe('Example 3');
		expect(normalizeKafkaExample({}, 0)).toBeNull();
		expect(normalizeKafkaExample('a string', 0)).toBeNull();
		expect(normalizeKafkaExample(null, 0)).toBeNull();
	});

	it('accepts camelCase keys too, in case the generator changes its mind', () => {
		const ex = normalizeKafkaExample({ request: 'r', requestHex: 'aabb', requestFields: [{ offset: 0, length: 2, field: 'x', value: 'y' }] });
		expect([...(ex?.request.bytes ?? [])]).toEqual([0xaa, 0xbb]);
		expect(ex?.request.fields[0].name).toBe('x');
	});
});

describe('the example fixture', () => {
	const examples = kafkaExamples(stage({ examples: fixture.examples }));

	it('reads as two complete examples', () => {
		expect(examples).toHaveLength(2);
		expect(examples[0].request.bytes?.length).toBe(41);
		expect(examples[0].response.bytes?.length).toBe(37);
		expect(examples[0].note).toContain('response');
	});

	it('is byte-identical to the request the site’s own codec builds', () => {
		// The fixture is hand-written in kafkatest's schema; this is what keeps it honest.
		const built = samples.find((s) => s.id === 'apiversions-v4');
		expect(built).toBeDefined();
		expect(toHex(examples[0].request.bytes!)).toBe(toHex(built!.bytes));
	});

	it('annotates every byte of each frame, in order, without gaps', () => {
		for (const side of [examples[0].request, examples[0].response]) {
			let cursor = 0;
			for (const f of side.fields) {
				expect(f.offset).toBe(cursor);
				cursor += f.length;
			}
			expect(cursor).toBe(side.bytes!.length);
		}
	});

	it('declares a size field matching the bytes that follow it', () => {
		for (const ex of examples) {
			for (const side of [ex.request, ex.response]) {
				const bytes = side.bytes!;
				const declared = (bytes[0] << 24) | (bytes[1] << 16) | (bytes[2] << 8) | bytes[3];
				expect(declared).toBe(bytes.length - 4);
			}
		}
	});
});

describe('the kafkatest extras: kind, env and varies', () => {
	it('reads the kind, and infers one when the generator does not send it', () => {
		expect(normalizeKafkaExample({ kind: 'silence', request: 'a connect', response: 'nothing' })?.kind).toBe('silence');
		expect(normalizeKafkaExample({ kind: 'text', request: 'r', response: 's' })?.kind).toBe('text');
		expect(normalizeKafkaExample({ request: 'r', request_hex: 'aabb' })?.kind).toBe('wire');
		expect(normalizeKafkaExample({ request: 'r' })?.kind).toBe('text');
		// a kind from the future is neither guessed at nor fatal
		expect(normalizeKafkaExample({ kind: 'hologram', request: 'r' })?.kind).toBe('other');
	});

	it('keeps the fixture the exchange was captured against', () => {
		const ex = normalizeKafkaExample({
			request: 'r',
			env: {
				topics: {
					t1: { name: 't1-ex121', id: '33df85ca-2e51-4259-986f-33bff3f3da4b', partitions: 1 },
					broken: { partitions: 2 }
				},
				group: 'kafkatest-group'
			}
		});
		expect(ex?.env?.group).toBe('kafkatest-group');
		// the entry with no name is dropped rather than rendered as an empty topic
		expect(ex?.env?.topics).toEqual([
			{ key: 't1', name: 't1-ex121', id: '33df85ca-2e51-4259-986f-33bff3f3da4b', partitions: 1 }
		]);
		expect(normalizeKafkaExample({ request: 'r' })?.env).toBeNull();
		expect(normalizeKafkaExample({ request: 'r', env: { topics: {} } })?.env).toBeNull();
	});

	it('marks a field the generator says changes between captures', () => {
		const ex = normalizeKafkaExample({
			request: 'r',
			request_hex: 'aabbccdd',
			request_fields: [
				{ offset: 0, length: 2, field: 'fixed', value: '1' },
				{ offset: 2, length: 2, field: 'topic_id', value: 'uuid', varies: true }
			]
		});
		expect(ex?.request.fields[0].varies).toBeUndefined();
		expect(ex?.request.fields[1].varies).toBe(true);
	});
});

describe('the real kafkatest catalog', () => {
	const catalog = getCatalog('kafka');
	const counted = catalog.stages.filter((s) => kafkaExampleCount(s) > 0);

	/** Everything kafkatest shipped, pulled through the same lazy loader the UI uses. */
	async function everything() {
		const out: { stage: StageSpec; raw: unknown[]; examples: KafkaExample[] }[] = [];
		for (const stage of counted) {
			const raw = (await loadKafkaExampleData(stage.number)) ?? [];
			out.push({ stage, raw, examples: await loadKafkaExamples(stage) });
		}
		return out;
	}

	it('keeps the examples out of the bundled catalog but remembers how many there are', () => {
		// The split is the whole point: the shared chunk must not carry 91 annotated frames.
		expect(counted.length).toBeGreaterThan(0);
		for (const stage of catalog.stages) expect(stage.examples).toBeUndefined();
		expect(JSON.stringify(catalog).length).toBeLessThan(400_000);
	});

	it('loads every counted stage, and nothing for a stage with none', async () => {
		for (const { stage, examples } of await everything()) {
			expect(examples.length, `stage ${stage.number}`).toBe(kafkaExampleCount(stage));
		}
		expect(await loadKafkaExamples(stage({ number: 9999 }))).toEqual([]);
		expect(await loadKafkaExampleData(9999)).toBeNull();
	});

	it('never annotates a byte that is not there, on any example', async () => {
		for (const { stage, examples } of await everything()) {
			for (const ex of examples) {
				for (const side of [ex.request, ex.response]) {
					const limit = side.bytes?.length ?? 0;
					for (const f of side.fields) {
						expect(
							f.offset + f.length,
							`stage ${stage.number} · ${ex.title} · ${f.name}`
						).toBeLessThanOrEqual(limit);
					}
				}
			}
		}
	});

	it('decodes every non-empty hex string it is given', async () => {
		for (const { raw, examples } of await everything()) {
			for (const ex of examples) {
				const source = raw.find((e) => (e as { title?: string }).title === ex.title) as
					| { request_hex?: string; response_hex?: string }
					| undefined;
				if (source?.request_hex) expect(ex.request.bytes, ex.title).not.toBeNull();
				if (source?.response_hex) expect(ex.response.bytes, ex.title).not.toBeNull();
			}
		}
	});

	it('gives a non-wire example prose on both sides instead of an empty dump', async () => {
		const all = (await everything()).flatMap((s) => s.examples);
		const quiet = all.filter((e) => e.kind !== 'wire');
		expect(quiet.length).toBeGreaterThan(0);
		for (const ex of quiet) expect(ex.request.summary || ex.response.summary).toBeTruthy();
	});

	it('carries the fixture and the varies flags kafkatest generates', async () => {
		const all = (await everything()).flatMap((s) => s.examples);
		expect(all.some((e) => e.env?.topics.length)).toBe(true);
		expect(all.some((e) => e.env?.group)).toBe(true);
		expect(
			all.some((e) => [...e.request.fields, ...e.response.fields].some((f) => f.varies))
		).toBe(true);
	});
});

describe('exampleCount', () => {
	it('counts per track and never throws on a stage with nothing', () => {
		const shell = getCatalog('shell').stages.find((s) => s.number === 5)!;
		expect(exampleCount('shell', shell)).toBeGreaterThan(0);
		expect(exampleCount('kafka', shell)).toBe(0);
		expect(exampleCount('kafka', stage({ examples: fixture.examples }))).toBe(2);
		expect(exampleCount('shell', null)).toBe(0);
	});

	it('reads the split catalog’s count without loading a single frame', () => {
		expect(kafkaExampleCount(stage({ exampleCount: 4 }))).toBe(4);
		// inline examples, when something supplies them, still win over the metadata
		expect(kafkaExampleCount(stage({ exampleCount: 4, examples: fixture.examples }))).toBe(2);
		expect(kafkaExampleCount(stage({}))).toBe(0);
		expect(kafkaExampleCount(null)).toBe(0);
		const real = getCatalog('kafka').stages.find((s) => s.number === 5)!;
		expect(exampleCount('kafka', real)).toBe(real.exampleCount);
	});
});
