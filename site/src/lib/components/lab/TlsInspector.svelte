<script lang="ts">
	/**
	 * The TLS 1.3 record and handshake inspector: paste a capture (or pick a sample) and see
	 * the record framing, the handshake messages inside it, every extension by name — and,
	 * next to it, the key schedule walked label by label, with the exact `HkdfLabel` bytes
	 * each derivation expands.
	 *
	 * Structure only: nothing here computes a secret, and that is the point. The bytes you
	 * can read with your eyes are the ones that are usually wrong first.
	 */
	import HexFields from './HexFields.svelte';
	import { decodeTls, hkdfLabelBytes, parseTlsHex, tlsSamples, KEY_SCHEDULE } from '$lib/lab/tls';

	let { initialSample = 'client-hello' }: { initialSample?: string } = $props();

	// svelte-ignore state_referenced_locally
	let chosen = $state(initialSample);
	let pasted = $state('');
	let usePasted = $state(false);
	let active = $state<number | null>(null);
	let openStep = $state<string | null>(null);

	const sample = $derived(tlsSamples.find((s) => s.id === chosen) ?? tlsSamples[0]);

	/**
	 * Parsing and its error are one value: writing to a `$state` from inside a `$derived`
	 * is forbidden in runes mode, and doing it here used to throw the moment a paste went
	 * wrong — which is exactly when the message was needed.
	 */
	const parsed = $derived.by(() => {
		if (!usePasted) return { bytes: sample.bytes, error: '' };
		try {
			const b = parseTlsHex(pasted);
			return { bytes: b, error: b.length === 0 ? 'Paste some record bytes as hex.' : '' };
		} catch (e) {
			return {
				bytes: new Uint8Array(),
				error: e instanceof Error ? e.message : 'Could not read that hex.'
			};
		}
	});

	const bytes = $derived(parsed.bytes);
	const pasteError = $derived(parsed.error);

	const decoded = $derived(decodeTls(bytes));
	const step = $derived(KEY_SCHEDULE.find((s) => s.id === openStep) ?? null);
	const labelBytes = $derived(step?.label ? hkdfLabelBytes(step.label, 32, 32) : null);

	function useSample(id: string) {
		chosen = id;
		usePasted = false;
		active = null;
	}

	function focus(start: number) {
		const i = decoded.fields.findIndex((f) => f.start === start);
		if (i >= 0) active = i;
	}

	function hex(b: Uint8Array): string {
		return [...b].map((x) => x.toString(16).padStart(2, '0')).join(' ');
	}
</script>

<div class="tls">
	<div class="picker">
		{#each tlsSamples as s (s.id)}
			<button class="btn btn-sm" class:on={!usePasted && chosen === s.id} onclick={() => useSample(s.id)}>
				{s.label}
			</button>
		{/each}
		<button class="btn btn-sm" class:on={usePasted} onclick={() => (usePasted = true)}>paste hex…</button>
	</div>

	{#if usePasted}
		<label class="field">
			<span class="eyebrow">record bytes (hex) — start at the content type</span>
			<textarea
				bind:value={pasted}
				rows="3"
				spellcheck="false"
				placeholder="16 03 01 00 c2 01 00 00 be 03 03 …"
				aria-label="Paste TLS record bytes as hex"
			></textarea>
		</label>
		{#if pasteError}<p class="err tiny">{pasteError}</p>{/if}
	{:else}
		<p class="blurb">{sample.blurb}</p>
	{/if}

	<div class="meta tiny">
		<span class="chip">{bytes.length} bytes</span>
		<span class="chip">{decoded.records.length} record{decoded.records.length === 1 ? '' : 's'}</span>
		{#if decoded.handshakes.length}
			<span class="chip">{decoded.handshakes.length} handshake message{decoded.handshakes.length === 1 ? '' : 's'}</span>
		{/if}
		{#if decoded.fromClient}<span class="chip">this is a client’s first flight</span>{/if}
	</div>

	{#if decoded.error}<p class="err tiny">⚠ {decoded.error}</p>{/if}

	{#if decoded.records.length}
		<ol class="records" aria-label="Records">
			{#each decoded.records as r, i (i)}
				<li>
					<button onclick={() => focus(r.start)}>
						<span class="rtype mono">{r.name}</span>
						<span class="rver tiny muted">{r.version}</span>
						<span class="rsum tiny">{r.summary}</span>
						<span class="rat tiny muted">@{r.start}, {r.length}B</span>
					</button>
					{#each r.handshakes as h, hi (hi)}
						<div class="hs">
							<button class="hsbtn" onclick={() => focus(h.start)}>
								<span class="hname mono">{h.name}</span>
								<span class="tiny muted">{h.summary}</span>
							</button>
							{#if h.cipherSuites.length}
								<p class="tiny suites">
									<span class="muted">cipher suites</span>
									{#each h.cipherSuites as cs, ci (ci)}<span class="chip">{cs}</span>{/each}
								</p>
							{/if}
							{#if h.extensions.length}
								<ul class="exts">
									{#each h.extensions as e, ei (ei)}
										<li>
											<span class="ename mono">{e.name}</span>
											<span class="etype tiny muted">({e.type})</span>
											<span class="eval tiny">{e.summary}</span>
										</li>
									{/each}
								</ul>
							{/if}
						</div>
					{/each}
				</li>
			{/each}
		</ol>
	{/if}

	<HexFields {bytes} fields={decoded.fields} bind:active />

	<section class="schedule">
		<h4>The key schedule</h4>
		<p class="tiny muted">
			RFC 8446 §7.1, top to bottom. Every rung is an HKDF call; the ones with a label expand
			<code>HkdfLabel</code>, whose bytes are shown when you open a step. Getting the transcript
			boundary wrong here is what <code>bad_record_mac</code> usually means.
		</p>
		<ol class="ladder">
			{#each KEY_SCHEDULE as s (s.id)}
				<li style="--d:{s.depth}" class:on={openStep === s.id}>
					<button onclick={() => (openStep = openStep === s.id ? null : s.id)}>
						<span class="out mono">{s.output}</span>
						<span class="op tiny">{s.op}</span>
						{#if s.label}<span class="label mono tiny">"tls13 {s.label}"</span>{/if}
					</button>
					{#if openStep === s.id}
						<div class="detail tiny">
							<p><span class="muted">inputs</span> {s.inputs}</p>
							{#if s.transcript}<p><span class="muted">transcript</span> {s.transcript}</p>{/if}
							<p>{s.why}</p>
							{#if labelBytes && step}
								<p class="muted">
									HkdfLabel for "tls13 {step.label}", length 32, with a 32-byte context (the
									transcript hash — shown as zeros because nothing is hashed here):
								</p>
								<pre class="bytes">{hex(labelBytes)}</pre>
							{/if}
						</div>
					{/if}
				</li>
			{/each}
		</ol>
	</section>
</div>

<style>
	.tls {
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
		background: var(--accent, var(--tls));
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
	}
	.meta {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px;
	}
	.records {
		list-style: none;
		margin: 0;
		padding: 0;
		border: 1px solid var(--line);
		border-radius: var(--r-2);
		overflow: hidden;
	}
	.records > li + li {
		border-top: 1px solid var(--line);
	}
	.records > li > button {
		display: grid;
		grid-template-columns: minmax(0, 10em) auto minmax(0, 1fr) auto;
		gap: var(--s-2);
		align-items: baseline;
		width: 100%;
		background: none;
		border: 0;
		text-align: left;
		padding: 6px var(--s-3);
		cursor: pointer;
		font-size: 0.8rem;
	}
	.records > li > button:hover {
		background: var(--bg-3);
	}
	.rtype {
		font-weight: 600;
		color: var(--accent, var(--tls));
	}
	.rsum {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.hs {
		padding: 0 var(--s-3) var(--s-3) var(--s-5);
		border-left: 2px solid color-mix(in srgb, var(--accent, var(--tls)) 35%, transparent);
		margin-left: var(--s-3);
	}
	.hsbtn {
		display: flex;
		gap: var(--s-2);
		align-items: baseline;
		background: none;
		border: 0;
		padding: 2px 0;
		cursor: pointer;
		text-align: left;
	}
	.hname {
		font-weight: 600;
		font-size: 0.8rem;
	}
	.suites {
		display: flex;
		flex-wrap: wrap;
		gap: 4px;
		align-items: baseline;
		margin: 2px 0;
	}
	.exts {
		list-style: none;
		margin: 2px 0 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 1px;
	}
	.exts li {
		display: flex;
		gap: 6px;
		align-items: baseline;
		flex-wrap: wrap;
	}
	.ename {
		font-size: 0.76rem;
		color: var(--ink);
	}
	.eval {
		color: var(--ink-2);
		min-width: 0;
		overflow-wrap: anywhere;
	}
	.schedule h4 {
		font-family: var(--font-display);
		font-size: 1rem;
		margin: 0 0 4px;
	}
	.ladder {
		list-style: none;
		margin: var(--s-2) 0 0;
		padding: 0;
		border: 1px solid var(--line);
		border-radius: var(--r-2);
		overflow: hidden;
	}
	.ladder li + li {
		border-top: 1px solid var(--line);
	}
	.ladder li.on {
		background: color-mix(in srgb, var(--accent, var(--tls)) 8%, transparent);
	}
	.ladder li > button {
		display: flex;
		gap: var(--s-3);
		align-items: baseline;
		flex-wrap: wrap;
		width: 100%;
		background: none;
		border: 0;
		text-align: left;
		cursor: pointer;
		padding: 5px var(--s-3) 5px calc(var(--s-3) + var(--d) * 16px);
		font-size: 0.8rem;
	}
	.ladder .out {
		font-weight: 600;
	}
	.ladder .op {
		color: var(--ink-3);
		font-family: var(--font-mono);
	}
	.ladder .label {
		color: var(--accent, var(--tls));
	}
	.detail {
		padding: 0 var(--s-3) var(--s-3) calc(var(--s-3) + var(--d) * 16px);
		color: var(--ink-2);
	}
	.detail p {
		margin: 0 0 3px;
	}
	.bytes {
		background: var(--bg-sunk);
		border: 1px solid var(--line);
		border-radius: var(--r-1);
		padding: 6px 8px;
		font-family: var(--font-mono);
		font-size: 0.7rem;
		white-space: pre-wrap;
		overflow-wrap: anywhere;
		margin-top: 3px;
	}
</style>
