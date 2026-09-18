import { error } from '@sveltejs/kit';
import { getCatalog, isTrack } from '$lib/catalog';
import type { PageLoad } from './$types';

export const prerender = true;

export function entries() {
	return [{ track: 'shell' }, { track: 'kafka' }];
}

export const load: PageLoad = ({ params }) => {
	if (!isTrack(params.track)) error(404, 'Unknown track');
	return { track: params.track, catalog: getCatalog(params.track) };
};
