<script lang="ts">
	import StageContent from '$lib/components/StageContent.svelte';
	import Petals from '$lib/components/garden/Petals.svelte';
	import ConnectionNote from '$lib/components/ConnectionNote.svelte';
	import { tracks, badgeFor, sectionOf } from '$lib/catalog';
	import { progress } from '$lib/stores/progress.svelte';
	import { activeReport } from '$lib/stores/reports.svelte';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';

	let { data } = $props();

	const track = $derived(data.track);
	const stage = $derived(data.stage);
	const meta = $derived(tracks[track]);
	const accent = $derived(meta.accent);
	const report = $derived(activeReport(track));
	const section = $derived(sectionOf(track, stage.number));
	const badge = $derived(section ? badgeFor(track, section.id) : null);

	let burst = $state(0);

	onMount(() => {
		progress.touch(track, stage.number);
	});

	function onKeydown(e: KeyboardEvent) {
		const t = e.target as HTMLElement | null;
		if (t && (/^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName) || t.isContentEditable)) return;
		if (e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
		// Client-side navigation, so the walk along the trail keeps its scroll and its state.
		if (e.key === 'ArrowLeft' && data.near.prev) {
			e.preventDefault();
			void goto(`/${track}/${data.near.prev.number}`);
		}
		if (e.key === 'ArrowRight' && data.near.next) {
			e.preventDefault();
			void goto(`/${track}/${data.near.next.number}`);
		}
	}
</script>

<svelte:head>
	<title>Stage {stage.number}: {stage.name} — {meta.title}</title>
	<meta name="description" content={stage.hints[0] ?? stage.name} />
</svelte:head>

<svelte:window onkeydown={onKeydown} />
<Petals {burst} {accent} />

<div class="wrap page" style="--accent:{accent}">
	<nav class="crumb tiny" aria-label="Breadcrumb">
		<a href="/">Journey</a>
		<span aria-hidden="true">›</span>
		<a href="/{track}">{meta.title}</a>
		{#if badge}<span aria-hidden="true">›</span><span class="muted">{badge.icon} {badge.badge}</span>{/if}
	</nav>

	<ConnectionNote {track} compact />

	<div class="grid">
		<div class="content card">
			<StageContent
				{track}
				{stage}
				catalog={data.catalog}
				{report}
				examples={data.examples}
				oncomplete={() => {
					if (progress.data.settings.petals) burst++;
				}}
			/>
		</div>

		<aside class="side">
			<div class="card pad">
				<h3 class="eyebrow">move along the trail</h3>
				<div class="prevnext">
					{#if data.near.prev}
						<a class="pn" href="/{track}/{data.near.prev.number}">
							<span class="tiny muted">← stage {data.near.prev.number}</span>
							<strong>{data.near.prev.name}</strong>
						</a>
					{/if}
					{#if data.near.next}
						<a class="pn next" href="/{track}/{data.near.next.number}">
							<span class="tiny muted">stage {data.near.next.number} →</span>
							<strong>{data.near.next.name}</strong>
						</a>
					{/if}
				</div>
				<p class="tiny muted kb">
					<kbd>←</kbd> <kbd>→</kbd> walk between stages
				</p>
			</div>

			<div class="card pad">
				<h3 class="eyebrow">this section</h3>
				<ul class="sib">
					{#each data.catalog.stages.filter((s) => section?.stages.includes(s.number)) as s (s.number)}
						<li
							class:here={s.number === stage.number}
							class:done={progress.isDone(track, s.number)}
							class:planned={s.planned}
						>
							<a href="/{track}/{s.number}">
								<span class="mono tiny">{String(s.number).padStart(2, '0')}</span>
								{s.name}
							</a>
						</li>
					{/each}
				</ul>
			</div>

			<a class="btn" href="/{track}">← back to the map</a>
		</aside>
	</div>
</div>

<style>
	.page {
		padding: var(--s-5) 0 var(--s-7);
		display: flex;
		flex-direction: column;
		gap: var(--s-4);
	}
	.crumb {
		display: flex;
		gap: 7px;
		align-items: center;
		color: var(--ink-2);
	}
	.grid {
		display: grid;
		grid-template-columns: minmax(0, 1fr) 280px;
		gap: var(--s-4);
		align-items: start;
	}
	@media (max-width: 900px) {
		.grid {
			grid-template-columns: 1fr;
		}
		.side {
			order: -1;
		}
	}
	.content {
		padding: var(--s-6);
	}
	@media (max-width: 560px) {
		.content {
			padding: var(--s-4) var(--s-3);
		}
	}
	.side {
		display: flex;
		flex-direction: column;
		gap: var(--s-3);
		position: sticky;
		top: 74px;
	}
	.pad {
		padding: var(--s-4);
	}
	.prevnext {
		display: flex;
		flex-direction: column;
		gap: var(--s-2);
		margin-top: var(--s-2);
	}
	.pn {
		display: flex;
		flex-direction: column;
		text-decoration: none;
		padding: var(--s-2) var(--s-3);
		border: 1px solid var(--line);
		border-radius: var(--r-2);
		transition: border-color var(--dur) var(--ease), background var(--dur) var(--ease);
	}
	.pn:hover {
		border-color: var(--accent);
		background: color-mix(in srgb, var(--accent) 7%, transparent);
	}
	.pn strong {
		font-weight: 500;
		font-size: 0.88rem;
	}
	.pn.next {
		text-align: right;
	}
	.kb {
		margin: var(--s-2) 0 0;
	}
	.sib {
		list-style: none;
		margin: var(--s-2) 0 0;
		padding: 0;
		max-height: 320px;
		overflow-y: auto;
	}
	.sib li a {
		display: flex;
		gap: 7px;
		align-items: baseline;
		padding: 3px 5px;
		border-radius: var(--r-1);
		text-decoration: none;
		font-size: 0.84rem;
	}
	.sib li a:hover {
		background: var(--bg-3);
	}
	.sib li.here a {
		background: color-mix(in srgb, var(--accent) 14%, transparent);
		font-weight: 600;
	}
	.sib li.done a {
		color: var(--ink-2);
	}
	.sib li .mono {
		color: var(--ink-3);
	}
	.sib li.planned a {
		font-style: italic;
		color: var(--ink-3);
	}
</style>
