/** The two example generators behind "What to build" (PLAN.md §4.2). */
import { shellExamples } from './shell';
import { kafkaExampleCount } from './kafka';
import type { StageSpec, TrackId } from '../types';

export { shellExamples } from './shell';
export {
	kafkaExamples,
	kafkaExampleCount,
	loadKafkaExamples,
	normalizeKafkaExample,
	normalizeKafkaExamples,
	readHex,
	hasBytes
} from './kafka';

/**
 * How many examples a stage can show — used to decide whether to render the section at all.
 * Synchronous on both tracks: the shell derives them from the catalog it already has, and
 * the Kafka catalog carries the count even though the examples themselves are a lazy chunk.
 */
export function exampleCount(track: TrackId, stage: StageSpec | null | undefined): number {
	if (!stage) return 0;
	return track === 'shell' ? shellExamples(stage).length : kafkaExampleCount(stage);
}
