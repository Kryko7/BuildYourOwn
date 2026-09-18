<script lang="ts">
	import { page } from '$app/state';
	import Cat from '$lib/components/garden/Cat.svelte';
	import { allTracks } from '$lib/tracks';
</script>

<div class="wrap box">
	<span class="lost" aria-hidden="true"><Cat size={120} /></span>
	<div>
		<p class="eyebrow">{page.status}</p>
		<h1>Off the trail</h1>
		<p class="muted">{page.error?.message ?? 'Nothing grows at that address.'}</p>
		<div class="row">
			<a class="btn btn-primary" href="/">Back to the garden</a>
			{#each allTracks as t (t.id)}
				<a class="btn" href="/{t.id}" style="--accent:{t.accent}">{t.short}</a>
			{/each}
		</div>
	</div>
</div>

<style>
	.box {
		padding: var(--s-8) 0;
		display: flex;
		align-items: center;
		gap: var(--s-5);
		max-width: 74ch;
	}
	.box > div {
		min-width: 0;
	}
	.lost {
		flex: none;
		animation: driftY 5.6s ease-in-out infinite;
	}
	h1 {
		margin: var(--s-2) 0 var(--s-3);
	}
	.row {
		margin-top: var(--s-5);
		flex-wrap: wrap;
	}
	@media (max-width: 600px) {
		.lost {
			display: none;
		}
	}
</style>
