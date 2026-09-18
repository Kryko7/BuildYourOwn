<script lang="ts">
	/** One shell example, drawn as the terminal session the tester actually runs. */
	import Transcript from './Transcript.svelte';
	import type { ShellExample } from '$lib/types';

	let { example }: { example: ShellExample } = $props();

	const chips = $derived([
		{ label: example.mode === 'pty' ? 'pseudo-terminal' : 'stdin' },
		...(example.exact
			? []
			: [
					{
						label: 'pattern',
						title:
							'The suite matches loosely here, so the output below is described rather than quoted'
					}
				])
	]);
</script>

<Transcript lines={example.lines} title={example.title} {chips} />
