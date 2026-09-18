<script lang="ts">
	/**
	 * One Kafka example: the request and the response side by side, each with the wire
	 * inspector's annotated byte view. Hovering a field lights up its bytes and hovering a
	 * byte selects its field, in both directions, with the request and the response tracked
	 * independently.
	 */
	import { printable, toHex } from '$lib/lab/kafka';
	import { hasBytes } from '$lib/examples/kafka';
	import type { KafkaExample, KafkaExampleSide } from '$lib/types';

	let { example, copyable = true }: { example: KafkaExample; copyable?: boolean } = $props();

	// One selection per side: hovering the response must not un-highlight the request.
	let active = $state<{ request: number | null; response: number | null }>({
		request: null,
		response: null
	});

	const sides = $derived(
		[
			{ key: 'request' as const, label: 'request →', side: example.request },
			{ key: 'response' as const, label: '← response', side: example.response }
		].filter((s) => s.side.summary || hasBytes(s.side))
	);

	/** Only worth a chip when it is *not* the ordinary case of bytes going both ways. */
	const kindLabel = $derived(
		example.kind === 'silence'
			? 'nothing on the wire'
			: example.kind === 'closed'
				? 'connection closed'
				: example.kind === 'text'
					? 'no frame here'
					: null
	);

	/**
	 * A real ApiVersions response is the better part of a kilobyte, and drawing it is one
	 * DOM node per byte. Nobody reads 800 bytes of hex in a stage drawer, so the dump stops
	 * at 256 and says so; asking for a field further in opens the rest.
	 */
	const DUMP_LIMIT = 256;
	let expanded = $state<{ request: boolean; response: boolean }>({ request: false, response: false });

	function shown(side: KafkaExampleSide, key: 'request' | 'response') {
		const total = side.bytes?.length ?? 0;
		return expanded[key] ? total : Math.min(total, DUMP_LIMIT);
	}

	function rows(side: KafkaExampleSide, key: 'request' | 'response') {
		const bytes = side.bytes;
		if (!bytes) return [];
		const limit = shown(side, key);
		const out: { offset: number; slice: number[] }[] = [];
		for (let i = 0; i < limit; i += 8)
			out.push({ offset: i, slice: [...bytes.subarray(i, Math.min(i + 8, limit))] });
		return out;
	}

	function fieldAt(side: KafkaExampleSide, index: number) {
		return side.fields.findIndex((f) => index >= f.offset && index < f.offset + f.length);
	}

	/** Selecting a field whose bytes are past the cut reveals them rather than doing nothing. */
	function select(key: 'request' | 'response', side: KafkaExampleSide, index: number) {
		const f = side.fields[index];
		if (f && f.offset + f.length > DUMP_LIMIT) expanded = { ...expanded, [key]: true };
		active = { ...active, [key]: index };
	}

	function inActive(side: KafkaExampleSide, key: 'request' | 'response', index: number) {
		const i = active[key];
		if (i === null) return false;
		const f = side.fields[i];
		return Boolean(f) && index >= f.offset && index < f.offset + f.length;
	}

	function copy(side: KafkaExampleSide) {
		if (side.bytes) void navigator.clipboard?.writeText(toHex(side.bytes));
	}
</script>

<article class="wex card">
	<header>
		<span class="petal" aria-hidden="true">✿</span>
		<h4>{example.title}</h4>
		{#if kindLabel}<span class="chip">{kindLabel}</span>{/if}
	</header>

	{#if example.env}
		<p class="env tiny">
			<span class="eyebrow">set up with</span>
			{#each example.env.topics as t (t.key)}
				<span class="fixture">
					<code>{t.name}</code>{#if t.partitions}<span class="muted">
							· {t.partitions} partition{t.partitions === 1 ? '' : 's'}</span
						>{/if}{#if t.id}<span class="muted" title="topic id {t.id}"> · id {t.id.slice(0, 8)}…</span
						>{/if}
				</span>
			{/each}
			{#if example.env.group}<span class="fixture">group <code>{example.env.group}</code></span>{/if}
		</p>
	{/if}

	<div class="sides" class:one={sides.length === 1}>
		{#each sides as { key, label, side } (key)}
			<section class="side {key}">
				<p class="lbl">
					<span class="eyebrow">{label}</span>
					{#if copyable && hasBytes(side)}
						<button class="btn btn-sm btn-ghost" onclick={() => copy(side)}>copy hex</button>
					{/if}
				</p>
				{#if side.summary}<p class="summary">{side.summary}</p>{/if}

				{#if hasBytes(side)}
					<div class="hex" role="group" aria-label="{label} bytes">
						{#each rows(side, key) as row (row.offset)}
							<div class="hexrow">
								<span class="off">{row.offset.toString(16).padStart(4, '0')}</span>
								<span class="cells">
									{#each row.slice as b, i (i)}
										{@const idx = row.offset + i}
										<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
										<span
											class="b"
											class:hot={inActive(side, key, idx)}
											class:known={fieldAt(side, idx) >= 0}
											onmouseenter={() => {
												const f = fieldAt(side, idx);
												if (f >= 0) active = { ...active, [key]: f };
											}}
											onclick={() => {
												const f = fieldAt(side, idx);
												if (f >= 0) active = { ...active, [key]: f };
											}}>{b.toString(16).padStart(2, '0')}</span
										>
									{/each}
								</span>
								<span class="ascii">{row.slice.map(printable).join('')}</span>
							</div>
						{/each}
						{#if shown(side, key) < (side.bytes?.length ?? 0)}
							<button
								class="btn btn-sm more"
								onclick={() => (expanded = { ...expanded, [key]: true })}
							>
								show all {side.bytes!.length} bytes
							</button>
						{/if}
					</div>

					{#if side.fields.length}
						<ol class="fields" aria-label="{label} fields">
							{#each side.fields as f, i (i)}
								<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
								<li class:on={active[key] === i} onmouseenter={() => select(key, side, i)}>
									<button onclick={() => select(key, side, i)}>
										<span class="fname mono">{f.name}</span>
										<span class="fval mono" class:varies={f.varies}>{f.value}</span>
										<span class="fat tiny muted">@{f.offset}·{f.length}B</span>
									</button>
									{#if f.varies && active[key] === i}
										<p class="vnote tiny">
											varies per run — a generated id, a timestamp or a CRC; match the shape, not
											this value
										</p>
									{/if}
								</li>
							{/each}
						</ol>
					{/if}
				{:else if !side.summary}
					<p class="tiny muted">Nothing on the wire for this one.</p>
				{/if}
			</section>
		{/each}
	</div>

	{#if example.note}
		<p class="note"><span aria-hidden="true">🌱</span> {example.note}</p>
	{/if}
</article>

<style>
	.wex {
		padding: var(--s-3) var(--s-4) var(--s-4);
		display: flex;
		flex-direction: column;
		gap: var(--s-3);
		background: var(--bg-2);
	}
	header {
		display: flex;
		align-items: center;
		gap: var(--s-2);
	}
	.petal {
		color: var(--accent, var(--pink-ink));
	}
	h4 {
		font-family: var(--font-display);
		font-size: 1rem;
		margin: 0;
	}
	.sides {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
		gap: var(--s-4);
		align-items: start;
	}
	.sides.one {
		grid-template-columns: minmax(0, 1fr);
	}
	@media (max-width: 780px) {
		.sides {
			grid-template-columns: minmax(0, 1fr);
		}
	}
	.side {
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 6px;
	}
	.lbl {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--s-2);
		margin: 0;
	}
	.summary {
		margin: 0;
		font-size: 0.86rem;
		color: var(--ink-2);
	}
	.hex {
		background: var(--bg-sunk);
		border: 1px solid var(--line);
		border-radius: var(--r-2);
		padding: var(--s-2) var(--s-3);
		font-family: var(--font-mono);
		font-size: 0.72rem;
		line-height: 1.8;
		overflow-x: auto;
		max-height: 260px;
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
		border-radius: 4px;
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
		background: var(--accent, var(--kafka));
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
		max-height: 240px;
		overflow-y: auto;
	}
	.fields li + li {
		border-top: 1px solid var(--line);
	}
	.fields li.on {
		background: color-mix(in srgb, var(--accent, var(--kafka)) 12%, transparent);
	}
	.fields button {
		display: grid;
		grid-template-columns: minmax(0, 1.2fr) minmax(0, 1fr) auto;
		gap: var(--s-2);
		align-items: baseline;
		width: 100%;
		background: none;
		border: 0;
		text-align: left;
		padding: 3px var(--s-3);
		font-size: 0.74rem;
		cursor: pointer;
	}
	.fname,
	.fval {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.fval {
		color: var(--accent, var(--kafka));
	}
	.fval.varies {
		color: var(--ink-2);
		font-style: italic;
		text-decoration: underline dotted color-mix(in srgb, var(--warn) 70%, transparent);
		text-underline-offset: 3px;
	}
	.vnote {
		margin: 0;
		padding: 0 var(--s-3) 6px;
		color: var(--warn);
	}
	.more {
		margin-top: 6px;
	}
	.env {
		display: flex;
		flex-wrap: wrap;
		align-items: baseline;
		gap: var(--s-2);
		margin: 0;
		padding: 6px var(--s-3);
		background: var(--butter-soft);
		border-radius: var(--r-2);
		color: var(--ink-2);
	}
	.env code {
		background: color-mix(in srgb, var(--bg-2) 70%, transparent);
	}
	.fixture {
		white-space: nowrap;
	}
	.note {
		margin: 0;
		font-size: 0.85rem;
		color: var(--ink-2);
		background: var(--mint-soft);
		border-radius: var(--r-2);
		padding: var(--s-2) var(--s-3);
	}
	@media (max-width: 520px) {
		.fat {
			display: none;
		}
		.fields button {
			grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
		}
	}
</style>
