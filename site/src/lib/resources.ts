import shellResources from './data/resources.shell.json';
import kafkaResources from './data/resources.kafka.json';
import type { Resource, TrackId } from './types';

/**
 * R1 drops verified files at these exact paths; the loader stays tolerant so a
 * replacement with more (or differently shaped) entries cannot break a page.
 */
/**
 * Concept tags are a filter menu, so they have to read as concepts. A bare number
 * ("126", "127") is a fact about a resource, not a heading anyone can pick out of a
 * dropdown — drop those, along with blanks and duplicates.
 */
function cleanConcepts(raw: unknown[]): string[] {
	const out: string[] = [];
	for (const value of raw) {
		const tag = String(value).trim();
		if (!tag) continue;
		if (/^[0-9]+$/.test(tag)) continue;
		if (!out.includes(tag)) out.push(tag);
	}
	return out;
}

function sanitize(raw: unknown, fallbackTrack: TrackId): Resource[] {
	if (!Array.isArray(raw)) return [];
	const out: Resource[] = [];
	for (const item of raw) {
		if (!item || typeof item !== 'object') continue;
		const r = item as Partial<Resource>;
		if (!r.id || !r.title || !r.url) continue;
		out.push({
			id: String(r.id),
			title: String(r.title),
			url: String(r.url),
			type: (r.type ?? 'doc') as Resource['type'],
			level: (r.level ?? 'core') as Resource['level'],
			track: (r.track ?? fallbackTrack) as Resource['track'],
			stages: Array.isArray(r.stages) ? r.stages.map(Number).filter(Number.isFinite) : [],
			concepts: Array.isArray(r.concepts) ? cleanConcepts(r.concepts) : [],
			why: String(r.why ?? ''),
			minutes: typeof r.minutes === 'number' ? r.minutes : undefined,
			free: r.free !== false,
			checkedAt: r.checkedAt ? String(r.checkedAt) : undefined
		});
	}
	return out;
}

const byTrack: Record<TrackId, Resource[]> = {
	shell: sanitize(shellResources, 'shell'),
	kafka: sanitize(kafkaResources, 'kafka')
};

const LEVEL_RANK: Record<Resource['level'], number> = { intro: 0, core: 1, deep: 2 };

/** The lowest stage a resource is attached to, or Infinity when it is background reading. */
export function firstStage(r: Resource): number {
	return r.stages.length ? Math.min(...r.stages) : Number.POSITIVE_INFINITY;
}

/**
 * Journey order: the library should read in the order you will need it. First stage first,
 * then intro before core before deep within the same stage, then alphabetically so the
 * ordering is total and stable.
 */
export function compareResources(a: Resource, b: Resource): number {
	const sa = firstStage(a);
	const sb = firstStage(b);
	if (sa !== sb) return sa - sb;
	const la = LEVEL_RANK[a.level] ?? 1;
	const lb = LEVEL_RANK[b.level] ?? 1;
	if (la !== lb) return la - lb;
	return a.title.localeCompare(b.title);
}

export function allResources(): Resource[] {
	const seen = new Set<string>();
	return [...byTrack.shell, ...byTrack.kafka]
		.filter((r) => {
			if (seen.has(r.id)) return false;
			seen.add(r.id);
			return true;
		})
		.sort(compareResources);
}

export function resourcesForTrack(track: TrackId): Resource[] {
	return allResources().filter((r) => r.track === track || r.track === 'both');
}

export function resourcesForStage(track: TrackId, stage: number): Resource[] {
	return resourcesForTrack(track)
		.filter((r) => r.stages.includes(stage))
		.sort((a, b) => (LEVEL_RANK[a.level] ?? 1) - (LEVEL_RANK[b.level] ?? 1) || a.title.localeCompare(b.title));
}

export function allConcepts(): string[] {
	return [...new Set(allResources().flatMap((r) => r.concepts))].sort();
}

export const typeLabels: Record<Resource['type'], string> = {
	spec: 'spec',
	doc: 'docs',
	book: 'book',
	article: 'article',
	video: 'video',
	course: 'course',
	repo: 'code',
	tool: 'tool',
	paper: 'paper'
};

export const typeGlyphs: Record<Resource['type'], string> = {
	spec: '§',
	doc: '📄',
	book: '📕',
	article: '✎',
	video: '▶',
	course: '◈',
	repo: '{ }',
	tool: '⚒',
	paper: '🗎'
};
