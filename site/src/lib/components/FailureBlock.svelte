<script lang="ts">
	import { caretNotation } from '$lib/report';
	import { lineDiff, hasDifference } from '$lib/diff';
	import type { ExpectRow, ReportTest, TestSpec } from '$lib/types';

	let { test, spec }: { test: ReportTest; spec?: TestSpec } = $props();

	const inputLines = $derived(
		spec?.input?.length
			? spec.input
			: spec?.keys?.length
				? spec.keys.map((k) => caretNotation(k))
				: spec?.steps?.length
					? spec.steps.map((s) => `${s.op}: ${caretNotation(s.value)}`)
					: []
	);

	function expectedFor(label: string): ExpectRow | undefined {
		return spec?.expect?.find((e) => e.label === label || (label === 'exit code' && e.label === 'exit code'));
	}

	const diffs = $derived(
		test.actual
			.map(([label, actual]) => {
				const expected = expectedFor(label);
				if (!expected || expected.kind !== 'exact') return null;
				const lines = lineDiff(expected.value, actual);
				return hasDifference(lines) ? { label, lines } : null;
			})
			.filter((d): d is { label: string; lines: ReturnType<typeof lineDiff> } => d !== null)
	);
</script>

<div class="terminal">
	{#each test.failures as f, fi (fi)}<span class="t-del">{f}</span>{'\n'}{/each}
	{#if inputLines.length}<span class="t-gut">┌</span> <span class="t-head"
			>{spec?.mode === 'pty' ? 'key steps sent to the terminal' : 'input sent on stdin'}</span
		>{'\n'}{#each inputLines as line, li (li)}<span class="t-gut">│</span> {line}{'\n'}{/each}{/if}
	{#if spec?.expect?.length}<span class="t-gut">┌</span> <span class="t-head">expected</span
		>{'\n'}{#each spec.expect as e, ei (ei)}<span class="t-gut">│</span> {e.label}: {e.kind ===
		'exact'
			? JSON.stringify(e.value)
			: `${e.kind} ${JSON.stringify(e.value)}`}{'\n'}{/each}{/if}
	{#if test.actual.length}<span class="t-gut">┌</span> <span class="t-head">actual (normalized)</span
		>{'\n'}{#each test.actual as [label, value], ai (ai)}<span class="t-gut">│</span> {label}: {JSON.stringify(
			caretNotation(value)
		)}{'\n'}{/each}{/if}
	{#each diffs as d, di (di)}<span class="t-gut">┌</span> <span class="t-head"
			>diff {d.label}  (-expected  +actual)</span
		>{'\n'}{#each d.lines as l, i (i)}<span class="t-gut">│</span> <span
				class={l.sign === '-' ? 't-del' : l.sign === '+' ? 't-add' : ''}>{l.sign}{l.text}</span
			>{'\n'}{/each}{/each}
</div>
