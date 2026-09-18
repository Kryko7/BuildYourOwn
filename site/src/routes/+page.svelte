<script lang="ts">
	import { journeyOwnerLabel, pageTitle } from '$lib/branding';
	import ProgressRing from '$lib/components/ProgressRing.svelte';
	import ConnectionNote from '$lib/components/ConnectionNote.svelte';
	import Cat from '$lib/components/garden/Cat.svelte';
	import Fox from '$lib/components/garden/Fox.svelte';
	import Bunny from '$lib/components/garden/Bunny.svelte';
	import Flutterers from '$lib/components/garden/Flutterers.svelte';
	import { catalogs, tracks, trackIds } from '$lib/catalog';
	import { progress, rankName } from '$lib/stores/progress.svelte';
	import { journey } from '$lib/stores/journey.svelte';
	import { activeReport } from '$lib/stores/reports.svelte';
	import { stageOutcome, formatDuration } from '$lib/report';
	import type { TrackId } from '$lib/types';

	const accents: Record<string, string> = { shell: 'var(--shell)', kafka: 'var(--kafka)' };

	const totals = $derived({
		stages: trackIds.reduce((n, t) => n + catalogs[t].totals.stages, 0),
		tests: trackIds.reduce((n, t) => n + catalogs[t].totals.tests, 0),
		done: trackIds.reduce((n, t) => n + progress.doneCount(t), 0)
	});

	const level = $derived(progress.level);
	const lastRun = $derived(journey.runs[0] ?? null);

	function redStages(track: TrackId) {
		const r = activeReport(track);
		if (!r) return 0;
		return catalogs[track].stages.filter((s) => {
			const o = stageOutcome(r, s.number);
			return o === 'red' || o === 'partial';
		}).length;
	}
</script>

<svelte:head>
	<title>{pageTitle()} — build your own shell and your own Kafka</title>
</svelte:head>

<section class="hero">
	<Flutterers count={6} seed={11} />
	<div class="wrap heroin">
		<div class="words">
			<p class="eyebrow">two trails · {totals.stages} stages · {totals.tests} tests</p>
			<h1>{journeyOwnerLabel} <span class="ink">Journey</span></h1>
			<p class="lede">
				A shell and a Kafka broker, grown from nothing, one flower at a time. Every stage tells you
				what to build, shows you what to expect, lists the tests it has to satisfy and the exact
				command to run — then blooms when your code earns it.
			</p>
			<div class="cta row">
				<a class="btn btn-primary" href="/shell" style="--accent:var(--shell)">Tend the shell →</a>
				<a class="btn" href="/kafka">Tend Kafka →</a>
				<a class="btn btn-ghost" href="/lab">Open the lab</a>
			</div>
		</div>
		<div class="critters" aria-hidden="true">
			<span class="cat"><Cat size={150} /></span>
		</div>
	</div>
	<div class="hedge" aria-hidden="true">
		<svg viewBox="0 0 1200 90" preserveAspectRatio="none">
			<path
				d="M0 90 L0 56 Q 40 30 86 52 Q 120 24 160 50 Q 205 22 250 52 Q 290 28 336 50 Q 380 22 424 52
				   Q 470 26 512 50 Q 556 22 600 52 Q 646 26 690 50 Q 734 22 780 52 Q 824 26 868 50
				   Q 912 22 958 52 Q 1000 26 1046 50 Q 1092 22 1136 52 Q 1170 32 1200 54 L1200 90 Z"
				fill="var(--bg-2)"
				stroke="var(--line)"
			/>
		</svg>
	</div>
</section>

<div class="wrap main">
	<ConnectionNote />

	<section class="stats card">
		<div class="stat">
			<span class="eyebrow">level</span>
			<strong>{level.level}</strong>
			<span class="tiny muted">{rankName(level.level)}</span>
			<div class="xpbar" aria-hidden="true">
				<i style="width:{Math.min(100, (level.into / level.span) * 100)}%"></i>
			</div>
			<span class="tiny muted">{progress.xp} XP · {Math.max(0, level.ceil - progress.xp)} to next</span>
		</div>
		<div class="stat">
			<span class="eyebrow">stages in bloom</span>
			<strong>{totals.done}<span class="of">/{totals.stages}</span></strong>
			<span class="tiny muted">across both trails</span>
		</div>
		<div class="stat">
			<span class="eyebrow">streak</span>
			<strong>{progress.streak}<span class="of"> day{progress.streak === 1 ? '' : 's'}</span></strong>
			<span class="tiny muted">
				{progress.streak > 0 ? 'keep watering' : 'finish a stage to start one'}
			</span>
		</div>
		<div class="stat">
			<span class="eyebrow">{lastRun ? 'last run' : 'tests in the suites'}</span>
			{#if lastRun}
				<strong class="small">{lastRun.passed}<span class="of">/{lastRun.passed + lastRun.failed}</span></strong>
				<span class="tiny muted">
					{tracks[lastRun.track].tester} · {lastRun.target} · {formatDuration(lastRun.elapsedMs)}
				</span>
			{:else}
				<strong>{totals.tests}</strong>
				<span class="tiny muted">generated from the testers</span>
			{/if}
		</div>
	</section>

	<section class="tracks">
		{#each trackIds as id (id)}
			{@const cat = catalogs[id]}
			{@const meta = tracks[id]}
			{@const done = progress.doneCount(id)}
			{@const next = progress.nextStage(id)}
			{@const red = redStages(id)}
			<a class="track card" href="/{id}" style="--accent:{accents[id]}">
				<div class="thead">
					<span class="mascot" aria-hidden="true">
						{#if id === 'shell'}<Fox size={62} />{:else}<Bunny size={62} />{/if}
					</span>
					<div>
						<h2>{meta.title}</h2>
						<p class="tiny muted">{meta.tagline}</p>
					</div>
					<ProgressRing value={done} total={cat.totals.stages} accent={accents[id]} size={72} />
				</div>
				<p class="blurb">{meta.blurb}</p>
				<div class="tmeta tiny">
					<span class="chip">{cat.totals.stages} stages</span>
					<span class="chip">{cat.totals.tests || '—'} tests</span>
					<span class="chip chip-ext">{cat.totals.ext} beyond the base track</span>
					{#if cat.pending}<span class="chip">catalog pending</span>{/if}
					{#if red}<span class="chip chip-bad">{red} wilting</span>{/if}
				</div>
				<div class="cont">
					<span class="eyebrow">continue at</span>
					<strong
						>Stage {String(next).padStart(2, '0')} — {cat.stages.find((s) => s.number === next)
							?.name}</strong
					>
				</div>
			</a>
		{/each}
	</section>

	{#if progress.recent.length}
		<section class="recent">
			<h2>Recently in bloom</h2>
			<ul>
				{#each progress.recent as a, ai (ai)}
					<li>
						<span class="dot" style="--accent:{accents[a.track]}" aria-hidden="true"></span>
						<a href="/{a.track}/{a.stage}">
							{tracks[a.track].tester} stage {String(a.stage).padStart(2, '0')}
						</a>
						<span class="muted tiny">{a.kind === 'done' ? 'completed' : 'reset'}</span>
						<time class="tiny muted" datetime={a.at}>{new Date(a.at).toLocaleDateString()}</time>
					</li>
				{/each}
			</ul>
		</section>
	{:else}
		<section class="recent empty">
			<h2>Nothing has flowered yet</h2>
			<p class="muted">
				Open a trail, read what stage 01 wants, run <code>byo test --stage 1</code>, and watch the
				first bud open. {#if journey.connected}Everything you do is recorded in byo’s database.{:else}Run
					<code>byo site</code> to record it properly — right now this page only remembers things in this
					browser.{/if}
			</p>
		</section>
	{/if}
</div>

<style>
	.hero {
		position: relative;
		padding: var(--s-8) 0 var(--s-8);
		overflow: hidden;
	}
	.hero::before {
		content: '';
		position: absolute;
		inset: 0;
		background:
			radial-gradient(64% 80% at 12% 0%, color-mix(in srgb, var(--pink) 40%, transparent), transparent 66%),
			radial-gradient(58% 70% at 88% 8%, color-mix(in srgb, var(--sky) 38%, transparent), transparent 62%),
			radial-gradient(46% 56% at 62% 96%, color-mix(in srgb, var(--butter) 34%, transparent), transparent 64%);
		pointer-events: none;
	}
	.heroin {
		position: relative;
		z-index: 2;
		display: flex;
		align-items: center;
		gap: var(--s-6);
	}
	.words {
		flex: 1;
		min-width: 0;
	}
	.critters {
		flex: none;
		display: grid;
		place-items: end center;
	}
	.cat {
		display: block;
		animation: driftY 6.5s ease-in-out infinite;
	}
	@media (max-width: 860px) {
		.critters {
			display: none;
		}
	}
	h1 {
		margin: var(--s-2) 0 var(--s-3);
		max-width: 16ch;
	}
	h1 .ink {
		color: var(--pink-ink);
		font-style: italic;
	}
	.lede {
		max-width: 58ch;
		font-size: 1.06rem;
		color: var(--ink-2);
	}
	.cta {
		margin-top: var(--s-5);
		flex-wrap: wrap;
	}
	.hedge {
		position: absolute;
		left: 0;
		right: 0;
		bottom: -1px;
		height: 64px;
		z-index: 1;
	}
	.hedge svg {
		width: 100%;
		height: 100%;
		display: block;
	}

	.main {
		display: flex;
		flex-direction: column;
		gap: var(--s-6);
		padding-bottom: var(--s-7);
		position: relative;
		z-index: 2;
	}

	.stats {
		display: grid;
		grid-template-columns: repeat(4, 1fr);
		gap: var(--s-4);
		padding: var(--s-4) var(--s-5);
	}
	@media (max-width: 760px) {
		.stats {
			grid-template-columns: repeat(2, 1fr);
		}
	}
	.stat {
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.stat strong {
		font-family: var(--font-display);
		font-size: 2.1rem;
		line-height: 1.1;
	}
	.stat strong.small {
		font-size: 1.7rem;
	}
	.stat .of {
		font-size: 0.5em;
		color: var(--ink-3);
	}
	.xpbar {
		height: 7px;
		background: var(--bg-3);
		border-radius: var(--r-full);
		overflow: hidden;
		margin: 5px 0 3px;
	}
	.xpbar i {
		display: block;
		height: 100%;
		background: linear-gradient(90deg, var(--pink), var(--butter));
		border-radius: inherit;
		transition: width 700ms var(--ease);
	}

	.tracks {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: var(--s-4);
	}
	@media (max-width: 860px) {
		.tracks {
			grid-template-columns: 1fr;
		}
	}
	.track {
		display: flex;
		flex-direction: column;
		gap: var(--s-3);
		padding: var(--s-5);
		text-decoration: none;
		transition:
			transform var(--dur) var(--ease),
			box-shadow var(--dur) var(--ease),
			border-color var(--dur) var(--ease);
		position: relative;
		overflow: hidden;
	}
	.track::after {
		content: '';
		position: absolute;
		inset: 0 0 auto 0;
		height: 4px;
		background: linear-gradient(90deg, var(--accent), color-mix(in srgb, var(--accent) 30%, var(--butter)));
	}
	.track:hover {
		transform: translateY(-4px);
		box-shadow: var(--shadow-2);
		border-color: color-mix(in srgb, var(--accent) 40%, var(--line));
	}
	.thead {
		display: flex;
		align-items: center;
		gap: var(--s-3);
	}
	.thead > div {
		flex: 1;
		min-width: 0;
	}
	.thead h2 {
		font-size: 1.32rem;
	}
	.mascot {
		flex: none;
		width: 62px;
		height: 62px;
		display: grid;
		place-items: center;
		transition: transform 260ms var(--bounce);
	}
	.track:hover .mascot {
		transform: translateY(-4px) rotate(-4deg);
	}
	.blurb {
		color: var(--ink-2);
		margin: 0;
		font-size: 0.92rem;
	}
	.tmeta {
		display: flex;
		flex-wrap: wrap;
		gap: 5px;
	}
	.cont {
		margin-top: auto;
		padding-top: var(--s-3);
		border-top: 1px dashed var(--line);
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.cont strong {
		font-weight: 600;
		font-size: 0.92rem;
	}

	.recent ul {
		list-style: none;
		margin: var(--s-3) 0 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.recent li {
		display: flex;
		align-items: baseline;
		gap: var(--s-2);
		padding: 6px 0;
		border-bottom: 1px solid var(--line);
	}
	.dot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--accent);
		flex: none;
	}
	.recent time {
		margin-left: auto;
	}
	.recent.empty p {
		max-width: 62ch;
		margin-top: var(--s-2);
	}
</style>
