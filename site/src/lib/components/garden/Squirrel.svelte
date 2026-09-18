<script lang="ts">
	/**
	 * The linker track's mascot: a squirrel that gathers loose objects and puts them
	 * together. Idle it breathes, blinks and flicks its tail; `walk` steps; `hop` bounces.
	 * Transforms only, so `prefers-reduced-motion` stops all of it (app.css).
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
	class="sq {mood}"
	class:flip
	width={size}
	height={size}
	viewBox="0 0 120 120"
	role={label ? 'img' : 'presentation'}
	aria-label={label || undefined}
	aria-hidden={label ? undefined : 'true'}
>
	<g class="whole">
		<!-- the tail: a big question mark of fur behind everything -->
		<g class="tail">
			<path
				class="fur"
				d="M34 100 C 6 96 2 56 22 34 C 38 16 66 20 70 36 C 56 26 38 32 30 48 C 20 68 24 88 42 94 Z"
			/>
			<path class="cream" d="M24 40 C 12 56 14 78 28 88 C 20 74 20 56 28 44 Z" />
		</g>

		<g class="legs">
			<ellipse class="fur-deep back" cx="50" cy="106" rx="10" ry="6" />
			<ellipse class="fur-deep front" cx="76" cy="106" rx="10" ry="6" />
		</g>

		<!-- body, sitting up -->
		<ellipse class="fur" cx="64" cy="82" rx="24" ry="26" />
		<ellipse class="cream" cx="66" cy="88" rx="15" ry="18" />

		<!-- front paws, holding an acorn: the object it just picked up -->
		<g class="paws">
			<ellipse class="fur-deep" cx="56" cy="80" rx="6" ry="5" />
			<ellipse class="fur-deep" cx="76" cy="80" rx="6" ry="5" />
			<g class="acorn">
				<path class="nut" d="M58 82 a 8 9 0 0 1 16 0 a 8 10 0 0 1 -16 0 Z" />
				<path class="cap" d="M57 80 q 9 -9 18 0 Z" />
			</g>
		</g>

		<g class="head">
			<circle class="fur" cx="68" cy="48" r="20" />
			<ellipse class="cream" cx="72" cy="54" rx="11" ry="8" />
			<path class="fur-deep ear" d="M55 30 q -2 -14 10 -9 q 4 4 2 11 Z" />
			<path class="fur-deep ear" d="M82 29 q 4 -13 12 -5 q 1 5 -4 10 Z" />

			<ellipse class="blush" cx="53" cy="54" rx="5" ry="3.2" />
			<ellipse class="blush" cx="85" cy="52" rx="5" ry="3.2" />

			<g class="eyes">
				<ellipse class="eye" cx="61" cy="45" rx="3.2" ry="4.1" />
				<ellipse class="eye" cx="78" cy="45" rx="3.2" ry="4.1" />
				<circle class="spark" cx="62.2" cy="43.4" r="1.1" />
				<circle class="spark" cx="79.2" cy="43.4" r="1.1" />
			</g>

			<path class="nose" d="M66 53 L74 53 L70 57 Z" />
			<path class="smile" d="M70 57 q -4 4 -7 1 M70 57 q 4 4 7 1" />
		</g>
	</g>
</svg>

<style>
	.sq {
		display: block;
		overflow: visible;
		--fur: color-mix(in srgb, var(--peach) 62%, var(--link-bright));
		--fur-deep: color-mix(in srgb, var(--link-bright) 70%, var(--link));
		--cream: color-mix(in srgb, var(--bg-2) 86%, var(--butter));
	}
	.sq.flip {
		transform: scaleX(-1);
	}
	.fur {
		fill: var(--fur);
	}
	.fur-deep {
		fill: var(--fur-deep);
	}
	.cream {
		fill: var(--cream);
	}
	.nut {
		fill: color-mix(in srgb, var(--link) 55%, var(--peach));
	}
	.cap {
		fill: var(--link);
	}
	.blush {
		fill: var(--pink);
		opacity: 0.5;
	}
	.eye {
		fill: color-mix(in srgb, var(--ink) 88%, var(--link));
	}
	.spark {
		fill: var(--bg-2);
	}
	.nose {
		fill: color-mix(in srgb, var(--ink) 74%, var(--link));
	}
	.smile {
		fill: none;
		stroke: color-mix(in srgb, var(--ink) 74%, var(--link));
		stroke-width: 1.9;
		stroke-linecap: round;
	}

	.whole {
		transform-origin: 60px 112px;
		animation: breathe 3.7s ease-in-out infinite;
	}
	.tail {
		transform-origin: 40px 98px;
		animation: flick 4.4s ease-in-out infinite;
	}
	@keyframes flick {
		0%,
		100% {
			transform: rotate(-3deg);
		}
		50% {
			transform: rotate(4deg);
		}
	}
	.head {
		transform-origin: 68px 60px;
		animation: sway 5.2s ease-in-out infinite;
	}
	.eyes {
		transform-origin: 70px 45px;
		animation: blink 5.8s ease-in-out infinite;
	}
	@keyframes blink {
		0%,
		91%,
		100% {
			transform: scaleY(1);
		}
		94% {
			transform: scaleY(0.08);
		}
	}

	.sq.walk .back {
		transform-origin: 50px 106px;
		animation: step 0.56s ease-in-out infinite;
	}
	.sq.walk .front {
		transform-origin: 76px 106px;
		animation: step 0.56s ease-in-out infinite reverse;
	}
	.sq.walk .whole {
		animation: bob 0.56s ease-in-out infinite;
	}
	.sq.walk .tail {
		animation: flick 0.56s ease-in-out infinite;
	}
	@keyframes step {
		0%,
		100% {
			transform: translateX(-3.5px);
		}
		50% {
			transform: translateX(3.5px);
		}
	}
	@keyframes bob {
		0%,
		100% {
			transform: translateY(0);
		}
		50% {
			transform: translateY(-3px);
		}
	}

	.sq.hop .whole {
		animation: hop 900ms var(--bounce) infinite;
	}
</style>
