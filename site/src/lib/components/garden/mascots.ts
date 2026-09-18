/**
 * Mascot name → component. The registry (`src/lib/tracks.ts`) names each track's animal as
 * a string so that it can stay a plain-data file that `npm run sync` can import; this is
 * the one place that turns those names into Svelte components.
 */
import type { Component } from 'svelte';
import Fox from './Fox.svelte';
import Bunny from './Bunny.svelte';
import Owl from './Owl.svelte';
import Hedgehog from './Hedgehog.svelte';
import Squirrel from './Squirrel.svelte';
import Duckling from './Duckling.svelte';
import Cat from './Cat.svelte';
import { tracks, type MascotName, type TrackId } from '$lib/tracks';

/** Every mascot takes the same props, which is what makes them interchangeable. */
export interface MascotProps {
	size?: number;
	mood?: 'idle' | 'walk' | 'hop';
	flip?: boolean;
	label?: string;
}

export type MascotComponent = Component<MascotProps>;

export const mascots: Record<MascotName, MascotComponent> = {
	fox: Fox as MascotComponent,
	bunny: Bunny as MascotComponent,
	owl: Owl as MascotComponent,
	hedgehog: Hedgehog as MascotComponent,
	squirrel: Squirrel as MascotComponent,
	duckling: Duckling as MascotComponent,
	cat: Cat as MascotComponent
};

/** The animal that walks a track's trail. */
export function mascotFor(track: TrackId): MascotComponent {
	return mascots[tracks[track].mascot];
}
