// @vitest-environment jsdom
/**
 * The home page was designed for two tracks and now carries six, so what is worth pinning
 * down is that it still reads as a garden: one card per registered track, laid out in beds
 * rather than in one long column, and a pending track saying what it is instead of
 * pretending to be a finished trail.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { mount, unmount, flushSync } from 'svelte';
import HomePage from './+page.svelte';
import StageContent from '$lib/components/StageContent.svelte';
import { catalogs, tracks, trackIds } from '$lib/catalog';

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

describe('the home page with every track', () => {
	it('draws one card per registered track, in trail order', () => {
		const app = mount(HomePage, { target: host });
		flushSync();
		const cards = [...host.querySelectorAll<HTMLAnchorElement>('a.track')];
		expect(cards).toHaveLength(trackIds.length);
		expect(cards.map((c) => new URL(c.href, 'http://x').pathname.replace(/\/$/, ''))).toEqual(
			trackIds.map((t) => `/${t}`)
		);
		for (const track of trackIds) {
			expect(host.textContent, track).toContain(tracks[track].title);
		}
		unmount(app);
		expect(noise).toEqual([]);
	});

	it('lays them out as beds, not as one list', () => {
		const app = mount(HomePage, { target: host });
		flushSync();
		const rows = [...host.querySelectorAll<HTMLElement>('section.tracks')];
		// Six tracks read as a row of two and a row of four; one row of six would be a table
		// of contents, and six rows of one would be a list.
		expect(rows.length).toBeGreaterThan(1);
		const perRow = rows.map((r) => r.querySelectorAll('a.track').length);
		expect(perRow.reduce((a, b) => a + b, 0)).toBe(trackIds.length);
		expect(rows[0].classList.contains('front')).toBe(true);
		expect(perRow[0]).toBe(2);
		for (const row of rows) expect(row.style.getPropertyValue('--cols')).toMatch(/^\d+$/);
		unmount(app);
		expect(noise).toEqual([]);
	});

	it('says a pending track is still being written instead of quoting a stage count', () => {
		const app = mount(HomePage, { target: host });
		flushSync();
		for (const track of trackIds) {
			const card = host.querySelector<HTMLElement>(`a.track[href^="/${track}"]`)!;
			if (catalogs[track].pending) {
				expect(card.textContent, track).toContain('still being written');
				expect(card.textContent, track).toContain(`≈${tracks[track].plannedStages} stages planned`);
				expect(card.textContent, track).toContain('first waypoint');
			} else {
				expect(card.textContent, track).toContain(`${catalogs[track].totals.stages} stages`);
				expect(card.textContent, track).toContain('continue at');
			}
		}
		unmount(app);
		expect(noise).toEqual([]);
	});

	it('counts every trail in the hero and the stats', () => {
		const app = mount(HomePage, { target: host });
		flushSync();
		const stages = trackIds.reduce((n, t) => n + catalogs[t].totals.stages, 0);
		const tests = trackIds.reduce((n, t) => n + catalogs[t].totals.tests, 0);
		expect(host.textContent).toContain(`${trackIds.length} trails · ${stages} stages · ${tests} tests`);
		expect(host.textContent).toContain(`across ${trackIds.length} trails`);
		unmount(app);
		expect(noise).toEqual([]);
	});
});

describe('a stage, on a track with ladders and on one without', () => {
	function mountStage(track: (typeof trackIds)[number], number: number) {
		const catalog = catalogs[track];
		const stage = catalog.stages.find((s) => s.number === number)!;
		return mount(StageContent, { target: host, props: { track, stage, catalog, report: null } });
	}

	it('names the rung a dist stage is on, and the tester it belongs to', () => {
		const app = mountStage('dist', 1);
		flushSync();
		expect(host.textContent).toContain('disttest');
		expect(host.textContent).toContain('Primitives');
		expect(host.querySelector('.chip.ladder')?.textContent).toContain('Primitives ladder');
		unmount(app);
		expect(noise).toEqual([]);
	});

	it('does not invent a ladder for a track that has none', () => {
		const app = mountStage('shell', 1);
		flushSync();
		expect(host.querySelector('.chip.ladder')).toBeNull();
		expect(host.textContent).toContain('shelltest');
		unmount(app);
		expect(noise).toEqual([]);
	});

	it('quotes the right target flag and target file per track', () => {
		for (const track of trackIds) {
			const app = mountStage(track, catalogs[track].stages[0].number);
			flushSync();
			expect(host.textContent, track).toContain(tracks[track].targetFlag);
			expect(host.textContent, track).toContain(tracks[track].targetFile);
			unmount(app);
			host.innerHTML = '';
		}
		expect(noise).toEqual([]);
	});

	it('warns that a planned stage has nothing to run yet', () => {
		// Every tester has landed, so no committed catalog is pending any more. The warning
		// still has to work: it is what a fresh clone sees for a track whose crate has not
		// been built yet, so the case is constructed rather than borrowed from whichever
		// track happens to be unfinished today.
		const pending = trackIds[trackIds.length - 1];
		const real = catalogs[pending];
		const catalog = { ...real, pending: true, stages: real.stages.map((s) => ({ ...s })) };
		catalog.stages[0] = { ...catalog.stages[0], planned: true };
		const app = mount(StageContent, {
			target: host,
			props: { track: pending, stage: catalog.stages[0], catalog, report: null }
		});
		flushSync();
		expect(host.textContent).toContain('not yet in the tester');
		expect(host.textContent).toContain('does not know this stage yet');
		// The message names the crate the learner is waiting on, not whichever tester
		// happened to need this wording first.
		expect(host.textContent).toContain(tracks[pending].tester);
		expect(host.textContent).not.toContain('kafkatest/PLAN.md');
		unmount(app);
		expect(noise).toEqual([]);
	});
});
