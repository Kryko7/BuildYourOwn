<script lang="ts">
	import { pageTitle } from '$lib/branding';
	import JourneyMap from '$lib/components/JourneyMap.svelte';
	import StageDrawer from '$lib/components/StageDrawer.svelte';
	import ProgressRing from '$lib/components/ProgressRing.svelte';
	import Petals from '$lib/components/garden/Petals.svelte';
	import ConnectionNote from '$lib/components/ConnectionNote.svelte';
	import { badgeFor, tracks } from '$lib/catalog';
	import { ladderOfSection } from '$lib/tracks';
	import { progress } from '$lib/stores/progress.svelte';
	import { reports, activeReport } from '$lib/stores/reports.svelte';
	import { journey } from '$lib/stores/journey.svelte';
	import { stageState, stateLabel } from '$lib/stage-state';
	import type { StageState } from '$lib/types';

	let { data } = $props();

	const track = $derived(data.track);
	const catalog = $derived(data.catalog);
	const meta = $derived(tracks[track]);
	const accent = $derived(meta.accent);

	let selected = $state<number | null>(null);
	let burst = $state(0);
	let map: JourneyMap | undefined = $state();

	const report = $derived(activeReport(track));
	const live = $derived(!journey.connected && reports.live[track] !== null);
	const fromDb = $derived(journey.connected && journey.latest[track] !== null);
	const done = $derived(progress.doneCount(track));
	const nextStage = $derived(progress.nextStage(track));
	const selectedStage = $derived(catalog.stages.find((s) => s.number === selected) ?? null);

	const tally = $derived.by(() => {
		const counts: Record<StageState, number> = {
			locked: 0,
			available: 0,
			next: 0,
			'in-progress': 0,
			done: 0,
			failing: 0
		};
		for (const s of catalog.stages) {
			counts[
				stageState({ track, stage: s.number, report, progress: progress.stage(track, s.number), nextStage })
			]++;
		}
		return counts;
	});

	const plannedCount = $derived(catalog.stages.filter((s) => s.planned).length);

	/**
	 * Tracks with ladders (dist) are climbed a rung at a time, so the trail says so: each
	 * ladder gets a rung with its own progress, and every camp below is labelled with the
	 * one it belongs to. A track with no ladders renders none of this.
	 */
	const ladders = $derived(
		(meta.ladders ?? []).map((l) => {
			const stages = catalog.stages.filter((s) => l.sections.includes(s.section.toUpperCase()));
			return {
				...l,
				stages,
				range: stageRange(stages.map((s) => s.number)),
				done: stages.filter((s) => progress.isDone(track, s.number)).length
			};
		})
	);

	/**
	 * Which stages a rung covers, as "56–77". A rung is not always where its number suggests —
	 * dist climbs its algorithms rung second but numbers it 56–77 — so the range says plainly
	 * what to run. Written as a list of runs, because nothing guarantees a rung is contiguous.
	 */
	function stageRange(numbers: number[]): string {
		const sorted = [...numbers].sort((a, b) => a - b);
		const runs: [number, number][] = [];
		for (const n of sorted) {
			const last = runs[runs.length - 1];
			if (last && n === last[1] + 1) last[1] = n;
			else runs.push([n, n]);
		}
		return runs.map(([a, b]) => (a === b ? `${a}` : `${a}–${b}`)).join(', ');
	}

	/**
	 * `recentre` is for openings that did not come from the map itself (the camp lists, the
	 * continue button, the drawer's prev/next): the trail scrolls to the waypoint so you can
	 * see where you have been sent. Clicking a node on the map must not make the map jump.
	 */
	function open(n: number, recentre = false) {
		selected = n;
		progress.touch(track, n);
		if (recentre) map?.focusStage(n);
	}
</script>

<svelte:head>
	<title>{pageTitle(meta.title)}</title>
</svelte:head>

<Petals {burst} {accent} />

<div class="wrap page" style="--accent:{accent}">
	<header class="head">
		<div class="titles">
			<p class="eyebrow">
				{meta.tester} ·
				{#if catalog.pending}
					{catalog.totals.stages} placeholder waypoints · ≈{meta.plannedStages} stages planned
				{:else}
					{catalog.totals.stages} stages · {catalog.totals.tests || '—'} tests
				{/if}
			</p>
			<h1>{meta.title}</h1>
			<p class="lede">{meta.blurb}</p>
			<div class="legend tiny">
				{#each Object.entries(tally) as [state, n] (state)}
					{#if n > 0}
						<span class="lg {state}"><i></i>{stateLabel[state as StageState]} {n}</span>
					{/if}
				{/each}
				{#if fromDb && report}
					<span class="lg live"><i></i>last run: {report.target}</span>
				{:else if live}
					<span class="lg live"><i></i>live from {meta.reportPath}</span>
				{:else if report}
					<span class="lg imported"><i></i>report: {report.target}</span>
				{/if}
			</div>
		</div>
		<div class="ringbox">
			<ProgressRing
				value={done}
				total={catalog.totals.stages}
				{accent}
				size={96}
				label={catalog.pending ? 'waypoints' : 'stages'}
				unit={catalog.pending ? 'placeholder waypoints' : 'stages'}
			/>
			<button class="btn btn-primary" onclick={() => open(nextStage, true)}>Continue at {nextStage}</button>
		</div>
	</header>

	<ConnectionNote {track} />

	{#if catalog.pending}
		<p class="pending tiny">
			<strong>The stages here are still being written.</strong>
			<code>{meta.tester}</code> has not published a <code>catalog.json</code> yet, so this trail is a
			placeholder: one waypoint per section, with what that section will cover and no tests to run.
			The sections themselves are real, and so is what you are building —
			{meta.building}, checked against {meta.reference}. About {meta.plannedStages} stages are planned.
			Run <code>npm run sync</code> once the tester lands and this page becomes the real trail.
		</p>
	{:else if plannedCount}
		<p class="pending tiny">
			{plannedCount} of these {catalog.totals.stages} stages are still
			<strong>planned</strong>: <code>{meta.tester}</code> lists them in its <code>PLAN.md</code> but has
			not shipped their tests yet, so they show what to build and nothing to run. Re-run
			<code>npm run sync</code> after the tester merges them.
		</p>
	{/if}

	<JourneyMap bind:this={map} {track} {catalog} {report} {selected} celebrate={burst} onselect={open} />

	{#if ladders.length}
		<section class="ladders" aria-label="Ladders">
			{#each ladders as l, li (l.id)}
				<article class="rung card">
					<div class="rhead">
						<span class="rnum eyebrow">rung {li + 1}</span>
						<h3>{l.title}</h3>
						<span class="chip {l.done === l.stages.length && l.stages.length ? 'chip-ok' : ''}">
							{l.done}/{l.stages.length}
						</span>
					</div>
					<p class="tiny muted">{l.note}</p>
					<div class="rbar" aria-hidden="true">
						<i style="width:{l.stages.length ? (l.done / l.stages.length) * 100 : 0}%"></i>
					</div>
					<p class="tiny muted rsec">
						stages {l.range} · sections {l.sections.join(', ')}
					</p>
				</article>
			{/each}
		</section>
	{/if}

	<section class="sections">
		{#each catalog.sections as section (section.id)}
			{@const badge = badgeFor(track, section.id)}
			{@const stages = catalog.stages.filter((s) => section.stages.includes(s.number))}
			{@const sectionDone = stages.filter((s) => progress.isDone(track, s.number)).length}
			<article class="camp card">
				<div class="chead">
					<span class="icon" aria-hidden="true">{badge.icon}</span>
					<div>
						<h3>{badge.badge}</h3>
						<p class="tiny muted">{section.id}. {section.title}</p>
					</div>
					<span class="chip {sectionDone === stages.length ? 'chip-ok' : ''}">{sectionDone}/{stages.length}</span>
				</div>
				<ul>
					{#each stages as s (s.number)}
						{@const st = stageState({
							track,
							stage: s.number,
							report,
							progress: progress.stage(track, s.number),
							nextStage
						})}
						<li class={st} class:planned={s.planned}>
							<button onclick={() => open(s.number, true)}>
								<span class="n mono">{String(s.number).padStart(2, '0')}</span>
								<span class="nm">{s.name}</span>
								{#if s.planned}<span class="chip" title="Not yet in the tester">planned</span>{/if}
								{#if s.ext}<span class="chip chip-ext">ext</span>{/if}
							</button>
						</li>
					{/each}
				</ul>
			</article>
		{/each}
	</section>
</div>

<StageDrawer
	{track}
	stage={selectedStage}
	{catalog}
	{report}
	onclose={() => (selected = null)}
	onnavigate={(n) => open(n, true)}
	oncomplete={() => {
		if (progress.data.settings.petals) burst++;
	}}
/>

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: var(--s-5);
		padding: var(--s-6) 0 var(--s-7);
	}
	.head {
		display: flex;
		gap: var(--s-5);
		align-items: flex-start;
		justify-content: space-between;
		flex-wrap: wrap;
	}
	.titles {
		max-width: 62ch;
	}
	h1 {
		margin: var(--s-1) 0 var(--s-2);
	}
	.lede {
		color: var(--ink-2);
	}
	.ringbox {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: var(--s-3);
	}
	.legend {
		display: flex;
		flex-wrap: wrap;
		gap: var(--s-3);
		color: var(--ink-2);
		margin-top: var(--s-3);
	}
	.lg {
		display: inline-flex;
		align-items: center;
		gap: 5px;
	}
	.lg i {
		width: 9px;
		height: 9px;
		border-radius: 50%;
		border: 2px solid var(--line-strong);
		background: var(--bg-2);
	}
	.lg.done i {
		background: var(--ok);
		border-color: var(--ok);
	}
	.lg.failing i {
		background: var(--bad);
		border-color: var(--bad);
	}
	.lg.next i {
		border-color: var(--accent);
	}
	.lg.in-progress i {
		background: color-mix(in srgb, var(--accent) 40%, transparent);
		border-color: var(--accent);
	}
	.lg.live i,
	.lg.imported i {
		background: var(--ok);
		border-color: var(--ok);
	}
	.lg.imported i {
		background: var(--ink-3);
		border-color: var(--ink-3);
	}
	.pending {
		background: var(--warn-soft);
		color: var(--warn);
		border-radius: var(--r-2);
		padding: var(--s-3);
		margin: 0;
	}

	.ladders {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
		gap: var(--s-4);
	}
	.rung {
		padding: var(--s-4);
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.rhead {
		display: flex;
		align-items: baseline;
		gap: var(--s-2);
	}
	.rhead h3 {
		font-family: var(--font-display);
		font-size: 1.05rem;
		flex: 1;
	}
	.rnum {
		color: var(--accent);
	}
	.rbar {
		height: 7px;
		background: var(--bg-3);
		border-radius: var(--r-full);
		overflow: hidden;
		margin-top: 4px;
	}
	.rbar i {
		display: block;
		height: 100%;
		background: var(--accent);
		border-radius: inherit;
		transition: width 700ms var(--ease);
	}
	.rsec {
		margin: 0;
		font-family: var(--font-mono);
	}
	.lad {
		color: var(--accent);
	}
	.sections {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
		gap: var(--s-4);
	}
	.camp {
		padding: var(--s-4);
	}
	.chead {
		display: flex;
		align-items: center;
		gap: var(--s-3);
		padding-bottom: var(--s-3);
		border-bottom: 1px dashed var(--line);
		margin-bottom: var(--s-2);
	}
	.chead > div {
		flex: 1;
		min-width: 0;
	}
	.chead h3 {
		font-family: var(--font-display);
		font-size: 1.02rem;
	}
	.chead p {
		margin: 0;
	}
	.icon {
		font-size: 1.2rem;
	}
	.camp ul {
		list-style: none;
		margin: 0;
		padding: 0;
	}
	.camp li button {
		display: flex;
		align-items: baseline;
		gap: var(--s-2);
		width: 100%;
		background: none;
		border: 0;
		padding: 4px 6px;
		border-radius: var(--r-1);
		text-align: left;
		cursor: pointer;
		font-size: 0.85rem;
	}
	.camp li button:hover {
		background: var(--bg-3);
	}
	.camp li .n {
		color: var(--ink-3);
		font-size: 0.74rem;
		flex: none;
	}
	.camp li .nm {
		flex: 1;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.camp li.done .nm {
		color: var(--ink-2);
		text-decoration: line-through;
		text-decoration-color: var(--ok);
	}
	.camp li.failing .nm {
		color: var(--bad);
	}
	.camp li.next .nm {
		font-weight: 600;
	}
	.camp li.next .n {
		color: var(--accent);
	}
	.camp li.locked {
		opacity: 0.62;
	}
	.camp li.planned .nm {
		font-style: italic;
		color: var(--ink-2);
	}
</style>
