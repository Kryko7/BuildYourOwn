<script lang="ts">
	/** One shell example, drawn as the terminal session the tester actually runs. */
	import type { ShellExample } from '$lib/types';

	let { example }: { example: ShellExample } = $props();

	const gutter: Record<string, string> = {
		input: '$',
		key: '⌨',
		stdout: ' ',
		stderr: '!',
		terminal: ' ',
		exit: ' ',
		note: ' '
	};
</script>

<figure class="ex">
	<figcaption>
		<span class="dots" aria-hidden="true"><i></i><i></i><i></i></span>
		<span class="title">{example.title}</span>
		<span class="chip">{example.mode === 'pty' ? 'pseudo-terminal' : 'stdin'}</span>
		{#if !example.exact}<span class="chip" title="The suite matches loosely here, so the output below is described rather than quoted">pattern</span>{/if}
	</figcaption>
	<pre class="screen"><code
			>{#each example.lines as line, i (i)}<span class="ln {line.kind}"
				><span class="gut" aria-hidden="true">{gutter[line.kind] ?? ' '}</span>{#if line.kind === 'exit'}<span
						class="exitlabel">exit status</span
					> {line.text}{:else}{line.text}{/if}</span
			>{'\n'}{/each}</code
		></pre>
</figure>

<style>
	.ex {
		margin: 0;
		border: 1px solid var(--line);
		border-radius: var(--r-2);
		overflow: hidden;
		background: var(--bg-sunk);
	}
	figcaption {
		display: flex;
		align-items: center;
		gap: var(--s-2);
		padding: 6px var(--s-3);
		background: var(--bg-3);
		border-bottom: 1px solid var(--line);
		font-size: 0.8rem;
		flex-wrap: wrap;
	}
	.dots {
		display: inline-flex;
		gap: 4px;
		flex: none;
	}
	.dots i {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--peach);
	}
	.dots i:nth-child(2) {
		background: var(--butter);
	}
	.dots i:nth-child(3) {
		background: var(--mint);
	}
	.title {
		flex: 1;
		min-width: 0;
		color: var(--ink-2);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.screen {
		padding: var(--s-3);
		font-family: var(--font-mono);
		font-size: 0.78rem;
		line-height: 1.65;
		overflow-x: auto;
		white-space: pre;
		tab-size: 4;
	}
	.gut {
		display: inline-block;
		width: 1.4em;
		color: var(--ink-3);
		user-select: none;
	}
	.ln.input {
		color: var(--ink);
		font-weight: 600;
	}
	.ln.input .gut {
		color: var(--mint-ink);
	}
	.ln.key {
		color: var(--lav-ink);
	}
	.ln.stdout,
	.ln.terminal {
		color: var(--ink-2);
	}
	.ln.stderr {
		color: var(--bad);
	}
	.ln.stderr .gut {
		color: var(--bad);
	}
	.ln.exit,
	.ln.note {
		color: var(--ink-3);
		font-style: italic;
	}
	.exitlabel {
		letter-spacing: 0.04em;
	}
</style>
