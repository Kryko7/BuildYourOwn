<script lang="ts">
	import FailureBlock from './FailureBlock.svelte';
	import { formatDuration } from '$lib/report';
	import type { ReportTest, TestSpec } from '$lib/types';

	let {
		tests,
		results = {},
		pending = false,
		planned = false,
		tester = 'the tester'
	}: {
		tests: TestSpec[];
		results?: Record<string, ReportTest>;
		/** The whole catalog is a placeholder (the tester has published no catalog.json). */
		pending?: boolean;
		/** This one stage is in the tester's PLAN.md but not in its catalog.json yet. */
		planned?: boolean;
		/** The tester's name, so the message names the crate the learner is waiting on. */
		tester?: string;
	} = $props();

	// Keyed by index, never by content: a suite may legitimately repeat a test name or a
	// stdin line, and a duplicate `{#each}` key is a hard Svelte error that kills the page.
	let expanded = $state<Record<number, boolean>>({});

	const statusOf = (t: TestSpec) => results[t.name]?.status ?? null;
	const counts = $derived({
		pass: tests.filter((t) => statusOf(t) === 'pass').length,
		fail: tests.filter((t) => statusOf(t) === 'fail').length,
		skip: tests.filter((t) => statusOf(t) === 'skip').length
	});
</script>

{#if tests.length === 0 && (planned || pending)}
	<p class="notyet">
		<strong>Not yet in the tester.</strong>
		{#if planned}
			This stage is listed in <code>{tester}</code>’s plan but has no suite in
			<code>catalog.json</code> yet, so there is nothing to run against it. What to build below comes
			straight from the plan; re-run <code>npm run sync</code> once the stage lands and its tests will
			appear here.
		{:else}
			<code>{tester}</code> has not published a <code>catalog.json</code> yet — the stage list and
			hints come from the plan in the meantime.
		{/if}
	</p>
{:else}
	<div class="summary tiny">
		<span class="muted">{tests.length} test{tests.length === 1 ? '' : 's'}</span>
		{#if counts.pass}<span class="chip chip-ok">{counts.pass} passing</span>{/if}
		{#if counts.fail}<span class="chip chip-bad">{counts.fail} failing</span>{/if}
		{#if counts.skip}<span class="chip">{counts.skip} skipped</span>{/if}
	</div>

	<ul class="tests">
		{#each tests as t, ti (ti)}
			{@const result = results[t.name]}
			{@const status = result?.status ?? null}
			{@const open = expanded[ti] ?? false}
			<li class={status ?? 'unknown'}>
				<div class="head">
					<span class="mark" aria-hidden="true"
						>{status === 'pass' ? '✔' : status === 'fail' ? '✘' : status === 'skip' ? '–' : '·'}</span
					>
					<button
						class="name"
						onclick={() => (expanded = { ...expanded, [ti]: !open })}
						aria-expanded={open}
					>
						{t.name}
					</button>
					<span class="flags">
						{#if t.ext}<span class="chip chip-ext">ext</span>{/if}
						{#if t.mode === 'pty'}<span class="chip">pty</span>{/if}
						{#if t.skipOn?.length}<span class="chip" title={t.reason}>skip on {t.skipOn.join(', ')}</span
							>{/if}
						{#if result}<span class="tiny muted">{formatDuration(result.durationMs)}</span>{/if}
					</span>
				</div>
				{#if status === 'fail' && result}
					<p class="why tiny">{result.failures[0] ?? 'failed'}</p>
				{:else if status === 'skip' && result?.skipReason}
					<p class="why tiny muted">skipped: {result.skipReason}</p>
				{/if}
				{#if open}
					<div class="detail">
						{#if result && (result.status === 'fail' || result.actual.length)}
							<FailureBlock test={result} spec={t} />
						{:else if t.expect?.length || t.input?.length || t.keys?.length}
							<div class="terminal">
								{#if t.input?.length}<span class="t-gut">┌</span> <span class="t-head">input sent on stdin</span
									>{'\n'}{#each t.input as line, li (li)}<span class="t-gut">│</span> {line}{'\n'}{/each}{/if}
								{#if t.keys?.length}<span class="t-gut">┌</span> <span class="t-head"
										>key steps sent to the terminal</span
									>{'\n'}{#each t.keys as kk, i (i)}<span class="t-gut">│</span> {JSON.stringify(kk)}{'\n'}{/each}{/if}
								{#if t.expect?.length}<span class="t-gut">┌</span> <span class="t-head">expected</span
									>{'\n'}{#each t.expect as e, ei (ei)}<span class="t-gut">│</span> {e.label}: {e.kind ===
									'exact'
										? JSON.stringify(e.value)
										: `${e.kind} ${JSON.stringify(e.value)}`}{'\n'}{/each}{/if}
							</div>
						{:else}
							<p class="tiny muted">No expectation recorded for this test yet.</p>
						{/if}
					</div>
				{/if}
			</li>
		{/each}
	</ul>
{/if}

<style>
	.notyet {
		margin: 0;
		padding: var(--s-3);
		border: 1px dashed color-mix(in srgb, var(--warn) 45%, var(--line));
		border-radius: var(--r-2);
		background: var(--warn-soft);
		color: var(--ink-2);
		font-size: 0.88rem;
	}
	.notyet strong {
		color: var(--warn);
	}
	.summary {
		display: flex;
		align-items: center;
		gap: var(--s-2);
		margin-bottom: var(--s-2);
	}
	ul.tests {
		list-style: none;
		margin: 0;
		padding: 0;
		border: 1px solid var(--line);
		border-radius: var(--r-2);
		overflow: hidden;
	}
	li {
		border-top: 1px solid var(--line);
		padding: var(--s-2) var(--s-3);
	}
	li:first-child {
		border-top: 0;
	}
	li.fail {
		background: color-mix(in srgb, var(--bad) 6%, transparent);
	}
	.head {
		display: flex;
		align-items: baseline;
		gap: var(--s-2);
	}
	.mark {
		font-family: var(--font-mono);
		width: 1em;
		flex: none;
		color: var(--ink-3);
	}
	li.pass .mark {
		color: var(--ok);
	}
	li.fail .mark {
		color: var(--bad);
	}
	li.skip .mark {
		color: var(--warn);
	}
	.name {
		flex: 1;
		text-align: left;
		background: none;
		border: 0;
		padding: 0;
		cursor: pointer;
		font-size: 0.88rem;
		line-height: 1.45;
		color: inherit;
	}
	.name:hover {
		text-decoration: underline;
		text-underline-offset: 3px;
	}
	.flags {
		display: flex;
		align-items: center;
		gap: 5px;
		flex: none;
	}
	.why {
		margin: 2px 0 0 calc(1em + var(--s-2));
		color: var(--bad);
		font-family: var(--font-mono);
	}
	.detail {
		margin-top: var(--s-2);
	}
	@media (max-width: 560px) {
		.flags {
			display: none;
		}
	}
</style>
