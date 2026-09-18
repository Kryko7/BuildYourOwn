import { browser, dev } from '$app/environment';
import { load, save, remove } from './persist';
import { parseReport } from '../report';
import { tracks, trackIds } from '../catalog';
import { journey } from './journey.svelte';
import type { Report, TrackId } from '../types';

const KEY = 'byo:reports:v1';
const POLL_MS = 2000;

class ReportStore {
	imported = $state<Record<TrackId, Report | null>>({ shell: null, kafka: null });
	live = $state<Record<TrackId, Report | null>>({ shell: null, kafka: null });
	liveAvailable = $state(false);
	lastLiveAt = $state<string | null>(null);

	#timer: ReturnType<typeof setInterval> | null = null;
	#signatures: Record<TrackId, string> = { shell: '', kafka: '' };

	constructor() {
		if (!browser) return;
		const stored = load<Record<string, unknown> | null>(KEY, null);
		if (stored) {
			for (const track of trackIds) {
				const raw = stored[track];
				if (!raw) continue;
				try {
					this.imported[track] = parseReport(raw, { track, origin: 'file' });
				} catch {
					/* stale shape: drop it silently */
				}
			}
		}
	}

	/** The report the map should colour with: a live run wins over an imported file. */
	active(track: TrackId): Report | null {
		return this.live[track] ?? this.imported[track];
	}

	setImported(track: TrackId, report: Report | null) {
		this.imported[track] = report;
		this.#persist();
	}

	clear(track?: TrackId) {
		if (track) this.imported[track] = null;
		else this.imported = { shell: null, kafka: null };
		this.#persist();
	}

	#persist() {
		if (!browser) return;
		const payload: Record<string, unknown> = {};
		for (const track of trackIds) {
			const r = this.imported[track];
			if (r) payload[track] = toRaw(r);
		}
		if (Object.keys(payload).length === 0) remove(KEY);
		else save(KEY, payload);
	}

	/**
	 * Poll the dev server's /__reports/*.json endpoints. No-ops in a static build, and
	 * stands down entirely while `byo site` is answering — the database wins (§4.5).
	 */
	startPolling() {
		if (!browser || !dev || this.#timer) return;
		const tick = async () => {
			if (journey.connected || document.visibilityState !== 'visible') return;
			for (const track of trackIds) {
				try {
					const res = await fetch(tracks[track].reportPath, { cache: 'no-store' });
					if (res.status === 204 || !res.ok) {
						this.live[track] = null;
						continue;
					}
					const text = await res.text();
					const signature = res.headers.get('x-report-mtime') ?? String(text.length);
					this.liveAvailable = true;
					if (signature === this.#signatures[track]) continue;
					this.#signatures[track] = signature;
					this.live[track] = parseReport(text, { track, origin: 'live' });
					this.lastLiveAt = new Date().toISOString();
				} catch {
					this.live[track] = null;
				}
			}
		};
		void tick();
		this.#timer = setInterval(() => void tick(), POLL_MS);
	}

	stopPolling() {
		if (this.#timer) clearInterval(this.#timer);
		this.#timer = null;
	}
}

/** Back to the testers' on-disk shape so a re-import round-trips. */
function toRaw(r: Report) {
	return {
		[r.track === 'kafka' ? 'target' : 'shell']: r.target,
		validate: r.validate,
		passed: r.passed,
		failed: r.failed,
		skipped: r.skipped,
		elapsed_ms: r.elapsedMs,
		stages: r.stages.map((s) => ({
			stage: s.stage,
			name: s.name,
			file: s.file,
			passed: s.passed,
			failed: s.failed,
			skipped: s.skipped,
			tests: s.tests.map((t) => ({
				name: t.name,
				status: t.status,
				ext: t.ext,
				duration_ms: t.durationMs,
				failures: t.failures,
				skip_reason: t.skipReason,
				actual: t.actual
			}))
		}))
	};
}

export const reports = new ReportStore();

/**
 * The report the maps and the stage pages should colour with. The byo database is the
 * source of truth when it is there; the dev-server poll and the last imported file are
 * only the offline fallback (PLAN.md §4.5).
 */
export function activeReport(track: TrackId): Report | null {
	return journey.connected ? journey.latest[track] : reports.active(track);
}
