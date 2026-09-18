// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { flushSync } from 'svelte';
import { JourneyStore, POLL_MS } from './journey.svelte';
import { ByoApi } from '../api/client';

/**
 * A byo server you can turn off mid-session. `state` is mutable so a test can change what
 * the database says between polls, exactly as `byo test` does.
 */
function server() {
	const state = {
		up: true,
		progress: {
			shell: { latestRunId: null as number | null, stages: {} as Record<string, unknown> },
			kafka: { latestRunId: null as number | null, stages: {} },
			streak: 0,
			xp: 0,
			level: 1
		},
		runs: [] as unknown[],
		latest: {} as Record<string, unknown>,
		postFails: false,
		posts: [] as { url: string; body: unknown }[]
	};

	const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
		const url = String(input);
		if (!state.up) throw new TypeError('Failed to fetch');
		const reply = (body: unknown, status = 200) =>
			({ ok: status < 300, status, json: async () => body }) as Response;

		if (url === '/api/health')
			return reply({ ok: true, version: '0.1.0', db: '/db', tracks: { shell: { project: { id: 1, track: 'shell', path: '/p', command: 'bash' } }, kafka: { project: null } } });
		if (url === '/api/progress') return reply(state.progress);
		if (url.startsWith('/api/runs/latest')) {
			const track = new URL(url, 'http://x').searchParams.get('track') ?? 'shell';
			const body = state.latest[track];
			return body ? reply(body) : reply({ error: 'no runs recorded yet' }, 404);
		}
		if (url.startsWith('/api/runs?')) return reply(state.runs);
		if (url.startsWith('/api/stages/')) {
			state.posts.push({ url, body: JSON.parse(String(init?.body ?? '{}')) });
			if (state.postFails) return reply({ error: 'database is locked' }, 500);
			const sent = JSON.parse(String(init?.body ?? '{}')) as { state?: string; note?: string };
			return reply({
				state: sent.state ?? 'todo',
				doneAt: sent.state === 'done' ? '2026-09-13T00:00:00Z' : null,
				note: sent.note ?? null,
				lastRunId: null
			});
		}
		return reply({ error: 'nope' }, 404);
	}) as unknown as typeof fetch;

	return { state, fetcher };
}

function makeStore(fetcher: typeof fetch) {
	return new JourneyStore(new ByoApi(fetcher));
}

const runDoc = (id: number, failed = 0) => ({
	id,
	target: 'bash',
	validate: false,
	passed: 3,
	failed,
	skipped: 0,
	elapsed_ms: 900,
	stages: [
		{
			stage: 1,
			name: 'Print the prompt',
			file: '01.yaml',
			passed: 3,
			failed,
			skipped: 0,
			tests: [
				{ name: 'a', status: 'pass', ext: false, duration_ms: 1, failures: [], skip_reason: null, actual: [] }
			]
		}
	]
});

beforeEach(() => {
	vi.useFakeTimers();
	Object.defineProperty(document, 'visibilityState', { value: 'visible', configurable: true });
});

afterEach(() => {
	vi.useRealTimers();
	vi.restoreAllMocks();
});

describe('JourneyStore', () => {
	it('starts out probing, then connects and takes the database as the source of truth', async () => {
		const { state, fetcher } = server();
		state.progress.shell.stages = {
			'1': { state: 'done', doneAt: '2026-09-13T00:00:00Z', note: 'watch the CRC', lastRunId: 1 }
		};
		state.progress.xp = 100;
		const store = makeStore(fetcher);
		expect(store.status).toBe('probing');
		expect(store.connected).toBe(false);

		await store.refresh();
		flushSync();

		expect(store.status).toBe('connected');
		expect(store.connected).toBe(true);
		expect(store.stageRow('shell', 1)?.state).toBe('done');
		expect(store.stageRow('shell', 1)?.note).toBe('watch the CRC');
		expect(store.stageRow('shell', 9)).toBeNull();
		expect(store.data.xp).toBe(100);
		expect(store.project('shell')?.command).toBe('bash');
		expect(store.needsInit('kafka')).toBe(true);
		expect(store.needsInit('shell')).toBe(false);
	});

	it('falls back to offline when nothing answers /api/health', async () => {
		const { state, fetcher } = server();
		state.up = false;
		const store = makeStore(fetcher);
		await store.refresh();
		expect(store.status).toBe('offline');
		expect(store.offline).toBe(true);
		expect(store.connected).toBe(false);
	});

	it('polls every two seconds while the tab is visible, and not while it is hidden', async () => {
		const { fetcher } = server();
		const store = makeStore(fetcher);
		const stop = store.start();
		await vi.advanceTimersByTimeAsync(1);
		const afterFirst = (fetcher as unknown as { mock: { calls: unknown[] } }).mock.calls.length;
		expect(afterFirst).toBeGreaterThan(0);

		await vi.advanceTimersByTimeAsync(POLL_MS + 5);
		const afterSecond = (fetcher as unknown as { mock: { calls: unknown[] } }).mock.calls.length;
		expect(afterSecond).toBeGreaterThan(afterFirst);

		Object.defineProperty(document, 'visibilityState', { value: 'hidden', configurable: true });
		await vi.advanceTimersByTimeAsync(POLL_MS * 3);
		expect((fetcher as unknown as { mock: { calls: unknown[] } }).mock.calls.length).toBe(afterSecond);
		stop();
	});

	it('picks up a new run within one poll, the way `byo test` finishing does', async () => {
		const { state, fetcher } = server();
		const store = makeStore(fetcher);
		await store.refresh();
		expect(store.latest.shell).toBeNull();

		state.progress.shell.latestRunId = 7;
		state.latest.shell = runDoc(7, 1);
		state.runs = [
			{ id: 7, track: 'shell', target: 'bash', startedAt: 'now', elapsedMs: 900, passed: 3, failed: 1, skipped: 0, args: '--until 3' }
		];

		await store.refresh();
		flushSync();
		expect(store.latest.shell?.target).toBe('bash');
		expect(store.latest.shell?.failed).toBe(1);
		expect(store.runs[0].args).toBe('--until 3');
	});

	it('keeps the last data and says "reconnecting" when byo goes away mid-session', async () => {
		const { state, fetcher } = server();
		state.progress.shell.stages = { '1': { state: 'done', doneAt: 'x', note: null, lastRunId: 1 } };
		const store = makeStore(fetcher);
		await store.refresh();
		expect(store.status).toBe('connected');

		state.up = false;
		await store.refresh();
		expect(store.status).toBe('reconnecting');
		// still connected enough to render: the map must not blank out
		expect(store.connected).toBe(true);
		expect(store.stageRow('shell', 1)?.state).toBe('done');

		state.up = true;
		await store.refresh();
		expect(store.status).toBe('connected');
	});

	it('writes a stage optimistically and rolls back when the write fails', async () => {
		const { state, fetcher } = server();
		const store = makeStore(fetcher);
		await store.refresh();

		const ok = await store.setStage('shell', 4, { state: 'done' });
		expect(ok).toBe(true);
		expect(store.stageRow('shell', 4)?.state).toBe('done');
		expect(state.posts[0]).toEqual({ url: '/api/stages/shell/4', body: { state: 'done' } });

		state.postFails = true;
		const failed = await store.setStage('shell', 5, { state: 'done' });
		expect(failed).toBe(false);
		expect(store.stageRow('shell', 5)).toBeNull();
	});
});
