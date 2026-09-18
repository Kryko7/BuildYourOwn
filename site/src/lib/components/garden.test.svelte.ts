// @vitest-environment jsdom
/**
 * The pastel-garden layer: the flower waypoints, the worked examples and the one banner
 * that tells the visitor where the numbers are coming from.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { mount, unmount, flushSync } from 'svelte';
import JourneyMap from './JourneyMap.svelte';
import Bloom from './garden/Bloom.svelte';
import Fox from './garden/Fox.svelte';
import Bunny from './garden/Bunny.svelte';
import Cat from './garden/Cat.svelte';
import Flutterers from './garden/Flutterers.svelte';
import ShellTranscript from './examples/ShellTranscript.svelte';
import WireExample from './examples/WireExample.svelte';
import StageExamples from './examples/StageExamples.svelte';
import ConnectionNote from './ConnectionNote.svelte';
import { getCatalog } from '$lib/catalog';
import { kafkaExamples, loadKafkaExampleData } from '$lib/examples/kafka';
import { shellExamples } from '$lib/examples/shell';
import { journey } from '$lib/stores/journey.svelte';
import fixture from '$lib/data/fixtures/kafka-examples.sample.json';
import type { StageSpec } from '$lib/types';

class StubResizeObserver {
	observe() {}
	unobserve() {}
	disconnect() {}
}
(globalThis as { ResizeObserver?: unknown }).ResizeObserver ??= StubResizeObserver;

let host: HTMLDivElement;
let errors: unknown[] = [];

beforeEach(() => {
	host = document.createElement('div');
	document.body.appendChild(host);
	errors = [];
	vi.spyOn(console, 'error').mockImplementation((...args) => errors.push(args));
	vi.spyOn(console, 'warn').mockImplementation((...args) => errors.push(args));
});

afterEach(() => {
	host.remove();
	vi.restoreAllMocks();
	journey.status = 'probing';
	journey.health = null;
});

const exampleStage = (examples: unknown[]): StageSpec => ({
	number: 5,
	slug: 'api-versions',
	name: 'ApiVersions v4 response body',
	ext: false,
	section: 'A',
	file: 'src/stages/s05.rs',
	planDone: true,
	hints: [],
	tests: [],
	examples
});

describe('Bloom', () => {
	it('draws a bud for a stage not started and petals for one that is done', () => {
		const bud = mount(Bloom, { target: host, props: { state: 'locked', number: 3 } });
		expect(host.querySelector('.bud')).toBeTruthy();
		expect(host.querySelectorAll('.petal')).toHaveLength(0);
		expect(host.querySelector('.num')!.textContent).toBe('3');
		unmount(bud);

		const flower = mount(Bloom, { target: host, props: { state: 'done', number: 3 } });
		expect(host.querySelectorAll('.petal')).toHaveLength(6);
		expect(host.querySelector('.bud')).toBeNull();
		unmount(flower);

		const poppy = mount(Bloom, { target: host, props: { state: 'failing', number: 9 } });
		expect(host.querySelector('.bloom')!.getAttribute('class')).toContain('failing');
		expect(host.querySelectorAll('.petal')).toHaveLength(6);
		unmount(poppy);
		expect(errors).toEqual([]);
	});

	it('gives every petal its own rotation, so none of them stack', () => {
		const c = mount(Bloom, { target: host, props: { state: 'done', number: 1 } });
		const rotations = [...host.querySelectorAll<SVGElement>('.petal')].map((p) =>
			p.getAttribute('style')
		);
		expect(new Set(rotations).size).toBe(6);
		unmount(c);
		expect(errors).toEqual([]);
	});
});

describe('the animals', () => {
	it('are decorative by default and images when named', () => {
		for (const Animal of [Fox, Bunny, Cat]) {
			const c = mount(Animal, { target: host, props: { size: 40 } });
			const svg = host.querySelector('svg')!;
			expect(svg.getAttribute('aria-hidden')).toBe('true');
			expect(svg.getAttribute('role')).toBe('presentation');
			unmount(c);
		}
		const named = mount(Fox, { target: host, props: { size: 40, label: 'A fox' } });
		expect(host.querySelector('svg')!.getAttribute('aria-label')).toBe('A fox');
		expect(host.querySelector('svg')!.getAttribute('role')).toBe('img');
		unmount(named);
		expect(errors).toEqual([]);
	});

	it('puts the same butterflies in the same places on every render', () => {
		const first = mount(Flutterers, { target: host, props: { count: 5, seed: 3 } });
		const a = [...host.querySelectorAll('.bug')].map((b) => b.getAttribute('style'));
		unmount(first);
		const second = mount(Flutterers, { target: host, props: { count: 5, seed: 3 } });
		const b = [...host.querySelectorAll('.bug')].map((el) => el.getAttribute('style'));
		expect(a).toEqual(b);
		expect(a).toHaveLength(5);
		unmount(second);
		expect(errors).toEqual([]);
	});
});

describe('JourneyMap, garden edition', () => {
	it('gives every stage a flower and the track its mascot', () => {
		const catalog = getCatalog('shell');
		const c = mount(JourneyMap, {
			target: host,
			props: { track: 'shell' as const, catalog, report: null, selected: null, onselect: () => {} }
		});
		expect(host.querySelectorAll('[data-node] .bloom').length).toBe(catalog.totals.stages);
		expect(host.querySelector('.mascot svg.fox')).toBeTruthy();
		// the meadow floor is decorative and must never reach the accessibility tree
		expect(host.querySelector('.scatter')!.getAttribute('aria-hidden')).toBe('true');
		expect(host.querySelector('.mascot')!.getAttribute('aria-hidden')).toBe('true');
		unmount(c);
		expect(errors).toEqual([]);
	});

	it('gives the kafka trail the bunny', () => {
		const catalog = getCatalog('kafka');
		const c = mount(JourneyMap, {
			target: host,
			props: { track: 'kafka' as const, catalog, report: null, selected: null, onselect: () => {} }
		});
		expect(host.querySelector('.mascot svg.bunny')).toBeTruthy();
		unmount(c);
		expect(errors).toEqual([]);
	});

	it('keeps a name, a state and one tab stop on every waypoint (a11y smoke)', () => {
		const catalog = getCatalog('shell');
		const c = mount(JourneyMap, {
			target: host,
			props: { track: 'shell' as const, catalog, report: null, selected: 4, onselect: () => {} }
		});
		const nodes = [...host.querySelectorAll('[data-node]')];
		expect(nodes.length).toBe(catalog.totals.stages);
		for (const n of nodes) {
			const label = n.getAttribute('aria-label') ?? '';
			expect(label).toMatch(/^Stage \d+: .+\. (not started|available|up next|in progress|done|failing)\./);
			expect(n.getAttribute('role')).toBe('button');
			expect(['0', '-1']).toContain(n.getAttribute('tabindex'));
			expect(n.getAttribute('aria-pressed')).toBe(String(n.getAttribute('data-node') === '4'));
		}
		expect(nodes.filter((n) => n.getAttribute('tabindex') === '0')).toHaveLength(1);
		expect(host.querySelector('[role="group"]')!.getAttribute('aria-label')).toBe(
			'shell garden trail'
		);
		unmount(c);
		expect(errors).toEqual([]);
	});
});

describe('ShellTranscript', () => {
	it('draws the derived transcript with a prompt on each input line', () => {
		const stage = getCatalog('shell').stages.find((s) => s.number === 5)!;
		const example = shellExamples(stage)[0];
		const c = mount(ShellTranscript, { target: host, props: { example } });
		expect(host.querySelectorAll('.ln.input').length).toBeGreaterThan(0);
		expect(host.querySelector('.ln.input .gut')!.textContent).toBe('$');
		expect(host.textContent).toContain(example.title);
		unmount(c);
		expect(errors).toEqual([]);
	});
});

describe('WireExample', () => {
	const example = kafkaExamples(exampleStage(fixture.examples))[0];

	it('shows request and response side by side with their bytes', () => {
		const c = mount(WireExample, { target: host, props: { example } });
		expect(host.querySelectorAll('.side').length).toBe(2);
		expect(host.textContent).toContain('ApiVersions(18) v4');
		expect(host.textContent).toContain('error_code 0');
		expect(host.querySelectorAll('.side.request .b').length).toBe(41);
		expect(host.querySelectorAll('.side.response .b').length).toBe(37);
		expect(host.textContent).toContain(example.note!.slice(0, 20));
		unmount(c);
		expect(errors).toEqual([]);
	});

	it('lights a field’s bytes when the field is hovered, and only on that side', () => {
		const c = mount(WireExample, { target: host, props: { example } });
		const apiKeyRow = [...host.querySelectorAll('.side.request .fields li')][1];
		apiKeyRow.dispatchEvent(new MouseEvent('mouseenter', { bubbles: false }));
		flushSync();

		const hotRequest = [...host.querySelectorAll('.side.request .b.hot')];
		// api_key is two bytes at offset 4
		expect(hotRequest).toHaveLength(2);
		expect(hotRequest.map((b) => b.textContent)).toEqual(['00', '12']);
		expect(host.querySelectorAll('.side.response .b.hot')).toHaveLength(0);
		unmount(c);
		expect(errors).toEqual([]);
	});

	it('finds the field when a byte is hovered', () => {
		const c = mount(WireExample, { target: host, props: { example } });
		const bytes = [...host.querySelectorAll<HTMLElement>('.side.response .b')];
		// offset 8-9 is the response's error_code
		bytes[8].dispatchEvent(new MouseEvent('mouseenter', { bubbles: false }));
		flushSync();
		const on = host.querySelector('.side.response .fields li.on')!;
		expect(on.textContent).toContain('error_code');
		expect(host.querySelectorAll('.side.response .b.hot')).toHaveLength(2);
		unmount(c);
		expect(errors).toEqual([]);
	});

	it('stops a huge frame at 256 bytes and opens the rest on request', () => {
		const big = Array.from({ length: 600 }, (_, i) => i % 256);
		const huge = kafkaExamples(
			exampleStage([
				{
					title: 'A long response',
					request: 'small',
					request_hex: '00 01',
					response: 'enormous',
					response_hex: big.map((b) => b.toString(16).padStart(2, '0')).join(''),
					response_fields: [
						{ offset: 0, length: 4, field: 'size', value: '596' },
						{ offset: 500, length: 4, field: 'way down there', value: 'x' }
					]
				}
			])
		)[0];
		const c = mount(WireExample, { target: host, props: { example: huge } });
		expect(host.querySelectorAll('.side.response .b')).toHaveLength(256);
		const more = [...host.querySelectorAll<HTMLButtonElement>('.side.response button')].find((b) =>
			b.textContent?.includes('show all 600 bytes')
		);
		expect(more).toBeDefined();
		more!.click();
		flushSync();
		expect(host.querySelectorAll('.side.response .b')).toHaveLength(600);
		unmount(c);
		expect(errors).toEqual([]);
	});

	it('opens the rest of a dump when a field past the cut is selected', () => {
		const big = Array.from({ length: 600 }, () => 0);
		const huge = kafkaExamples(
			exampleStage([
				{
					title: 'A long response',
					response: 'enormous',
					response_hex: big.map(() => '00').join(''),
					response_fields: [
						{ offset: 0, length: 4, field: 'size', value: '596' },
						{ offset: 500, length: 4, field: 'way down there', value: 'x' }
					]
				}
			])
		)[0];
		const c = mount(WireExample, { target: host, props: { example: huge } });
		expect(host.querySelectorAll('.side.response .b')).toHaveLength(256);
		const rows = [...host.querySelectorAll('.side.response .fields li')];
		rows[1].dispatchEvent(new MouseEvent('mouseenter'));
		flushSync();
		expect(host.querySelectorAll('.side.response .b')).toHaveLength(600);
		expect(host.querySelectorAll('.side.response .b.hot')).toHaveLength(4);
		unmount(c);
		expect(errors).toEqual([]);
	});

	it('flags a value that changes every run instead of presenting it as a constant', () => {
		const ex = kafkaExamples(
			exampleStage([
				{
					title: 'Topic id',
					response: 'a topic',
					response_hex: 'aabbccdd',
					response_fields: [
						{ offset: 0, length: 2, field: 'fixed', value: '1' },
						{ offset: 2, length: 2, field: 'topic_id', value: 'uuid', varies: true }
					]
				}
			])
		)[0];
		const c = mount(WireExample, { target: host, props: { example: ex } });
		expect(host.querySelector('.fval.varies')).toBeTruthy();
		[...host.querySelectorAll('.side.response .fields li')][1].dispatchEvent(
			new MouseEvent('mouseenter')
		);
		flushSync();
		expect(host.textContent).toContain('varies per run');
		unmount(c);
		expect(errors).toEqual([]);
	});

	it('labels a stage where the lesson is that nothing goes on the wire', () => {
		const ex = kafkaExamples(
			exampleStage([
				{
					title: 'acks = 0',
					kind: 'silence',
					request: 'Produce with acks 0',
					request_hex: '',
					response: 'nothing at all',
					response_hex: ''
				}
			])
		)[0];
		const c = mount(WireExample, { target: host, props: { example: ex } });
		expect(host.textContent).toContain('nothing on the wire');
		expect(host.querySelectorAll('.hexrow')).toHaveLength(0);
		unmount(c);
		expect(errors).toEqual([]);
	});

	it('shows the fixture an example was captured against', () => {
		const ex = kafkaExamples(
			exampleStage([
				{
					title: 'Describe',
					request: 'DescribeTopicPartitions',
					env: {
						topics: { t1: { name: 't1-ex131', id: 'c637cd48-8953-49b7', partitions: 3 } },
						group: 'kafkatest-group'
					}
				}
			])
		)[0];
		const c = mount(WireExample, { target: host, props: { example: ex } });
		expect(host.textContent).toContain('set up with');
		expect(host.textContent).toContain('t1-ex131');
		expect(host.textContent).toContain('3 partitions');
		expect(host.textContent).toContain('kafkatest-group');
		unmount(c);
		expect(errors).toEqual([]);
	});

	it('renders a text-only side without inventing a hex dump', () => {
		const prose = kafkaExamples(
			exampleStage([{ title: 'Bind', request: 'a TCP connect', response: 'nothing at all' }])
		)[0];
		const c = mount(WireExample, { target: host, props: { example: prose } });
		expect(host.querySelectorAll('.hexrow')).toHaveLength(0);
		expect(host.textContent).toContain('a TCP connect');
		unmount(c);
		expect(errors).toEqual([]);
	});
});

describe('StageExamples', () => {
	it('renders shell transcripts for the shell track', () => {
		const stage = getCatalog('shell').stages.find((s) => s.number === 5)!;
		const c = mount(StageExamples, { target: host, props: { track: 'shell' as const, stage } });
		expect(host.querySelectorAll('figure.ex').length).toBeGreaterThan(0);
		unmount(c);
		expect(errors).toEqual([]);
	});

	it('renders wire examples for the kafka track', () => {
		const c = mount(StageExamples, {
			target: host,
			props: { track: 'kafka' as const, stage: exampleStage(fixture.examples) }
		});
		expect(host.querySelectorAll('article.wex').length).toBe(2);
		unmount(c);
		expect(errors).toEqual([]);
	});

	it('renders the examples kafkatest actually shipped, as the stage page preloads them', async () => {
		const stage = getCatalog('kafka').stages.find((s) => (s.exampleCount ?? 0) > 0);
		// kafkatest ships these; if it ever stops, this test says so rather than passing empty
		expect(stage).toBeDefined();
		const preloaded = await loadKafkaExampleData(stage!.number);
		expect(preloaded).not.toBeNull();
		const c = mount(StageExamples, {
			target: host,
			props: { track: 'kafka' as const, stage: stage!, preloaded }
		});
		expect(host.querySelectorAll('article.wex').length).toBeGreaterThan(0);
		expect(host.querySelectorAll('.side').length).toBeGreaterThan(0);
		unmount(c);
		expect(errors).toEqual([]);
	});

	it('fetches a stage’s frames on its own when nothing preloaded them (the drawer)', async () => {
		const stage = getCatalog('kafka').stages.find((s) => (s.exampleCount ?? 0) > 0)!;
		const c = mount(StageExamples, {
			target: host,
			props: { track: 'kafka' as const, stage }
		});
		flushSync();
		// the lazy chunk has not arrived yet, so the section says it is coming
		expect(host.textContent).toContain('Fetching');
		await vi.waitFor(() => {
			flushSync();
			expect(host.querySelectorAll('article.wex').length).toBeGreaterThan(0);
		});
		unmount(c);
		expect(errors).toEqual([]);
	});

	it('renders nothing when the catalog has no examples yet', () => {
		const c = mount(StageExamples, {
			target: host,
			props: { track: 'kafka' as const, stage: exampleStage([]) }
		});
		expect(host.textContent!.trim()).toBe('');
		unmount(c);
		expect(errors).toEqual([]);
	});
});

describe('ConnectionNote', () => {
	it('says nothing while the probe is still in flight', () => {
		journey.status = 'probing';
		const c = mount(ConnectionNote, { target: host, props: {} });
		expect(host.textContent!.trim()).toBe('');
		unmount(c);
		expect(errors).toEqual([]);
	});

	it('tells an unconnected visitor exactly what to run', () => {
		journey.status = 'offline';
		const c = mount(ConnectionNote, { target: host, props: {} });
		expect(host.textContent).toContain('Not connected to byo');
		expect(host.textContent).toContain('byo site');
		unmount(c);
		expect(errors).toEqual([]);
	});

	it('says it is reconnecting rather than blanking the page', () => {
		journey.status = 'reconnecting';
		const c = mount(ConnectionNote, { target: host, props: {} });
		expect(host.textContent).toContain('stopped answering');
		unmount(c);
		expect(errors).toEqual([]);
	});

	it('asks for `byo init` when byo is up but the track has no project', () => {
		journey.status = 'connected';
		journey.health = {
			ok: true,
			version: '0.1.0',
			db: '/db',
			tracks: { shell: { project: null }, kafka: { project: null } }
		};
		const c = mount(ConnectionNote, { target: host, props: { track: 'kafka' as const } });
		expect(host.textContent).toContain('byo init kafka');
		unmount(c);

		journey.health.tracks.shell = {
			project: { id: 1, track: 'shell', path: '/p', command: 'bash' }
		};
		const quiet = mount(ConnectionNote, { target: host, props: { track: 'shell' as const } });
		expect(host.textContent!.trim()).toBe('');
		unmount(quiet);
		expect(errors).toEqual([]);
	});
});
