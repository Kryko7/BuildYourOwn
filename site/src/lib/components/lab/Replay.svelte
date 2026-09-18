<script lang="ts">
	import FailureBlock from '../FailureBlock.svelte';
	import { failingTests, caretNotation } from '$lib/report';
	import { activeReport } from '$lib/stores/reports.svelte';
	import { getCatalog, tracks } from '$lib/catalog';
	import { trackIds } from '$lib/catalog';
	import type { TrackId } from '$lib/types';

	let track = $state<TrackId>(trackIds[0]);
	let picked = $state(0);
	let step = $state(0);

	const report = $derived(activeReport(track));
	const failures = $derived(failingTests(report));
	const current = $derived(failures[Math.min(picked, Math.max(0, failures.length - 1))]);

	const spec = $derived.by(() => {
		if (!current) return undefined;
		const stage = getCatalog(track).stages.find((s) => s.number === current.stage);
		return stage?.tests.find((t) => t.name === current.test.name);
	});

	const steps = $derived.by(() => {
		if (!spec) return [] as { label: string; body: string }[];
		if (spec.input?.length) return spec.input.map((l, i) => ({ label: `stdin line ${i + 1}`, body: l }));
		if (spec.keys?.length) return spec.keys.map((k, i) => ({ label: `key batch ${i + 1}`, body: caretNotation(k) }));
		if (spec.steps?.length) return spec.steps.map((s, i) => ({ label: `${i + 1}. ${s.op}`, body: caretNotation(s.value) }));
		return [];
	});

	$effect(() => {
		picked;
		step = 0;
	});
</script>

<div class="replay">
	<div class="pickers">
		<div class="row">
			{#each trackIds as t (t)}
				<button class="btn btn-sm" class:on={track === t} onclick={() => { track = t; picked = 0; }}>
					{tracks[t].tester}
				</button>
			{/each}
		</div>
		{#if report}
			<p class="tiny muted">
				{report.target} · {report.failed} failing of {report.passed + report.failed} run ·
				{report.origin === 'api'
					? 'byo’s latest run'
					: report.origin === 'live'
						? 'live from disk'
						: 'cached in this browser'}
			</p>
		{/if}
	</div>

	{#if !report}
		<p class="empty muted">
			Run <code>byo test</code> for this track — with <code>byo site</code> serving this page — and
			every failing test in the newest run becomes steppable here. The whole history is on
			<a href="/progress">Runs</a>.
		</p>
	{:else if failures.length === 0}
		<p class="empty ok">Nothing is failing in this report. Good place to be.</p>
	{:else}
		<div class="body">
			<ol class="list" aria-label="Failing tests">
				{#each failures as f, i (i)}
					<li>
						<button class:on={i === picked} onclick={() => (picked = i)}>
							<span class="st mono">{String(f.stage).padStart(2, '0')}</span>
							<span class="nm">{f.test.name}</span>
						</button>
					</li>
				{/each}
			</ol>

			<div class="detail">
				{#if current}
					<h4>
						Stage {current.stage} — {current.stageName}
					</h4>
					<p class="tiny muted">{current.test.name}</p>

					{#if steps.length}
						<div class="stepper">
							<button
								class="btn btn-sm"
								onclick={() => (step = Math.max(0, step - 1))}
								disabled={step === 0}
								aria-label="Previous step">←</button
							>
							<span class="tiny mono">{step + 1} / {steps.length} · {steps[step].label}</span>
							<button
								class="btn btn-sm"
								onclick={() => (step = Math.min(steps.length - 1, step + 1))}
								disabled={step >= steps.length - 1}
								aria-label="Next step">→</button
							>
						</div>
						<div class="terminal tape">
							{#each steps as s, i (i)}<span class:future={i > step} class:now={i === step}
									><span class="t-gut">{i === step ? '▸' : ' '}</span> {s.body}</span
								>{'\n'}{/each}
						</div>
					{/if}

					<FailureBlock test={current.test} {spec} />

					<a class="btn btn-sm" href="/{track}/{current.stage}">open stage {current.stage} →</a>
				{/if}
			</div>
		</div>
	{/if}
</div>

<style>
	.replay {
		display: flex;
		flex-direction: column;
		gap: var(--s-3);
	}
	.pickers .on {
		background: var(--accent, var(--ink));
		color: var(--bg-2);
		border-color: transparent;
	}
	.empty {
		padding: var(--s-5);
		text-align: center;
		border: 1px dashed var(--line-strong);
		border-radius: var(--r-2);
	}
	.empty.ok {
		color: var(--ok);
	}
	.body {
		display: grid;
		grid-template-columns: minmax(0, 260px) minmax(0, 1fr);
		gap: var(--s-4);
		align-items: start;
	}
	@media (max-width: 800px) {
		.body {
			grid-template-columns: 1fr;
		}
	}
	.list {
		list-style: none;
		margin: 0;
		padding: 0;
		border: 1px solid var(--line);
		border-radius: var(--r-2);
		max-height: 420px;
		overflow-y: auto;
	}
	.list li + li button {
		border-top: 1px solid var(--line);
	}
	.list button {
		display: flex;
		gap: var(--s-2);
		width: 100%;
		background: none;
		border: 0;
		text-align: left;
		padding: 6px var(--s-3);
		font-size: 0.78rem;
		cursor: pointer;
		align-items: baseline;
	}
	.list button.on {
		background: color-mix(in srgb, var(--bad) 12%, transparent);
	}
	.st {
		color: var(--ink-3);
		flex: none;
	}
	.nm {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.detail {
		display: flex;
		flex-direction: column;
		gap: var(--s-3);
		min-width: 0;
	}
	.detail h4 {
		margin: 0;
	}
	.detail p {
		margin: 0;
	}
	.stepper {
		display: flex;
		align-items: center;
		gap: var(--s-3);
	}
	.tape .future {
		opacity: 0.35;
	}
	.tape .now {
		color: var(--accent, var(--ink));
		font-weight: 700;
	}
	.detail > a {
		align-self: flex-start;
	}
</style>
