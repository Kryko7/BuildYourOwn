<script lang="ts">
	import { pageTitle } from '$lib/branding';
	import { allResources, compareResources, typeGlyphs, typeLabels } from '$lib/resources';
	import { progress } from '$lib/stores/progress.svelte';
	import { stageOf, tracks, trackIds } from '$lib/catalog';
	import type { Resource } from '$lib/types';

	const resources = allResources();
	const types = [...new Set(resources.map((r) => r.type))].sort();
	const concepts = [...new Set(resources.flatMap((r) => r.concepts))].sort();

	let q = $state('');
	let track = $state<'all' | 'shell' | 'kafka'>('all');
	let type = $state<'all' | Resource['type']>('all');
	let level = $state<'all' | Resource['level']>('all');
	let concept = $state('all');
	let onlyCurrent = $state(false);

	const currentStages = $derived({
		shell: progress.nextStage('shell'),
		kafka: progress.nextStage('kafka')
	});

	function isCurrent(r: Resource) {
		return (
			(r.track !== 'kafka' && r.stages.includes(currentStages.shell)) ||
			(r.track !== 'shell' && r.stages.includes(currentStages.kafka))
		);
	}

	const filtered = $derived(
		resources.filter((r) => {
			if (track !== 'all' && r.track !== track && r.track !== 'both') return false;
			if (type !== 'all' && r.type !== type) return false;
			if (level !== 'all' && r.level !== level) return false;
			if (concept !== 'all' && !r.concepts.includes(concept)) return false;
			if (onlyCurrent && !isCurrent(r)) return false;
			const needle = q.trim().toLowerCase();
			if (!needle) return true;
			return `${r.title} ${r.why} ${r.concepts.join(' ')} ${r.type} ${r.level}`.toLowerCase().includes(needle);
		})
	);

	const grouped = $derived.by(() => {
		const groups = new Map<string, Resource[]>();
		for (const r of filtered) {
			const key = r.track === 'both' ? 'Both tracks' : tracks[r.track].title;
			if (!groups.has(key)) groups.set(key, []);
			groups.get(key)!.push(r);
		}
		// Journey order, not the alphabet: the library reads in the order the trail needs it.
		for (const list of groups.values()) list.sort(compareResources);
		// The groups themselves keep a fixed order, so the page does not reshuffle its
		// headings every time a filter changes which resource happens to come first.
		const order = [...trackIds.map((t) => tracks[t].title), 'Both tracks'];
		return [...groups.entries()].sort(
			([a], [b]) => order.indexOf(a) - order.indexOf(b)
		);
	});

	/** A resource may cite a stage the tester has not published — link only what exists. */
	function stageHref(r: Resource, stage: number): string | null {
		const track = r.track === 'both' ? 'shell' : r.track;
		return stageOf(track, stage) ? `/${track}/${stage}` : null;
	}

	function reset() {
		q = '';
		track = 'all';
		type = 'all';
		level = 'all';
		concept = 'all';
		onlyCurrent = false;
	}
</script>

<svelte:head>
	<title>{pageTitle('Resources')}</title>
</svelte:head>

<div class="wrap page">
	<header>
		<p class="eyebrow">the library</p>
		<h1>What to read, and why</h1>
		<p class="lede">
			Specs, manuals and papers tied to the stages that need them. Every entry says what it buys you —
			no link dumps.
		</p>
	</header>

	<div class="filters card">
		<label class="search">
			<span class="visually-hidden">Search resources</span>
			<input bind:value={q} placeholder="Search titles, concepts, reasons…" spellcheck="false" />
		</label>
		<label>
			<span class="tiny muted">track</span>
			<select bind:value={track}>
				<option value="all">all</option>
				{#each trackIds as t (t)}<option value={t}>{t}</option>{/each}
			</select>
		</label>
		<label>
			<span class="tiny muted">type</span>
			<select bind:value={type}>
				<option value="all">all</option>
				{#each types as t (t)}<option value={t}>{typeLabels[t]}</option>{/each}
			</select>
		</label>
		<label>
			<span class="tiny muted">level</span>
			<select bind:value={level}>
				<option value="all">all</option>
				<option value="intro">intro</option>
				<option value="core">core</option>
				<option value="deep">deep</option>
			</select>
		</label>
		<label>
			<span class="tiny muted">concept</span>
			<select bind:value={concept}>
				<option value="all">all</option>
				{#each concepts as c (c)}<option value={c}>{c}</option>{/each}
			</select>
		</label>
		<label class="check">
			<input type="checkbox" bind:checked={onlyCurrent} />
			<span class="tiny">only my current stages ({currentStages.shell} / {currentStages.kafka})</span>
		</label>
		<button class="btn btn-sm btn-ghost" onclick={reset}>reset</button>
	</div>

	<p class="count tiny muted">
		{filtered.length} of {resources.length} resources · ordered by the stage that first needs them
	</p>

	{#each grouped as [groupName, list] (groupName)}
		<section>
			<h2>{groupName}</h2>
			<ul class="cards">
				{#each list as r (r.id)}
					<li class="res card" class:current={isCurrent(r)}>
						<a class="title" href={r.url} target="_blank" rel="noopener noreferrer">
							<span class="g" aria-hidden="true">{typeGlyphs[r.type]}</span>
							<span>{r.title}</span>
							<span class="ext" aria-hidden="true">↗</span>
						</a>
						<p class="why">{r.why}</p>
						<div class="meta tiny">
							<span class="chip">{typeLabels[r.type]}</span>
							<span class="chip">{r.level}</span>
							{#if r.minutes}<span class="chip">{r.minutes} min</span>{/if}
							{#if !r.free}<span class="chip chip-ext">paid</span>{/if}
							{#if isCurrent(r)}<span class="chip chip-ok">for your current stage</span>{/if}
						</div>
						{#if r.stages.length}
							<div class="stages tiny">
								<span class="muted">stages</span>
								{#each r.stages.slice(0, 12) as s, si (si)}
									{@const href = stageHref(r, s)}
									{#if href}
										<a {href}>{s}</a>
									{:else}
										<span class="nostage" title="Not a stage the tester publishes">{s}</span>
									{/if}
								{/each}
								{#if r.stages.length > 12}<span class="muted">+{r.stages.length - 12}</span>{/if}
							</div>
						{/if}
						{#if r.concepts.length}
							<div class="concepts tiny muted">{r.concepts.join(' · ')}</div>
						{/if}
					</li>
				{/each}
			</ul>
		</section>
	{/each}

	{#if filtered.length === 0}
		<p class="empty muted">Nothing matches those filters.</p>
	{/if}
</div>

<style>
	.page {
		padding: var(--s-6) 0 var(--s-7);
		display: flex;
		flex-direction: column;
		gap: var(--s-4);
	}
	header {
		max-width: 62ch;
	}
	h1 {
		margin: var(--s-2) 0;
	}
	.lede {
		color: var(--ink-2);
	}
	.filters {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--s-3);
		padding: var(--s-3) var(--s-4);
	}
	.filters label {
		display: flex;
		align-items: center;
		gap: 6px;
	}
	.search {
		flex: 1;
		min-width: 220px;
	}
	.search input {
		width: 100%;
		background: var(--bg-sunk);
		border: 1px solid var(--line);
		border-radius: var(--r-2);
		padding: 7px 11px;
	}
	select {
		background: var(--bg-3);
		border: 1px solid var(--line);
		border-radius: var(--r-1);
		padding: 4px 7px;
		font-size: 0.82rem;
	}
	.count {
		margin: 0;
	}
	h2 {
		font-size: 1.15rem;
		margin-bottom: var(--s-3);
	}
	.cards {
		list-style: none;
		margin: 0;
		padding: 0;
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
		gap: var(--s-3);
	}
	.res {
		padding: var(--s-4);
		display: flex;
		flex-direction: column;
		gap: var(--s-2);
	}
	.res.current {
		border-color: color-mix(in srgb, var(--ok) 45%, var(--line));
	}
	.title {
		display: flex;
		gap: 7px;
		align-items: baseline;
		text-decoration: none;
		font-weight: 600;
		font-family: var(--font-display);
		font-size: 1.02rem;
		line-height: 1.25;
		min-width: 0;
	}
	/* Some titles are file paths ("kafka/clients/src/main/.../MetadataRequest.json"); let
	   them break rather than push the card out of the grid. */
	.title span:nth-child(2) {
		min-width: 0;
		overflow-wrap: anywhere;
	}
	.title:hover span:nth-child(2) {
		text-decoration: underline;
		text-underline-offset: 3px;
	}
	.g {
		color: var(--shell);
		flex: none;
	}
	.ext {
		color: var(--ink-3);
		margin-left: auto;
		flex: none;
	}
	.why {
		margin: 0;
		color: var(--ink-2);
		font-size: 0.88rem;
	}
	.meta,
	.stages,
	.concepts {
		display: flex;
		flex-wrap: wrap;
		gap: 5px;
		align-items: center;
	}
	.stages a {
		font-family: var(--font-mono);
		font-size: 0.7rem;
		padding: 1px 5px;
		border-radius: var(--r-1);
		background: var(--bg-3);
		text-decoration: none;
		color: var(--ink-2);
	}
	.stages a:hover {
		background: var(--shell);
		color: var(--bg-2);
	}
	.nostage {
		font-family: var(--font-mono);
		font-size: 0.7rem;
		padding: 1px 5px;
		border-radius: var(--r-1);
		border: 1px dashed var(--line-strong);
		color: var(--ink-3);
	}
	.concepts {
		margin-top: auto;
	}
	.empty {
		padding: var(--s-7) 0;
		text-align: center;
	}
</style>
