/** The example generators behind "What to build" (PLAN.md §4.2, §5.2). */
import { shellExamples } from './shell';
import { byteExampleCount } from './bytes';
import { tracks } from '../tracks';
import type { StageSpec, TrackId } from '../types';

export { shellExamples } from './shell';
export {
	normalizeByteExample,
	normalizeByteExamples,
	inlineExamples,
	loadExamples,
	loadExampleData,
	byteExampleCount,
	blockOf,
	hasBytes,
	labelForBlock,
	readHex
} from './bytes';

/**
 * How many examples a stage can show — used to decide whether to render the section at all.
 * Synchronous on every track: the shell derives them from the catalog it already has, and
 * the byte tracks carry the count even though the examples themselves are a lazy chunk.
 */
export function exampleCount(track: TrackId, stage: StageSpec | null | undefined): number {
	if (!stage) return 0;
	return tracks[track].examples === 'transcript' ? shellExamples(stage).length : byteExampleCount(stage);
}
