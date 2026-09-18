/**
 * The connection to `byo site` — and, when it answers, the single source of truth for
 * progress (PLAN.md §4.5).
 *
 * On load the site probes `GET /api/health`. If the byo server is there, every stage
 * state, note, XP number and run comes from the SQLite database behind it and every
 * "mark done" is a `POST /api/stages/:track/:n`. If it is not there (static hosting,
 * `npm run dev`), the store stays `offline` and the localStorage stores take over as a
 * clearly-labelled fallback.
 *
 * While the tab is visible the store polls every two seconds, so a `byo test` running in
 * another terminal turns the map green without a reload.
 */
import { browser } from '$app/environment';
import { ByoApi, type ApiHealth, type ApiProgress, type ApiRunSummary, type ApiStageRow } from '../api/client';
import { trackIds } from '../catalog';
import type { Report, TrackId } from '../types';

export const POLL_MS = 2000;

export type ConnectionStatus = 'probing' | 'connected' | 'reconnecting' | 'offline';

const emptyProgress = (): ApiProgress => ({
	shell: { latestRunId: null, stages: {} },
	kafka: { latestRunId: null, stages: {} },
	streak: 0,
	xp: 0,
	level: 1
});

export class JourneyStore {
	status = $state<ConnectionStatus>('probing');
	health = $state<ApiHealth | null>(null);
	data = $state<ApiProgress>(emptyProgress());
	runs = $state<ApiRunSummary[]>([]);
	latest = $state<Record<TrackId, Report | null>>({ shell: null, kafka: null });
	lastSyncAt = $state<string | null>(null);
	/** Bumped on every successful poll that changed something — a cheap "did it move" signal. */
	revision = $state(0);

	#api: ByoApi;
	#timer: ReturnType<typeof setInterval> | null = null;
	#inFlight = false;
	#latestIds: Record<TrackId, number | null> = { shell: null, kafka: null };
	#runsSignature = '';

	constructor(api: ByoApi = new ByoApi()) {
		this.#api = api;
	}

	/** True whenever the database is (or very recently was) answering. */
	get connected() {
		return this.status === 'connected' || this.status === 'reconnecting';
	}

	/** True once we know for sure there is no byo server (so the fallback banner can show). */
	get offline() {
		return this.status === 'offline';
	}

	project(track: TrackId) {
		return this.health?.tracks?.[track]?.project ?? null;
	}

	/** True when byo is running but `byo init <track>` has never been run here. */
	needsInit(track: TrackId) {
		return this.connected && this.project(track) === null;
	}

	stageRow(track: TrackId, stage: number): ApiStageRow | null {
		return this.data[track]?.stages?.[String(stage)] ?? null;
	}

	/** Begin probing and polling. Returns the teardown for `onMount`. */
	start(): () => void {
		if (!browser) return () => {};
		void this.refresh();
		const onVisibility = () => {
			if (document.visibilityState === 'visible') void this.refresh();
		};
		document.addEventListener('visibilitychange', onVisibility);
		this.#timer = setInterval(() => {
			// Throttled when the tab is hidden: nobody is watching the map go green.
			if (document.visibilityState !== 'visible') return;
			void this.refresh();
		}, POLL_MS);
		return () => {
			document.removeEventListener('visibilitychange', onVisibility);
			this.stop();
		};
	}

	stop() {
		if (this.#timer) clearInterval(this.#timer);
		this.#timer = null;
	}

	/** One poll: health when we have none, then progress, the run list and any new run. */
	async refresh(): Promise<void> {
		if (this.#inFlight) return;
		this.#inFlight = true;
		try {
			// Health is re-probed while anything about it could still change: before the first
			// success, after a drop, and while a track has no project (so the "run `byo init`"
			// note disappears on its own once you do).
			const stale =
				!this.health ||
				this.status !== 'connected' ||
				!this.health.tracks.shell.project ||
				!this.health.tracks.kafka.project;
			if (stale) {
				const health = await this.#api.health();
				if (!health) {
					// A server that was there and is now gone keeps its data on screen.
					this.status = this.health ? 'reconnecting' : 'offline';
					return;
				}
				this.health = health;
			}
			const progress = await this.#api.progress();
			this.data = progress;
			this.status = 'connected';
			this.lastSyncAt = new Date().toISOString();

			let changed = false;
			for (const track of trackIds) {
				const id = progress[track]?.latestRunId ?? null;
				if (id === this.#latestIds[track]) continue;
				this.#latestIds[track] = id;
				changed = true;
				this.latest[track] = id === null ? null : await this.#api.latest(track);
			}
			if (changed || this.runs.length === 0) {
				const runs = await this.#api.runs(undefined, 25);
				const signature = runs.map((r) => `${r.id}:${r.passed}/${r.failed}`).join(',');
				if (signature !== this.#runsSignature) {
					this.#runsSignature = signature;
					this.runs = runs;
					changed = true;
				}
			}
			if (changed) this.revision++;
		} catch {
			// A transient error mid-session is "reconnecting", not "there is no byo".
			this.status = this.health ? 'reconnecting' : 'offline';
		} finally {
			this.#inFlight = false;
		}
	}

	/** Fetch one run in full (for the run history table's expanders). */
	run(id: number, track?: TrackId): Promise<Report> {
		return this.#api.run(id, track);
	}

	/**
	 * Write a stage through the API, optimistically. Returns false when the write failed,
	 * which the caller shows as "could not reach byo" rather than pretending it worked.
	 */
	async setStage(
		track: TrackId,
		stage: number,
		body: { state?: 'done' | 'todo'; note?: string }
	): Promise<boolean> {
		const key = String(stage);
		const before = this.data[track].stages[key];
		const optimistic: ApiStageRow = {
			state: body.state ? body.state : (before?.state ?? 'todo'),
			doneAt: body.state === 'done' ? new Date().toISOString() : body.state === 'todo' ? null : (before?.doneAt ?? null),
			note: body.note === undefined ? (before?.note ?? null) : body.note === '' ? null : body.note,
			lastRunId: before?.lastRunId ?? null,
			updatedAt: new Date().toISOString()
		};
		this.data[track].stages[key] = optimistic;
		try {
			const row = await this.#api.setStage(track, stage, body);
			this.data[track].stages[key] = row;
			this.status = 'connected';
			return true;
		} catch {
			if (before) this.data[track].stages[key] = before;
			else delete this.data[track].stages[key];
			this.status = this.health ? 'reconnecting' : 'offline';
			return false;
		}
	}
}

export const journey = new JourneyStore();
