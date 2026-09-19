/**
 * The registry is the thing every page now reads, so it is the thing worth pinning down:
 * one entry per track, an accent that is actually defined and actually readable, a mascot
 * that exists, and section badges that cover the catalog the sync script generated.
 */
import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { allTracks, badgeFor, byTrack, isTrack, ladderOfSection, tracks, trackIds } from './tracks';
import { catalogs, commandFor, totalsAcrossTracks, untilCommand } from './catalog';
import { conventions } from './conventions';
import { resourcesForTrack } from './resources';

const css = readFileSync(fileURLToPath(new URL('../app.css', import.meta.url)), 'utf8');

/** The tokens declared in one CSS block, as `{ '--wasm': '#6845b6' }`. */
function tokensIn(selector: string): Record<string, string> {
	const at = css.indexOf(selector);
	expect(at, `${selector} is in app.css`).toBeGreaterThanOrEqual(0);
	const block = css.slice(at, css.indexOf('\n}', at));
	const out: Record<string, string> = {};
	for (const [, name, value] of block.matchAll(/(--[a-z0-9-]+):\s*([^;]+);/g)) out[name] = value.trim();
	return out;
}

const lightTokens = tokensIn(':root {');
const darkTokens = { ...lightTokens, ...tokensIn(":root[data-theme='dark']") };

function luminance(hex: string): number {
	const parts = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);
	const [r, g, b] = parts.map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
	return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: string, b: string): number {
	const x = luminance(a);
	const y = luminance(b);
	return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}

describe('the track registry', () => {
	it('has one complete entry per id, and no two tracks share an identity', () => {
		expect(allTracks.map((t) => t.id)).toEqual([...trackIds]);
		for (const t of allTracks) {
			expect(t.title, t.id).toMatch(/\S/);
			expect(t.short, t.id).toMatch(/\S/);
			expect(t.blurb.length, t.id).toBeGreaterThan(40);
			expect(t.building, t.id).toMatch(/^an? /);
			expect(t.targetFlag, t.id).toMatch(/^--[a-z]+$/);
			expect(t.reportPath, t.id).toBe(`/__reports/${t.id}.json`);
			expect(t.dir, t.id).toBe(t.tester);
			expect(t.plannedStages, t.id).toBeGreaterThan(20);
			expect(['transcript', 'bytes']).toContain(t.examples);
		}
		const unique = (xs: string[]) => new Set(xs).size === xs.length;
		expect(unique(allTracks.map((t) => t.accent))).toBe(true);
		expect(unique(allTracks.map((t) => t.tester))).toBe(true);
		expect(unique(allTracks.map((t) => t.short))).toBe(true);
		expect(unique(allTracks.map((t) => t.mascot))).toBe(true);
	});

	it('is the only place a track id is spelled out', () => {
		expect(isTrack('wasm')).toBe(true);
		expect(isTrack('dist')).toBe(true);
		expect(isTrack('redis')).toBe(false);
		expect(isTrack('')).toBe(false);
		expect(Object.keys(byTrack(() => 1))).toEqual([...trackIds]);
	});

	it('gives every track an accent that is defined in both themes and AA on both', () => {
		for (const t of allTracks) {
			const name = t.accent.replace(/^var\(|\)$/g, '');
			expect(t.accentBright).toBe(`var(${name}-bright)`);
			expect(t.accentSoft).toBe(`var(${name}-soft)`);
			for (const [theme, tokens] of [
				['light', lightTokens],
				['dusk', darkTokens]
			] as const) {
				const ink = tokens[name];
				expect(ink, `${name} in ${theme}`).toMatch(/^#[0-9a-f]{6}$/i);
				expect(tokens[`${name}-bright`], `${name}-bright in ${theme}`).toMatch(/^#[0-9a-f]{6}$/i);
				expect(tokens[`${name}-soft`], `${name}-soft in ${theme}`).toMatch(/^#[0-9a-f]{6}$/i);
				// Track accents are used as text on the page and the card surfaces behind them.
				for (const surface of ['--bg', '--bg-2']) {
					expect(
						contrast(ink, tokens[surface]),
						`${name} on ${surface} in ${theme}`
					).toBeGreaterThanOrEqual(4.5);
				}
			}
		}
	});

	it('names every section of every generated catalog', () => {
		for (const t of allTracks) {
			for (const section of catalogs[t.id].sections) {
				const badge = badgeFor(t.id, section.id);
				expect(badge.badge, `${t.id} ${section.id}`).not.toMatch(/^Section /);
				expect(badge.icon, `${t.id} ${section.id}`).toMatch(/\S/);
			}
		}
		// An id the registry has never heard of still gets a name rather than undefined.
		expect(badgeFor('shell', 'Z')).toEqual({ badge: 'Section Z', icon: '🌿' });
	});

	it('puts every dist section on exactly one ladder, and no other track on any', () => {
		const dist = tracks.dist;
		expect(dist.ladders?.map((l) => l.id)).toEqual(['primitives', 'algorithms', 'node', 'cluster']);
		for (const section of catalogs.dist.sections) {
			const ladder = ladderOfSection('dist', section.id);
			expect(ladder, `dist section ${section.id}`).not.toBeNull();
			const owners = dist.ladders!.filter((l) => l.sections.includes(section.id));
			expect(owners).toHaveLength(1);
		}
		for (const t of allTracks) {
			if (t.id === 'dist') continue;
			expect(t.ladders, t.id).toBeUndefined();
			expect(ladderOfSection(t.id, 'A')).toBeNull();
		}
	});

	it('every stage of every track carries its ladder when the track has ladders', () => {
		for (const stage of catalogs.dist.stages) {
			expect(stage.ladder, `dist stage ${stage.number}`).toBe(
				ladderOfSection('dist', stage.section)?.id
			);
		}
		for (const stage of catalogs.shell.stages) expect(stage.ladder).toBeUndefined();
	});

	it('builds the same command the tester documents, for every track', () => {
		for (const t of allTracks) {
			expect(commandFor(t.id, 7)).toBe(`${t.binary} ${t.targetFlag} ${t.targetExample} --stage 7`);
			expect(untilCommand(t.id, 7, 'x')).toBe(`${t.binary} ${t.targetFlag} x --until 7 --json report.json`);
		}
	});

	it('has a catalog, a conventions list and a resource file for every track', () => {
		for (const id of trackIds) {
			expect(catalogs[id].track, id).toBe(id);
			expect(catalogs[id].stages.length, id).toBeGreaterThan(0);
			expect(conventions[id].length, id).toBeGreaterThan(0);
			// May legitimately be empty — a track whose reading list is not curated yet.
			expect(Array.isArray(resourcesForTrack(id)), id).toBe(true);
		}
	});

	it('adds up the trails for the home page', () => {
		const totals = totalsAcrossTracks();
		expect(totals.stages).toBe(trackIds.reduce((n, t) => n + catalogs[t].totals.stages, 0));
		expect(totals.tests).toBe(trackIds.reduce((n, t) => n + catalogs[t].totals.tests, 0));
		expect(totals.pending).toBe(trackIds.filter((t) => catalogs[t].pending).length);
	});

	it('gives a pending track an honest catalog rather than an empty one', () => {
		for (const id of trackIds) {
			const catalog = catalogs[id];
			if (!catalog.pending) continue;
			// A placeholder still has to prerender and still has to say what it is.
			expect(catalog.stages.length, id).toBeGreaterThan(0);
			expect(catalog.totals.tests, id).toBe(0);
			expect(catalog.stages.every((s) => s.planned), id).toBe(true);
			expect(catalog.stages.every((s) => s.hints.length > 0), id).toBe(true);
			expect(catalog.source, id).toMatch(/placeholder|PLAN\.md/);
		}
	});
});
