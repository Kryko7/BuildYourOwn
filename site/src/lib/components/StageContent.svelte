<script lang="ts">
	import CopyButton from './CopyButton.svelte';
	import TestList from './TestList.svelte';
	import StageExamples from './examples/StageExamples.svelte';
	import StageLab from './lab/StageLab.svelte';
	import { badgeFor, commandFor, ladderOfSection, sectionOf, tracks, untilCommand } from '$lib/catalog';
	import { journey } from '$lib/stores/journey.svelte';
	import { conventionsForStage } from '$lib/conventions';
	import { resourcesForStage, typeGlyphs, typeLabels } from '$lib/resources';
	import { statusIndex } from '$lib/report';
	import { stageState, stateLabel } from '$lib/stage-state';
	import { progress } from '$lib/stores/progress.svelte';
	import { exampleCount } from '$lib/examples';
	import type { Catalog, Report, StageSpec, TrackId } from '$lib/types';

	let {
		track,
		stage,
		catalog,
		report,
		examples: preloadedExamples = null,
		oncomplete
	}: {
		track: TrackId;
		stage: StageSpec;
		catalog: Catalog;
		report: Report | null;
		/** Raw Kafka examples the route already loaded, so the stage page can prerender them. */
		examples?: unknown[] | null;
		oncomplete?: () => void;
	} = $props();

	const meta = $derived(tracks[track]);
	const section = $derived(sectionOf(track, stage.number));
	const badge = $derived(section ? badgeFor(track, section.id) : { badge: '', icon: '' });
	// Tracks with ladders (dist) say which rung a stage is on, in the crumbs and as a chip.
	const ladder = $derived(
		section ? ladderOfSection(track, section.id) : null
	);
	const target = $derived(progress.data.targets[track]);
	// With byo connected the command is the installed one, run from inside the project.
	const command = $derived(
		journey.connected ? `byo test --stage ${stage.number}` : commandFor(track, stage.number, target)
	);
	const runAll = $derived(
		journey.connected ? `byo test --until ${stage.number}` : untilCommand(track, stage.number, target)
	);
	const results = $derived(statusIndex(report)[stage.number] ?? {});
	const state = $derived(
		stageState({
			track,
			stage: stage.number,
			report,
			progress: progress.stage(track, stage.number),
			nextStage: progress.nextStage(track)
		})
	);
	const stageResult = $derived(report?.stages.find((s) => s.stage === stage.number));
	const conventions = $derived(conventionsForStage(track, stage.number));
	const resources = $derived(resourcesForStage(track, stage.number));
	const notes = $derived(progress.stage(track, stage.number).notes ?? '');
	const examples = $derived(exampleCount(track, stage));

	function toggleDone() {
		const newlyDone = progress.toggle(track, stage.number);
		if (newlyDone) oncomplete?.();
	}
</script>

<article class="stage">
	<header>
		<div class="crumbs eyebrow">
			<span>{meta.tester}</span>
			{#if ladder}<span aria-hidden="true">/</span><span>{ladder.title}</span>{/if}
			<span aria-hidden="true">/</span>
			<span>{badge.icon} {badge.badge}</span>
			{#if section}<span aria-hidden="true">/</span><span>{section.title}</span>{/if}
		</div>
		<h2>
			<span class="num mono">{String(stage.number).padStart(2, '0')}</span>
			{stage.name}
		</h2>
		<div class="flags">
			<span class="chip chip-{state === 'done' ? 'ok' : state === 'failing' ? 'bad' : ''}">{stateLabel[state]}</span>
			{#if ladder}<span class="chip ladder" title={ladder.note}>{ladder.title} ladder</span>{/if}
			{#if stage.ext}<span class="chip chip-ext">beyond the base track</span>{/if}
			{#if stage.planned}<span class="chip chip-ext">not yet in the tester</span>{/if}
			{#if stage.file}<span class="chip">{stage.file}</span>{/if}
			{#if stageResult}
				<span class="chip {stageResult.failed ? 'chip-bad' : 'chip-ok'}">
					{stageResult.passed}/{stageResult.passed + stageResult.failed} passing
				</span>
			{/if}
		</div>
	</header>

	<section>
		<h3>What to build</h3>
		<ul class="hints">
			{#each stage.hints as hint, hi (hi)}
				<li>{hint}</li>
			{/each}
		</ul>
		{#if stage.hints.length === 0}
			<p class="muted">No hints recorded for this stage yet.</p>
		{/if}
	</section>

	{#if examples}
		<section>
			<h3>What to expect</h3>
			<StageExamples {track} {stage} preloaded={preloadedExamples} />
		</section>
	{/if}

	<section>
		<h3>Run it</h3>
		{#if stage.planned}
			<p class="tiny muted plannednote">
				<code>{meta.tester}</code> does not know this stage yet, so the command below will not select
				anything until it lands. It is here so the shape of the run is already familiar.
			</p>
		{/if}
		<div class="cmd">
			<code>{command}</code>
			<CopyButton text={command} />
		</div>
		<div class="cmd secondary">
			<code>{runAll}</code>
			<CopyButton text={runAll} label="copy" />
		</div>
		<label class="target tiny">
			<span class="muted">{meta.targetFlag}</span>
			<input
				value={target}
				oninput={(e) => progress.setTarget(track, e.currentTarget.value)}
				aria-label="Your {track} under test"
				spellcheck="false"
			/>
			<span class="muted">— the name in {meta.targetFile} or a path</span>
		</label>
	</section>

	<section>
		<h3>Tests</h3>
		<TestList
			tests={stage.tests}
			{results}
			pending={catalog.pending ?? false}
			planned={stage.planned ?? false}
			tester={meta.tester}
		/>
	</section>

	{#if conventions.length}
		<section>
			<h3>Conventions that apply here</h3>
			<dl class="conv">
				{#each conventions as c (c.id)}
					<dt>{c.topic}</dt>
					<dd>{c.rule} <span class="src tiny muted">— {c.source}</span></dd>
				{/each}
			</dl>
		</section>
	{/if}

	{#if resources.length}
		<section>
			<h3>Read for this stage</h3>
			<ul class="res">
				{#each resources as r (r.id)}
					<li>
						<a href={r.url} target="_blank" rel="noopener noreferrer">
							<span class="g" aria-hidden="true">{typeGlyphs[r.type]}</span>
							<span class="t">{r.title}</span>
							<span class="chip">{typeLabels[r.type]}</span>
							<span class="chip">{r.level}</span>
							{#if r.minutes}<span class="tiny muted">{r.minutes} min</span>{/if}
						</a>
						<p class="tiny muted">{r.why}</p>
					</li>
				{/each}
			</ul>
		</section>
	{/if}

	<StageLab {track} stage={stage.number} />

	<section>
		<h3>Notes</h3>
		<textarea
			rows="3"
			placeholder="What tripped you up here? Future you will want this."
			value={notes}
			oninput={(e) => progress.setNotes(track, stage.number, e.currentTarget.value)}
			aria-label="Notes for stage {stage.number}"
		></textarea>
	</section>

	<footer class="actions">
		<button class="btn btn-primary" onclick={toggleDone}>
			{progress.isDone(track, stage.number) ? 'Reset this stage' : 'Mark stage done'}
		</button>
		{#if state === 'failing'}
			<span class="tiny" style="color:var(--bad)">The tester is red on this stage right now.</span>
		{/if}
	</footer>
</article>

<style>
	.stage {
		display: flex;
		flex-direction: column;
		gap: var(--s-5);
	}
	header {
		display: flex;
		flex-direction: column;
		gap: var(--s-2);
	}
	.crumbs {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
	}
	h2 {
		display: flex;
		align-items: baseline;
		gap: var(--s-3);
	}
	.num {
		color: var(--accent, var(--ink));
		font-size: 0.62em;
		font-weight: 500;
	}
	.flags {
		display: flex;
		flex-wrap: wrap;
		gap: 5px;
	}
	h3 {
		font-family: var(--font-mono);
		font-size: 0.72rem;
		text-transform: uppercase;
		letter-spacing: 0.14em;
		color: var(--ink-3);
		font-weight: 500;
		margin-bottom: var(--s-2);
	}
	.hints {
		margin: 0;
		padding: 0;
		list-style: none;
		display: flex;
		flex-direction: column;
		gap: var(--s-2);
	}
	.hints li {
		position: relative;
		padding-left: var(--s-5);
		line-height: 1.55;
	}
	.hints li::before {
		content: '✿';
		position: absolute;
		left: 0;
		top: 0;
		color: var(--accent, var(--ink));
		font-size: 0.7em;
		line-height: 2.3;
	}
	.cmd {
		display: flex;
		align-items: center;
		gap: var(--s-2);
		background: var(--bg-sunk);
		border: 1px solid var(--line);
		border-radius: var(--r-2);
		padding: var(--s-2) var(--s-2) var(--s-2) var(--s-3);
		overflow-x: auto;
	}
	.cmd code {
		background: none;
		padding: 0;
		white-space: nowrap;
		flex: 1;
		font-size: 0.82rem;
	}
	.plannednote {
		margin: 0 0 var(--s-2);
	}
	.cmd.secondary {
		margin-top: 5px;
		opacity: 0.72;
	}
	.target {
		display: flex;
		align-items: center;
		gap: 6px;
		margin-top: var(--s-2);
		flex-wrap: wrap;
	}
	.target input {
		background: var(--bg-3);
		border: 1px solid var(--line);
		border-radius: var(--r-1);
		padding: 2px 7px;
		font-family: var(--font-mono);
		font-size: 0.76rem;
		width: 12em;
	}
	.conv {
		margin: 0;
		display: grid;
		gap: var(--s-2);
	}
	.conv dt {
		font-weight: 600;
		font-size: 0.86rem;
	}
	.conv dd {
		margin: 0 0 var(--s-2);
		color: var(--ink-2);
		font-size: 0.88rem;
		border-left: 2px solid var(--line-strong);
		padding-left: var(--s-3);
	}
	.res {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--s-3);
	}
	.res a {
		display: flex;
		align-items: center;
		gap: 7px;
		flex-wrap: wrap;
		text-decoration: none;
		font-weight: 500;
	}
	.res a:hover .t {
		text-decoration: underline;
		text-underline-offset: 3px;
	}
	.res .g {
		color: var(--accent, var(--ink));
	}
	.res p {
		margin: 2px 0 0;
	}
	.ladder {
		color: var(--accent, var(--ink-2));
		border-color: color-mix(in srgb, var(--accent, var(--line)) 36%, transparent);
		background: color-mix(in srgb, var(--accent, var(--bg-3)) 10%, transparent);
	}
	textarea {
		width: 100%;
		background: var(--bg-sunk);
		border: 1px solid var(--line);
		border-radius: var(--r-2);
		padding: var(--s-3);
		font-family: var(--font-body);
		font-size: 0.9rem;
		resize: vertical;
	}
	.actions {
		display: flex;
		align-items: center;
		gap: var(--s-3);
		flex-wrap: wrap;
		padding-top: var(--s-2);
		border-top: 1px solid var(--line);
	}
</style>
