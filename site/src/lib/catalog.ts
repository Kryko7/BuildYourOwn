/**
 * The generated catalogs, keyed by track.
 *
 * The track *registry* — titles, accents, mascots, tester names — lives in `tracks.ts`;
 * this module is only about the stage data `npm run sync` writes, plus the handful of
 * helpers that combine the two (the command to run, the garden name of a section).
 */
import shellCatalog from './data/catalog.shell.json';
import kafkaCatalog from './data/catalog.kafka.json';
import wasmCatalog from './data/catalog.wasm.json';
import tlsCatalog from './data/catalog.tls.json';
import linkCatalog from './data/catalog.link.json';
import distCatalog from './data/catalog.dist.json';
import { tracks, trackIds } from './tracks';
import type { Catalog, StageSpec, TrackId } from './types';

export { tracks, trackIds, allTracks, isTrack, byTrack, badgeFor, ladderOfSection } from './tracks';
export type { TrackMeta, TrackId, MascotName, ExampleKind, Ladder } from './tracks';

export const catalogs: Record<TrackId, Catalog> = {
	shell: shellCatalog as unknown as Catalog,
	kafka: kafkaCatalog as unknown as Catalog,
	wasm: wasmCatalog as unknown as Catalog,
	tls: tlsCatalog as unknown as Catalog,
	link: linkCatalog as unknown as Catalog,
	dist: distCatalog as unknown as Catalog
};

export function getCatalog(track: TrackId): Catalog {
	return catalogs[track];
}

export function allStages(track: TrackId): StageSpec[] {
	return catalogs[track].stages;
}

export function stageOf(track: TrackId, n: number): StageSpec | undefined {
	return catalogs[track].stages.find((s) => s.number === n);
}

export function sectionOf(track: TrackId, n: number) {
	return catalogs[track].sections.find((s) => s.stages.includes(n));
}

export function neighbours(track: TrackId, n: number) {
	const stages = catalogs[track].stages;
	const i = stages.findIndex((s) => s.number === n);
	return {
		prev: i > 0 ? stages[i - 1] : undefined,
		next: i >= 0 && i < stages.length - 1 ? stages[i + 1] : undefined
	};
}

/** The exact command the user should run for one stage. */
export function commandFor(track: TrackId, n: number, target?: string): string {
	const t = tracks[track];
	return `${t.binary} ${t.targetFlag} ${target ?? t.targetExample} --stage ${n}`;
}

export function untilCommand(track: TrackId, n: number, target?: string): string {
	const t = tracks[track];
	return `${t.binary} ${t.targetFlag} ${target ?? t.targetExample} --until ${n} --json report.json`;
}

/** Totals across every track — the numbers the home page hero quotes. */
export function totalsAcrossTracks() {
	return trackIds.reduce(
		(acc, id) => {
			const c = catalogs[id];
			acc.stages += c.totals.stages;
			acc.tests += c.totals.tests;
			acc.ext += c.totals.ext;
			if (c.pending) acc.pending++;
			return acc;
		},
		{ stages: 0, tests: 0, ext: 0, pending: 0 }
	);
}
