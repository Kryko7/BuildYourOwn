<script lang="ts">
	/**
	 * The "Try it" box in a stage drawer: whichever lab instrument belongs to this track and
	 * this stage, opened on the sample that fits it. Renders nothing when the track has no
	 * instrument for the stage, so a track without one costs the page nothing.
	 */
	import { toolForStage } from './tools';
	import type { TrackId } from '$lib/types';

	let { track, stage }: { track: TrackId; stage: number } = $props();

	const chosen = $derived(toolForStage(track, stage));
</script>

{#if chosen}
	{@const Tool = chosen.tool.component}
	<section>
		<h3>Try it</h3>
		<div class="labbox">
			<Tool initialSample={chosen.sample || undefined} />
		</div>
	</section>
{/if}

<style>
	h3 {
		font-family: var(--font-mono);
		font-size: 0.72rem;
		text-transform: uppercase;
		letter-spacing: 0.14em;
		color: var(--ink-3);
		font-weight: 500;
		margin-bottom: var(--s-2);
	}
	.labbox {
		border: 1px solid var(--line);
		border-radius: var(--r-2);
		padding: var(--s-3);
		background: var(--bg-2);
	}
</style>
