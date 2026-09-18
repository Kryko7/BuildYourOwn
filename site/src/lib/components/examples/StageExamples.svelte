<script lang="ts">
	/**
	 * "What to expect" for one stage (PLAN.md §4.2) — a terminal transcript on the shell
	 * track, an annotated request/response pair on the Kafka track. Renders nothing at all
	 * when the catalog has nothing truthful to show, so a stage never gets a heading over
	 * an empty box.
	 *
	 * Kafka examples are a lazy chunk (one file per stage), so they arrive a moment after
	 * the rest of the page — except on `/kafka/<n>`, where the route's `load` has already
	 * fetched them and passes them in as `preloaded`, and they are server-rendered.
	 */
	import ShellTranscript from './ShellTranscript.svelte';
	import WireExample from './WireExample.svelte';
	import { shellExamples } from '$lib/examples/shell';
	import { kafkaExamples, loadKafkaExamples, normalizeKafkaExamples } from '$lib/examples/kafka';
	import type { KafkaExample, StageSpec, TrackId } from '$lib/types';

	let {
		track,
		stage,
		limit = 3,
		preloaded = null
	}: {
		track: TrackId;
		stage: StageSpec;
		limit?: number;
		/** Raw catalog entries the page already had; skips the lazy fetch entirely. */
		preloaded?: unknown[] | null;
	} = $props();

	const shell = $derived(track === 'shell' ? shellExamples(stage, limit) : []);

	let fetched = $state<KafkaExample[]>([]);
	let loading = $state(false);

	/** Anything already in hand — the route's preload, or examples carried on the stage. */
	const immediate = $derived(
		track !== 'kafka'
			? []
			: preloaded
				? normalizeKafkaExamples(preloaded)
				: kafkaExamples(stage)
	);

	const kafka = $derived(
		(immediate.length ? immediate : fetched).slice(0, limit)
	);

	$effect(() => {
		if (track !== 'kafka' || immediate.length) return;
		const n = stage.number;
		let cancelled = false;
		loading = true;
		void loadKafkaExamples(stage).then((list) => {
			if (cancelled || n !== stage.number) return;
			fetched = list;
			loading = false;
		});
		return () => {
			cancelled = true;
		};
	});
</script>

{#if shell.length}
	<div class="examples">
		<p class="tiny muted lead">
			Straight from the suite: this is what <code>{stage.file || 'the tester'}</code> sends and what it
			expects back.
		</p>
		{#each shell as ex, i (i)}
			<ShellTranscript example={ex} />
		{/each}
	</div>
{:else if kafka.length}
	<div class="examples">
		<p class="tiny muted lead">
			Real frames, byte for byte. Hover a field to find its bytes, or a byte to find its field.
		</p>
		{#each kafka as ex, i (i)}
			<WireExample example={ex} />
		{/each}
	</div>
{:else if track === 'kafka' && loading}
	<p class="tiny muted lead">Fetching this stage’s frames…</p>
{/if}

<style>
	.examples {
		display: flex;
		flex-direction: column;
		gap: var(--s-3);
	}
	.lead {
		margin: 0;
	}
</style>
