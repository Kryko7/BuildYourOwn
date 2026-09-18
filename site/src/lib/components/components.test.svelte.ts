// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { mount, unmount, flushSync } from 'svelte';
import ProgressRing from './ProgressRing.svelte';
import TestList from './TestList.svelte';
import JourneyMap from './JourneyMap.svelte';
import StageDrawer from './StageDrawer.svelte';
import Tokenizer from './lab/Tokenizer.svelte';
import WireInspector from './lab/WireInspector.svelte';
import { getCatalog } from '$lib/catalog';
import { parseReport } from '$lib/report';
import type { Catalog } from '$lib/types';
import broken from '$lib/data/fixtures/report.shell.broken.json';

// jsdom has neither of these; svelte's `bind:clientWidth` needs the observer.
class StubResizeObserver {
	observe() {}
	unobserve() {}
	disconnect() {}
}
(globalThis as { ResizeObserver?: unknown }).ResizeObserver ??= StubResizeObserver;

let host: HTMLDivElement;
let errors: unknown[] = [];

/**
 * The real kafka catalog fills up as kafkatest merges its stages, so the gapped case — some
 * stages shipped, the rest filled in from the plan with `planned: true` — is built here
 * rather than read off disk, and keeps being tested after the tester is complete.
 */
function gappedKafka(): Catalog {
	const real = getCatalog('kafka');
	const stages = real.stages.slice(0, 6).map((s, i) =>
		i < 3
			? { ...s, planned: undefined }
			: { ...s, planned: true, tests: [], file: '', hints: s.hints.length ? s.hints : ['from the plan'] }
	);
	return {
		...real,
		sections: [{ id: 'A', title: 'Bootstrap & framing', stages: stages.map((s) => s.number) }],
		stages,
		totals: {
			stages: stages.length,
			tests: stages.reduce((n, s) => n + s.tests.length, 0),
			ext: stages.filter((s) => s.ext).length
		}
	};
}

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
});

describe('ProgressRing', () => {
	it('renders the rounded percentage and an accessible total', () => {
		const c = mount(ProgressRing, { target: host, props: { value: 3, total: 4, label: 'stages' } });
		expect(host.textContent).toContain('75');
		expect(host.textContent).toContain('3 of 4 complete');
		unmount(c);
		expect(errors).toEqual([]);
	});
});

describe('TestList', () => {
	const tests = [
		{ name: 'passing one' },
		{ name: 'failing one' },
		{ name: 'ext one', ext: true },
		{ name: 'skipped one', skipOn: ['zsh'] }
	];
	const results = {
		'passing one': { name: 'passing one', status: 'pass' as const, ext: false, durationMs: 4, failures: [], skipReason: null, actual: [] },
		'failing one': { name: 'failing one', status: 'fail' as const, ext: false, durationMs: 9, failures: ['stdout: output differs'], skipReason: null, actual: [['stdout', 'nope'] as [string, string]] },
		'skipped one': { name: 'skipped one', status: 'skip' as const, ext: false, durationMs: 0, failures: [], skipReason: 'zsh words it differently', actual: [] }
	};

	it('summarizes pass/fail/skip and shows the failure message', () => {
		const c = mount(TestList, { target: host, props: { tests, results } });
		expect(host.textContent).toContain('1 passing');
		expect(host.textContent).toContain('1 failing');
		expect(host.textContent).toContain('1 skipped');
		expect(host.textContent).toContain('stdout: output differs');
		expect(host.querySelectorAll('li').length).toBe(4);
		unmount(c);
		expect(errors).toEqual([]);
	});

	it('explains an empty pending catalog instead of showing nothing', () => {
		const c = mount(TestList, { target: host, props: { tests: [], pending: true } });
		expect(host.textContent).toMatch(/kafkatest/);
		unmount(c);
	});

	it('says a planned stage is not in the tester yet', () => {
		const c = mount(TestList, { target: host, props: { tests: [], planned: true } });
		expect(host.textContent).toContain('Not yet in the tester');
		expect(host.textContent).toMatch(/PLAN\.md/);
		unmount(c);
		expect(errors).toEqual([]);
	});

	// Real suites repeat stdin lines all the time (`echo hi` twice, a prompt line, a blank
	// line). Keying an {#each} by content makes Svelte throw each_key_duplicate and takes the
	// whole drawer down with it, so every list here is keyed by index.
	it('renders a test whose stdin has two identical lines', () => {
		const repeated = [
			{
				name: 'repeats a line',
				input: ['echo hi', 'echo hi', '', ''],
				expect: [
					{ label: 'stdout', kind: 'exact' as const, value: 'hi\nhi' },
					{ label: 'stdout', kind: 'exact' as const, value: 'hi\nhi' }
				]
			}
		];
		const c = mount(TestList, { target: host, props: { tests: repeated } });
		host.querySelector<HTMLButtonElement>('button.name')!.click();
		flushSync();
		const terminal = host.querySelector('.terminal')!;
		expect(terminal.textContent!.match(/echo hi/g)).toHaveLength(2);
		unmount(c);
		expect(errors).toEqual([]);
	});

	it('renders two tests that share a name', () => {
		const dupes = [{ name: 'same name' }, { name: 'same name' }, { name: 'same name' }];
		const c = mount(TestList, { target: host, props: { tests: dupes } });
		expect(host.querySelectorAll('li').length).toBe(3);
		// expanding one must not expand its twin
		host.querySelectorAll<HTMLButtonElement>('button.name')[1].click();
		flushSync();
		expect(host.querySelectorAll('[aria-expanded="true"]').length).toBe(1);
		unmount(c);
		expect(errors).toEqual([]);
	});

	it('renders a failing test whose failures and actual rows repeat', () => {
		const spec = [
			{
				name: 'flaky',
				input: ['ls', 'ls'],
				expect: [{ label: 'stdout', kind: 'exact' as const, value: 'a' }]
			}
		];
		const result = {
			flaky: {
				name: 'flaky',
				status: 'fail' as const,
				ext: false,
				durationMs: 2,
				failures: ['stdout: output differs', 'stdout: output differs'],
				skipReason: null,
				actual: [['stdout', 'b'] as [string, string], ['stdout', 'b'] as [string, string]]
			}
		};
		const c = mount(TestList, { target: host, props: { tests: spec, results: result } });
		host.querySelector<HTMLButtonElement>('button.name')!.click();
		flushSync();
		expect(host.querySelector('.terminal')!.textContent).toContain('output differs');
		unmount(c);
		expect(errors).toEqual([]);
	});
});

describe('JourneyMap', () => {
	it('draws a waypoint per stage with an aria label and a camp per section', () => {
		const catalog = getCatalog('shell');
		const report = parseReport(broken, { track: 'shell' });
		const clicked: number[] = [];
		const c = mount(JourneyMap, {
			target: host,
			props: { track: 'shell', catalog, report, selected: null, onselect: (n: number) => clicked.push(n) }
		});
		const nodes = host.querySelectorAll('[data-node]');
		expect(nodes.length).toBe(catalog.totals.stages);
		expect(nodes[0].getAttribute('aria-label')).toMatch(/^Stage 1: Print the prompt/);
		expect(host.querySelectorAll('.camp').length).toBe(catalog.sections.length);

		// the broken-shell report fails stages 2 and 5, so those waypoints must read as failing
		const five = host.querySelector('[data-node="5"]')!;
		expect(five.getAttribute('class')).toContain('failing');

		(nodes[3] as HTMLElement).dispatchEvent(new MouseEvent('click', { bubbles: true }));
		flushSync();
		expect(clicked).toEqual([4]);
		unmount(c);
		expect(errors).toEqual([]);
	});

	it('opens a stage on Enter and on Space, not only on click', () => {
		const catalog = getCatalog('shell');
		const clicked: number[] = [];
		const c = mount(JourneyMap, {
			target: host,
			props: { track: 'shell', catalog, report: null, selected: null, onselect: (n: number) => clicked.push(n) }
		});
		const node = host.querySelector('[data-node="7"]')!;
		node.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
		flushSync();
		node.dispatchEvent(new KeyboardEvent('keydown', { key: ' ', bubbles: true }));
		flushSync();
		expect(clicked).toEqual([7, 7]);
		unmount(c);
		expect(errors).toEqual([]);
	});

	it('walks the trail with the arrow keys and keeps one roving tab stop', () => {
		const catalog = getCatalog('shell');
		const c = mount(JourneyMap, {
			target: host,
			props: { track: 'shell', catalog, report: null, selected: null, onselect: () => {} }
		});
		const tabStops = () =>
			[...host.querySelectorAll('[data-node]')]
				.filter((n) => n.getAttribute('tabindex') === '0')
				.map((n) => n.getAttribute('data-node'));

		// nothing done yet, so stage 1 is "up next" and holds the only tab stop
		expect(tabStops()).toEqual(['1']);

		host.querySelector('[data-node="1"]')!.dispatchEvent(
			new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true })
		);
		flushSync();
		expect(tabStops()).toEqual(['2']);

		host.querySelector('[data-node="2"]')!.dispatchEvent(
			new KeyboardEvent('keydown', { key: 'ArrowLeft', bubbles: true })
		);
		flushSync();
		expect(tabStops()).toEqual(['1']);

		host.querySelector('[data-node="1"]')!.dispatchEvent(
			new KeyboardEvent('keydown', { key: 'End', bubbles: true })
		);
		flushSync();
		expect(tabStops()).toEqual([String(catalog.stages[catalog.stages.length - 1].number)]);

		host.querySelector(`[data-node="${catalog.stages[catalog.stages.length - 1].number}"]`)!.dispatchEvent(
			new KeyboardEvent('keydown', { key: 'Home', bubbles: true })
		);
		flushSync();
		expect(tabStops()).toEqual([String(catalog.stages[0].number)]);
		unmount(c);
		expect(errors).toEqual([]);
	});

	it('names every camp after the expedition, never "Section b"', () => {
		for (const track of ['shell', 'kafka'] as const) {
			const catalog = getCatalog(track);
			const c = mount(JourneyMap, {
				target: host,
				props: { track, catalog, report: null, selected: null, onselect: () => {} }
			});
			expect(host.querySelectorAll('[data-node]').length).toBe(catalog.totals.stages);
			expect(host.querySelectorAll('.camp').length).toBe(catalog.sections.length);
			expect(host.textContent).not.toMatch(/Section [A-Za-z]\b/);
			unmount(c);
		}
		expect(errors).toEqual([]);
	});

	it('draws a gapped catalog in full, planned waypoints marked as such', () => {
		const catalog = gappedKafka();
		const c = mount(JourneyMap, {
			target: host,
			props: { track: 'kafka' as const, catalog, report: null, selected: null, onselect: () => {} }
		});
		expect(host.querySelectorAll('[data-node]').length).toBe(catalog.stages.length);
		for (const s of catalog.stages) {
			const node = host.querySelector(`[data-node="${s.number}"]`)!;
			expect(Boolean(node.getAttribute('class')?.includes('planned'))).toBe(s.planned === true);
			expect(node.getAttribute('aria-label')!.includes('Not yet in the tester')).toBe(
				s.planned === true
			);
		}
		unmount(c);
		expect(errors).toEqual([]);
	});
});

describe('StageDrawer', () => {
	const catalog = getCatalog('shell');

	it('renders the selected stage with its command, tests and prev/next', () => {
		const stage = catalog.stages.find((s) => s.number === 5)!;
		const navigated: number[] = [];
		let closed = 0;
		const c = mount(StageDrawer, {
			target: host,
			props: {
				track: 'shell' as const,
				stage,
				catalog,
				report: null,
				onclose: () => closed++,
				onnavigate: (n: number) => navigated.push(n)
			}
		});
		expect(host.textContent).toContain(stage.name);
		expect(host.textContent).toContain('--shell');
		expect(host.querySelector('.drawer')).toBeTruthy();

		const [prev, next] = host.querySelectorAll<HTMLButtonElement>('.nav .btn');
		next.click();
		prev.click();
		flushSync();
		expect(navigated).toEqual([6, 4]);

		host.querySelector<HTMLElement>('.scrim')!.click();
		flushSync();
		expect(closed).toBe(1);
		unmount(c);
		expect(errors).toEqual([]);
	});

	it('renders nothing at all when no stage is selected', () => {
		const c = mount(StageDrawer, {
			target: host,
			props: {
				track: 'shell' as const,
				stage: null,
				catalog,
				report: null,
				onclose: () => {},
				onnavigate: () => {}
			}
		});
		expect(host.querySelector('.drawer')).toBeNull();
		unmount(c);
		expect(errors).toEqual([]);
	});

	it('shows a planned kafka stage as not yet in the tester', () => {
		const kafka = gappedKafka();
		const planned = kafka.stages.find((s) => s.planned)!;
		const c = mount(StageDrawer, {
			target: host,
			props: {
				track: 'kafka' as const,
				stage: planned,
				catalog: kafka,
				report: null,
				onclose: () => {},
				onnavigate: () => {}
			}
		});
		expect(host.textContent).toContain('Not yet in the tester');
		expect(host.textContent).toContain(planned.hints[0]);
		// …and a shipped stage in the same catalog does not claim to be planned
		unmount(c);

		const shipped = kafka.stages.find((s) => !s.planned)!;
		const c2 = mount(StageDrawer, {
			target: host,
			props: {
				track: 'kafka' as const,
				stage: shipped,
				catalog: kafka,
				report: null,
				onclose: () => {},
				onnavigate: () => {}
			}
		});
		expect(host.textContent).not.toContain('Not yet in the tester');
		unmount(c2);
		expect(errors).toEqual([]);
	});
});

describe('Tokenizer playground', () => {
	it('colours the line and lists the words it produces', () => {
		const c = mount(Tokenizer, { target: host, props: { initial: `echo "a b" 'c' | wc -l > out.txt` } });
		expect(host.querySelectorAll('.s-double').length).toBeGreaterThan(0);
		expect(host.querySelectorAll('.s-single').length).toBeGreaterThan(0);
		expect(host.querySelector('.words')!.textContent).toContain('a b');
		expect(host.textContent).toContain('out.txt');
		expect(host.textContent).toContain('truncate and write stdout');
		unmount(c);
		expect(errors).toEqual([]);
	});
});

describe('Wire inspector', () => {
	it('renders a hex dump and the decoded fields of the chosen sample', () => {
		const c = mount(WireInspector, { target: host, props: { initialSample: 'produce-v11' } });
		expect(host.textContent).toContain('Produce(0) v11');
		expect(host.textContent).toContain('batch.crc');
		expect(host.querySelectorAll('.hexrow').length).toBeGreaterThan(2);
		unmount(c);
		expect(errors).toEqual([]);
	});
});
