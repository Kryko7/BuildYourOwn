<script lang="ts">
	import { tokenize, spanLegend } from '$lib/lab/tokenizer';

	let { initial = `echo "hi $USER" 'and $LITERAL' | grep -n hi 2>> err.log > out.txt` }: { initial?: string } =
		$props();

	// svelte-ignore state_referenced_locally
	let line = $state(initial);
	let hovered = $state<number | null>(null);

	const result = $derived(tokenize(line));

	const examples = [
		`echo "hi $USER" 'and $LITERAL' | grep -n hi 2>> err.log > out.txt`,
		`echo he"llo"'wor'ld`,
		`cat < in.txt | wc -l > count.txt`,
		`echo "a \\"quoted\\" word" \\ space`,
		`ls /tmp # a trailing comment`,
		`echo "unterminated`
	];
</script>

<div class="tok">
	<label class="field">
		<span class="eyebrow">command line</span>
		<input bind:value={line} spellcheck="false" autocomplete="off" aria-label="Command line to tokenize" />
	</label>

	<div class="samples">
		{#each examples as ex (ex)}
			<button class="btn btn-sm btn-ghost" onclick={() => (line = ex)}
				>{ex.slice(0, 26)}{ex.length > 26 ? '…' : ''}</button
			>
		{/each}
	</div>

	<div class="render" aria-hidden="true">
		{#each result.spans as span, spi (spi)}<span
				class="s-{span.kind}"
				class:dim={hovered !== null && span.token !== hovered}
				>{line.slice(span.start, span.end) || ' '}</span
			>{/each}{#if result.spans.length === 0}<span class="muted">type a command line…</span>{/if}
	</div>

	<ul class="legend tiny">
		{#each spanLegend as l (l.kind)}
			<li><i class="sw s-{l.kind}"></i>{l.label}</li>
		{/each}
	</ul>

	{#if result.warnings.length}
		<ul class="warnings tiny">
			{#each result.warnings as w, wi (wi)}<li>⚠ {w}</li>{/each}
		</ul>
	{/if}

	<div class="cols">
		<section>
			<h4 class="eyebrow">words after quote removal</h4>
			<ol class="words">
				{#each result.tokens.filter((t) => t.kind === 'word') as t (t.index)}
					<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
					<li
						onmouseenter={() => (hovered = t.index)}
						onmouseleave={() => (hovered = null)}
						class:quoted={t.quoted}
					>
						<code>{t.value === '' ? '∅' : t.value}</code>
						{#if t.quoted}<span class="chip">quoted</span>{/if}
						{#if t.expansions.length}<span class="chip chip-ext">expands {t.expansions.join(', ')}</span>{/if}
					</li>
				{/each}
			</ol>
			{#if result.tokens.filter((t) => t.kind === 'word').length === 0}
				<p class="tiny muted">No words — the whole line is operators, comment or whitespace.</p>
			{/if}
		</section>

		<section>
			<h4 class="eyebrow">pipeline &amp; redirections</h4>
			<div class="graph">
				{#each result.pipeline as cmd, i (i)}
					{#if i > 0}
						<div class="pipe" aria-hidden="true">
							<span class="op mono">{result.separators[i - 1]}</span>
						</div>
					{/if}
					<div class="stage-box">
						<div class="argv mono">{cmd.argv.length ? cmd.argv.join(' ') : '(empty)'}</div>
						{#each cmd.redirects as r, ri (ri)}
							<div class="redir tiny">
								<span class="mono">fd {r.fd ?? '1&2'}</span>
								<span class="arrow" aria-hidden="true">{r.op.includes('<') ? '←' : '→'}</span>
								<code>{r.target}</code>
								<span class="muted">{r.description}</span>
							</div>
						{/each}
					</div>
				{/each}
			</div>
		</section>
	</div>
</div>

<style>
	.tok {
		display: flex;
		flex-direction: column;
		gap: var(--s-3);
	}
	.field {
		display: block;
	}
	.field span {
		display: block;
		margin-bottom: 4px;
	}
	input {
		width: 100%;
		background: var(--bg-sunk);
		border: 1px solid var(--line);
		border-radius: var(--r-2);
		padding: 10px 12px;
		font-family: var(--font-mono);
		font-size: 0.88rem;
	}
	.samples {
		display: flex;
		flex-wrap: wrap;
		gap: 4px;
	}
	.samples button {
		font-family: var(--font-mono);
		font-size: 0.7rem;
	}
	.render {
		background: var(--bg-sunk);
		border: 1px solid var(--line);
		border-radius: var(--r-2);
		padding: var(--s-3);
		font-family: var(--font-mono);
		font-size: 0.95rem;
		white-space: pre-wrap;
		word-break: break-all;
		min-height: 2.6em;
		line-height: 1.9;
	}
	.render span {
		border-radius: 3px;
		padding: 2px 0;
		transition: opacity 150ms var(--ease);
	}
	.render span.dim {
		opacity: 0.28;
	}
	.s-plain {
		color: var(--ink);
	}
	.s-single {
		color: var(--ok);
		background: color-mix(in srgb, var(--ok) 13%, transparent);
	}
	.s-double {
		color: var(--kafka);
		background: color-mix(in srgb, var(--kafka) 14%, transparent);
	}
	.s-escape {
		color: var(--bad);
		background: color-mix(in srgb, var(--bad) 13%, transparent);
	}
	.s-expansion {
		color: var(--warn);
		background: color-mix(in srgb, var(--warn) 18%, transparent);
	}
	.s-operator {
		color: var(--shell);
		font-weight: 700;
	}
	.s-comment {
		color: var(--ink-3);
		font-style: italic;
	}
	.s-unterminated {
		color: var(--bad);
		background: color-mix(in srgb, var(--bad) 22%, transparent);
		text-decoration: underline wavy;
	}
	.s-space {
		background: color-mix(in srgb, var(--ink-3) 10%, transparent);
	}
	.legend {
		list-style: none;
		display: flex;
		flex-wrap: wrap;
		gap: var(--s-3);
		margin: 0;
		padding: 0;
		color: var(--ink-2);
	}
	.legend li {
		display: flex;
		align-items: center;
		gap: 5px;
	}
	.sw {
		width: 11px;
		height: 11px;
		border-radius: 3px;
		display: inline-block;
		border: 1px solid var(--line-strong);
	}
	.warnings {
		list-style: none;
		margin: 0;
		padding: var(--s-2) var(--s-3);
		border-radius: var(--r-2);
		background: var(--warn-soft);
		color: var(--warn);
	}
	.cols {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: var(--s-4);
	}
	@media (max-width: 720px) {
		.cols {
			grid-template-columns: 1fr;
		}
	}
	.words {
		list-style: none;
		margin: 0;
		padding: 0;
		counter-reset: w;
	}
	.words li {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 3px 0;
		counter-increment: w;
	}
	.words li::before {
		content: counter(w);
		font-family: var(--font-mono);
		font-size: 0.7rem;
		color: var(--ink-3);
		width: 1.4em;
	}
	.graph {
		display: flex;
		flex-direction: column;
		gap: 0;
	}
	.stage-box {
		border: 1px solid var(--line);
		border-radius: var(--r-2);
		background: var(--bg-3);
		padding: var(--s-2) var(--s-3);
	}
	.argv {
		font-size: 0.82rem;
	}
	.redir {
		display: flex;
		align-items: baseline;
		gap: 6px;
		margin-top: 3px;
		flex-wrap: wrap;
	}
	.arrow {
		color: var(--shell);
	}
	.pipe {
		display: flex;
		justify-content: center;
		padding: 2px 0;
	}
	.op {
		font-size: 0.78rem;
		color: var(--shell);
		border-left: 2px dashed var(--line-strong);
		padding-left: 8px;
	}
</style>
