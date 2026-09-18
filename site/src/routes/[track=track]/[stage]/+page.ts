import { error } from '@sveltejs/kit';
import { catalogs, getCatalog, isTrack, neighbours, trackIds, tracks } from '$lib/catalog';
import { loadExampleData } from '$lib/examples/bytes';
import type { PageLoad } from './$types';

export const prerender = true;

export function entries() {
	return trackIds.flatMap((track) =>
		catalogs[track].stages.map((s) => ({ track, stage: String(s.number) }))
	);
}

export const load: PageLoad = async ({ params }) => {
	if (!isTrack(params.track)) error(404, 'Unknown track');
	const number = Number(params.stage);
	const catalog = getCatalog(params.track);
	const stage = catalog.stages.find((s) => s.number === number);
	if (!stage) error(404, `No stage ${params.stage} on the ${params.track} track`);
	// Byte examples live in a chunk of their own (one per stage). Pulling this stage's in
	// here means the deep-linked page renders them server-side — no flash, no layout shift —
	// while every other page still pays nothing for them.
	const examples =
		tracks[params.track].examples === 'bytes' ? await loadExampleData(params.track, number) : null;
	return { track: params.track, catalog, stage, examples, near: neighbours(params.track, number) };
};
