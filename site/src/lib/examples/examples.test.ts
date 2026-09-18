import { describe, it, expect } from 'vitest';
import { shellExamples } from './shell';
import {
	blockOf,
	byteExampleCount,
	hasBytes,
	inlineExamples,
	loadExampleData,
	loadExamples,
	normalizeByteExample,
	readHex
} from './bytes';
import { exampleCount } from './index';
import { getCatalog, trackIds } from '../catalog';
import { samples } from '../lab/samples';
import { toHex } from '../lab/kafka';
import fixture from '../data/fixtures/kafka-examples.sample.json';
import type { ByteExample, ExampleBlock, StageSpec } from '../types';

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

describe('the byte-example reader (tolerant by design)', () => {
	it('returns nothing when the catalog has no examples at all', () => {
		expect(inlineExamples(stage({}))).toEqual([]);
		expect(inlineExamples(stage({ examples: [] }))).toEqual([]);
		expect(inlineExamples(stage({ examples: 'nope' as unknown as unknown[] }))).toEqual([]);
		expect(inlineExamples(null)).toEqual([]);
	});

	it('ignores fields it has never heard of and keeps the ones it knows', () => {
		const [ex] = inlineExamples(
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
		expect([...(blockOf(ex, 'request')!.bytes ?? [])]).toEqual([0, 1]);
		// an empty hex string means "this side is prose, not a frame"
		expect(blockOf(ex, 'response')!.bytes).toBeNull();
		expect(blockOf(ex, 'response')!.summary).toBe('a response');
		expect(blockOf(ex, 'request')!.fields).toEqual([{ offset: 0, length: 2, name: 'size', value: '1' }]);
	});

	it('clamps a field range that runs off the end of the frame', () => {
		const [ex] = inlineExamples(
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
		expect(blockOf(ex, 'request')!.fields).toEqual([
			{ offset: 2, length: 2, name: 'body', value: '…' },
			{ offset: 4, length: 0, name: 'past the end', value: '' }
		]);
	});

	it('names an untitled example and drops an entirely empty one', () => {
		expect(normalizeByteExample({ request: 'only a summary' }, 2)?.title).toBe('Example 3');
		expect(normalizeByteExample({}, 0)).toBeNull();
		expect(normalizeByteExample('a string', 0)).toBeNull();
		expect(normalizeByteExample(null, 0)).toBeNull();
	});

	it('accepts camelCase keys too, in case the generator changes its mind', () => {
		const ex = normalizeByteExample({ request: 'r', requestHex: 'aabb', requestFields: [{ offset: 0, length: 2, field: 'x', value: 'y' }] });
		expect([...(blockOf(ex!, 'request')!.bytes ?? [])]).toEqual([0xaa, 0xbb]);
		expect(blockOf(ex!, 'request')!.fields[0].name).toBe('x');
	});
});

describe('the example fixture', () => {
	const examples = inlineExamples(stage({ examples: fixture.examples }));

	it('reads as two complete examples', () => {
		expect(examples).toHaveLength(2);
		expect(blockOf(examples[0], 'request')!.bytes?.length).toBe(41);
		expect(blockOf(examples[0], 'response')!.bytes?.length).toBe(37);
		expect(examples[0].note).toContain('response');
	});

	it('is byte-identical to the request the site’s own codec builds', () => {
		// The fixture is hand-written in kafkatest's schema; this is what keeps it honest.
		const built = samples.find((s) => s.id === 'apiversions-v4');
		expect(built).toBeDefined();
		expect(toHex(blockOf(examples[0], 'request')!.bytes!)).toBe(toHex(built!.bytes));
	});

	it('annotates every byte of each frame, in order, without gaps', () => {
		for (const side of examples[0].blocks) {
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
			for (const side of ex.blocks) {
				const bytes = side.bytes!;
				const declared = (bytes[0] << 24) | (bytes[1] << 16) | (bytes[2] << 8) | bytes[3];
				expect(declared).toBe(bytes.length - 4);
			}
		}
	});
});

describe('the kafkatest extras: kind, env and varies', () => {
	it('reads the kind, and infers one when the generator does not send it', () => {
		expect(normalizeByteExample({ kind: 'silence', request: 'a connect', response: 'nothing' })?.kind).toBe('silence');
		expect(normalizeByteExample({ kind: 'text', request: 'r', response: 's' })?.kind).toBe('text');
		expect(normalizeByteExample({ request: 'r', request_hex: 'aabb' })?.kind).toBe('wire');
		expect(normalizeByteExample({ request: 'r' })?.kind).toBe('text');
		// a kind from the future is neither guessed at nor fatal
		expect(normalizeByteExample({ kind: 'hologram', request: 'r' })?.kind).toBe('other');
	});

	it('keeps the fixture the exchange was captured against', () => {
		const ex = normalizeByteExample({
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
		expect(normalizeByteExample({ request: 'r' })?.env).toBeNull();
		expect(normalizeByteExample({ request: 'r', env: { topics: {} } })?.env).toBeNull();
	});

	it('marks a field the generator says changes between captures', () => {
		const ex = normalizeByteExample({
			request: 'r',
			request_hex: 'aabbccdd',
			request_fields: [
				{ offset: 0, length: 2, field: 'fixed', value: '1' },
				{ offset: 2, length: 2, field: 'topic_id', value: 'uuid', varies: true }
			]
		});
		expect(blockOf(ex!, 'request')!.fields[0].varies).toBeUndefined();
		expect(blockOf(ex!, 'request')!.fields[1].varies).toBe(true);
	});
});

describe('the real kafkatest catalog', () => {
	const catalog = getCatalog('kafka');
	const counted = catalog.stages.filter((s) => byteExampleCount(s) > 0);

	/** Everything kafkatest shipped, pulled through the same lazy loader the UI uses. */
	async function everything() {
		const out: { stage: StageSpec; raw: unknown[]; examples: ByteExample[] }[] = [];
		for (const stage of counted) {
			const raw = (await loadExampleData('kafka', stage.number)) ?? [];
			out.push({ stage, raw, examples: await loadExamples('kafka', stage) });
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
			expect(examples.length, `stage ${stage.number}`).toBe(byteExampleCount(stage));
		}
		expect(await loadExamples('kafka', stage({ number: 9999 }))).toEqual([]);
		expect(await loadExampleData('kafka', 9999)).toBeNull();
	});

	it('never annotates a byte that is not there, on any example', async () => {
		for (const { stage, examples } of await everything()) {
			for (const ex of examples) {
				for (const side of ex.blocks) {
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
				if (source?.request_hex) expect(blockOf(ex, 'request')!.bytes, ex.title).not.toBeNull();
				if (source?.response_hex) expect(blockOf(ex, 'response')!.bytes, ex.title).not.toBeNull();
			}
		}
	});

	it('gives a non-wire example prose on both sides instead of an empty dump', async () => {
		const all = (await everything()).flatMap((s) => s.examples);
		const quiet = all.filter((e) => e.kind !== 'wire');
		expect(quiet.length).toBeGreaterThan(0);
		for (const ex of quiet) expect(ex.blocks.some((b: ExampleBlock) => b.summary)).toBe(true);
	});

	it('carries the fixture and the varies flags kafkatest generates', async () => {
		const all = (await everything()).flatMap((s) => s.examples);
		expect(all.some((e) => e.env?.topics.length)).toBe(true);
		expect(all.some((e) => e.env?.group)).toBe(true);
		expect(
			all.some((e) => e.blocks.some((b) => b.fields.some((f) => f.varies)))
		).toBe(true);
	});
});

describe('the generic block shapes (every track past kafka)', () => {
	it('turns any `<name>_hex` key into a labelled block of bytes', () => {
		const ex = normalizeByteExample({
			title: 'add(2, 3)',
			kind: 'module',
			module: 'four sections, one export',
			module_hex: '00 61 73 6d',
			module_fields: [{ offset: 0, length: 4, field: 'magic', value: '\\0asm' }]
		})!;
		expect(ex.blocks).toHaveLength(1);
		expect(ex.blocks[0].key).toBe('module');
		expect(ex.blocks[0].label).toBe('module');
		expect(ex.blocks[0].summary).toBe('four sections, one export');
		expect([...ex.blocks[0].bytes!]).toEqual([0x00, 0x61, 0x73, 0x6d]);
		expect(ex.blocks[0].fields[0].name).toBe('magic');
	});

	it('keeps several blocks, request and response first', () => {
		const ex = normalizeByteExample({
			client_hello_hex: 'aa',
			response_hex: 'bb',
			request_hex: 'cc',
			certificate_hex: 'dd'
		})!;
		expect(ex.blocks.map((b) => b.key)).toEqual([
			'request',
			'response',
			'client_hello',
			'certificate'
		]);
		expect(ex.blocks[2].label).toBe('client hello');
	});

	it('takes an explicit `blocks` array, with its own labels and order', () => {
		const ex = normalizeByteExample({
			blocks: [
				{ key: 'object', label: 'main.o', summary: 'one undefined symbol', hex: '7f 45 4c 46' },
				{ name: 'rela_text', hex: '00', fields: [{ offset: 0, length: 1, name: 'r_offset', value: '0' }] }
			]
		})!;
		expect(ex.blocks.map((b) => b.label)).toEqual(['main.o', 'rela text']);
		expect(ex.blocks[1].fields[0].name).toBe('r_offset');
	});

	it('labels the conventional names by kind, so a module is not a "request"', () => {
		const wire = normalizeByteExample({ request: 'a frame', request_hex: '00' })!;
		expect(wire.blocks[0].label).toBe('request →');
		const module = normalizeByteExample({ kind: 'module', request: 'a module', request_hex: '00', response: '7' })!;
		expect(module.blocks.map((b) => b.label)).toEqual(['the module', 'it prints']);
		// An explicit label from the generator always wins.
		const named = normalizeByteExample({ kind: 'module', request_hex: '00', request_label: 'the object' })!;
		expect(named.blocks[0].label).toBe('the object');
	});

	it('labels a linker example by what its two sides actually are', () => {
		const ex = normalizeByteExample({
			kind: 'object',
			request: 'hello.o',
			request_hex: '7f 45 4c 46',
			response: 'an ET_EXEC with one PT_LOAD',
			command: 'ld -o prog hello.o',
			stdout: 'hello, linker',
			exit_status: 0
		})!;
		expect(ex.blocks.map((b) => b.label)).toEqual(['the input object', 'what your linker must emit']);
		// `response` is prose here, so it renders as a summary and no hex dump.
		expect(hasBytes(blockOf(ex, 'response'))).toBe(false);
		expect(ex.transcript.at(-1)).toEqual({ kind: 'exit', text: '0' });
	});

	it('builds a transcript from what the example says running it prints', () => {
		const ex = normalizeByteExample({
			module_hex: '00',
			invoke: 'run --invoke add add.wasm 2 3',
			stdout: '5',
			stderr: 'warning: slow',
			exit_code: 0
		})!;
		expect(ex.transcript).toEqual([
			{ kind: 'input', text: 'run --invoke add add.wasm 2 3' },
			{ kind: 'stdout', text: '5' },
			{ kind: 'stderr', text: 'warning: slow' },
			{ kind: 'exit', text: '0' }
		]);
	});

	it('takes an explicit transcript, as strings or as rows, and splits newlines', () => {
		expect(normalizeByteExample({ request: 'x', transcript: ['a', 'b'] })!.transcript).toEqual([
			{ kind: 'stdout', text: 'a' },
			{ kind: 'stdout', text: 'b' }
		]);
		expect(
			normalizeByteExample({
				request: 'x',
				transcript: [{ kind: 'stderr', text: 'one\ntwo' }, { stream: 'nonsense', line: 'three' }]
			})!.transcript
		).toEqual([
			{ kind: 'stderr', text: 'one' },
			{ kind: 'stderr', text: 'two' },
			{ kind: 'stdout', text: 'three' }
		]);
	});

	it('drops the trailing newline a captured stdout carries, but keeps a blank line inside', () => {
		const ex = normalizeByteExample({ request: 'x', stdout: 'hi\nZZ\n' })!;
		expect(ex.transcript).toEqual([
			{ kind: 'stdout', text: 'hi' },
			{ kind: 'stdout', text: 'ZZ' }
		]);
		const spaced = normalizeByteExample({ request: 'x', stdout: 'a\n\nb\n' })!;
		expect(spaced.transcript.map((l) => l.text)).toEqual(['a', '', 'b']);
	});

	it('keeps an example that is only a transcript, and drops one that is nothing', () => {
		expect(normalizeByteExample({ stdout: 'hello' })?.transcript).toHaveLength(1);
		expect(normalizeByteExample({ note: 'just a note' })).toBeNull();
	});

	it('finds a block by key and says whether it has bytes worth drawing', () => {
		const ex = normalizeByteExample({ module_hex: '00 01', response: 'prose' })!;
		expect(blockOf(ex, 'module')?.bytes?.length).toBe(2);
		expect(blockOf(ex, 'nope')).toBeUndefined();
		expect(hasBytes(blockOf(ex, 'module'))).toBe(true);
		expect(hasBytes(blockOf(ex, 'response'))).toBe(false);
		expect(hasBytes(undefined)).toBe(false);
	});
});

describe('the worked examples every tester has shipped so far', () => {
	const withExamples = trackIds
		.map((track) => ({
			track,
			stages: getCatalog(track).stages.filter((s) => byteExampleCount(s) > 0)
		}))
		.filter((t) => t.stages.length > 0);

	it('has at least one track shipping byte examples', () => {
		expect(withExamples.length).toBeGreaterThan(0);
	});

	it('reads every one of them without losing a block or overrunning a frame', async () => {
		for (const { track, stages } of withExamples) {
			for (const stage of stages) {
				const examples = await loadExamples(track, stage);
				expect(examples.length, `${track} ${stage.number}`).toBe(byteExampleCount(stage));
				for (const ex of examples) {
					expect(ex.title, `${track} ${stage.number}`).toMatch(/\S/);
					expect(ex.blocks.length, `${track} ${stage.number} ${ex.title}`).toBeGreaterThan(0);
					for (const block of ex.blocks) {
						const limit = block.bytes?.length ?? 0;
						for (const f of block.fields) {
							expect(
								f.offset + f.length,
								`${track} ${stage.number} · ${ex.title} · ${f.name}`
							).toBeLessThanOrEqual(limit);
						}
					}
				}
			}
		}
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
		expect(byteExampleCount(stage({ exampleCount: 4 }))).toBe(4);
		// inline examples, when something supplies them, still win over the metadata
		expect(byteExampleCount(stage({ exampleCount: 4, examples: fixture.examples }))).toBe(2);
		expect(byteExampleCount(stage({}))).toBe(0);
		expect(byteExampleCount(null)).toBe(0);
		const real = getCatalog('kafka').stages.find((s) => s.number === 5)!;
		expect(exampleCount('kafka', real)).toBe(real.exampleCount);
	});
});
