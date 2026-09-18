<script lang="ts">
	import { goto } from '$app/navigation';
	import { catalogs, trackIds, tracks } from '$lib/catalog';
	import { allResources } from '$lib/resources';

	let open = $state(false);
	let query = $state('');
	let cursor = $state(0);
	let input: HTMLInputElement | undefined = $state();

	interface Item {
		id: string;
		label: string;
		hint: string;
		group: string;
		href: string;
		external?: boolean;
	}

	const items: Item[] = [
		{ id: 'p-home', label: 'Home', hint: 'Both tracks, XP and streak', group: 'Pages', href: '/' },
		{
			id: 'p-shell',
			label: 'Shell garden trail',
			hint: `${catalogs.shell.totals.stages} stages`,
			group: 'Pages',
			href: '/shell'
		},
		{
			id: 'p-kafka',
			label: 'Kafka garden trail',
			hint: `${catalogs.kafka.totals.stages} stages`,
			group: 'Pages',
			href: '/kafka'
		},
		{ id: 'p-res', label: 'Resources', hint: 'Curated reading', group: 'Pages', href: '/resources' },
		{ id: 'p-lab', label: 'Lab', hint: 'Tokenizer, wire inspector, batches, replay', group: 'Pages', href: '/lab' },
		{
			id: 'p-prog',
			label: 'Runs',
			hint: 'Run history from byo’s database',
			group: 'Pages',
			href: '/progress'
		},
		...trackIds.flatMap((track) =>
			catalogs[track].stages.map((s) => ({
				id: `${track}-${s.number}`,
				label: `Stage ${String(s.number).padStart(2, '0')} — ${s.name}`,
				hint: `${tracks[track].tester}${s.ext ? ' · ext' : ''}${s.planned ? ' · planned' : ''}`,
				group: track === 'shell' ? 'Shell stages' : 'Kafka stages',
				href: `/${track}/${s.number}`
			}))
		),
		...allResources().map((r) => ({
			id: `r-${r.id}`,
			label: r.title,
			hint: `${r.type} · ${r.level}`,
			group: 'Resources',
			href: r.url,
			external: true
		}))
	];

	function score(item: Item, q: string): number {
		if (!q) return 1;
		const hay = `${item.label} ${item.hint} ${item.group}`.toLowerCase();
		const needle = q.toLowerCase();
		if (hay.includes(needle)) return 100 - hay.indexOf(needle);
		// subsequence match, so "s24" finds "Stage 24"
		let i = 0;
		for (const ch of needle) {
			i = hay.indexOf(ch, i);
			if (i < 0) return 0;
			i++;
		}
		return 1;
	}

	/**
	 * Best matches first, but each group is shown once, as a block: a list that flips between
	 * "Resources", "Kafka stages", "Resources" again is noise, not ranking.
	 */
	const results = $derived.by(() => {
		const scored = items
			.map((item) => ({ item, s: score(item, query.trim()) }))
			.filter((r) => r.s > 0)
			.sort((a, b) => b.s - a.s)
			.slice(0, 40);
		const groups = new Map<string, Item[]>();
		for (const { item } of scored) {
			if (!groups.has(item.group)) groups.set(item.group, []);
			groups.get(item.group)!.push(item);
		}
		return [...groups.values()].flat();
	});

	export function show() {
		open = true;
		query = '';
		cursor = 0;
		queueMicrotask(() => input?.focus());
	}

	function hide() {
		open = false;
	}

	function choose(item: Item) {
		hide();
		if (item.external) window.open(item.href, '_blank', 'noopener');
		else void goto(item.href);
	}

	function onKeydown(e: KeyboardEvent) {
		if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') {
			e.preventDefault();
			open ? hide() : show();
			return;
		}
		if (!open) return;
		if (e.key === 'Escape') {
			e.preventDefault();
			hide();
		} else if (e.key === 'ArrowDown') {
			e.preventDefault();
			cursor = Math.min(cursor + 1, results.length - 1);
		} else if (e.key === 'ArrowUp') {
			e.preventDefault();
			cursor = Math.max(cursor - 1, 0);
		} else if (e.key === 'Enter') {
			e.preventDefault();
			const item = results[cursor];
			if (item) choose(item);
		}
	}

	$effect(() => {
		query;
		cursor = 0;
	});
</script>

<svelte:window onkeydown={onKeydown} />

{#if open}
	<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
	<div class="scrim" onclick={hide}></div>
	<div class="palette card" role="dialog" aria-modal="true" aria-label="Command palette">
		<div class="head">
			<span aria-hidden="true">⌕</span>
			<input
				bind:this={input}
				bind:value={query}
				placeholder="Jump to a stage, a page or a resource…"
				aria-label="Search"
				spellcheck="false"
				autocomplete="off"
			/>
			<kbd>esc</kbd>
		</div>
		<ul role="listbox" aria-label="Results">
			{#each results as item, i (item.id)}
				{@const first = i === 0 || results[i - 1].group !== item.group}
				{#if first}
					<li class="grouphead eyebrow" role="presentation">{item.group}</li>
				{/if}
				<li role="option" aria-selected={i === cursor}>
					<button class:active={i === cursor} onclick={() => choose(item)} onmouseenter={() => (cursor = i)}>
						<span class="lab">{item.label}</span>
						<span class="hint tiny muted">{item.hint}{item.external ? ' ↗' : ''}</span>
					</button>
				</li>
			{:else}
				<li class="empty muted">Nothing matches “{query}”.</li>
			{/each}
		</ul>
	</div>
{/if}

<style>
	.scrim {
		position: fixed;
		inset: 0;
		background: color-mix(in srgb, var(--bg-sunk) 70%, transparent);
		backdrop-filter: blur(2px);
		z-index: 100;
	}
	.palette {
		position: fixed;
		z-index: 101;
		top: 12vh;
		left: 50%;
		transform: translateX(-50%);
		width: min(100% - 2rem, 620px);
		max-height: 70vh;
		display: flex;
		flex-direction: column;
		overflow: hidden;
		box-shadow: var(--shadow-2);
		animation: pop 180ms cubic-bezier(0.22, 0.61, 0.36, 1);
	}
	@keyframes pop {
		from {
			opacity: 0;
			transform: translateX(-50%) translateY(-8px) scale(0.985);
		}
	}
	.head {
		display: flex;
		align-items: center;
		gap: var(--s-3);
		padding: var(--s-3) var(--s-4);
		border-bottom: 1px solid var(--line);
		color: var(--ink-3);
	}
	input {
		flex: 1;
		border: 0;
		background: transparent;
		outline: none;
		font-size: 1rem;
		color: var(--ink);
	}
	ul {
		list-style: none;
		margin: 0;
		padding: var(--s-2);
		overflow-y: auto;
	}
	.grouphead {
		padding: var(--s-3) var(--s-3) var(--s-1);
	}
	li button {
		display: flex;
		width: 100%;
		align-items: baseline;
		justify-content: space-between;
		gap: var(--s-3);
		background: transparent;
		border: 0;
		border-radius: var(--r-2);
		padding: 7px 11px;
		text-align: left;
		cursor: pointer;
	}
	li button.active {
		background: var(--bg-3);
	}
	.lab {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.hint {
		flex: none;
	}
	.empty {
		padding: var(--s-5);
		text-align: center;
	}
</style>
