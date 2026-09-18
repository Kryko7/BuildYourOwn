// @vitest-environment jsdom
/**
 * The lab used to die during hydration on a duplicate `{#each}` key, which took the whole
 * page with it ("the lab tab is not working"). These tests mount the real route component,
 * walk all four tabs, and fail on any console error or warning — a duplicate key is a
 * console error, so a regression cannot pass quietly.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { mount, unmount, flushSync } from 'svelte';
import LabPage from './+page.svelte';
import BatchBuilder from '$lib/components/lab/BatchBuilder.svelte';

class StubResizeObserver {
	observe() {}
	unobserve() {}
	disconnect() {}
}
(globalThis as { ResizeObserver?: unknown }).ResizeObserver ??= StubResizeObserver;

let host: HTMLDivElement;
let noise: unknown[] = [];

beforeEach(() => {
	host = document.createElement('div');
	document.body.appendChild(host);
	noise = [];
	vi.spyOn(console, 'error').mockImplementation((...args) => noise.push(args));
	vi.spyOn(console, 'warn').mockImplementation((...args) => noise.push(args));
});

afterEach(() => {
	host.remove();
	vi.restoreAllMocks();
});

function tabs() {
	return [...host.querySelectorAll<HTMLButtonElement>('[role="tab"]')];
}

describe('the lab page', () => {
	it('mounts from cold with no console errors and shows the first playground', () => {
		const app = mount(LabPage, { target: host });
		flushSync();
		expect(host.querySelector('h1')?.textContent).toBe('The lab');
		expect(tabs()).toHaveLength(4);
		expect(tabs()[0].getAttribute('aria-selected')).toBe('true');
		// the tokenizer is the default panel
		expect(host.querySelector('[role="tabpanel"]')?.id).toBe('panel-tokenizer');
		unmount(app);
		expect(noise).toEqual([]);
	});

	it('switches to every playground without errors', () => {
		const app = mount(LabPage, { target: host });
		flushSync();
		const ids = ['tokenizer', 'wire', 'batch', 'replay'];
		for (const [i, id] of ids.entries()) {
			tabs()[i].click();
			flushSync();
			expect(host.querySelector('[role="tabpanel"]')?.id).toBe(`panel-${id}`);
			expect(tabs()[i].getAttribute('aria-selected')).toBe('true');
			expect(host.querySelector('[role="tabpanel"]')?.children.length).toBeGreaterThan(0);
		}
		// and back again, which is where a keyed-list bug tends to bite
		tabs()[0].click();
		flushSync();
		expect(host.querySelector('[role="tabpanel"]')?.id).toBe('panel-tokenizer');
		unmount(app);
		expect(noise).toEqual([]);
	});

	it('renders the wire inspector with a decoded frame and the replay empty state', () => {
		const app = mount(LabPage, { target: host });
		flushSync();
		tabs()[1].click();
		flushSync();
		expect(host.textContent).toContain('ApiVersions(18)');
		expect(host.querySelectorAll('.hexrow').length).toBeGreaterThan(0);

		tabs()[3].click();
		flushSync();
		expect(host.textContent).toContain('byo test');
		unmount(app);
		expect(noise).toEqual([]);
	});
});

describe('the batch builder byte rows', () => {
	/**
	 * Two fields can legitimately start at the same offset (a zero-length field, or a
	 * record whose key is empty), so the anatomy list must not be keyed by offset alone.
	 */
	it('survives records that produce zero-length and repeated-offset fields', () => {
		const app = mount(BatchBuilder, { target: host });
		flushSync();

		const values = [...host.querySelectorAll<HTMLInputElement>('input.v')];
		const keys = [...host.querySelectorAll<HTMLInputElement>('input.k')];
		expect(values.length).toBeGreaterThan(0);

		// empty value and empty key on every record: as degenerate as the encoder gets
		for (const input of [...values, ...keys]) {
			input.value = '';
			input.dispatchEvent(new Event('input', { bubbles: true }));
			flushSync();
		}

		const addRecord = [...host.querySelectorAll<HTMLButtonElement>('button')].find((b) =>
			b.textContent?.includes('+ record')
		);
		expect(addRecord).toBeDefined();
		addRecord!.click();
		flushSync();
		addRecord!.click();
		flushSync();

		const rows = [...host.querySelectorAll('.fields li')];
		expect(rows.length).toBeGreaterThan(0);
		expect(host.querySelectorAll('.hexrow').length).toBeGreaterThan(0);
		unmount(app);
		expect(noise).toEqual([]);
	});
});
