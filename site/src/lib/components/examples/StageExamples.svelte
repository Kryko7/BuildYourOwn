<script lang="ts">
	/**
	 * "What to expect" for one stage (PLAN.md §4.2) — a terminal transcript on the shell
	 * track, annotated bytes on every other. Which of the two a track uses is one field in
	 * the registry (`tracks[track].examples`), so a new track needs no change here.
	 *
	 * Byte examples are a lazy chunk (one file per stage per track), so they arrive a moment
	 * after the rest of the page — except on `/<track>/<n>`, where the route's `load` has
	 * already fetched them and passes them in as `preloaded`, and they are server-rendered.
	 */
	import ShellTranscript from './ShellTranscript.svelte';
	import ByteExampleView from './ByteExample.svelte';
	import { shellExamples } from '$lib/examples/shell';
	import { inlineExamples, loadExamples, normalizeByteExamples } from '$lib/examples/bytes';
	import { tracks } from '$lib/tracks';
	import type { ByteExample, StageSpec, TrackId } from '$lib/types';

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

	const kind = $derived(tracks[track].examples);
	const shell = $derived(kind === 'transcript' ? shellExamples(stage, limit) : []);

	let fetched = $state<ByteExample[]>([]);
	let loading = $state(false);

	/** Anything already in hand — the route's preload, or examples carried on the stage. */
	const immediate = $derived(
		kind !== 'bytes' ? [] : preloaded ? normalizeByteExamples(preloaded) : inlineExamples(stage)
	);

	const byteExamples = $derived((immediate.length ? immediate : fetched).slice(0, limit));

	$effect(() => {
		if (kind !== 'bytes' || immediate.length) return;
		const n = stage.number;
		let cancelled = false;
		loading = true;
		void loadExamples(track, stage).then((list) => {
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
{:else if byteExamples.length}
	<div class="examples">
		<p class="tiny muted lead">
			Real bytes, byte for byte. Hover a field to find its bytes, or a byte to find its field.
		</p>
		{#each byteExamples as ex, i (i)}
			<ByteExampleView example={ex} />
		{/each}
	</div>
{:else if kind === 'bytes' && loading}
	<p class="tiny muted lead">Fetching this stage’s bytes…</p>
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
