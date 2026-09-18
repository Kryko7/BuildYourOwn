<script lang="ts">
	/**
	 * Butterflies and a bee drifting across a hero. Purely decorative: absolutely
	 * positioned, `pointer-events: none`, transform-only keyframes, paused with the tab and
	 * stopped entirely under `prefers-reduced-motion` (see app.css).
	 */
	import { seeded } from '$lib/motion';

	let { count = 5, seed = 7 }: { count?: number; seed?: number } = $props();

	const palette = ['var(--pink)', 'var(--lav)', 'var(--sky)', 'var(--butter)', 'var(--mint)'];

	// Deterministic: the same hero looks the same on every render and after hydration.
	const bugs = $derived.by(() => {
		const rnd = seeded(seed);
		return Array.from({ length: Math.max(0, Math.min(8, count)) }, (_, i) => ({
			id: i,
			kind: i % 4 === 3 ? ('bee' as const) : ('butterfly' as const),
			top: 8 + rnd() * 68,
			left: 4 + rnd() * 84,
			scale: 0.62 + rnd() * 0.6,
			duration: 16 + rnd() * 16,
			delay: -rnd() * 18,
			rise: 26 + rnd() * 44,
			drift: 40 + rnd() * 90,
			colour: palette[i % palette.length]
		}));
	});
</script>

<div class="flutterers" aria-hidden="true">
	{#each bugs as b (b.id)}
		<span
			class="bug {b.kind}"
			style="
				top:{b.top}%; left:{b.left}%;
				--scale:{b.scale};
				--dur:{b.duration}s;
				--delay:{b.delay}s;
				--rise:{-b.rise}px;
				--drift:{b.drift}px;
				--wing:{b.colour};"
		>
			{#if b.kind === 'butterfly'}
				<svg viewBox="0 0 40 32" width="30" height="24">
					<g class="wings">
						<path
							class="w left"
							d="M20 16 C 10 2 0 4 3 13 C 5 20 13 21 20 16 C 13 24 4 26 5 30 C 9 32 17 26 20 16 Z"
						/>
						<path
							class="w right"
							d="M20 16 C 30 2 40 4 37 13 C 35 20 27 21 20 16 C 27 24 36 26 35 30 C 31 32 23 26 20 16 Z"
						/>
					</g>
					<ellipse class="torso" cx="20" cy="16" rx="1.8" ry="8" />
					<path class="antenna" d="M20 9 L16 3 M20 9 L24 3" />
				</svg>
			{:else}
				<svg viewBox="0 0 40 30" width="26" height="20">
					<g class="wings">
						<ellipse class="bw" cx="16" cy="9" rx="8" ry="5" />
						<ellipse class="bw" cx="26" cy="9" rx="8" ry="5" />
					</g>
					<ellipse class="beebody" cx="21" cy="17" rx="11" ry="7.5" />
					<path class="stripe" d="M18 10.5 L18 23.5 M24 11 L24 23" />
					<circle class="beeeye" cx="31" cy="15" r="1.7" />
				</svg>
			{/if}
		</span>
	{/each}
</div>

<style>
	.flutterers {
		position: absolute;
		inset: 0;
		pointer-events: none;
		overflow: hidden;
		z-index: 1;
	}
	.bug {
		position: absolute;
		display: block;
		transform: scale(var(--scale));
		animation: drift var(--dur) ease-in-out var(--delay) infinite alternate;
		will-change: transform;
	}
	@keyframes drift {
		0% {
			transform: translate3d(0, 0, 0) scale(var(--scale)) rotate(-4deg);
		}
		35% {
			transform: translate3d(calc(var(--drift) * 0.45), var(--rise), 0) scale(var(--scale))
				rotate(6deg);
		}
		70% {
			transform: translate3d(calc(var(--drift) * 0.8), calc(var(--rise) * 0.3), 0)
				scale(var(--scale)) rotate(-3deg);
		}
		100% {
			transform: translate3d(var(--drift), calc(var(--rise) * 1.2), 0) scale(var(--scale))
				rotate(5deg);
		}
	}
	svg {
		display: block;
		overflow: visible;
	}
	.w {
		fill: var(--wing);
		opacity: 0.78;
	}
	.wings {
		transform-origin: 50% 50%;
		animation: flutter 340ms ease-in-out infinite alternate;
	}
	@keyframes flutter {
		from {
			transform: scaleX(1);
		}
		to {
			transform: scaleX(0.56);
		}
	}
	.torso {
		fill: color-mix(in srgb, var(--ink) 70%, var(--wing));
	}
	.antenna {
		stroke: color-mix(in srgb, var(--ink) 60%, var(--wing));
		stroke-width: 1.2;
		fill: none;
		stroke-linecap: round;
	}
	.bw {
		fill: var(--sky);
		opacity: 0.55;
	}
	.beebody {
		fill: var(--butter);
	}
	.stripe {
		stroke: var(--peach-ink);
		stroke-width: 3;
		stroke-linecap: round;
		opacity: 0.75;
		fill: none;
	}
	.beeeye {
		fill: color-mix(in srgb, var(--ink) 80%, var(--butter));
	}
	@media (max-width: 640px) {
		/* three is a garden; six on a phone is a swarm */
		.bug:nth-child(n + 4) {
			display: none;
		}
	}
</style>
