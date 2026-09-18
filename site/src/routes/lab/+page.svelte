<script lang="ts">
	import { pageTitle } from '$lib/branding';
	import Tokenizer from '$lib/components/lab/Tokenizer.svelte';
	import WireInspector from '$lib/components/lab/WireInspector.svelte';
	import BatchBuilder from '$lib/components/lab/BatchBuilder.svelte';
	import Replay from '$lib/components/lab/Replay.svelte';

	const tabs = [
		{
			id: 'tokenizer',
			label: 'Shell tokenizer',
			accent: 'var(--shell)',
			blurb:
				'Type a command line and watch the quote state machine run: every character coloured by the state that produced it, the words that survive quote removal, and the pipeline and redirection graph.',
			stages: '/shell/13'
		},
		{
			id: 'wire',
			label: 'Kafka wire inspector',
			accent: 'var(--kafka)',
			blurb:
				'Four real requests, byte by byte. Hover a byte to find its field, hover a field to find its bytes; varints, compact arrays and tagged fields are spelled out. Paste your own hex to debug a frame.',
			stages: '/kafka/5'
		},
		{
			id: 'batch',
			label: 'RecordBatch anatomy',
			accent: 'var(--kafka)',
			blurb:
				'Build a v2 record batch from records and watch the bytes appear, with a live CRC32C and the attribute bits that select a compression codec.',
			stages: '/kafka/34'
		},
		{
			id: 'replay',
			label: 'Journey replay',
			accent: 'var(--bad)',
			blurb:
				'Import a tester report and step through a failing test: the input it sent, the output it got, and the diff against what the suite expected.',
			stages: '/progress'
		}
	];

	let active = $state('tokenizer');
	const current = $derived(tabs.find((t) => t.id === active) ?? tabs[0]);
</script>

<svelte:head>
	<title>{pageTitle('Lab')}</title>
</svelte:head>

<div class="wrap page" style="--accent:{current.accent}">
	<header>
		<p class="eyebrow">playgrounds</p>
		<h1>The lab</h1>
		<p class="lede">
			Four instruments for the two tracks. Each one is also embedded in the stages it belongs to, so
			you can poke at the thing you are about to implement.
		</p>
	</header>

	<div class="tabs" role="tablist" aria-label="Playgrounds">
		{#each tabs as t (t.id)}
			<button
				role="tab"
				aria-selected={active === t.id}
				aria-controls="panel-{t.id}"
				id="tab-{t.id}"
				class:on={active === t.id}
				style="--accent:{t.accent}"
				onclick={() => (active = t.id)}
			>
				{t.label}
			</button>
		{/each}
	</div>

	<p class="blurb">{current.blurb} <a href={current.stages}>related stage →</a></p>

	<div class="panel card" role="tabpanel" id="panel-{current.id}" aria-labelledby="tab-{current.id}">
		{#if active === 'tokenizer'}
			<Tokenizer />
		{:else if active === 'wire'}
			<WireInspector />
		{:else if active === 'batch'}
			<BatchBuilder />
		{:else}
			<Replay />
		{/if}
	</div>
</div>

<style>
	.page {
		padding: var(--s-6) 0 var(--s-7);
		display: flex;
		flex-direction: column;
		gap: var(--s-4);
	}
	header {
		max-width: 64ch;
	}
	h1 {
		margin: var(--s-2) 0;
	}
	.lede {
		color: var(--ink-2);
	}
	.tabs {
		display: flex;
		gap: 4px;
		flex-wrap: wrap;
		border-bottom: 1px solid var(--line);
		padding-bottom: 0;
	}
	.tabs button {
		background: none;
		border: 1px solid transparent;
		border-bottom: 0;
		border-radius: var(--r-2) var(--r-2) 0 0;
		padding: 8px 14px;
		cursor: pointer;
		color: var(--ink-2);
		font-size: 0.9rem;
		position: relative;
		top: 1px;
	}
	.tabs button:hover {
		color: var(--ink);
		background: var(--bg-3);
	}
	.tabs button.on {
		color: var(--ink);
		background: var(--bg-2);
		border-color: var(--line);
		box-shadow: inset 0 2px 0 var(--accent);
	}
	.blurb {
		margin: 0;
		color: var(--ink-2);
		max-width: 76ch;
		font-size: 0.92rem;
	}
	.panel {
		padding: var(--s-5);
	}
	@media (max-width: 560px) {
		.panel {
			padding: var(--s-3);
		}
	}
</style>
