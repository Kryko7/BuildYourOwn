/**
 * The `byo site` JSON API (byo/API.md).
 *
 * Everything here is a plain function over an injectable `fetch`, so the store and the
 * tests can drive it without a server. Nothing throws for an expected miss: a 404 on
 * `/api/runs/latest` is "no runs yet", not an error.
 */
import type { Report, TrackId } from '../types';
import { parseReport } from '../report';

export interface ApiProject {
	id: number;
	track: TrackId;
	path: string;
	command: string;
	targetKind?: string;
	createdAt?: string;
}

export interface ApiHealth {
	ok: boolean;
	version: string;
	db: string;
	dataDir?: string;
	schemaVersion?: number;
	tracks: Record<TrackId, { project: ApiProject | null }>;
}

export type ApiStageState = 'todo' | 'in_progress' | 'failed' | 'done';

export interface ApiStageRow {
	state: ApiStageState;
	doneAt: string | null;
	note: string | null;
	lastRunId: number | null;
	updatedAt?: string | null;
}

export interface ApiTrackProgress {
	latestRunId: number | null;
	stages: Record<string, ApiStageRow>;
}

export interface ApiProgress {
	shell: ApiTrackProgress;
	kafka: ApiTrackProgress;
	streak: number;
	xp: number;
	level: number;
}

export interface ApiRunSummary {
	id: number;
	track: TrackId;
	target: string;
	startedAt: string;
	elapsedMs: number;
	passed: number;
	failed: number;
	skipped: number;
	args: string;
}

export type Fetcher = typeof fetch;

export class ApiError extends Error {
	constructor(
		message: string,
		readonly status: number
	) {
		super(message);
		this.name = 'ApiError';
	}
}

const STATES: ApiStageState[] = ['todo', 'in_progress', 'failed', 'done'];

function isTrackId(v: unknown): v is TrackId {
	return v === 'shell' || v === 'kafka';
}

function num(v: unknown, fallback = 0): number {
	const n = Number(v);
	return Number.isFinite(n) ? n : fallback;
}

function str(v: unknown, fallback = ''): string {
	return typeof v === 'string' ? v : fallback;
}

/** Tolerant read of one `stage_progress` row: an unknown state degrades to `todo`. */
export function normalizeStageRow(raw: unknown): ApiStageRow {
	const o = (raw ?? {}) as Record<string, unknown>;
	const state = STATES.includes(o.state as ApiStageState) ? (o.state as ApiStageState) : 'todo';
	return {
		state,
		doneAt: typeof o.doneAt === 'string' ? o.doneAt : null,
		note: typeof o.note === 'string' && o.note !== '' ? o.note : null,
		lastRunId: typeof o.lastRunId === 'number' ? o.lastRunId : null,
		updatedAt: typeof o.updatedAt === 'string' ? o.updatedAt : null
	};
}

function normalizeTrackProgress(raw: unknown): ApiTrackProgress {
	const o = (raw ?? {}) as Record<string, unknown>;
	const stages: Record<string, ApiStageRow> = {};
	const src = (o.stages ?? {}) as Record<string, unknown>;
	if (src && typeof src === 'object') {
		for (const [key, value] of Object.entries(src)) {
			const n = Number(key);
			if (!Number.isFinite(n)) continue;
			stages[String(n)] = normalizeStageRow(value);
		}
	}
	return {
		latestRunId: typeof o.latestRunId === 'number' ? o.latestRunId : null,
		stages
	};
}

export function normalizeProgress(raw: unknown): ApiProgress {
	const o = (raw ?? {}) as Record<string, unknown>;
	return {
		shell: normalizeTrackProgress(o.shell),
		kafka: normalizeTrackProgress(o.kafka),
		streak: num(o.streak),
		xp: num(o.xp),
		level: Math.max(1, num(o.level, 1))
	};
}

function normalizeRunSummary(raw: unknown): ApiRunSummary | null {
	const o = (raw ?? {}) as Record<string, unknown>;
	if (!isTrackId(o.track)) return null;
	const id = Number(o.id);
	if (!Number.isFinite(id)) return null;
	return {
		id,
		track: o.track,
		target: str(o.target, 'unknown'),
		startedAt: str(o.startedAt),
		elapsedMs: num(o.elapsedMs),
		passed: num(o.passed),
		failed: num(o.failed),
		skipped: num(o.skipped),
		args: str(o.args)
	};
}

/** The API client, bound to one `fetch`. `base` exists so tests can use absolute URLs. */
export class ByoApi {
	constructor(
		private readonly fetcher: Fetcher = fetch,
		private readonly base = ''
	) {}

	private url(path: string) {
		return `${this.base}${path}`;
	}

	private async json(path: string, init?: RequestInit): Promise<unknown> {
		const res = await this.fetcher(this.url(path), { cache: 'no-store', ...init });
		if (!res.ok) {
			let message = `${res.status} from ${path}`;
			try {
				const body = (await res.json()) as { error?: string };
				if (body?.error) message = body.error;
			} catch {
				/* not JSON — the status line is all we have */
			}
			throw new ApiError(message, res.status);
		}
		return res.json();
	}

	/** `true` when `byo site` is on the other end. Never throws. */
	async health(): Promise<ApiHealth | null> {
		try {
			const raw = (await this.json('/api/health')) as Record<string, unknown>;
			if (!raw || raw.ok !== true) return null;
			const tracksRaw = (raw.tracks ?? {}) as Record<string, { project?: ApiProject | null }>;
			return {
				ok: true,
				version: str(raw.version, '?'),
				db: str(raw.db),
				dataDir: str(raw.dataDir),
				schemaVersion: num(raw.schemaVersion, 1),
				tracks: {
					shell: { project: tracksRaw.shell?.project ?? null },
					kafka: { project: tracksRaw.kafka?.project ?? null }
				}
			};
		} catch {
			return null;
		}
	}

	async progress(): Promise<ApiProgress> {
		return normalizeProgress(await this.json('/api/progress'));
	}

	async runs(track?: TrackId, limit = 20): Promise<ApiRunSummary[]> {
		const params = new URLSearchParams();
		if (track) params.set('track', track);
		params.set('limit', String(limit));
		const raw = await this.json(`/api/runs?${params.toString()}`);
		if (!Array.isArray(raw)) return [];
		return raw.map(normalizeRunSummary).filter((r): r is ApiRunSummary => r !== null);
	}

	/** One run, already in the site's `Report` shape. */
	async run(id: number, track?: TrackId): Promise<Report> {
		const raw = await this.json(`/api/runs/${id}`);
		return parseReport(raw, { track, origin: 'api' });
	}

	/** The newest run for a track, or `null` when the track has never been run (404). */
	async latest(track: TrackId): Promise<Report | null> {
		try {
			const raw = await this.json(`/api/runs/latest?track=${track}`);
			return parseReport(raw, { track, origin: 'api' });
		} catch (e) {
			if (e instanceof ApiError && e.status === 404) return null;
			throw e;
		}
	}

	async setStage(
		track: TrackId,
		stage: number,
		body: { state?: 'done' | 'todo'; note?: string }
	): Promise<ApiStageRow & { track: TrackId; stage: number }> {
		const raw = (await this.json(`/api/stages/${track}/${stage}`, {
			method: 'POST',
			headers: { 'content-type': 'application/json' },
			body: JSON.stringify(body)
		})) as Record<string, unknown>;
		return { ...normalizeStageRow(raw), track, stage };
	}

	/** The catalog the installed testers know about; `null` when the data dir has none. */
	async catalog(track: TrackId): Promise<unknown | null> {
		try {
			return await this.json(`/api/catalog/${track}`);
		} catch (e) {
			if (e instanceof ApiError && e.status === 404) return null;
			throw e;
		}
	}
}

export const api = new ByoApi();
