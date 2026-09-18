<script lang="ts">
	import { samples, type Sample } from '$lib/lab/samples';
	import { decodeRequest, parseHex, toHex, printable, crc32c } from '$lib/lab/kafka';

	let { initialSample = 'apiversions-v4' }: { initialSample?: string } = $props();

	// svelte-ignore state_referenced_locally
	let chosen = $state(initialSample);
	let pasted = $state('');
	let usePasted = $state(false);
	let active = $state<number | null>(null);
	let pasteError = $state('');

	const sample = $derived<Sample>(samples.find((s) => s.id === chosen) ?? samples[0]);

	const bytes = $derived.by(() => {
		if (!usePasted) return sample.bytes;
		try {
			const b = parseHex(pasted);
			pasteError = b.length === 0 ? 'Paste some hex bytes.' : '';
			return b;
		} catch (e) {
			pasteError = e instanceof Error ? e.message : 'Could not read that hex.';
			return new Uint8Array();
		}
	});

	const decoded = $derived(bytes.length ? decodeRequest(bytes) : null);
	const fields = $derived(decoded?.fields ?? []);
	const crc = $derived(bytes.length ? crc32c(bytes) : 0);

	const rows = $derived.by(() => {
		const out: { offset: number; slice: number[] }[] = [];
		for (let i = 0; i < bytes.length; i += 16) out.push({ offset: i, slice: [...bytes.subarray(i, i + 16)] });
		return out;
	});

	function fieldAt(index: number): number {
		return fields.findIndex((f) => index >= f.start && index < f.end);
	}

	const activeRange = $derived(active !== null && fields[active] ? fields[active] : null);

	function inActive(i: number) {
		return activeRange ? i >= activeRange.start && i < activeRange.end : false;
	}

	function useSample(id: string) {
		chosen = id;
		usePasted = false;
		active = null;
	}

	function copyHex() {
		void navigator.clipboard?.writeText(toHex(bytes));
	}
</script>

<div class="wire">
	<div class="picker">
		{#each samples as s (s.id)}
			<button
				class="btn btn-sm"
				class:on={!usePasted && chosen === s.id}
				onclick={() => useSample(s.id)}
			>
				{s.label}
			</button>
		{/each}
		<button class="btn btn-sm" class:on={usePasted} onclick={() => (usePasted = true)}>paste hex…</button>
	</div>

	{#if usePasted}
		<label class="field">
			<span class="eyebrow">framed request bytes (hex)</span>
			<textarea
				bind:value={pasted}
				rows="3"
				spellcheck="false"
				placeholder="00 00 00 23 00 12 00 04 …"
				aria-label="Paste hex bytes"
			></textarea>
		</label>
		{#if pasteError}<p class="err tiny">{pasteError}</p>{/if}
	{:else}
		<p class="blurb">{sample.blurb}</p>
	{/if}

	{#if decoded}
		<div class="meta tiny">
			<span class="chip">{decoded.apiName}({decoded.apiKey}) v{decoded.apiVersion}</span>
			<span class="chip">{bytes.length} bytes</span>
			<span class="chip">CRC32C of the frame 0x{crc.toString(16).padStart(8, '0')}</span>
			<button class="btn btn-sm btn-ghost" onclick={copyHex}>copy hex</button>
		</div>
		{#if decoded.error}
			<p class="err tiny">⚠ {decoded.error}</p>
		{/if}
	{/if}

	<div class="split">
		<div class="hex" role="group" aria-label="Hex dump">
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
								onmouseenter={() => {
									const f = fieldAt(idx);
									if (f >= 0) active = f;
								}}
								onclick={() => {
									const f = fieldAt(idx);
									if (f >= 0) active = f;
								}}>{b.toString(16).padStart(2, '0')}</span
							>
						{/each}
					</span>
					<span class="ascii">{row.slice.map(printable).join('')}</span>
				</div>
			{/each}
			{#if rows.length === 0}<p class="tiny muted">No bytes to show.</p>{/if}
		</div>

		<ol class="fields" aria-label="Decoded fields">
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
			{/each}
		</ol>
	</div>
</div>

<style>
	.wire {
		display: flex;
		flex-direction: column;
		gap: var(--s-3);
	}
	.picker {
		display: flex;
		flex-wrap: wrap;
		gap: 5px;
	}
	.picker .on {
		background: var(--accent, var(--ink));
		color: var(--bg-2);
		border-color: transparent;
	}
	.blurb {
		margin: 0;
		color: var(--ink-2);
		font-size: 0.9rem;
	}
	.field span {
		display: block;
		margin-bottom: 4px;
	}
	textarea {
		width: 100%;
		background: var(--bg-sunk);
		border: 1px solid var(--line);
		border-radius: var(--r-2);
		padding: 10px 12px;
		font-family: var(--font-mono);
		font-size: 0.8rem;
		resize: vertical;
	}
	.err {
		color: var(--bad);
		margin: 0;
		font-family: var(--font-mono);
	}
	.meta {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px;
	}
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
		max-height: 420px;
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
		transition: background 120ms var(--ease), color 120ms var(--ease);
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
		max-height: 420px;
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
		padding: 0 var(--s-3) 7px calc(var(--s-3));
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
