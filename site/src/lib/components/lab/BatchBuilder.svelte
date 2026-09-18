<script lang="ts">
	import { encodeRecordBatch, gzip, type Codec, type RecordInput } from '$lib/lab/recordbatch';
	import { toHex, printable } from '$lib/lab/kafka';

	let records = $state<RecordInput[]>([
		{ key: null, value: 'hello world' },
		{ key: 'k2', value: 'second record' }
	]);
	let codec = $state<Codec>('none');
	let transactional = $state(false);
	let control = $state(false);
	let producerId = $state(-1);
	let compressed = $state<Uint8Array | null>(null);
	let gzipNote = $state('');
	let active = $state<number | null>(null);

	// gzip is the one codec a browser can actually do (CompressionStream); the rest
	// only flip the attribute bits so you can see where they live.
	$effect(() => {
		const plain = encodeRecordBatch(records, { producerId: BigInt(producerId) });
		if (codec !== 'gzip') {
			compressed = null;
			gzipNote =
				codec === 'none'
					? ''
					: `${codec} is not implemented in the browser — the attribute bits change, the payload does not.`;
			return;
		}
		let cancelled = false;
		void gzip(plain.recordsPayload).then((out) => {
			if (cancelled) return;
			compressed = out;
			gzipNote = out
				? `gzip: ${plain.recordsPayload.length} → ${out.length} bytes (the whole record run, not per record)`
				: 'CompressionStream is unavailable here — showing the uncompressed payload.';
		});
		return () => {
			cancelled = true;
		};
	});

	const batch = $derived(
		encodeRecordBatch(records, {
			codec,
			transactional,
			control,
			producerId: BigInt(producerId),
			baseSequence: producerId >= 0 ? 0 : -1,
			producerEpoch: producerId >= 0 ? 0 : -1,
			compressedPayload: codec === 'gzip' && compressed ? compressed : undefined
		})
	);

	const rows = $derived.by(() => {
		const out: { offset: number; slice: number[] }[] = [];
		for (let i = 0; i < batch.bytes.length; i += 16)
			out.push({ offset: i, slice: [...batch.bytes.subarray(i, i + 16)] });
		return out;
	});

	function fieldAt(i: number) {
		return batch.fields.findIndex((f) => i >= f.start && i < f.start + f.length);
	}
	const hot = $derived(active !== null ? batch.fields[active] : null);
	const inHot = (i: number) => (hot ? i >= hot.start && i < hot.start + hot.length : false);

	function addRecord() {
		records = [...records, { key: null, value: `record ${records.length + 1}` }];
	}
	function removeRecord(i: number) {
		records = records.filter((_, j) => j !== i);
	}
</script>

<div class="bb">
	<div class="editor">
		<h4 class="eyebrow">records</h4>
		{#each records as r, i (i)}
			<div class="rec">
				<input
					class="k"
					value={r.key ?? ''}
					placeholder="(null key)"
					aria-label="Record {i} key"
					oninput={(e) => {
						const v = e.currentTarget.value;
						records[i] = { ...records[i], key: v === '' ? null : v };
					}}
				/>
				<input
					class="v"
					value={r.value}
					aria-label="Record {i} value"
					oninput={(e) => (records[i] = { ...records[i], value: e.currentTarget.value })}
				/>
				<button class="btn btn-sm btn-ghost" onclick={() => removeRecord(i)} aria-label="Remove record {i}">✕</button>
			</div>
		{/each}
		<div class="row">
			<button class="btn btn-sm" onclick={addRecord}>+ record</button>
		</div>

		<h4 class="eyebrow" style="margin-top:var(--s-4)">attributes</h4>
		<div class="opts">
			<label>
				compression
				<select bind:value={codec} aria-label="Compression codec">
					<option value="none">none (0)</option>
					<option value="gzip">gzip (1)</option>
					<option value="snappy">snappy (2)</option>
					<option value="lz4">lz4 (3)</option>
					<option value="zstd">zstd (4)</option>
				</select>
			</label>
			<label><input type="checkbox" bind:checked={transactional} /> transactional (bit 4)</label>
			<label><input type="checkbox" bind:checked={control} /> control batch (bit 5)</label>
			<label>
				producer_id
				<input type="number" bind:value={producerId} style="width:7em" aria-label="Producer id" />
			</label>
		</div>
		{#if gzipNote}<p class="tiny muted">{gzipNote}</p>{/if}
		<p class="tiny muted">
			attributes = 0x{batch.attributes.toString(16).padStart(4, '0')} · crc32c = 0x{batch.crc
				.toString(16)
				.padStart(8, '0')} · {batch.bytes.length} bytes on disk
		</p>
	</div>

	<div class="anatomy">
		<h4 class="eyebrow">anatomy</h4>
		<ol class="fields">
			{#each batch.fields as f, i (f.name + i)}
				<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
				<li class:on={active === i} class:rec={f.group === 'record'} onmouseenter={() => (active = i)}>
					<button onclick={() => (active = i)}>
						<span class="mono n">{f.name}</span>
						<span class="mono v">{f.value}</span>
						<span class="tiny muted">{f.length}B</span>
					</button>
					{#if f.note && active === i}<p class="note tiny">{f.note}</p>{/if}
				</li>
			{/each}
		</ol>
	</div>

	<div class="hex">
		<h4 class="eyebrow">bytes</h4>
		<div class="dump">
			{#each rows as row (row.offset)}
				<div class="hexrow">
					<span class="off">{row.offset.toString(16).padStart(4, '0')}</span>
					<span class="cells">
						{#each row.slice as b, i (i)}
							{@const idx = row.offset + i}
							<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
							<span
								class="b"
								class:hot={inHot(idx)}
								onmouseenter={() => {
									const f = fieldAt(idx);
									if (f >= 0) active = f;
								}}>{b.toString(16).padStart(2, '0')}</span
							>
						{/each}
					</span>
					<span class="ascii">{row.slice.map(printable).join('')}</span>
				</div>
			{/each}
		</div>
	</div>
</div>

<style>
	.bb {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
		gap: var(--s-4);
	}
	.hex {
		grid-column: 1 / -1;
	}
	@media (max-width: 860px) {
		.bb {
			grid-template-columns: 1fr;
		}
	}
	h4 {
		margin-bottom: var(--s-2);
	}
	.rec {
		display: grid;
		grid-template-columns: 8em 1fr auto;
		gap: 5px;
		margin-bottom: 5px;
	}
	input,
	select {
		background: var(--bg-sunk);
		border: 1px solid var(--line);
		border-radius: var(--r-1);
		padding: 5px 8px;
		font-family: var(--font-mono);
		font-size: 0.78rem;
		min-width: 0;
	}
	.opts {
		display: flex;
		flex-wrap: wrap;
		gap: var(--s-3);
		font-size: 0.82rem;
	}
	.opts label {
		display: inline-flex;
		align-items: center;
		gap: 5px;
	}
	.fields {
		list-style: none;
		margin: 0;
		padding: 0;
		border: 1px solid var(--line);
		border-radius: var(--r-2);
		max-height: 330px;
		overflow-y: auto;
	}
	.fields li {
		border-top: 1px solid var(--line);
	}
	.fields li:first-child {
		border-top: 0;
	}
	.fields li.rec {
		background: color-mix(in srgb, var(--kafka) 8%, transparent);
	}
	.fields li.on {
		background: color-mix(in srgb, var(--accent, var(--ink)) 12%, transparent);
	}
	.fields button {
		display: grid;
		grid-template-columns: minmax(0, 1.1fr) minmax(0, 1fr) auto;
		gap: var(--s-2);
		width: 100%;
		background: none;
		border: 0;
		text-align: left;
		padding: 4px var(--s-3);
		font-size: 0.75rem;
		cursor: pointer;
		align-items: baseline;
	}
	.fields .v {
		color: var(--accent, var(--ink));
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.note {
		margin: 0;
		padding: 0 var(--s-3) 6px;
		color: var(--ink-2);
	}
	.dump {
		background: var(--bg-sunk);
		border: 1px solid var(--line);
		border-radius: var(--r-2);
		padding: var(--s-3);
		font-family: var(--font-mono);
		font-size: 0.74rem;
		line-height: 1.85;
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
	}
	.cells {
		display: inline-flex;
		gap: 4px;
	}
	.b {
		border-radius: 3px;
		padding: 0 2px;
	}
	.b.hot {
		background: var(--accent, var(--ink));
		color: var(--bg-2);
	}
	.ascii {
		color: var(--ink-3);
	}
</style>
