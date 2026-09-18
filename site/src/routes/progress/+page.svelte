<script lang="ts">
	/**
	 * Runs — the history side of the journey (PLAN.md §4.5).
	 *
	 * Everything here comes from byo's database: the run list, each run's per-test results
	 * and the terminal-style failure blocks. There is no report upload any more; a report
	 * gets here by running `byo test`, which is the only way it can also move the map.
	 */
	import { pageTitle } from '$lib/branding';
	import FailureBlock from '$lib/components/FailureBlock.svelte';
	import CopyButton from '$lib/components/CopyButton.svelte';
	import ConnectionNote from '$lib/components/ConnectionNote.svelte';
	import Bunny from '$lib/components/garden/Bunny.svelte';
	import { formatDuration } from '$lib/report';
	import { reports } from '$lib/stores/reports.svelte';
	import { journey } from '$lib/stores/journey.svelte';
	import { progress } from '$lib/stores/progress.svelte';
	import { catalogs, tracks, trackIds } from '$lib/catalog';
	import type { Report, TrackId } from '$lib/types';

	let message = $state('');
	let error = $state('');
	let confirmClear = $state(false);

	/* ---- the expanded run ---- */
	let openRunId = $state<number | null>(null);
	let openRun = $state<Report | null>(null);
	let loadingRun = $state(false);
	let runError = $state('');
	let openStage = $state<string | null>(null);

	async function toggleRun(id: number, track: TrackId) {
		openStage = null;
		if (openRunId === id) {
			openRunId = null;
			openRun = null;
			return;
		}
		openRunId = id;
		openRun = null;
		runError = '';
		loadingRun = true;
		try {
			openRun = await journey.run(id, track);
		} catch (e) {
			runError = e instanceof Error ? e.message : 'Could not load that run.';
		} finally {
			loadingRun = false;
		}
	}

	const quickstart = [
		{
			cmd: 'byo init shell --command ./your_program.sh',
			why: 'once, in the repo where you are writing your shell (or `byo init kafka`)'
		},
		{ cmd: 'byo test --stage 1', why: 'run one stage; `--until 12` or `--all` for more' },
		{ cmd: 'byo status', why: 'the same progress, in the terminal' },
		{ cmd: 'byo site', why: 'serve this page with your real data and open a browser' }
	];

	/** The newest run per track, for the two summary cards. */
	const latest = $derived(
		trackIds
			.map((t) => ({ track: t, report: journey.latest[t] }))
			.filter((x): x is { track: TrackId; report: Report } => x.report !== null)
	);

	/** Offline only: whatever the dev-server poll or an old localStorage copy still holds. */
	const fallbackReports = $derived(
		journey.connected
			? []
			: trackIds
					.map((t) => ({ track: t, report: reports.active(t) }))
					.filter((x): x is { track: TrackId; report: Report } => x.report !== null)
	);

	function download(name: string, body: string) {
		const url = URL.createObjectURL(new Blob([body], { type: 'application/json' }));
		const a = document.createElement('a');
		a.href = url;
		a.download = name;
		a.click();
		URL.revokeObjectURL(url);
	}

	async function onStateFile(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		if (file) {
			const res = progress.import(await file.text());
			if (res.ok) {
				message = 'Local state restored.';
				error = '';
			} else {
				error = res.error;
			}
		}
		input.value = '';
	}

	const when = (iso: string) => {
		const d = new Date(iso);
		return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
	};
</script>

<svelte:head>
	<title>{pageTitle('Runs')}</title>
</svelte:head>

<div class="wrap page">
	<header class="head">
		<div>
			<p class="eyebrow">every `byo test`, remembered</p>
			<h1>Runs</h1>
			<p class="lede">
				<code>byo test</code> streams the tester's output to your terminal and writes the result into
				its database. This is that database: which stages went green, which tests failed and exactly
				what they saw.
			</p>
		</div>
		<span class="peek" aria-hidden="true"><Bunny size={92} /></span>
	</header>

	<ConnectionNote />

	<section class="quick card">
		<h2>The four commands</h2>
		<ul>
			{#each quickstart as q (q.cmd)}
				<li>
					<code>{q.cmd}</code>
					<CopyButton text={q.cmd} label="copy" />
					<span class="tiny muted why">{q.why}</span>
				</li>
			{/each}
		</ul>
		{#if journey.health}
			<p class="tiny muted dbline">
				byo {journey.health.version} · database <code>{journey.health.db}</code>
				{#each trackIds as t (t)}
					{@const p = journey.project(t)}
					· {t}: {p ? `${p.command} in ${p.path}` : 'no project yet'}
				{/each}
			</p>
		{/if}
	</section>

	{#if journey.connected}
		{#if latest.length}
			<section class="latestrow">
				{#each latest as { track, report } (track)}
					<article class="card sum" style="--accent:{track === 'shell' ? 'var(--shell)' : 'var(--kafka)'}">
						<div class="sumhead">
							<div>
								<p class="eyebrow">latest {tracks[track].tester} run</p>
								<h3>{report.target}</h3>
							</div>
							<div class="totals">
								<span class="chip chip-ok">{report.passed} passed</span>
								{#if report.failed}<span class="chip chip-bad">{report.failed} failed</span>{/if}
								{#if report.skipped}<span class="chip">{report.skipped} skipped</span>{/if}
							</div>
						</div>
						<div class="bars">
							{#each report.stages as s (s.stage)}
								{@const total = s.passed + s.failed + s.skipped}
								<a
									class="bar"
									class:red={s.failed > 0}
									href="/{track}/{s.stage}"
									title="Stage {s.stage}: {s.name} — {s.passed}/{total} passing"
									aria-label="Stage {s.stage} {s.name}, {s.passed} of {total} passing"
								>
									<i style="height:{total ? (s.passed / total) * 100 : 0}%"></i>
									<span class="tiny">{s.stage}</span>
								</a>
							{/each}
						</div>
						<p class="tiny muted">
							{report.stages.length} stages · {formatDuration(report.elapsedMs)}
							{#if report.validate} · <span class="chip">--validate</span>{/if}
						</p>
					</article>
				{/each}
			</section>
		{/if}

		<section class="card history">
			<h2>Run history</h2>
			{#if journey.runs.length === 0}
				<p class="empty muted">
					No runs recorded yet. Run <code>byo test --stage 1</code> in your project and this table fills
					in within a couple of seconds.
				</p>
			{:else}
				<table>
					<thead>
						<tr>
							<th scope="col">when</th>
							<th scope="col">track</th>
							<th scope="col">target</th>
							<th scope="col">flags</th>
							<th scope="col" class="num">passed</th>
							<th scope="col" class="num">failed</th>
							<th scope="col" class="num">took</th>
							<th scope="col"><span class="visually-hidden">details</span></th>
						</tr>
					</thead>
					<tbody>
						{#each journey.runs as r (r.id)}
							<tr class:bad={r.failed > 0} class:on={openRunId === r.id}>
								<td class="tiny">{when(r.startedAt)}</td>
								<td><span class="chip">{r.track}</span></td>
								<td class="mono tiny">{r.target}</td>
								<td class="mono tiny args">{r.args || '—'}</td>
								<td class="num ok">{r.passed}</td>
								<td class="num" class:red={r.failed > 0}>{r.failed}</td>
								<td class="num tiny muted">{formatDuration(r.elapsedMs)}</td>
								<td class="num">
									<button
										class="btn btn-sm btn-ghost"
										onclick={() => toggleRun(r.id, r.track)}
										aria-expanded={openRunId === r.id}
									>
										{openRunId === r.id ? 'hide' : 'open'}
									</button>
								</td>
							</tr>
							{#if openRunId === r.id}
								<tr class="detailrow">
									<td colspan="8">
										{#if loadingRun}
											<p class="tiny muted">Loading run {r.id}…</p>
										{:else if runError}
											<p class="tiny err">⚠ {runError}</p>
										{:else if openRun}
											<ul class="stages">
												{#each openRun.stages as s (s.stage)}
													{@const key = `${r.id}:${s.stage}`}
													{@const spec = catalogs[r.track].stages.find((x) => x.number === s.stage)}
													<li class:bad={s.failed > 0}>
														<button
															class="shead"
															onclick={() => (openStage = openStage === key ? null : key)}
															aria-expanded={openStage === key}
														>
															<span class="mono n">{String(s.stage).padStart(2, '0')}</span>
															<span class="nm">{s.name}</span>
															<span class="score mono tiny" class:red={s.failed > 0}>
																{s.passed}/{s.passed + s.failed} passed{s.skipped
																	? ` (${s.skipped} skipped)`
																	: ''}
															</span>
														</button>
														{#if openStage === key}
															<div class="sbody">
																{#each s.tests as t, ti (ti)}
																	<div class="test {t.status}">
																		<span class="mark" aria-hidden="true"
																			>{t.status === 'pass' ? '✔' : t.status === 'fail' ? '✘' : '–'}</span
																		>
																		<span class="tname">{t.name}</span>
																		<span class="tiny muted">{formatDuration(t.durationMs)}</span>
																	</div>
																	{#if t.status === 'fail'}
																		<FailureBlock
																			test={t}
																			spec={spec?.tests.find((x) => x.name === t.name)}
																		/>
																	{:else if t.status === 'skip' && t.skipReason}
																		<p class="tiny muted skip">skipped: {t.skipReason}</p>
																	{/if}
																{/each}
																<a class="btn btn-sm" href="/{r.track}/{s.stage}"
																	>open stage {s.stage} →</a
																>
															</div>
														{/if}
													</li>
												{/each}
											</ul>
										{/if}
									</td>
								</tr>
							{/if}
						{/each}
					</tbody>
				</table>
			{/if}
		</section>
	{:else}
		<section class="card fallback">
			<h2>Fallback: this browser only</h2>
			<p class="muted">
				With no byo server there is no run history to show. What follows is the local copy the site
				keeps so it still works on static hosting — it is <strong>not</strong> what
				<code>byo status</code> reports.
			</p>
			{#each fallbackReports as { track, report } (track)}
				<div class="fb">
					<p class="tiny">
						<strong>{tracks[track].tester}</strong> · {report.target} ·
						{report.origin === 'live' ? 'polled from disk by the dev server' : 'imported earlier'} ·
						{report.passed} passed, {report.failed} failed
						<button class="btn btn-sm btn-ghost" onclick={() => reports.clear(track)}>forget</button>
					</p>
				</div>
			{:else}
				<p class="tiny muted">Nothing cached locally either.</p>
			{/each}
		</section>
	{/if}

	<section class="local card">
		<h2>Local state</h2>
		<p class="muted">
			The target names, the celebration setting and — when byo is not running — completions and
			notes are kept in this browser's localStorage. Nothing is ever uploaded anywhere.
		</p>
		<div class="row wrapme">
			<button class="btn" onclick={() => download('byo-journey-state.json', progress.export())}>
				Export state
			</button>
			<label class="btn">
				Import state…
				<input type="file" accept="application/json,.json" onchange={onStateFile} />
			</label>
			{#if confirmClear}
				<button
					class="btn btn-primary"
					style="--accent:var(--bad)"
					onclick={() => {
						progress.clear();
						reports.clear();
						confirmClear = false;
						message = 'Local state cleared. Anything in byo’s database is untouched.';
					}}>Really clear local state</button
				>
				<button class="btn btn-ghost" onclick={() => (confirmClear = false)}>cancel</button>
			{:else}
				<button class="btn btn-ghost" onclick={() => (confirmClear = true)}>Clear local state…</button>
			{/if}
		</div>
		<label class="toggle tiny">
			<input
				type="checkbox"
				checked={progress.data.settings.petals}
				onchange={(e) => progress.setSetting('petals', e.currentTarget.checked)}
			/>
			scatter petals when a stage blooms
		</label>
		{#if message}<p class="ok tiny">{message}</p>{/if}
		{#if error}<p class="err tiny">⚠ {error}</p>{/if}
	</section>
</div>

<style>
	.page {
		padding: var(--s-6) 0 var(--s-7);
		display: flex;
		flex-direction: column;
		gap: var(--s-4);
	}
	.head {
		display: flex;
		align-items: flex-end;
		justify-content: space-between;
		gap: var(--s-4);
	}
	.head > div {
		max-width: 68ch;
	}
	h1 {
		margin: var(--s-2) 0;
	}
	.lede {
		color: var(--ink-2);
	}
	.peek {
		flex: none;
	}
	@media (max-width: 760px) {
		.peek {
			display: none;
		}
	}

	.quick {
		padding: var(--s-4) var(--s-5);
	}
	.quick h2 {
		font-size: 1.1rem;
		margin-bottom: var(--s-3);
	}
	.quick ul {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--s-2);
	}
	.quick li {
		display: flex;
		align-items: center;
		gap: var(--s-2);
		flex-wrap: wrap;
	}
	.quick code {
		background: var(--bg-sunk);
		border: 1px solid var(--line);
		padding: 4px 10px;
		border-radius: var(--r-1);
		font-size: 0.8rem;
	}
	.quick .why {
		flex: 1;
		min-width: 12ch;
	}
	.dbline {
		margin: var(--s-3) 0 0;
		word-break: break-word;
	}

	.latestrow {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
		gap: var(--s-4);
	}
	.sum {
		padding: var(--s-4) var(--s-5);
		display: flex;
		flex-direction: column;
		gap: var(--s-3);
	}
	.sumhead {
		display: flex;
		justify-content: space-between;
		gap: var(--s-3);
		flex-wrap: wrap;
		align-items: flex-start;
	}
	.sumhead p {
		margin: 0;
	}
	.sum p:last-child {
		margin: 0;
	}
	.totals {
		display: flex;
		gap: 5px;
		align-items: center;
		flex-wrap: wrap;
	}
	.bars {
		display: flex;
		align-items: flex-end;
		gap: 3px;
		height: 64px;
		overflow-x: auto;
		padding-bottom: 2px;
	}
	.bar {
		position: relative;
		width: 18px;
		height: 100%;
		flex: none;
		background: var(--bg-3);
		border: 0;
		border-radius: var(--r-1) var(--r-1) 3px 3px;
		cursor: pointer;
		display: flex;
		flex-direction: column;
		justify-content: flex-end;
		padding: 0;
		overflow: hidden;
		text-decoration: none;
	}
	.bar i {
		display: block;
		width: 100%;
		background: var(--ok-bright);
		transition: height 500ms var(--ease);
	}
	.bar.red i {
		background: var(--bad-bright);
	}
	.bar span {
		position: absolute;
		bottom: 1px;
		left: 0;
		right: 0;
		text-align: center;
		font-family: var(--font-mono);
		font-size: 0.56rem;
		color: var(--ink-2);
	}

	.history {
		padding: var(--s-4) var(--s-5) var(--s-5);
		overflow-x: auto;
	}
	.history h2 {
		font-size: 1.1rem;
		margin-bottom: var(--s-3);
	}
	table {
		width: 100%;
		border-collapse: collapse;
		font-size: 0.86rem;
	}
	th {
		text-align: left;
		font-family: var(--font-mono);
		font-size: 0.66rem;
		text-transform: uppercase;
		letter-spacing: 0.12em;
		color: var(--ink-3);
		font-weight: 500;
		padding: 0 var(--s-2) 6px;
		white-space: nowrap;
	}
	td {
		padding: 6px var(--s-2);
		border-top: 1px solid var(--line);
		vertical-align: middle;
	}
	tr.bad td {
		background: color-mix(in srgb, var(--bad) 5%, transparent);
	}
	tr.on td {
		background: color-mix(in srgb, var(--lav) 12%, transparent);
	}
	.num {
		text-align: right;
		font-variant-numeric: tabular-nums;
	}
	td.ok {
		color: var(--ok);
		font-weight: 600;
	}
	td.red {
		color: var(--bad);
		font-weight: 600;
	}
	.args {
		max-width: 22ch;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.detailrow td {
		padding: var(--s-3) var(--s-2) var(--s-4);
		background: var(--bg-3);
	}

	.stages {
		list-style: none;
		margin: 0;
		padding: 0;
		border: 1px solid var(--line);
		border-radius: var(--r-2);
		background: var(--bg-2);
		overflow: hidden;
	}
	.stages li + li {
		border-top: 1px solid var(--line);
	}
	.stages li.bad {
		background: color-mix(in srgb, var(--bad) 6%, transparent);
	}
	.shead {
		display: flex;
		gap: var(--s-3);
		align-items: baseline;
		width: 100%;
		background: none;
		border: 0;
		padding: 7px var(--s-3);
		cursor: pointer;
		text-align: left;
	}
	.shead .n {
		color: var(--ink-3);
		flex: none;
	}
	.shead .nm {
		flex: 1;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.score {
		color: var(--ok);
		flex: none;
	}
	.score.red {
		color: var(--bad);
	}
	.sbody {
		padding: 0 var(--s-3) var(--s-3);
		display: flex;
		flex-direction: column;
		gap: var(--s-2);
	}
	.test {
		display: flex;
		gap: var(--s-2);
		align-items: baseline;
		font-size: 0.84rem;
	}
	.test .mark {
		font-family: var(--font-mono);
		color: var(--ink-3);
	}
	.test.pass .mark {
		color: var(--ok);
	}
	.test.fail .mark {
		color: var(--bad);
	}
	.test.fail .tname {
		color: var(--bad);
	}
	.test .tname {
		flex: 1;
		min-width: 0;
	}
	.skip {
		margin: 0 0 0 var(--s-4);
	}
	.sbody > a {
		align-self: flex-start;
	}

	.fallback,
	.local {
		padding: var(--s-4) var(--s-5) var(--s-5);
	}
	.fallback h2,
	.local h2 {
		font-size: 1.1rem;
		margin-bottom: var(--s-2);
	}
	.fb p {
		margin: var(--s-2) 0 0;
	}
	.empty {
		padding: var(--s-4) 0;
	}
	input[type='file'] {
		display: none;
	}
	.ok {
		color: var(--ok);
		margin: var(--s-3) 0 0;
	}
	.err {
		color: var(--bad);
		margin: var(--s-3) 0 0;
	}
	.wrapme {
		flex-wrap: wrap;
		margin-top: var(--s-3);
	}
	.toggle {
		display: flex;
		align-items: center;
		gap: 6px;
		margin-top: var(--s-3);
		color: var(--ink-2);
	}
</style>
