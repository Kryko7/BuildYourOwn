import { describe, it, expect, vi } from 'vitest';
import { ByoApi, ApiError, normalizeProgress, normalizeStageRow } from './client';

/** A fetch that answers from a table of routes; anything unlisted is a 404. */
function fakeFetch(routes: Record<string, { status?: number; body: unknown }>) {
	const calls: string[] = [];
	const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
		const url = String(input);
		calls.push(`${init?.method ?? 'GET'} ${url}`);
		const hit = routes[url];
		const status = hit?.status ?? (hit ? 200 : 404);
		return {
			ok: status >= 200 && status < 300,
			status,
			json: async () => (hit ? hit.body : { error: 'not found' })
		} as Response;
	});
	return { fetcher: fetcher as unknown as typeof fetch, calls };
}

const HEALTH = {
	ok: true,
	version: '0.1.0',
	db: '/home/you/.local/share/byo/byo.db',
	dataDir: '/home/you/.local/share/byo',
	schemaVersion: 1,
	tracks: {
		shell: {
			project: { id: 1, track: 'shell', path: '/home/you/my-shell', command: 'bash' }
		},
		kafka: { project: null }
	}
};

const PROGRESS = {
	shell: {
		latestRunId: 3,
		stages: {
			'1': { state: 'done', doneAt: '2026-09-13T02:04:19Z', note: 'hi', lastRunId: 3 },
			'2': { state: 'failed', doneAt: null, note: null, lastRunId: 3 },
			'3': { state: 'in_progress', doneAt: null, note: '', lastRunId: 3 }
		}
	},
	kafka: { latestRunId: null, stages: {} },
	streak: 2,
	xp: 320,
	level: 1
};

const RUN = {
	id: 3,
	target: 'bash',
	validate: false,
	passed: 2,
	failed: 1,
	skipped: 0,
	elapsed_ms: 1200,
	stages: [
		{
			stage: 1,
			name: 'Print the prompt',
			file: '01_prompt.yaml',
			passed: 2,
			failed: 1,
			skipped: 0,
			tests: [
				{ name: 'a', status: 'pass', ext: false, duration_ms: 5, failures: [], skip_reason: null, actual: [] },
				{ name: 'b', status: 'pass', ext: false, duration_ms: 6, failures: [], skip_reason: null, actual: [] },
				{
					name: 'c',
					status: 'fail',
					ext: false,
					duration_ms: 7,
					failures: ['stdout differs'],
					skip_reason: null,
					actual: [['stdout', 'nope']]
				}
			]
		}
	]
};

describe('normalizeStageRow', () => {
	it('defaults an unknown state to todo and empties a blank note', () => {
		expect(normalizeStageRow({ state: 'exploded', note: '' })).toEqual({
			state: 'todo',
			doneAt: null,
			note: null,
			lastRunId: null,
			updatedAt: null
		});
	});
});

describe('normalizeProgress', () => {
	it('survives an empty body', () => {
		const p = normalizeProgress(undefined);
		expect(p.shell.stages).toEqual({});
		expect(p.kafka.latestRunId).toBeNull();
		expect(p.level).toBe(1);
	});

	it('keeps the four real states and drops non-numeric stage keys', () => {
		const p = normalizeProgress({
			...PROGRESS,
			shell: { ...PROGRESS.shell, stages: { ...PROGRESS.shell.stages, oops: { state: 'done' } } }
		});
		expect(Object.keys(p.shell.stages).sort()).toEqual(['1', '2', '3']);
		expect(p.shell.stages['2'].state).toBe('failed');
		expect(p.streak).toBe(2);
		expect(p.xp).toBe(320);
	});
});

describe('ByoApi', () => {
	it('detects a live byo server', async () => {
		const { fetcher } = fakeFetch({ '/api/health': { body: HEALTH } });
		const health = await new ByoApi(fetcher).health();
		expect(health?.version).toBe('0.1.0');
		expect(health?.tracks.shell.project?.path).toBe('/home/you/my-shell');
		expect(health?.tracks.kafka.project).toBeNull();
	});

	it('returns null (not an error) when there is no server at all', async () => {
		const fetcher = vi.fn(async () => {
			throw new TypeError('Failed to fetch');
		}) as unknown as typeof fetch;
		expect(await new ByoApi(fetcher).health()).toBeNull();
	});

	it('returns null when something else is serving that path', async () => {
		const { fetcher } = fakeFetch({ '/api/health': { body: { hello: 'world' } } });
		expect(await new ByoApi(fetcher).health()).toBeNull();
	});

	it('reads progress', async () => {
		const { fetcher } = fakeFetch({ '/api/progress': { body: PROGRESS } });
		const p = await new ByoApi(fetcher).progress();
		expect(p.shell.stages['1'].note).toBe('hi');
		expect(p.shell.stages['3'].note).toBeNull();
	});

	it('lists runs and drops rows with an unknown track', async () => {
		const { fetcher, calls } = fakeFetch({
			'/api/runs?track=shell&limit=5': {
				body: [
					{ id: 2, track: 'shell', target: 'bash', startedAt: 'x', elapsedMs: 10, passed: 1, failed: 0, skipped: 0, args: '--all' },
					{ id: 1, track: 'perl', target: 'nope' }
				]
			}
		});
		const runs = await new ByoApi(fetcher).runs('shell', 5);
		expect(runs).toHaveLength(1);
		expect(runs[0].args).toBe('--all');
		expect(calls[0]).toBe('GET /api/runs?track=shell&limit=5');
	});

	it('parses a run into the site report shape', async () => {
		const { fetcher } = fakeFetch({ '/api/runs/3': { body: RUN } });
		const report = await new ByoApi(fetcher).run(3, 'shell');
		expect(report.track).toBe('shell');
		expect(report.target).toBe('bash');
		expect(report.origin).toBe('api');
		expect(report.stages[0].tests).toHaveLength(3);
		expect(report.failed).toBe(1);
	});

	it('turns a 404 on the latest run into null', async () => {
		const { fetcher } = fakeFetch({});
		expect(await new ByoApi(fetcher).latest('kafka')).toBeNull();
	});

	it('surfaces the server error sentence on a real failure', async () => {
		const { fetcher } = fakeFetch({
			'/api/progress': { status: 500, body: { error: 'database is locked' } }
		});
		await expect(new ByoApi(fetcher).progress()).rejects.toThrow('database is locked');
		await expect(new ByoApi(fetcher).progress()).rejects.toBeInstanceOf(ApiError);
	});

	it('posts a stage write as JSON and reads the row back', async () => {
		const { fetcher, calls } = fakeFetch({
			'/api/stages/shell/4': {
				body: { track: 'shell', stage: 4, state: 'done', doneAt: 'now', note: 'x', lastRunId: null }
			}
		});
		const row = await new ByoApi(fetcher).setStage('shell', 4, { state: 'done' });
		expect(row.state).toBe('done');
		expect(row.stage).toBe(4);
		expect(calls[0]).toBe('POST /api/stages/shell/4');
	});

	it('treats a missing catalog as "use the bundled one"', async () => {
		const { fetcher } = fakeFetch({});
		expect(await new ByoApi(fetcher).catalog('shell')).toBeNull();
	});
});
