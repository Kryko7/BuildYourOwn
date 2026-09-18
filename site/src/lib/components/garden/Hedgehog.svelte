<script lang="ts">
	/**
	 * The TLS track's mascot: a rose hedgehog, all spines and no trust in strangers.
	 * Idle it breathes and blinks; `walk` steps its little feet and bobs; `hop` bounces.
	 * Transforms only, so `prefers-reduced-motion` stops every bit of it (app.css).
	 */
	let {
		size = 96,
		mood = 'idle',
		flip = false,
		label = ''
	}: {
		size?: number;
		mood?: 'idle' | 'walk' | 'hop';
		flip?: boolean;
		label?: string;
	} = $props();
</script>

<svg
	class="hog {mood}"
	class:flip
	width={size}
	height={size}
	viewBox="0 0 120 120"
	role={label ? 'img' : 'presentation'}
	aria-label={label || undefined}
	aria-hidden={label ? undefined : 'true'}
>
	<g class="whole">
		<g class="feet">
			<ellipse class="paw back" cx="44" cy="104" rx="9" ry="5.5" />
			<ellipse class="paw front" cx="72" cy="104" rx="9" ry="5.5" />
		</g>

		<!-- the spiny back: one saw-toothed dome, drawn as a single path -->
		<path
			class="spines"
			d="M26 100 C 22 66 40 42 66 42 C 92 42 106 64 104 100 Z
			   M30 78 l-9 -9 l12 -2 Z M36 62 l-6 -12 l12 3 Z M48 50 l-2 -14 l11 7 Z
			   M64 45 l3 -14 l9 10 Z M80 49 l9 -11 l5 12 Z M94 62 l12 -6 l1 12 Z"
		/>
		<path class="spines-lit" d="M34 92 q 10 -30 34 -34 q -18 12 -22 34 Z" />

		<!-- face: a soft snout poking out from under the spines -->
		<g class="head">
			<path class="skin" d="M22 92 C 14 84 16 68 30 64 C 44 60 54 70 54 82 C 54 94 40 100 22 92 Z" />
			<ellipse class="blush" cx="36" cy="88" rx="6" ry="3.6" />
			<g class="eyes">
				<ellipse class="eye" cx="33" cy="76" rx="3.2" ry="4" />
				<circle class="spark" cx="34.2" cy="74.5" r="1.1" />
			</g>
			<circle class="nose" cx="17" cy="81" r="4.2" />
			<path class="smile" d="M22 88 q 5 4 9 1" />
			<path class="whisker" d="M14 88 l-8 4 M15 92 l-7 6" />
		</g>
	</g>
</svg>

<style>
	.hog {
		display: block;
		overflow: visible;
		--spine: color-mix(in srgb, var(--tls) 42%, var(--bg-3));
		--spine-lit: color-mix(in srgb, var(--tls) 22%, var(--bg-2));
		--skin: color-mix(in srgb, var(--peach) 60%, var(--bg-2));
	}
	.hog.flip {
		transform: scaleX(-1);
	}
	.spines {
		fill: var(--spine);
	}
	.spines-lit {
		fill: var(--spine-lit);
		opacity: 0.75;
	}
	.skin {
		fill: var(--skin);
	}
	.paw {
		fill: color-mix(in srgb, var(--skin) 76%, var(--tls));
	}
	.eye {
		fill: color-mix(in srgb, var(--ink) 88%, var(--tls));
	}
	.spark {
		fill: var(--bg-2);
	}
	.nose {
		fill: color-mix(in srgb, var(--ink) 78%, var(--tls));
	}
	.blush {
		fill: var(--pink);
		opacity: 0.6;
	}
	.smile,
	.whisker {
		fill: none;
		stroke: color-mix(in srgb, var(--ink) 70%, var(--tls));
		stroke-width: 1.8;
		stroke-linecap: round;
	}
	.whisker {
		opacity: 0.55;
	}

	.whole {
		transform-origin: 60px 110px;
		animation: breathe 3.9s ease-in-out infinite;
	}
	.head {
		transform-origin: 44px 82px;
		animation: sway 5.8s ease-in-out infinite;
	}
	.eyes {
		transform-origin: 33px 76px;
		animation: blink 6.6s ease-in-out infinite;
	}
	@keyframes blink {
		0%,
		92%,
		100% {
			transform: scaleY(1);
		}
		95% {
			transform: scaleY(0.08);
		}
	}

	.hog.walk .back {
		transform-origin: 44px 104px;
		animation: scurry 0.5s ease-in-out infinite;
	}
	.hog.walk .front {
		transform-origin: 72px 104px;
		animation: scurry 0.5s ease-in-out infinite reverse;
	}
	.hog.walk .whole {
		animation: bob 0.5s ease-in-out infinite;
	}
	@keyframes scurry {
		0%,
		100% {
			transform: translateX(-3px);
		}
		50% {
			transform: translateX(3px);
		}
	}
	@keyframes bob {
		0%,
		100% {
			transform: translateY(0);
		}
		50% {
			transform: translateY(-2px);
		}
	}

	.hog.hop .whole {
		animation: hop 900ms var(--bounce) infinite;
	}
</style>
