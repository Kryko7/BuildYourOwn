<script lang="ts">
	import HexFields from './HexFields.svelte';
	import { samples, type Sample } from '$lib/lab/samples';
	import { decodeRequest, parseHex, toHex, crc32c } from '$lib/lab/kafka';

	let { initialSample = 'apiversions-v4' }: { initialSample?: string } = $props();

	// svelte-ignore state_referenced_locally
	let chosen = $state(initialSample);
	let pasted = $state('');
	let usePasted = $state(false);
	let active = $state<number | null>(null);

	const sample = $derived<Sample>(samples.find((s) => s.id === chosen) ?? samples[0]);

	/**
	 * Parsing and its error are one value: writing to a `$state` from inside a `$derived`
	 * is forbidden in runes mode, and doing it here used to throw the moment a paste went
	 * wrong — which is exactly when the message was needed.
	 */
	const parsed = $derived.by(() => {
		if (!usePasted) return { bytes: sample.bytes, error: '' };
		try {
			const b = parseHex(pasted);
			return { bytes: b, error: b.length === 0 ? 'Paste some hex bytes.' : '' };
		} catch (e) {
			return {
				bytes: new Uint8Array(),
				error: e instanceof Error ? e.message : 'Could not read that hex.'
			};
		}
	});

	const bytes = $derived(parsed.bytes);
	const pasteError = $derived(parsed.error);

	const decoded = $derived(bytes.length ? decodeRequest(bytes) : null);
	const fields = $derived(decoded?.fields ?? []);
	const crc = $derived(bytes.length ? crc32c(bytes) : 0);

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

	<HexFields {bytes} {fields} bind:active />
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
</style>
