<script lang="ts">
	/**
	 * The ELF viewer: pick a sample object or paste one as hex, and read the file header,
	 * the section table, the symbols with their bindings, and the relocations with the
	 * formula each one applies. It is the same walk a linker's front end does, with names
	 * instead of numbers.
	 *
	 * Self-contained: the reader and the sample objects are both `$lib/lab/elf`.
	 */
	import HexFields from './HexFields.svelte';
	import { elfSamples, parseElf, parseElfHex } from '$lib/lab/elf';

	let { initialSample = 'main-o' }: { initialSample?: string } = $props();

	// svelte-ignore state_referenced_locally
	let chosen = $state(initialSample);
	let pasted = $state('');
	let usePasted = $state(false);
	let active = $state<number | null>(null);
	let tab = $state<'sections' | 'symbols' | 'relocations' | 'segments'>('sections');

	const sample = $derived(elfSamples.find((s) => s.id === chosen) ?? elfSamples[0]);

	/**
	 * Parsing and its error are one value: writing to a `$state` from inside a `$derived`
	 * is forbidden in runes mode, and doing it here used to throw the moment a paste went
	 * wrong — which is exactly when the message was needed.
	 */
	const parsed = $derived.by(() => {
		if (!usePasted) return { bytes: sample.bytes, error: '' };
		try {
			const b = parseElfHex(pasted);
			return { bytes: b, error: b.length === 0 ? 'Paste an object file as hex bytes.' : '' };
		} catch (e) {
			return {
				bytes: new Uint8Array(),
				error: e instanceof Error ? e.message : 'Could not read that hex.'
			};
		}
	});

	const bytes = $derived(parsed.bytes);
	const pasteError = $derived(parsed.error);

	const elf = $derived(parseElf(bytes));

	const tabs = $derived(
		[
			{ id: 'sections' as const, label: `sections (${elf.sections.length})`, has: elf.sections.length > 0 },
			{ id: 'symbols' as const, label: `symbols (${elf.symbols.length})`, has: elf.symbols.length > 0 },
			{
				id: 'relocations' as const,
				label: `relocations (${elf.relocations.length})`,
				has: elf.relocations.length > 0
			},
			{ id: 'segments' as const, label: `segments (${elf.segments.length})`, has: elf.segments.length > 0 }
		].filter((t) => t.has)
	);

	// A file with no symbols must not leave the viewer stuck on an empty tab.
	$effect(() => {
		if (tabs.length && !tabs.some((t) => t.id === tab)) tab = tabs[0].id;
	});

	function useSample(id: string) {
		chosen = id;
		usePasted = false;
		active = null;
	}

	/** Clicking a section jumps the hex view to the header that describes it. */
	function focusSection(index: number) {
		const i = elf.fields.findIndex((f) => f.name === `shdr[${index}].sh_name`);
		if (i >= 0) active = i;
	}

	const hexAddr = (v: bigint) => `0x${v.toString(16)}`;
</script>

<div class="elf">
	<div class="picker">
		{#each elfSamples as s (s.id)}
			<button class="btn btn-sm" class:on={!usePasted && chosen === s.id} onclick={() => useSample(s.id)}>
				{s.label}
			</button>
		{/each}
		<button class="btn btn-sm" class:on={usePasted} onclick={() => (usePasted = true)}>paste an object…</button>
	</div>

	{#if usePasted}
		<label class="field">
			<span class="eyebrow">object bytes (hex)</span>
			<textarea
				bind:value={pasted}
				rows="3"
				spellcheck="false"
				placeholder="7f 45 4c 46 02 01 01 00 …"
				aria-label="Paste an ELF file as hex"
			></textarea>
		</label>
		{#if pasteError}<p class="err tiny">{pasteError}</p>{/if}
	{:else}
		<p class="blurb">{sample.blurb}</p>
		{#if sample.runs}
			<p class="tiny runs">
				<span class="muted">linked and run, it prints</span> <code>{sample.runs}</code>
			</p>
		{/if}
	{/if}

	<div class="meta tiny">
		<span class="chip">{bytes.length} bytes</span>
		{#if elf.class}<span class="chip">{elf.class}, {elf.endianness}</span>{/if}
		{#if elf.fileType}<span class="chip">{elf.fileType}</span>{/if}
		{#if elf.machine}<span class="chip">{elf.machine}</span>{/if}
		<span class="chip">entry {hexAddr(elf.entry)}</span>
	</div>

	{#if elf.error}<p class="err tiny">⚠ {elf.error}</p>{/if}

	{#if tabs.length}
		<div class="tabs" role="tablist" aria-label="ELF tables">
			{#each tabs as t (t.id)}
				<button
					role="tab"
					aria-selected={tab === t.id}
					class="btn btn-sm"
					class:on={tab === t.id}
					onclick={() => (tab = t.id)}>{t.label}</button
				>
			{/each}
		</div>

		<div class="table" role="tabpanel">
			{#if tab === 'sections'}
				<table>
					<thead>
						<tr><th>#</th><th>name</th><th>type</th><th>flags</th><th>offset</th><th>size</th><th>link/info</th></tr>
					</thead>
					<tbody>
						{#each elf.sections as s (s.index)}
							<tr>
								<td class="mono">
									<button class="jump" onclick={() => focusSection(s.index)} title="Show this section header in the hex view">
										{s.index}
									</button>
								</td>
								<td class="mono nm">{s.name || '—'}</td>
								<td>{s.type}</td>
								<td class="mono">{s.flags}</td>
								<td class="mono">{s.offset}</td>
								<td class="mono">{s.size}</td>
								<td class="mono">{s.link}/{s.info}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			{:else if tab === 'symbols'}
				<table>
					<thead>
						<tr><th>#</th><th>name</th><th>bind</th><th>type</th><th>vis</th><th>section</th><th>value</th><th>size</th></tr>
					</thead>
					<tbody>
						{#each elf.symbols as s, i (i)}
							<tr class:undef={s.section === 'SHN_UNDEF'} class:weak={s.bind === 'WEAK'}>
								<td class="mono">{s.index}</td>
								<td class="mono nm">{s.name}</td>
								<td>{s.bind}</td>
								<td>{s.type}</td>
								<td>{s.visibility}</td>
								<td class="mono">{s.section}</td>
								<td class="mono">{hexAddr(s.value)}</td>
								<td class="mono">{s.size}</td>
							</tr>
						{/each}
					</tbody>
				</table>
				<p class="tiny muted legend">
					An <strong>SHN_UNDEF</strong> global is a promise someone else has to keep; a
					<strong>WEAK</strong> one is a promise the link is allowed to break.
				</p>
			{:else if tab === 'relocations'}
				<table>
					<thead>
						<tr><th>section</th><th>offset</th><th>type</th><th>symbol</th><th>addend</th><th>formula</th></tr>
					</thead>
					<tbody>
						{#each elf.relocations as r, i (i)}
							<tr>
								<td class="mono">{r.section}</td>
								<td class="mono">{hexAddr(r.offset)}</td>
								<td class="mono rt">{r.type}</td>
								<td class="mono nm">{r.symbol}</td>
								<td class="mono">{r.addend}</td>
								<td class="tiny">{r.formula}</td>
							</tr>
						{/each}
					</tbody>
				</table>
				<p class="tiny muted legend">
					<strong>S</strong> is the symbol's final address, <strong>A</strong> the addend,
					<strong>P</strong> the address being patched. A PC-relative relocation whose result does
					not fit in 32 bits signed is an overflow, not a truncation.
				</p>
			{:else}
				<table>
					<thead>
						<tr><th>type</th><th>flags</th><th>offset</th><th>vaddr</th><th>filesz</th><th>memsz</th><th>align</th></tr>
					</thead>
					<tbody>
						{#each elf.segments as s, i (i)}
							<tr>
								<td class="mono">{s.type}</td>
								<td class="mono">{s.flags}</td>
								<td class="mono">{s.offset}</td>
								<td class="mono">{hexAddr(s.vaddr)}</td>
								<td class="mono">{s.filesz}</td>
								<td class="mono">{s.memsz}</td>
								<td class="mono">{s.align}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			{/if}
		</div>
	{/if}

	<HexFields {bytes} fields={elf.fields} bind:active />
</div>

<style>
	.elf {
		display: flex;
		flex-direction: column;
		gap: var(--s-3);
	}
	.picker,
	.tabs {
		display: flex;
		flex-wrap: wrap;
		gap: 5px;
	}
	.picker .on,
	.tabs .on {
		background: var(--accent, var(--link));
		color: var(--bg-2);
		border-color: transparent;
	}
	.blurb {
		margin: 0;
		color: var(--ink-2);
		font-size: 0.9rem;
	}
	.runs {
		margin: 0;
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
	.table {
		border: 1px solid var(--line);
		border-radius: var(--r-2);
		overflow-x: auto;
		max-height: 320px;
		overflow-y: auto;
	}
	table {
		border-collapse: collapse;
		width: 100%;
		font-size: 0.76rem;
	}
	th {
		position: sticky;
		top: 0;
		background: var(--bg-3);
		text-align: left;
		font-weight: 600;
		font-size: 0.7rem;
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: var(--ink-3);
		padding: 5px var(--s-3);
		white-space: nowrap;
	}
	td {
		padding: 3px var(--s-3);
		border-top: 1px solid var(--line);
		white-space: nowrap;
	}
	tbody tr:hover {
		background: color-mix(in srgb, var(--accent, var(--link)) 8%, transparent);
	}
	.nm {
		color: var(--accent, var(--link));
		font-weight: 600;
	}
	.rt {
		color: var(--ink);
	}
	tr.undef .nm {
		color: var(--bad);
	}
	tr.weak .nm {
		font-style: italic;
	}
	.jump {
		background: none;
		border: 0;
		padding: 0;
		cursor: pointer;
		font: inherit;
		color: var(--ink-2);
		text-decoration: underline dotted;
		text-underline-offset: 3px;
	}
	.legend {
		margin: var(--s-2) 0 0;
		padding: 0 var(--s-3) var(--s-2);
	}
</style>
