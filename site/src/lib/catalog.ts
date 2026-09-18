import shellCatalog from './data/catalog.shell.json';
import kafkaCatalog from './data/catalog.kafka.json';
import type { Catalog, StageSpec, TrackId } from './types';

export const catalogs: Record<TrackId, Catalog> = {
	shell: shellCatalog as unknown as Catalog,
	kafka: kafkaCatalog as unknown as Catalog
};

export interface TrackMeta {
	id: TrackId;
	title: string;
	tagline: string;
	blurb: string;
	glyph: string;
	tester: string;
	binary: string;
	targetFlag: string;
	targetExample: string;
	reportPath: string;
}

export const tracks: Record<TrackId, TrackMeta> = {
	shell: {
		id: 'shell',
		title: 'Build your own Shell',
		tagline: 'From a bare prompt to job control',
		blurb:
			'A POSIX shell, written by you, driven as a black box over pipes and a real pseudo-terminal. 57 stages from printing `$ ` to pipelines, history and line editing.',
		glyph: '❯',
		tester: 'shelltest',
		binary: './target/release/shelltest',
		targetFlag: '--shell',
		targetExample: './my_shell',
		reportPath: '/__reports/shell.json'
	},
	kafka: {
		id: 'kafka',
		title: 'Build your own Kafka',
		tagline: 'From a TCP accept to consumer groups',
		blurb:
			'A Kafka broker speaking the real wire protocol, checked against Apache Kafka 4.1. 45 stages from echoing a correlation id to idempotent produce and group rebalances.',
		glyph: '⟐',
		tester: 'kafkatest',
		binary: './target/release/kafkatest',
		targetFlag: '--broker',
		targetExample: 'my_broker',
		reportPath: '/__reports/kafka.json'
	}
};

export const trackIds: TrackId[] = ['shell', 'kafka'];

export function isTrack(value: string): value is TrackId {
	return value === 'shell' || value === 'kafka';
}

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

/**
 * Section names — the garden motif: every section of the trail is a meadow or a glade, and
 * the stages in it are the flowers you make bloom on the way through.
 */
export const sectionBadges: Record<TrackId, Record<string, { badge: string; icon: string }>> = {
	shell: {
		A: { badge: 'The Seedbed', icon: '🌱' },
		B: { badge: 'Quote Hollow', icon: '🌾' },
		C: { badge: 'Redirect Brook', icon: '💧' },
		D: { badge: 'Completion Grove', icon: '🌼' },
		E: { badge: 'Pipeline Glade', icon: '🍃' },
		F: { badge: 'Memory Meadow', icon: '🌸' },
		G: { badge: 'The Orchard', icon: '🌳' }
	},
	kafka: {
		A: { badge: 'Sprout Field', icon: '🌱' },
		B: { badge: 'Signpost Meadow', icon: '🌷' },
		C: { badge: 'Fetch Falls', icon: '💧' },
		D: { badge: 'Pollen Plateau', icon: '🌻' },
		E: { badge: 'Hive Hollow', icon: '🐝' },
		F: { badge: 'The Orchard', icon: '🌳' }
	}
};

export function badgeFor(track: TrackId, sectionId: string) {
	return sectionBadges[track][sectionId] ?? { badge: `Section ${sectionId}`, icon: '🌿' };
}
