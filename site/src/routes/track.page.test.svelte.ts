// @vitest-environment jsdom
/**
 * The dist trail climbs its algorithms rung second but numbers those stages 56–77, so a rung
 * has to say which stages it actually covers. That is what this pins down: the rungs in the
 * order the tester lists them, each naming its own range.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { mount, unmount, flushSync } from 'svelte';
import TrackPage from './[track=track]/+page.svelte';
import { catalogs } from '$lib/catalog';
import { tracks } from '$lib/tracks';

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

function mountTrack(track: 'dist' | 'shell') {
	return mount(TrackPage, {
		target: host,
		props: { data: { track, catalog: catalogs[track], report: null } }
	});
}

describe('the ladders on a track page', () => {
	it('draws a rung per ladder, in the order the tester lists them', () => {
		const app = mountTrack('dist');
		flushSync();
		const rungs = [...host.querySelectorAll<HTMLElement>('article.rung')];
		expect(rungs.map((r) => r.querySelector('h3')?.textContent?.trim())).toEqual(
			tracks.dist.ladders!.map((l) => l.title)
		);
		unmount(app);
		expect(noise).toEqual([]);
	});

	it('names the stages each rung covers, not just its sections', () => {
		const app = mountTrack('dist');
		flushSync();
		const text = (i: number) =>
			host.querySelectorAll<HTMLElement>('article.rung')[i].querySelector('.rsec')!.textContent!;
		// The rung climbed second is numbered last: that is exactly why the range is printed.
		expect(text(1)).toContain('stages 56–88');
		expect(text(1)).toContain('sections I, J, K, L, M, N, O, P');
		expect(text(0)).toContain('stages 1–20');
		expect(text(2)).toContain('stages 21–35');
		expect(text(3)).toContain('stages 36–55');
		unmount(app);
		expect(noise).toEqual([]);
	});

	it('counts every stage of the track across the rungs exactly once', () => {
		const app = mountTrack('dist');
		flushSync();
		const counts = [...host.querySelectorAll<HTMLElement>('article.rung .chip')].map((c) =>
			Number(c.textContent!.trim().split('/')[1])
		);
		expect(counts.reduce((a, b) => a + b, 0)).toBe(catalogs.dist.totals.stages);
		unmount(app);
		expect(noise).toEqual([]);
	});

	it('draws no rungs at all for a track without ladders', () => {
		const app = mountTrack('shell');
		flushSync();
		expect(host.querySelector('article.rung')).toBeNull();
		unmount(app);
		expect(noise).toEqual([]);
	});
});
