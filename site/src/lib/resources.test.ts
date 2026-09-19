import { describe, it, expect } from 'vitest';
import {
	allConcepts,
	allResources,
	compareResources,
	firstStage,
	resourcesForStage,
	resourcesForTrack
} from './resources';
import { getCatalog, trackIds } from './catalog';
import type { Resource } from './types';

function res(partial: Partial<Resource> & { id: string }): Resource {
	return {
		title: partial.id,
		url: `https://example.invalid/${partial.id}`,
		type: 'doc',
		level: 'core',
		track: 'shell',
		stages: [],
		concepts: [],
		why: '',
		free: true,
		...partial
	};
}

describe('resource ordering', () => {
	it('sorts by the first stage that needs the resource', () => {
		const list = [
			res({ id: 'late', stages: [40, 41] }),
			res({ id: 'early', stages: [3, 30] }),
			res({ id: 'middle', stages: [12] })
		];
		expect([...list].sort(compareResources).map((r) => r.id)).toEqual(['early', 'middle', 'late']);
	});

	it('breaks a tie on stage with intro before core before deep', () => {
		const list = [
			res({ id: 'deep', stages: [5], level: 'deep' }),
			res({ id: 'core', stages: [5], level: 'core' }),
			res({ id: 'intro', stages: [5], level: 'intro' })
		];
		expect([...list].sort(compareResources).map((r) => r.id)).toEqual(['intro', 'core', 'deep']);
	});

	it('breaks a full tie alphabetically so the order is stable', () => {
		const list = [
			res({ id: 'b', title: 'Beta', stages: [5], level: 'core' }),
			res({ id: 'a', title: 'Alpha', stages: [5], level: 'core' })
		];
		expect([...list].sort(compareResources).map((r) => r.id)).toEqual(['a', 'b']);
	});

	it('puts stage-less background reading last, not first', () => {
		const list = [res({ id: 'background' }), res({ id: 'stage-57', stages: [57] })];
		expect([...list].sort(compareResources).map((r) => r.id)).toEqual(['stage-57', 'background']);
		expect(firstStage(list[0])).toBe(Number.POSITIVE_INFINITY);
	});

	it('reads the real library in journey order, not the alphabet', () => {
		const shell = resourcesForTrack('shell');
		const firsts = shell.map(firstStage);
		expect(firsts).toEqual([...firsts].sort((a, b) => a - b));
		// the regression the walkthrough found: "Bash Startup Files" used to lead the track
		const startup = shell.findIndex((r) => /startup files/i.test(r.title));
		const first = shell[0];
		expect(startup).toBeGreaterThan(0);
		expect(firstStage(first)).toBeLessThanOrEqual(firstStage(shell[startup]));
	});

	it('orders a stage’s own reading list intro first', () => {
		const rank = { intro: 0, core: 1, deep: 2 };
		for (const track of ['shell', 'kafka'] as const) {
			for (const stage of getCatalog(track).stages) {
				const list = resourcesForStage(track, stage.number).map((r) => rank[r.level]);
				expect(list).toEqual([...list].sort((a, b) => a - b));
			}
		}
	});
});

describe('the concept filter', () => {
	it('has no bare numbers in it', () => {
		const numeric = allConcepts().filter((c) => /^[0-9]+$/.test(c));
		expect(numeric).toEqual([]);
	});

	it('kept the meaning of the numbers it dropped', () => {
		expect(allConcepts()).toContain('exit-status-126-127');
	});

	it('is a sorted list of unique, non-empty tags', () => {
		const concepts = allConcepts();
		expect(concepts).toEqual([...concepts].sort());
		expect(new Set(concepts).size).toBe(concepts.length);
		expect(concepts.every((c) => c.trim().length > 0)).toBe(true);
	});
});

describe('the real resource library', () => {
	const all = allResources();

	it('has at least 40 entries per track, as the plan asks', () => {
		// Every track, not just the two that had a library first: an empty reading list is
		// the failure this catches, and four tracks shipped with one.
		for (const track of trackIds) {
			const n = resourcesForTrack(track).length;
			expect({ track, enough: n >= 40 }).toEqual({ track, enough: true });
		}
	});

	it('gives every stage of every track something to read', () => {
		for (const track of trackIds) {
			for (const stage of getCatalog(track).stages) {
				const list = resourcesForStage(track, stage.number);
				expect({ track, stage: stage.number, some: list.length > 0 }).toEqual({
					track,
					stage: stage.number,
					some: true
				});
			}
		}
	});

	it('only cites stages the site can actually open', () => {
		for (const r of all) {
			const tracks = r.track === 'both' ? (['shell', 'kafka'] as const) : ([r.track] as const);
			for (const stage of r.stages) {
				const known = tracks.some((t) =>
					getCatalog(t).stages.some((s) => s.number === stage)
				);
				expect({ id: r.id, stage, known }).toEqual({ id: r.id, stage, known: true });
			}
		}
	});

	it('has a unique id and a "why read it" line for every entry', () => {
		expect(new Set(all.map((r) => r.id)).size).toBe(all.length);
		expect(all.every((r) => r.why.length > 0)).toBe(true);
		expect(all.every((r) => /^https?:\/\//.test(r.url))).toBe(true);
	});
});
