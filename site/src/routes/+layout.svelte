<script lang="ts">
	import { journeyTitle } from '$lib/branding';
	import '../app.css';
	import { onMount } from 'svelte';
	import Nav from '$lib/components/Nav.svelte';
	import CommandPalette from '$lib/components/CommandPalette.svelte';
	import { reports } from '$lib/stores/reports.svelte';
	import { journey } from '$lib/stores/journey.svelte';
	import { trackTabVisibility } from '$lib/motion';
	import { allTracks } from '$lib/tracks';

	/** The footer names its sources, from the registry rather than from memory. */
	const testers = allTracks.map((t) => t.tester);

	let { children } = $props();
	let palette: CommandPalette | undefined = $state();

	onMount(() => {
		// The byo database first — it is the source of truth when it answers (§4.5).
		const stopJourney = journey.start();
		// The dev-server report poll stands down on its own while byo is connected.
		reports.startPolling();
		const stopVisibility = trackTabVisibility();
		return () => {
			stopJourney();
			reports.stopPolling();
			stopVisibility();
		};
	});
</script>

<svelte:head>
	<title>{journeyTitle}</title>
	<meta
		name="description"
		content="A garden trail through building your own shell, Kafka broker, WebAssembly runtime, TLS 1.3 server, ELF linker and distributed store: what to build at every stage, worked examples, the tests, the command to run, and live red/green from the byo database."
	/>
</svelte:head>

<a class="skip-link" href="#main">Skip to content</a>
<Nav onpalette={() => palette?.show()} />
<CommandPalette bind:this={palette} />

<main id="main">
	{@render children?.()}
</main>

<footer>
	<div class="wrap spread">
		<p class="tiny muted">
			Grown from <code>{testers[0]}</code>, <code>{testers[1]}</code> and {testers.length - 2} more —
			every stage, hint and test on this site is generated from the testers themselves, and your
			progress comes from <code>byo</code>’s database.
		</p>
		<p class="tiny muted">{journeyTitle} <span aria-hidden="true">🌷</span></p>
	</div>
</footer>

<style>
	main {
		min-height: calc(100vh - 58px - 90px);
	}
	footer {
		border-top: 1px solid var(--line);
		margin-top: var(--s-8);
		padding: var(--s-5) 0;
		background: color-mix(in srgb, var(--bg-2) 60%, transparent);
	}
	footer :global(p) {
		margin: 0;
	}
	@media (max-width: 640px) {
		footer :global(.spread) {
			flex-direction: column;
			align-items: flex-start;
			gap: var(--s-1);
		}
	}
</style>
