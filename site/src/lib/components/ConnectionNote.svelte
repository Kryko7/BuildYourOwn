<script lang="ts">
	/**
	 * The one place the site admits where its numbers come from (PLAN.md §4.5).
	 *
	 * Silent while everything is normal — byo connected, a project registered — and
	 * otherwise says exactly which of the three things is true and what to type to fix it.
	 */
	import CopyButton from './CopyButton.svelte';
	import { journey } from '$lib/stores/journey.svelte';
	import { tracks } from '$lib/catalog';
	import type { TrackId } from '$lib/types';

	let { track = null, compact = false }: { track?: TrackId | null; compact?: boolean } = $props();

	const initCommand = $derived(track ? `byo init ${track}` : 'byo init shell');
	const needsInit = $derived(track !== null && journey.needsInit(track));
</script>

{#if journey.status === 'offline'}
	<aside class="note off" class:compact>
		<span class="icon" aria-hidden="true">🌙</span>
		<div>
			<strong>Not connected to byo.</strong>
			Run <code>byo site</code> in a terminal to see your real progress — stage states, notes and run
			history all live in byo’s database. Until then this page keeps a copy in this browser only, and
			nothing you tick here will show up in <code>byo status</code>.
		</div>
		<CopyButton text="byo site" label="copy" />
	</aside>
{:else if journey.status === 'reconnecting'}
	<aside class="note warn" class:compact>
		<span class="icon spin" aria-hidden="true">🌀</span>
		<div>
			<strong>byo stopped answering.</strong>
			Showing the last thing it sent{journey.lastSyncAt
				? `, from ${new Date(journey.lastSyncAt).toLocaleTimeString()}`
				: ''}. Reconnecting every couple of seconds — if you stopped <code>byo site</code>, start it
			again.
		</div>
	</aside>
{:else if needsInit && track}
	<aside class="note init" class:compact>
		<span class="icon" aria-hidden="true">🌱</span>
		<div>
			<strong>No {tracks[track].tester} project yet.</strong>
			byo is running, but nothing has been registered for this track. In the repo where you are writing
			your own {track === 'shell' ? 'shell' : 'broker'}, run <code>{initCommand}</code>, then
			<code>byo test --stage 1</code>.
		</div>
		<CopyButton text={initCommand} label="copy" />
	</aside>
{/if}

<style>
	.note {
		display: flex;
		align-items: flex-start;
		gap: var(--s-3);
		border-radius: var(--r-3);
		padding: var(--s-3) var(--s-4);
		font-size: 0.88rem;
		line-height: 1.55;
		border: 1px solid transparent;
	}
	.note div {
		flex: 1;
		min-width: 0;
	}
	.note.compact {
		padding: var(--s-2) var(--s-3);
		font-size: 0.82rem;
	}
	.icon {
		font-size: 1.15rem;
		line-height: 1.3;
		flex: none;
	}
	.off {
		background: var(--lav-soft);
		border-color: color-mix(in srgb, var(--lav) 45%, transparent);
		color: var(--ink-2);
	}
	.off strong {
		color: var(--lav-ink);
	}
	.warn {
		background: var(--warn-soft);
		border-color: color-mix(in srgb, var(--warn) 40%, transparent);
		color: var(--ink-2);
	}
	.warn strong {
		color: var(--warn);
	}
	.init {
		background: var(--mint-soft);
		border-color: color-mix(in srgb, var(--mint) 50%, transparent);
		color: var(--ink-2);
	}
	.init strong {
		color: var(--ok);
	}
	.spin {
		display: inline-block;
		animation: spin 2.4s linear infinite;
	}
	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
</style>
