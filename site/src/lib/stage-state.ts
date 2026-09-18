import { stageOutcome } from './report';
import type { Report, StageState, TrackId } from './types';
import type { StageProgress } from './stores/progress.svelte';

export interface StageStateInput {
	track: TrackId;
	stage: number;
	report: Report | null;
	progress: StageProgress;
	nextStage: number;
}

/**
 * One stage's state on the map. A red test always wins — a stage you ticked off but
 * that the tester now fails should shout, not sit there looking finished.
 *
 * When the byo database is the source it has already done this reasoning across the whole
 * run (`done` / `in_progress` / `failed`), so its verdict is taken as given; the report
 * path below is the localStorage fallback.
 */
export function stageState({ stage, report, progress, nextStage }: StageStateInput): StageState {
	if (progress.dbState === 'failed') return 'failing';
	if (progress.dbState === 'done') return 'done';
	if (progress.dbState === 'in_progress') return 'in-progress';
	const outcome = stageOutcome(report, stage);
	if (outcome === 'red' || outcome === 'partial') return 'failing';
	if (progress.done) return 'done';
	if (outcome === 'green') return 'in-progress';
	if (stage === nextStage) return 'next';
	if (progress.startedAt || progress.notes) return 'in-progress';
	return stage < nextStage ? 'available' : 'locked';
}

export const stateLabel: Record<StageState, string> = {
	locked: 'not started',
	available: 'available',
	next: 'up next',
	'in-progress': 'in progress',
	done: 'done',
	failing: 'failing'
};

export const stateGlyph: Record<StageState, string> = {
	locked: '',
	available: '',
	next: '◆',
	'in-progress': '◐',
	done: '✓',
	failing: '✕'
};
