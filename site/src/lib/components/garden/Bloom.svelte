<script lang="ts">
	/**
	 * One waypoint on the garden trail, drawn in the map's own coordinates and centred on
	 * (0, 0). A stage starts as a closed bud, glows when it is the one you are up to, opens
	 * halfway while it is in progress, blooms when it goes green, and turns into a red poppy
	 * when the tester is red on it.
	 *
	 * The petals animate in when the `done` group is created — i.e. exactly when a stage is
	 * completed, not on every re-render — and `prefers-reduced-motion` removes the motion.
	 */
	import type { StageState } from '$lib/types';

	let {
		state = 'locked',
		number,
		planned = false
	}: { state?: StageState; number: number; planned?: boolean } = $props();

	// Six petals, evenly spaced; the stagger makes the flower unfurl rather than pop.
	const petals = [0, 60, 120, 180, 240, 300];
</script>

<g class="bloom {state}" class:planned>
	<!-- stem and leaf, always there: even a bud grows out of something -->
	<path class="stem" d="M0 12 C 1 20 -1 26 0 31" />
	<path class="leaf" d="M0 24 C -6 20 -12 22 -13 27 C -8 31 -2 29 0 24 Z" />

	{#if state === 'done'}
		<g class="petals">
			{#each petals as angle, i (angle)}
				<ellipse class="petal" cx="0" cy="-11" rx="7.4" ry="11" style="--i:{i};--rot:{angle}deg" />
			{/each}
		</g>
		<circle class="core" r="7.6" />
	{:else if state === 'failing'}
		<g class="petals poppy">
			{#each petals as angle, i (angle)}
				<ellipse
					class="petal"
					cx="0"
					cy="-11.5"
					rx="8"
					ry="11.5"
					style="--i:{i};--rot:{angle + 30}deg"
				/>
			{/each}
		</g>
		<circle class="core" r="7.2" />
	{:else if state === 'in-progress'}
		<g class="petals half">
			{#each [300, 0, 60] as angle, i (angle)}
				<ellipse class="petal" cx="0" cy="-10" rx="6.6" ry="10" style="--i:{i};--rot:{angle}deg" />
			{/each}
		</g>
		<path class="bud" d="M0 -12 C 7.5 -6.5 7.5 5.5 0 11.5 C -7.5 5.5 -7.5 -6.5 0 -12 Z" />
		<circle class="core small" r="6.4" />
	{:else}
		<path class="bud" d="M0 -13 C 8 -7 8 6 0 12.5 C -8 6 -8 -7 0 -13 Z" />
		<path class="budline" d="M0 -9 C 3 -4 3 4 0 8.5" />
		<circle class="core small" r="6.2" />
	{/if}

	<text class="num" y="2.4">{number}</text>
</g>

<style>
	.stem {
		fill: none;
		stroke: var(--stem);
		stroke-width: 2.4;
		stroke-linecap: round;
	}
	.leaf {
		fill: var(--leaf);
	}
	.bud {
		fill: color-mix(in srgb, var(--leaf) 42%, var(--bg-2));
		stroke: var(--stem);
		stroke-width: 1.8;
	}
	.budline {
		fill: none;
		stroke: var(--stem);
		stroke-width: 1.2;
		opacity: 0.6;
	}
	.core {
		fill: var(--bg-2);
		stroke: var(--line-strong);
		stroke-width: 1.6;
	}
	.core.small {
		fill: color-mix(in srgb, var(--bg-2) 88%, var(--leaf));
		stroke: none;
	}
	.num {
		text-anchor: middle;
		font-family: var(--font-mono);
		font-size: 9.4px;
		font-weight: 600;
		fill: var(--ink-2);
		pointer-events: none;
	}

	/* locked: a bud still waiting its turn */
	.bloom.locked .bud,
	.bloom.locked .leaf,
	.bloom.locked .stem {
		opacity: 0.55;
	}
	.bloom.locked .num {
		opacity: 0.6;
	}
	.bloom.planned .bud {
		stroke-dasharray: 3 3;
	}

	/* available: the bud has colour but no glow */
	.bloom.available .bud {
		fill: color-mix(in srgb, var(--accent) 18%, var(--bg-2));
	}

	/* next: the one you are up to — a bud about to open, gently breathing */
	.bloom.next .bud {
		fill: color-mix(in srgb, var(--accent) 32%, var(--bg-2));
		stroke: var(--accent);
		stroke-width: 2.4;
	}
	.bloom.next .core.small {
		fill: var(--butter);
	}
	.bloom.next .num {
		fill: var(--ink);
	}
	.bloom.next {
		transform-origin: 0 0;
		animation: budbreathe 2.8s ease-in-out infinite;
	}
	@keyframes budbreathe {
		0%,
		100% {
			transform: scale(1);
		}
		50% {
			transform: scale(1.09);
		}
	}

	/* in progress: half open */
	.bloom.in-progress .petal {
		fill: color-mix(in srgb, var(--accent) 45%, var(--bg-2));
	}
	.bloom.in-progress .num {
		fill: var(--ink);
	}

	/* done: a full bloom in the track's own pastel, with a butter-yellow heart */
	.bloom.done .petal {
		fill: color-mix(in srgb, var(--accent) 52%, var(--bg-2));
		stroke: color-mix(in srgb, var(--accent) 70%, transparent);
		stroke-width: 1.2;
	}
	.bloom.done .core {
		fill: var(--butter);
		stroke: var(--butter-ink);
		stroke-width: 1.2;
	}
	.bloom.done .num {
		fill: var(--butter-ink);
	}

	/* failing: a red poppy — impossible to miss, still a flower */
	.bloom.failing .petal {
		fill: var(--bad-bright);
		stroke: var(--bad);
		stroke-width: 1.2;
	}
	.bloom.failing .core {
		fill: var(--bad);
		stroke: none;
	}
	.bloom.failing .num {
		fill: var(--bg-2);
	}

	/* The rotation lives in CSS, not in a transform attribute, so the unfurl keyframe can
	   keep it while it scales the petal up from nothing. */
	.petals .petal {
		transform-origin: 0 0;
		transform: rotate(var(--rot, 0deg));
		animation: unfurl 520ms var(--bounce) backwards;
		animation-delay: calc(var(--i) * 55ms);
	}
	@keyframes unfurl {
		from {
			transform: rotate(calc(var(--rot, 0deg) - 40deg)) scale(0.12);
			opacity: 0;
		}
	}
</style>
