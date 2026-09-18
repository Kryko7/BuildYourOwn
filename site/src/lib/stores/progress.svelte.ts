/**
 * Progress, with two sources and one shape.
 *
 * When `byo site` is serving the page, every read comes from the SQLite database through
 * `journey` and every write is a `POST /api/stages/:track/:n` (PLAN.md §4.5). When it is
 * not — static hosting, `npm run dev`, a plain `npm run preview` — the same API falls back
 * to localStorage so the site still works, with the UI saying so.
 *
 * Everything the components call lives here, so no call site has to know which of the two
 * it is talking to.
 */
import { browser } from '$app/environment';
import { load, save, remove } from './persist';
import { journey } from './journey.svelte';
import { allStages } from '../catalog';
import { byTrack, trackIds, tracks } from '../tracks';
import type { TrackId } from '../types';
import type { ApiStageState } from '../api/client';

const KEY = 'byo:progress:v1';

export interface StageProgress {
	done: boolean;
	doneAt?: string;
	startedAt?: string;
	notes?: string;
	/** Present only when the byo database is the source: it knows about failing runs. */
	dbState?: ApiStageState;
	lastRunId?: number | null;
}

export interface ActivityEntry {
	at: string;
	track: TrackId;
	stage: number;
	kind: 'done' | 'reset';
}

export interface ProgressData {
	version: 1;
	stages: Record<string, StageProgress>;
	activity: ActivityEntry[];
	targets: Record<TrackId, string>;
	lastVisited: Record<TrackId, number>;
	settings: { sound: boolean; petals: boolean };
}

function empty(): ProgressData {
	return {
		version: 1,
		stages: {},
		activity: [],
		targets: byTrack((t) => tracks[t].targetExample),
		lastVisited: byTrack(() => 1),
		settings: { sound: false, petals: true }
	};
}

function merge(stored: Partial<ProgressData> | null): ProgressData {
	const base = empty();
	if (!stored || typeof stored !== 'object') return base;
	const settings = (stored.settings ?? {}) as Record<string, unknown>;
	return {
		version: 1,
		stages: { ...base.stages, ...(stored.stages ?? {}) },
		activity: Array.isArray(stored.activity) ? stored.activity.slice(-200) : [],
		targets: { ...base.targets, ...(stored.targets ?? {}) },
		lastVisited: { ...base.lastVisited, ...(stored.lastVisited ?? {}) },
		settings: {
			...base.settings,
			...settings,
			// v1 of this file called the celebration "confetti"; it is petals now.
			petals:
				typeof settings.petals === 'boolean'
					? settings.petals
					: typeof settings.confetti === 'boolean'
						? (settings.confetti as boolean)
						: base.settings.petals
		}
	};
}

const key = (track: TrackId, stage: number) => `${track}:${stage}`;

/** XP is deliberately simple: a stage is 10, an [ext] stage is 15. */
export function xpForStage(track: TrackId, stage: number): number {
	const s = allStages(track).find((x) => x.number === stage);
	return s?.ext ? 15 : 10;
}

const LEVELS = [0, 40, 110, 220, 380, 600, 880, 1220, 1620, 2080, 2600];
export function levelFor(xp: number) {
	let level = 1;
	for (let i = 0; i < LEVELS.length; i++) if (xp >= LEVELS[i]) level = i + 1;
	const floor = LEVELS[level - 1] ?? 0;
	const ceil = LEVELS[level] ?? floor + 600;
	return { level, floor, ceil, into: xp - floor, span: Math.max(1, ceil - floor) };
}

/** The API's own scale: 100 XP per done stage, a level every 500. */
function levelForApi(xp: number, level: number) {
	const floor = (level - 1) * 500;
	const ceil = level * 500;
	return { level, floor, ceil, into: xp - floor, span: 500 };
}

const RANKS = [
	'Seedling',
	'Sprout',
	'Gardener',
	'Bloomer',
	'Pollinator',
	'Orchardist',
	'Beekeeper',
	'Grove Keeper',
	'Meadow Warden',
	'Garden Sage',
	'Head Gardener'
];
export const rankName = (level: number) => RANKS[Math.min(Math.max(level, 1) - 1, RANKS.length - 1)];

class ProgressStore {
	data = $state<ProgressData>(empty());
	#loaded = false;
	#noteTimers = new Map<string, ReturnType<typeof setTimeout>>();

	constructor() {
		if (browser) {
			this.data = merge(load<Partial<ProgressData> | null>(KEY, null));
			this.#loaded = true;
		}
	}

	#flush() {
		if (browser) save(KEY, this.data);
	}

	get loaded() {
		return this.#loaded;
	}

	/** Which store answered: `byo` (the database) or `local` (this browser). */
	get source(): 'byo' | 'local' {
		return journey.connected ? 'byo' : 'local';
	}

	stage(track: TrackId, stage: number): StageProgress {
		if (journey.connected) {
			const row = journey.stageRow(track, stage);
			const local = this.data.stages[key(track, stage)];
			return {
				done: row?.state === 'done',
				doneAt: row?.doneAt ?? undefined,
				startedAt: local?.startedAt,
				notes: row?.note ?? undefined,
				dbState: row?.state ?? 'todo',
				lastRunId: row?.lastRunId ?? null
			};
		}
		return this.data.stages[key(track, stage)] ?? { done: false };
	}

	isDone(track: TrackId, stage: number) {
		return this.stage(track, stage).done;
	}

	/** Returns true when this call newly completed the stage (drives the petal burst). */
	markDone(track: TrackId, stage: number): boolean {
		const was = this.isDone(track, stage);
		if (journey.connected) {
			void journey.setStage(track, stage, { state: 'done' });
			return !was;
		}
		const k = key(track, stage);
		this.data.stages[k] = {
			...(this.data.stages[k] ?? {}),
			done: true,
			doneAt: new Date().toISOString()
		};
		if (!was) {
			this.data.activity = [
				...this.data.activity,
				{ at: new Date().toISOString(), track, stage, kind: 'done' as const }
			].slice(-200);
		}
		this.#flush();
		return !was;
	}

	reset(track: TrackId, stage: number) {
		if (journey.connected) {
			void journey.setStage(track, stage, { state: 'todo' });
			return;
		}
		const k = key(track, stage);
		const prev = this.data.stages[k];
		if (!prev) return;
		this.data.stages[k] = { ...prev, done: false, doneAt: undefined };
		this.data.activity = [
			...this.data.activity,
			{ at: new Date().toISOString(), track, stage, kind: 'reset' as const }
		].slice(-200);
		this.#flush();
	}

	toggle(track: TrackId, stage: number): boolean {
		return this.isDone(track, stage) ? (this.reset(track, stage), false) : this.markDone(track, stage);
	}

	setNotes(track: TrackId, stage: number, notes: string) {
		const k = key(track, stage);
		if (journey.connected) {
			// Typing is not a write per keystroke; the API sees the note when you pause.
			const existing = this.#noteTimers.get(k);
			if (existing) clearTimeout(existing);
			this.#noteTimers.set(
				k,
				setTimeout(() => {
					this.#noteTimers.delete(k);
					void journey.setStage(track, stage, { note: notes });
				}, 450)
			);
			const row = journey.stageRow(track, stage);
			journey.data[track].stages[String(stage)] = {
				state: row?.state ?? 'todo',
				doneAt: row?.doneAt ?? null,
				note: notes === '' ? null : notes,
				lastRunId: row?.lastRunId ?? null,
				updatedAt: new Date().toISOString()
			};
			return;
		}
		this.data.stages[k] = { ...(this.data.stages[k] ?? { done: false }), notes };
		this.#flush();
	}

	/** Local-only bookkeeping: which stage you were last looking at. */
	touch(track: TrackId, stage: number) {
		const k = key(track, stage);
		const prev = this.data.stages[k];
		if (!prev?.startedAt) {
			this.data.stages[k] = {
				...(prev ?? { done: false }),
				startedAt: new Date().toISOString()
			};
		}
		this.data.lastVisited = { ...this.data.lastVisited, [track]: stage };
		this.#flush();
	}

	setTarget(track: TrackId, target: string) {
		this.data.targets = { ...this.data.targets, [track]: target };
		this.#flush();
	}

	setSetting<K extends keyof ProgressData['settings']>(k: K, v: ProgressData['settings'][K]) {
		this.data.settings = { ...this.data.settings, [k]: v };
		this.#flush();
	}

	doneCount(track: TrackId) {
		return allStages(track).filter((s) => this.isDone(track, s.number)).length;
	}

	/** The lowest stage not yet done — where the "continue" CTA points. */
	nextStage(track: TrackId) {
		const stages = allStages(track);
		return (
			stages.find((s) => !this.isDone(track, s.number))?.number ??
			stages[stages.length - 1]?.number ??
			1
		);
	}

	get xp() {
		if (journey.connected) return journey.data.xp;
		let total = 0;
		for (const track of trackIds) {
			for (const s of allStages(track))
				if (this.isDone(track, s.number)) total += xpForStage(track, s.number);
		}
		return total;
	}

	get level() {
		return journey.connected
			? levelForApi(journey.data.xp, journey.data.level)
			: levelFor(this.xp);
	}

	/** Consecutive days (ending today or yesterday) with at least one completion. */
	get streak() {
		if (journey.connected) return journey.data.streak;
		const days = new Set(
			this.data.activity.filter((a) => a.kind === 'done').map((a) => a.at.slice(0, 10))
		);
		if (days.size === 0) return 0;
		const day = (offset: number) => {
			const d = new Date();
			d.setDate(d.getDate() - offset);
			return d.toISOString().slice(0, 10);
		};
		const start = days.has(day(0)) ? 0 : days.has(day(1)) ? 1 : -1;
		if (start < 0) return 0;
		let n = 0;
		while (days.has(day(start + n))) n++;
		return n;
	}

	/** Recent completions: from the database when connected, from this browser otherwise. */
	get recent(): ActivityEntry[] {
		if (journey.connected) {
			const out: ActivityEntry[] = [];
			for (const track of trackIds) {
				for (const [n, row] of Object.entries(journey.data[track].stages)) {
					if (row.state !== 'done' || !row.doneAt) continue;
					out.push({ at: row.doneAt, track, stage: Number(n), kind: 'done' });
				}
			}
			return out.sort((a, b) => b.at.localeCompare(a.at)).slice(0, 8);
		}
		return [...this.data.activity].reverse().slice(0, 8);
	}

	export(): string {
		return JSON.stringify({ kind: 'byo-journey-state', ...this.data }, null, 2);
	}

	import(text: string): { ok: true } | { ok: false; error: string } {
		try {
			const parsed = JSON.parse(text) as Partial<ProgressData>;
			if (!parsed || typeof parsed !== 'object' || !parsed.stages) {
				return { ok: false, error: 'That file has no "stages" object.' };
			}
			this.data = merge(parsed);
			this.#flush();
			return { ok: true };
		} catch (e) {
			return { ok: false, error: e instanceof Error ? e.message : 'Could not read that file.' };
		}
	}

	clear() {
		this.data = empty();
		remove(KEY);
	}
}

export const progress = new ProgressStore();
