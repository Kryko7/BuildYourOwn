<script lang="ts">
	import StageContent from './StageContent.svelte';
	import { neighbours } from '$lib/catalog';
	import type { Catalog, Report, StageSpec, TrackId } from '$lib/types';

	let {
		track,
		stage,
		catalog,
		report,
		onclose,
		onnavigate,
		oncomplete
	}: {
		track: TrackId;
		stage: StageSpec | null;
		catalog: Catalog;
		report: Report | null;
		onclose: () => void;
		onnavigate: (n: number) => void;
		oncomplete?: () => void;
	} = $props();

	let panel: HTMLElement | undefined = $state();
	let drawer: HTMLElement | undefined = $state();
	let returnFocusTo: HTMLElement | null = null;
	const near = $derived(stage ? neighbours(track, stage.number) : { prev: undefined, next: undefined });
	const accent = $derived(track === 'shell' ? 'var(--shell)' : 'var(--kafka)');

	const FOCUSABLE =
		'a[href], button:not([disabled]), input, textarea, select, [tabindex]:not([tabindex="-1"])';

	function onKeydown(e: KeyboardEvent) {
		if (!stage) return;
		if (e.key === 'Escape') {
			e.preventDefault();
			onclose();
			return;
		}
		// The drawer covers the map, so Tab must stay inside it until it is closed.
		if (e.key === 'Tab' && drawer) {
			const items = [...drawer.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
				(el) => el.offsetParent !== null || el === document.activeElement
			);
			if (items.length === 0) return;
			const first = items[0];
			const last = items[items.length - 1];
			const active = document.activeElement as HTMLElement | null;
			if (!drawer.contains(active)) {
				e.preventDefault();
				(e.shiftKey ? last : first).focus();
			} else if (e.shiftKey && active === first) {
				e.preventDefault();
				last.focus();
			} else if (!e.shiftKey && active === last) {
				e.preventDefault();
				first.focus();
			}
		}
	}

	// Opening scrolls the panel back to the top and moves focus into it; closing hands focus
	// back to whatever opened it (the waypoint you pressed Enter on, usually).
	let shownFor: number | null = null;
	$effect(() => {
		const number = stage?.number ?? null;
		if (number === shownFor) return;
		const opening = shownFor === null && number !== null;
		shownFor = number;
		if (number === null) {
			returnFocusTo?.focus();
			returnFocusTo = null;
			return;
		}
		if (opening) returnFocusTo = document.activeElement as HTMLElement | null;
		if (panel) panel.scrollTop = 0;
		queueMicrotask(() => drawer?.focus());
	});
</script>

<svelte:window onkeydown={onKeydown} />

{#if stage}
	<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
	<div class="scrim" onclick={onclose}></div>
	<div
		class="drawer"
		bind:this={drawer}
		style="--accent:{accent}"
		role="dialog"
		aria-modal="true"
		aria-label="Stage {stage.number}: {stage.name}"
		tabindex="-1"
	>
		<div class="bar">
			<div class="nav">
				<button
					class="btn btn-sm"
					disabled={!near.prev}
					onclick={() => near.prev && onnavigate(near.prev.number)}
					aria-label="Previous stage">←</button
				>
				<button
					class="btn btn-sm"
					disabled={!near.next}
					onclick={() => near.next && onnavigate(near.next.number)}
					aria-label="Next stage">→</button
				>
				<a class="btn btn-sm btn-ghost" href="/{track}/{stage.number}">permalink</a>
			</div>
			<button class="btn btn-sm btn-ghost" onclick={onclose} aria-label="Close">✕</button>
		</div>
		<div class="scroll" bind:this={panel}>
			<StageContent {track} {stage} {catalog} {report} {oncomplete} />
		</div>
	</div>
{/if}

<style>
	.scrim {
		position: fixed;
		inset: 0;
		background: color-mix(in srgb, var(--bg-sunk) 55%, transparent);
		z-index: 70;
		animation: fade 200ms var(--ease);
	}
	@keyframes fade {
		from {
			opacity: 0;
		}
	}
	.drawer:focus {
		outline: none;
	}
	.drawer {
		position: fixed;
		top: 0;
		right: 0;
		bottom: 0;
		z-index: 71;
		width: min(100%, 640px);
		background: var(--bg-2);
		border-left: 1px solid var(--line);
		box-shadow: var(--shadow-2);
		display: flex;
		flex-direction: column;
		animation: slide 260ms var(--ease);
	}
	@keyframes slide {
		from {
			transform: translateX(24px);
			opacity: 0;
		}
	}
	.bar {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--s-3);
		padding: var(--s-2) var(--s-3);
		border-bottom: 1px solid var(--line);
		flex: none;
	}
	.nav {
		display: flex;
		gap: 5px;
	}
	.scroll {
		overflow-y: auto;
		padding: var(--s-5);
		flex: 1;
	}
	@media (max-width: 560px) {
		.scroll {
			padding: var(--s-4) var(--s-3);
		}
	}
</style>
