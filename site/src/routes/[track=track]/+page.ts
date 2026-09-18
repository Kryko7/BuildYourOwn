import { error } from '@sveltejs/kit';
import { getCatalog, isTrack, trackIds } from '$lib/catalog';
import type { PageLoad } from './$types';

export const prerender = true;

export function entries() {
	return trackIds.map((track) => ({ track }));
}

export const load: PageLoad = ({ params }) => {
	if (!isTrack(params.track)) error(404, 'Unknown track');
	return { track: params.track, catalog: getCatalog(params.track) };
};
