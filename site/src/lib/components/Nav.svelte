<script lang="ts">
	import { journeyTitle } from '$lib/branding';
	import { page } from '$app/state';
	import Bird from './garden/Bird.svelte';
	import { theme } from '$lib/stores/theme.svelte';
	import { reports } from '$lib/stores/reports.svelte';
	import { journey } from '$lib/stores/journey.svelte';
	import { progress, rankName } from '$lib/stores/progress.svelte';
	import { allTracks, trackIds } from '$lib/tracks';
	import { catalogs } from '$lib/catalog';

	let { onpalette }: { onpalette: () => void } = $props();

	/**
	 * Six trails will not fit next to the brand and the tools, so they live behind one
	 * "Trails" button; the pages that are not trails stay as their own links. The menu is
	 * built from the registry, so a new track appears in it with no change here.
	 */
	const trailLinks = allTracks.map((t) => ({
		href: `/${t.id}`,
		label: t.short,
		title: t.title,
		accent: t.accent,
		pending: catalogs[t.id].pending === true
	}));

	const pageLinks = [
		{ href: '/resources', label: 'Resources' },
		{ href: '/lab', label: 'Lab' },
		{ href: '/progress', label: 'Runs' }
	];

	const current = $derived(page.url.pathname.replace(/\/+$/, '') || '/');
	const live = $derived(trackIds.some((t) => reports.live[t] !== null));
	/* Three states worth a pill: connected to byo, connected-but-stalled, and no byo at all.
	   `probing` shows nothing — a flash of "offline" on every load helps nobody. */
	const link = $derived(journey.status);
	const themeIcon = $derived(theme.choice === 'system' ? '◐' : theme.choice === 'dark' ? '☾' : '☀');
	const themeName = $derived(
		theme.choice === 'system' ? 'follow the system' : theme.choice === 'dark' ? 'dark' : 'light'
	);

	const isActive = (href: string) => current === href || current.startsWith(href + '/');
	const onATrail = $derived(trailLinks.some((l) => isActive(l.href)));
	const currentTrail = $derived(trailLinks.find((l) => isActive(l.href)) ?? null);

	/* Two popovers: the trails menu at every width (six trails will not fit in a row), and
	   the whole nav below 760 px, where even the three page links push the bar sideways. */
	let trailsOpen = $state(false);
	let menuOpen = $state(false);
	let header: HTMLElement | undefined = $state();

	// Any navigation closes both; `current` changes on every route change.
	$effect(() => {
		current;
		menuOpen = false;
		trailsOpen = false;
	});

	function onWindowKeydown(e: KeyboardEvent) {
		if (e.key !== 'Escape') return;
		menuOpen = false;
		trailsOpen = false;
	}

	function onWindowPointerDown(e: PointerEvent) {
		if (!menuOpen && !trailsOpen) return;
		if (header && !header.contains(e.target as Node)) {
			menuOpen = false;
			trailsOpen = false;
		}
	}
</script>

<svelte:window onkeydown={onWindowKeydown} onpointerdown={onWindowPointerDown} />

<header bind:this={header}>
	<div class="wrap bar">
		<a class="brand" href="/">
			<span class="mark" aria-hidden="true">
				<svg viewBox="0 0 32 32" width="30" height="30" class="petal-mark">
					<g class="turn">
						{#each [0, 72, 144, 216, 288] as a (a)}
							<ellipse cx="16" cy="9.5" rx="5" ry="7.2" transform="rotate({a} 16 16)" />
						{/each}
					</g>
					<circle cx="16" cy="16" r="4.1" class="heart" />
				</svg>
				<span class="perch"><Bird size={20} /></span>
			</span>
			<span class="name">{journeyTitle.replace(' ', '\u00a0')}</span>
		</a>

		<nav class="wide" aria-label="Primary">
			<div class="trailsbox">
				<button
					class="trailsbtn"
					class:active={onATrail}
					aria-expanded={trailsOpen}
					aria-controls="trails-menu"
					onclick={() => (trailsOpen = !trailsOpen)}
					style={currentTrail ? `--accent:${currentTrail.accent}` : ''}
				>
					{currentTrail ? currentTrail.label : 'Trails'}
					<span class="caret" aria-hidden="true">▾</span>
				</button>
				<div id="trails-menu" class="trails" class:open={trailsOpen} hidden={!trailsOpen}>
					{#each trailLinks as l (l.href)}
						<a href={l.href} class:active={isActive(l.href)} style="--accent:{l.accent}" title={l.title}>
							<span class="swatch" aria-hidden="true"></span>
							<span class="tlabel">{l.label}</span>
							{#if l.pending}<span class="tiny muted soon">being written</span>{/if}
						</a>
					{/each}
				</div>
			</div>
			{#each pageLinks as l (l.href)}
				<a href={l.href} class:active={isActive(l.href)}>{l.label}</a>
			{/each}
		</nav>

		<div class="tools">
			{#if link === 'connected'}
				<span class="pill ok" title="Progress is coming from byo's database">
					<i aria-hidden="true"></i> byo
				</span>
			{:else if link === 'reconnecting'}
				<span class="pill warn" title="byo stopped answering — showing the last data it gave">
					<i aria-hidden="true"></i> reconnecting
				</span>
			{:else if link === 'offline' && live}
				<span class="pill ok" title="A tester report is being polled from disk (dev mode)">
					<i aria-hidden="true"></i> live file
				</span>
			{:else if link === 'offline'}
				<a class="pill off" href="/progress" title="Not connected to byo — run `byo site`">
					<i aria-hidden="true"></i> local only
				</a>
			{/if}
			<span class="rank tiny muted" title="XP {progress.xp}">
				Lv{progress.level.level} · {rankName(progress.level.level)}
			</span>
			<button class="btn btn-sm btn-ghost kbd-btn" onclick={onpalette} aria-label="Open the command palette">
				<span aria-hidden="true">⌕</span><kbd>Ctrl K</kbd>
			</button>
			<button
				class="btn btn-sm btn-ghost"
				onclick={() => theme.cycle()}
				aria-label="Theme: {themeName}. Click to change."
				title="Theme: {themeName}"
			>
				<span aria-hidden="true">{themeIcon}</span>
			</button>
			<button
				class="btn btn-sm btn-ghost menubtn"
				onclick={() => (menuOpen = !menuOpen)}
				aria-expanded={menuOpen}
				aria-controls="nav-menu"
				aria-label={menuOpen ? 'Close the menu' : 'Open the menu'}
			>
				<span aria-hidden="true">{menuOpen ? '✕' : '☰'}</span>
			</button>
		</div>
	</div>

	<nav id="nav-menu" class="drop" class:open={menuOpen} aria-label="Primary" hidden={!menuOpen}>
		<div class="wrap">
			<span class="group eyebrow">trails</span>
			{#each trailLinks as l (l.href)}
				<a href={l.href} class:active={isActive(l.href)} style="--accent:{l.accent}">
					<span class="swatch" aria-hidden="true"></span>
					{l.label}
					{#if l.pending}<span class="tiny muted soon">being written</span>{/if}
				</a>
			{/each}
			<span class="group eyebrow">the rest</span>
			{#each pageLinks as l (l.href)}
				<a href={l.href} class:active={isActive(l.href)}>{l.label}</a>
			{/each}
			<span class="rank tiny muted">Lv{progress.level.level} · {rankName(progress.level.level)}</span>
		</div>
	</nav>
</header>

<style>
	header {
		position: sticky;
		top: 0;
		z-index: 60;
		background: color-mix(in srgb, var(--bg) 88%, transparent);
		backdrop-filter: blur(10px) saturate(1.2);
		border-bottom: 1px solid var(--line);
	}
	.bar {
		display: flex;
		align-items: center;
		gap: var(--s-5);
		height: 58px;
	}
	.brand {
		display: flex;
		align-items: center;
		/* wide enough that the perched bird never sits on the wordmark */
		gap: var(--s-3);
		text-decoration: none;
		flex: none;
		min-width: 0;
	}
	.mark {
		position: relative;
		display: grid;
		place-items: center;
		flex: none;
		width: 30px;
		height: 30px;
	}
	.petal-mark ellipse {
		fill: var(--pink);
	}
	.petal-mark .heart {
		fill: var(--butter);
	}
	.petal-mark .turn {
		transform-origin: 16px 16px;
		transition: transform 700ms var(--bounce);
	}
	.brand:hover .petal-mark .turn {
		transform: rotate(36deg);
	}
	/* A bird perched on the flower, always there but quiet; it leans in when you hover the
	   brand so it reads as alive rather than as a second logo. */
	.perch {
		position: absolute;
		right: -9px;
		top: -11px;
		pointer-events: none;
		transform-origin: 50% 100%;
		transition: transform 320ms var(--bounce);
	}
	.brand:hover .perch,
	.brand:focus-visible .perch {
		transform: translateY(-3px) rotate(-8deg);
	}
	.name {
		font-family: var(--font-display);
		font-weight: 600;
		font-size: 1.06rem;
		letter-spacing: -0.01em;
		white-space: nowrap;
	}
	nav.wide {
		display: flex;
		gap: var(--s-1);
		flex: 1 1 auto;
		min-width: 0;
		overflow-x: auto;
		overscroll-behavior-x: contain;
		scrollbar-width: none;
	}
	nav.wide::-webkit-scrollbar {
		display: none;
	}
	nav a {
		padding: 5px 13px;
		border-radius: var(--r-full);
		text-decoration: none;
		color: var(--ink-2);
		font-size: 0.88rem;
		white-space: nowrap;
		transition: background var(--dur) var(--ease), color var(--dur) var(--ease);
	}
	nav a:hover {
		background: var(--bg-3);
		color: var(--ink);
	}
	nav a.active {
		color: var(--ink);
		background: var(--pink-soft);
		font-weight: 700;
	}
	/* the trails popover */
	.trailsbox {
		position: relative;
		flex: none;
	}
	.trailsbtn {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		background: none;
		border: 0;
		border-radius: var(--r-full);
		padding: 5px 13px;
		font-size: 0.88rem;
		color: var(--ink-2);
		cursor: pointer;
		white-space: nowrap;
	}
	.trailsbtn:hover {
		background: var(--bg-3);
		color: var(--ink);
	}
	.trailsbtn.active {
		color: var(--ink);
		background: color-mix(in srgb, var(--accent, var(--pink)) 16%, transparent);
		font-weight: 700;
	}
	.caret {
		font-size: 0.7em;
		color: var(--ink-3);
	}
	.trails {
		position: absolute;
		top: calc(100% + 6px);
		left: 0;
		z-index: 70;
		min-width: 220px;
		background: var(--bg-2);
		border: 1px solid var(--line);
		border-radius: var(--r-2);
		box-shadow: var(--shadow-2);
		padding: 5px;
		display: flex;
		flex-direction: column;
		gap: 1px;
	}
	.trails a {
		display: flex;
		align-items: center;
		gap: var(--s-2);
		padding: 6px 9px;
	}
	.swatch {
		width: 9px;
		height: 9px;
		border-radius: 50%;
		background: var(--accent, var(--ink-3));
		flex: none;
	}
	.tlabel {
		flex: 1;
	}
	.soon {
		font-size: 0.68rem;
	}
	nav.wide a.active::after {
		content: '';
		position: absolute;
		left: 50%;
		bottom: 1px;
		width: 5px;
		height: 5px;
		border-radius: 50%;
		background: var(--pink-ink);
		transform: translateX(-50%);
	}
	nav.wide a {
		position: relative;
	}
	.tools {
		display: flex;
		align-items: center;
		gap: var(--s-2);
		flex: none;
		margin-left: auto;
	}
	.rank {
		font-family: var(--font-mono);
	}
	.pill {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-family: var(--font-mono);
		font-size: 0.68rem;
		text-transform: uppercase;
		letter-spacing: 0.1em;
		border-radius: var(--r-full);
		padding: 2px 10px;
		text-decoration: none;
		border: 1px solid transparent;
	}
	.pill i {
		width: 7px;
		height: 7px;
		border-radius: 50%;
		background: currentColor;
	}
	.pill.ok {
		color: var(--ok);
		background: var(--ok-soft);
		border-color: color-mix(in srgb, var(--ok) 28%, transparent);
	}
	.pill.ok i {
		box-shadow: 0 0 0 0 color-mix(in srgb, var(--ok) 55%, transparent);
		animation: pulse 2.2s infinite;
	}
	.pill.warn {
		color: var(--warn);
		background: var(--warn-soft);
		border-color: color-mix(in srgb, var(--warn) 28%, transparent);
	}
	.pill.off {
		color: var(--ink-3);
		background: var(--bg-3);
		border-color: var(--line);
	}
	@keyframes pulse {
		70% {
			box-shadow: 0 0 0 7px transparent;
		}
		100% {
			box-shadow: 0 0 0 0 transparent;
		}
	}
	.kbd-btn kbd {
		margin-left: 2px;
	}

	/* the collapsed menu — only ever shown on narrow screens */
	.menubtn {
		display: none;
		font-size: 1rem;
		line-height: 1;
	}
	nav.drop {
		display: none;
		border-top: 1px solid var(--line);
		background: var(--bg-2);
		padding: var(--s-2) 0 var(--s-3);
		box-shadow: var(--shadow-1);
	}
	nav.drop .wrap {
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	nav.drop a {
		padding: 9px 11px;
	}
	nav.drop .rank {
		padding: var(--s-2) 11px 0;
	}
	nav.drop .group {
		padding: var(--s-2) 11px 2px;
	}
	nav.drop a {
		display: flex;
		align-items: center;
		gap: var(--s-2);
	}

	@media (max-width: 900px) {
		.rank,
		.pill,
		.kbd-btn kbd {
			display: none;
		}
		nav.drop .rank {
			display: block;
		}
	}
	@media (max-width: 760px) {
		.bar {
			gap: var(--s-3);
		}
		nav.wide {
			display: none;
		}
		.menubtn {
			display: inline-flex;
		}
		nav.drop.open {
			display: block;
		}
	}
	@media (max-width: 400px) {
		.name {
			display: none;
		}
	}
</style>
