<script lang="ts">
	/**
	 * The WebAssembly module inspector: pick a sample or paste a module, and see the section
	 * tree, every LEB128 value next to the bytes it came from, and each function body
	 * disassembled to instruction names.
	 *
	 * Self-contained — the decoder and the samples are both `$lib/lab/wasm`, and nothing here
	 * touches the network or instantiates anything.
	 */
	import HexFields from './HexFields.svelte';
	import { decodeModule, parseWasmHex, wasmSamples } from '$lib/lab/wasm';

	let { initialSample = 'add' }: { initialSample?: string } = $props();

	// svelte-ignore state_referenced_locally
	let chosen = $state(initialSample);
	let pasted = $state('');
	let usePasted = $state(false);
	let active = $state<number | null>(null);
	let openFunction = $state(0);

	const sample = $derived(wasmSamples.find((s) => s.id === chosen) ?? wasmSamples[0]);

	/**
	 * Parsing and its error are one value: writing to a `$state` from inside a `$derived`
	 * is forbidden in runes mode, and doing it here used to throw the moment a paste went
	 * wrong — which is exactly when the message was needed.
	 */
	const parsed = $derived.by(() => {
		if (!usePasted) return { bytes: sample.bytes, error: '' };
		try {
			const b = parseWasmHex(pasted);
			return { bytes: b, error: b.length === 0 ? 'Paste a module as hex bytes.' : '' };
		} catch (e) {
			return {
				bytes: new Uint8Array(),
				error: e instanceof Error ? e.message : 'Could not read that hex.'
			};
		}
	});

	const bytes = $derived(parsed.bytes);
	const pasteError = $derived(parsed.error);

	const module = $derived(decodeModule(bytes));
	const fn = $derived(module.functions[openFunction] ?? module.functions[0] ?? null);

	function useSample(id: string) {
		chosen = id;
		usePasted = false;
		active = null;
		openFunction = 0;
	}

	/** Selecting a section jumps the field list to the field that starts it. */
	function focusSection(start: number) {
		const i = module.fields.findIndex((f) => f.start === start);
		if (i >= 0) active = i;
	}

	function copyHex() {
		void navigator.clipboard?.writeText(
			[...bytes].map((b) => b.toString(16).padStart(2, '0')).join(' ')
		);
	}
</script>

<div class="wasm">
	<div class="picker">
		{#each wasmSamples as s (s.id)}
			<button class="btn btn-sm" class:on={!usePasted && chosen === s.id} onclick={() => useSample(s.id)}>
				{s.label}
			</button>
		{/each}
		<button class="btn btn-sm" class:on={usePasted} onclick={() => (usePasted = true)}>paste a module…</button>
	</div>

	{#if usePasted}
		<label class="field">
			<span class="eyebrow">module bytes (hex)</span>
			<textarea
				bind:value={pasted}
				rows="3"
				spellcheck="false"
				placeholder="00 61 73 6d 01 00 00 00 …"
				aria-label="Paste a wasm module as hex"
			></textarea>
		</label>
		{#if pasteError}<p class="err tiny">{pasteError}</p>{/if}
	{:else}
		<p class="blurb">{sample.blurb}</p>
		<div class="run">
			<code>./your_program.sh {sample.invoke}</code>
			<span class="arrow" aria-hidden="true">→</span>
			<code class="out">{sample.expected}</code>
		</div>
	{/if}

	<div class="meta tiny">
		<span class="chip">{bytes.length} bytes</span>
		{#if module.version !== null}<span class="chip">version {module.version}</span>{/if}
		<span class="chip">{module.sections.length} section{module.sections.length === 1 ? '' : 's'}</span>
		{#if module.memory}<span class="chip">memory: {module.memory}</span>{/if}
		{#if module.imports.length}<span class="chip">{module.imports.length} import{module.imports.length === 1 ? '' : 's'}</span>{/if}
		<button class="btn btn-sm btn-ghost" onclick={copyHex}>copy hex</button>
	</div>

	{#if module.error}
		<p class="err tiny">⚠ {module.error}</p>
	{/if}

	{#if module.sections.length}
		<ol class="sections" aria-label="Section tree">
			{#each module.sections as s (s.start)}
				<li>
					<button onclick={() => focusSection(s.start)}>
						<span class="sid mono">{s.id}</span>
						<span class="sname">{s.name}</span>
						<span class="ssum tiny muted">{s.summary}</span>
						<span class="sat tiny muted">@{s.start}, {s.size}B</span>
					</button>
				</li>
			{/each}
		</ol>
	{/if}

	{#if module.exports.length || module.imports.length}
		<div class="tables">
			{#if module.imports.length}
				<div>
					<p class="eyebrow">imports — these take the low function indices</p>
					<ul class="plain tiny">
						{#each module.imports as im, i (i)}
							<li><code>{im.module}.{im.name}</code> <span class="muted">({im.kind})</span></li>
						{/each}
					</ul>
				</div>
			{/if}
			{#if module.exports.length}
				<div>
					<p class="eyebrow">exports — what the host can reach</p>
					<ul class="plain tiny">
						{#each module.exports as ex, i (i)}
							<li><code>{ex.name}</code> <span class="muted">{ex.kind} {ex.index}</span></li>
						{/each}
					</ul>
				</div>
			{/if}
		</div>
	{/if}

	{#if module.functions.length}
		<div class="code">
			<div class="ftabs">
				{#each module.functions as f, i (f.index)}
					<button class="btn btn-sm" class:on={openFunction === i} onclick={() => (openFunction = i)}>
						{f.exportName ?? `func ${f.index}`}
					</button>
				{/each}
			</div>
			{#if fn}
				<p class="sig tiny">
					<span class="mono">{fn.signature}</span>
					{#if fn.locals.length}<span class="muted">· locals {fn.locals.join(', ')}</span>{/if}
					{#if fn.error}<span class="err">· {fn.error}</span>{/if}
				</p>
				<ol class="listing" aria-label="Disassembly">
					{#each fn.instructions as ins, i (i)}
						<li style="--d:{ins.depth}">
							<span class="at tiny muted">{ins.offset.toString(16).padStart(4, '0')}</span>
							<span class="op mono">{ins.name}</span>
							<span class="args mono">{ins.args}</span>
						</li>
					{/each}
				</ol>
			{/if}
		</div>
	{/if}

	{#if module.data.length}
		<div class="data">
			<p class="eyebrow">data segments — memory before `_start` runs</p>
			{#each module.data as d, i (i)}
				<p class="tiny">
					<span class="mono">{d.offset || 'passive'}</span>
					<span class="muted">·</span>
					<code>{new TextDecoder().decode(d.bytes)}</code>
				</p>
			{/each}
		</div>
	{/if}

	<HexFields {bytes} fields={module.fields} bind:active />
</div>

<style>
	.wasm {
		display: flex;
		flex-direction: column;
		gap: var(--s-3);
	}
	.picker,
	.ftabs {
		display: flex;
		flex-wrap: wrap;
		gap: 5px;
	}
	.picker .on,
	.ftabs .on {
		background: var(--accent, var(--wasm));
		color: var(--bg-2);
		border-color: transparent;
	}
	.blurb {
		margin: 0;
		color: var(--ink-2);
		font-size: 0.9rem;
	}
	.run {
		display: flex;
		align-items: center;
		gap: var(--s-2);
		flex-wrap: wrap;
		font-size: 0.82rem;
	}
	.run .out {
		background: var(--mint-soft);
		color: var(--ok);
	}
	.arrow {
		color: var(--ink-3);
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
	}
	.meta {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px;
	}
	.sections {
		list-style: none;
		margin: 0;
		padding: 0;
		border: 1px solid var(--line);
		border-radius: var(--r-2);
		overflow: hidden;
	}
	.sections li + li {
		border-top: 1px solid var(--line);
	}
	.sections button {
		display: grid;
		grid-template-columns: 2.2em minmax(0, 8em) minmax(0, 1fr) auto;
		gap: var(--s-2);
		align-items: baseline;
		width: 100%;
		background: none;
		border: 0;
		text-align: left;
		padding: 5px var(--s-3);
		cursor: pointer;
		font-size: 0.8rem;
	}
	.sections button:hover {
		background: var(--bg-3);
	}
	.sid {
		color: var(--ink-3);
	}
	.sname {
		font-weight: 600;
		color: var(--accent, var(--wasm));
	}
	.ssum {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.tables {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
		gap: var(--s-4);
	}
	.plain {
		list-style: none;
		margin: 4px 0 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.code {
		display: flex;
		flex-direction: column;
		gap: var(--s-2);
	}
	.sig {
		margin: 0;
	}
	.listing {
		list-style: none;
		margin: 0;
		padding: var(--s-2) 0;
		background: var(--bg-sunk);
		border: 1px solid var(--line);
		border-radius: var(--r-2);
		font-size: 0.76rem;
		max-height: 260px;
		overflow-y: auto;
	}
	.listing li {
		display: flex;
		gap: var(--s-3);
		padding: 1px var(--s-3) 1px calc(var(--s-3) + var(--d) * 14px);
	}
	.listing li:hover {
		background: color-mix(in srgb, var(--accent, var(--wasm)) 8%, transparent);
	}
	.op {
		color: var(--ink);
		font-weight: 600;
	}
	.args {
		color: var(--accent, var(--wasm));
	}
	.data p {
		margin: 2px 0 0;
	}
</style>
