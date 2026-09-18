<script lang="ts">
	/**
	 * The WebAssembly track's mascot: a lavender owl who reads bytes for a living. Hand-drawn
	 * paths, no external assets. Idle it breathes and blinks; `walk` shuffles its talons and
	 * bobs; `hop` bounces the whole bird. Every animation is a transform on a group, so
	 * nothing reflows and `prefers-reduced-motion` stops all of it (app.css).
	 */
	let {
		size = 96,
		mood = 'idle',
		flip = false,
		label = ''
	}: {
		size?: number;
		/** `walk` shuffles the talons, `hop` bounces the whole bird once per cycle. */
		mood?: 'idle' | 'walk' | 'hop';
		flip?: boolean;
		/** Given a label the owl is an image; without one it is decoration. */
		label?: string;
	} = $props();
</script>

<svg
	class="owl {mood}"
	class:flip
	width={size}
	height={size}
	viewBox="0 0 120 120"
	role={label ? 'img' : 'presentation'}
	aria-label={label || undefined}
	aria-hidden={label ? undefined : 'true'}
>
	<g class="whole">
		<g class="talons">
			<path class="feet back" d="M48 106 l0 7 M44 113 h9" />
			<path class="feet front" d="M72 106 l0 7 M68 113 h9" />
		</g>

		<!-- body: one plump teardrop, with a paler chest -->
		<path class="plume" d="M60 22 C 88 22 98 50 96 74 C 94 97 79 108 60 108 C 41 108 26 97 24 74 C 22 50 32 22 60 22 Z" />
		<path class="chest" d="M60 52 C 74 52 82 65 81 78 C 80 92 71 100 60 100 C 49 100 40 92 39 78 C 38 65 46 52 60 52 Z" />
		<!-- a few chest feathers, so the belly is not a blank oval -->
		<path class="feather" d="M52 70 q 4 4 8 0 M62 70 q 4 4 8 0 M56 80 q 4 4 8 0" />

		<g class="wings">
			<path class="wing left" d="M27 56 C 18 68 20 86 30 94 C 30 80 28 66 27 56 Z" />
			<path class="wing right" d="M93 56 C 102 68 100 86 90 94 C 90 80 92 66 93 56 Z" />
		</g>

		<g class="head">
			<!-- ear tufts -->
			<path class="plume" d="M34 30 L30 12 L48 24 Z" />
			<path class="plume" d="M86 30 L90 12 L72 24 Z" />

			<g class="eyes">
				<circle class="disc" cx="47" cy="45" r="14" />
				<circle class="disc" cx="73" cy="45" r="14" />
				<g class="lids">
					<circle class="eye" cx="47" cy="45" r="7.5" />
					<circle class="eye" cx="73" cy="45" r="7.5" />
					<circle class="spark" cx="49.6" cy="42.4" r="2.4" />
					<circle class="spark" cx="75.6" cy="42.4" r="2.4" />
				</g>
			</g>

			<path class="beak" d="M54 52 L66 52 L60 64 Z" />
			<ellipse class="blush" cx="35" cy="58" rx="5.5" ry="3.4" />
			<ellipse class="blush" cx="85" cy="58" rx="5.5" ry="3.4" />
		</g>
	</g>
</svg>

<style>
	.owl {
		display: block;
		overflow: visible;
		--plume: var(--lav);
		--plume-deep: color-mix(in srgb, var(--lav) 66%, var(--wasm));
		--chest: color-mix(in srgb, var(--bg-2) 82%, var(--lav));
	}
	.owl.flip {
		transform: scaleX(-1);
	}
	.plume {
		fill: var(--plume);
	}
	.chest {
		fill: var(--chest);
	}
	.wing {
		fill: var(--plume-deep);
	}
	.feather {
		fill: none;
		stroke: color-mix(in srgb, var(--plume-deep) 55%, transparent);
		stroke-width: 2;
		stroke-linecap: round;
	}
	.disc {
		fill: color-mix(in srgb, var(--bg-2) 88%, var(--butter));
	}
	.eye {
		fill: color-mix(in srgb, var(--ink) 88%, var(--wasm));
	}
	.spark {
		fill: var(--bg-2);
	}
	.beak {
		fill: var(--butter);
	}
	.blush {
		fill: var(--pink);
		opacity: 0.5;
	}
	.feet {
		fill: none;
		stroke: var(--butter);
		stroke-width: 3.4;
		stroke-linecap: round;
	}

	.whole {
		transform-origin: 60px 112px;
		animation: breathe 4s ease-in-out infinite;
	}
	.head {
		transform-origin: 60px 60px;
		animation: sway 6.2s ease-in-out infinite;
	}
	/* An owl blinks with the whole eye, not with a lid slit — squash the pupils. */
	.lids {
		transform-origin: 60px 45px;
		animation: blink 5.4s ease-in-out infinite;
	}
	@keyframes blink {
		0%,
		90%,
		100% {
			transform: scaleY(1);
		}
		94% {
			transform: scaleY(0.06);
		}
	}

	.owl.walk .back {
		transform-origin: 48px 106px;
		animation: shuffle 0.66s ease-in-out infinite;
	}
	.owl.walk .front {
		transform-origin: 72px 106px;
		animation: shuffle 0.66s ease-in-out infinite reverse;
	}
	.owl.walk .whole {
		animation: bob 0.66s ease-in-out infinite;
	}
	.owl.walk .wings {
		transform-origin: 60px 70px;
		animation: flutter 0.66s ease-in-out infinite;
	}
	@keyframes shuffle {
		0%,
		100% {
			transform: translateY(0);
		}
		50% {
			transform: translateY(-3px);
		}
	}
	@keyframes bob {
		0%,
		100% {
			transform: translateY(0);
		}
		50% {
			transform: translateY(-2.5px);
		}
	}
	@keyframes flutter {
		0%,
		100% {
			transform: scaleX(1);
		}
		50% {
			transform: scaleX(1.07);
		}
	}

	.owl.hop .whole {
		animation: hop 900ms var(--bounce) infinite;
	}
</style>
