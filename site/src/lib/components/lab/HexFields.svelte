<script lang="ts">
	/**
	 * The instrument every lab decoder is read through: a hex dump on one side, the decoded
	 * fields on the other, wired both ways. Hover a byte to select its field; hover a field
	 * to light its bytes. Indentation comes from each field's `depth`, so a nested structure
	 * reads as an outline without the list becoming a tree widget.
	 *
	 * It knows nothing about any protocol — Kafka frames, wasm modules, TLS records and ELF
	 * objects all arrive here as `Field[]`.
	 */
	import { printable } from '$lib/lab/kafka';
	import type { Field } from '$lib/lab/field';

	let {
		bytes,
		fields,
		columns = 16,
		active = $bindable(null),
		emptyText = 'No bytes to show.',
		maxHeight = '420px'
	}: {
		bytes: Uint8Array;
		fields: Field[];
		columns?: number;
		/** Index into `fields`, two-way so a parent can drive the selection. */
		active?: number | null;
		emptyText?: string;
		maxHeight?: string;
	} = $props();

	const rows = $derived.by(() => {
		const out: { offset: number; slice: number[] }[] = [];
		for (let i = 0; i < bytes.length; i += columns)
			out.push({ offset: i, slice: [...bytes.subarray(i, i + columns)] });
		return out;
	});

	/** The innermost field covering a byte — the deepest one wins, so leaves beat containers. */
	function fieldAt(index: number): number {
		let best = -1;
		let bestDepth = -1;
		for (let i = 0; i < fields.length; i++) {
			const f = fields[i];
			if (index < f.start || index >= f.end) continue;
			if (f.depth >= bestDepth) {
				best = i;
				bestDepth = f.depth;
			}
		}
		return best;
	}

	const activeRange = $derived(active !== null && fields[active] ? fields[active] : null);

	function inActive(i: number) {
		return activeRange ? i >= activeRange.start && i < activeRange.end : false;
	}

	function pick(byteIndex: number) {
		const f = fieldAt(byteIndex);
		if (f >= 0) active = f;
	}
</script>

<div class="split">
	<div class="hex" role="group" aria-label="Hex dump" style="max-height:{maxHeight}">
		{#each rows as row (row.offset)}
			<div class="hexrow">
				<span class="off">{row.offset.toString(16).padStart(4, '0')}</span>
				<span class="cells">
					{#each row.slice as b, i (i)}
						{@const idx = row.offset + i}
						<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
						<span
							class="b"
							class:hot={inActive(idx)}
							class:known={fieldAt(idx) >= 0}
							onmouseenter={() => pick(idx)}
							onclick={() => pick(idx)}>{b.toString(16).padStart(2, '0')}</span
						>
					{/each}
				</span>
				<span class="ascii">{row.slice.map(printable).join('')}</span>
			</div>
		{/each}
		{#if rows.length === 0}<p class="tiny muted">{emptyText}</p>{/if}
	</div>

	<ol class="fields" aria-label="Decoded fields" style="max-height:{maxHeight}">
		{#each fields as f, i (i)}
			<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
			<li
				class:on={active === i}
				style="--depth:{f.depth}"
				onmouseenter={() => (active = i)}
				onfocusin={() => (active = i)}
			>
				<button class="frow" onclick={() => (active = i)}>
					<span class="fname mono">{f.name}</span>
					<span class="ftype tiny muted">{f.type}</span>
					<span class="fval mono">{f.value}</span>
					<span class="fbytes tiny muted">@{f.start}..{f.end}</span>
				</button>
				{#if f.note && active === i}<p class="note tiny">{f.note}</p>{/if}
			</li>
		{:else}
			<li class="empty tiny muted">Nothing decoded yet.</li>
		{/each}
	</ol>
</div>

<style>
	.split {
		display: grid;
		grid-template-columns: minmax(0, 0.95fr) minmax(0, 1.05fr);
		gap: var(--s-4);
		align-items: start;
	}
	@media (max-width: 860px) {
		.split {
			grid-template-columns: 1fr;
		}
	}
	.hex {
		background: var(--bg-sunk);
		border: 1px solid var(--line);
		border-radius: var(--r-2);
		padding: var(--s-3);
		font-family: var(--font-mono);
		font-size: 0.74rem;
		line-height: 1.85;
		overflow-x: auto;
		overflow-y: auto;
	}
	.hexrow {
		display: flex;
		gap: var(--s-3);
		white-space: nowrap;
	}
	.off {
		color: var(--ink-3);
		user-select: none;
	}
	.cells {
		display: inline-flex;
		gap: 4px;
	}
	.b {
		border-radius: 3px;
		padding: 0 2px;
		cursor: pointer;
		transition:
			background 120ms var(--ease),
			color 120ms var(--ease);
	}
	.b.known {
		color: var(--ink);
	}
	.b:not(.known) {
		color: var(--ink-3);
	}
	.b.hot {
		background: var(--accent, var(--ink));
		color: var(--bg-2);
	}
	.ascii {
		color: var(--ink-3);
		letter-spacing: 0.06em;
	}
	.fields {
		list-style: none;
		margin: 0;
		padding: 0;
		border: 1px solid var(--line);
		border-radius: var(--r-2);
		overflow-y: auto;
	}
	.fields li {
		border-top: 1px solid var(--line);
		padding-left: calc(var(--depth) * 12px);
	}
	.fields li:first-child {
		border-top: 0;
	}
	.fields li.on {
		background: color-mix(in srgb, var(--accent, var(--ink)) 10%, transparent);
	}
	.fields li.empty {
		padding: var(--s-3);
	}
	.frow {
		display: grid;
		grid-template-columns: minmax(0, 1.35fr) auto minmax(0, 1fr) auto;
		align-items: baseline;
		gap: var(--s-2);
		width: 100%;
		background: none;
		border: 0;
		text-align: left;
		padding: 4px var(--s-3);
		cursor: pointer;
		font-size: 0.76rem;
	}
	.fname {
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.fval {
		color: var(--accent, var(--ink));
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.note {
		margin: 0;
		padding: 0 var(--s-3) 7px var(--s-3);
		color: var(--ink-2);
	}
	@media (max-width: 560px) {
		.frow {
			grid-template-columns: 1fr auto;
		}
		.ftype,
		.fbytes {
			display: none;
		}
	}
</style>
