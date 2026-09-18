<script lang="ts">
	import { pageTitle } from '$lib/branding';
	import { labTools, accentFor } from '$lib/components/lab/tools';
	import { tracks } from '$lib/tracks';

	let active = $state(labTools[0].id);
	const current = $derived(labTools.find((t) => t.id === active) ?? labTools[0]);
	const Instrument = $derived(current.component);
	const accent = $derived(accentFor(current));
</script>

<svelte:head>
	<title>{pageTitle('Lab')}</title>
</svelte:head>

<div class="wrap page" style="--accent:{accent}">
	<header>
		<p class="eyebrow">playgrounds</p>
		<h1>The lab</h1>
		<p class="lede">
			{labTools.length} instruments for the trails. Each one is also embedded in the stages it belongs
			to, so you can poke at the thing you are about to implement — and every one of them runs
			entirely in this page, with no network and nothing installed.
		</p>
	</header>

	<div class="tabs" role="tablist" aria-label="Playgrounds">
		{#each labTools as t (t.id)}
			<button
				role="tab"
				aria-selected={active === t.id}
				aria-controls="panel-{t.id}"
				id="tab-{t.id}"
				class:on={active === t.id}
				style="--accent:{accentFor(t)}"
				onclick={() => (active = t.id)}
			>
				{t.label}
			</button>
		{/each}
	</div>

	<p class="blurb">
		{current.blurb}
		<a href={current.stageHref}>
			{current.track ? `${tracks[current.track].short} stage →` : 'your runs →'}
		</a>
	</p>

	<div class="panel card" role="tabpanel" id="panel-{current.id}" aria-labelledby="tab-{current.id}">
		{#key current.id}
			<Instrument />
		{/key}
	</div>
</div>

<style>
	.page {
		padding: var(--s-6) 0 var(--s-7);
		display: flex;
		flex-direction: column;
		gap: var(--s-4);
	}
	header {
		max-width: 64ch;
	}
	h1 {
		margin: var(--s-2) 0;
	}
	.lede {
		color: var(--ink-2);
	}
	.tabs {
		display: flex;
		gap: 4px;
		flex-wrap: wrap;
		border-bottom: 1px solid var(--line);
		padding-bottom: 0;
	}
	.tabs button {
		background: none;
		border: 1px solid transparent;
		border-bottom: 0;
		border-radius: var(--r-2) var(--r-2) 0 0;
		padding: 8px 14px;
		cursor: pointer;
		color: var(--ink-2);
		font-size: 0.9rem;
		position: relative;
		top: 1px;
	}
	.tabs button:hover {
		color: var(--ink);
		background: var(--bg-3);
	}
	.tabs button.on {
		color: var(--ink);
		background: var(--bg-2);
		border-color: var(--line);
		box-shadow: inset 0 2px 0 var(--accent);
	}
	.blurb {
		margin: 0;
		color: var(--ink-2);
		max-width: 76ch;
		font-size: 0.92rem;
	}
	.panel {
		padding: var(--s-5);
	}
	@media (max-width: 560px) {
		.panel {
			padding: var(--s-3);
		}
	}
</style>
