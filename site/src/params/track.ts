import { isTrack } from '$lib/tracks';
import type { ParamMatcher } from '@sveltejs/kit';

/** `/[track]` matches any id in the registry (`src/lib/tracks.ts`). */
export const match: ParamMatcher = (param) => isTrack(param);
