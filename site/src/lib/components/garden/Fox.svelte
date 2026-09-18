<script lang="ts">
	/**
	 * The shell track's mascot. Hand-drawn paths, no external assets: a peach fox that
	 * breathes, blinks, and can walk or hop on request. Every animation is a transform on a
	 * group, so nothing reflows and `prefers-reduced-motion` stops all of it (app.css).
	 */
	let {
		size = 96,
		mood = 'idle',
		flip = false,
		label = ''
	}: {
		size?: number;
		/** `walk` swings the legs, `hop` bounces the whole animal once per cycle. */
		mood?: 'idle' | 'walk' | 'hop';
		flip?: boolean;
		/** Given a label the fox is an image; without one it is decoration. */
		label?: string;
	} = $props();
</script>

<svg
	class="fox {mood}"
	class:flip
	width={size}
	height={size}
	viewBox="0 0 120 120"
	role={label ? 'img' : 'presentation'}
	aria-label={label || undefined}
	aria-hidden={label ? undefined : 'true'}
>
	<g class="whole">
		<!-- tail: a big sweep behind the body, cream at the tip -->
		<path class="fur" d="M30 88 C 2 86 0 48 21 38 C 16 58 25 73 41 79 Z" />
		<path class="cream" d="M21 38 C 8 45 6 58 11 67 C 9 54 14 44 21 38 Z" />

		<g class="legs">
			<rect class="fur-deep back" x="35" y="96" width="13" height="16" rx="6.5" />
			<rect class="fur-deep front" x="69" y="96" width="13" height="16" rx="6.5" />
		</g>

		<ellipse class="fur" cx="56" cy="84" rx="32" ry="22" />
		<ellipse class="cream" cx="62" cy="92" rx="21" ry="13" />

		<g class="head">
			<path class="fur" d="M66 34 L59 7 L85 23 Z" />
			<path class="blush-fill" d="M68 32 L64 15 L80 25 Z" />
			<path class="fur" d="M103 34 L111 7 L85 22 Z" />
			<path class="blush-fill" d="M101 32 L105 15 L89 24 Z" />

			<ellipse class="fur" cx="84" cy="52" rx="26" ry="23" />
			<ellipse class="cream" cx="87" cy="62" rx="16" ry="11" />

			<ellipse class="blush" cx="66" cy="59" rx="5.5" ry="3.4" />
			<ellipse class="blush" cx="105" cy="57" rx="5.5" ry="3.4" />

			<g class="eyes">
				<ellipse class="eye" cx="76" cy="48" rx="3.4" ry="4.3" />
				<ellipse class="eye" cx="96" cy="48" rx="3.4" ry="4.3" />
				<circle class="spark" cx="77.2" cy="46.4" r="1.1" />
				<circle class="spark" cx="97.2" cy="46.4" r="1.1" />
			</g>

			<ellipse class="eye" cx="87" cy="57" rx="4.4" ry="3.3" />
			<path class="smile" d="M87 61 q -5 5 -9 1 M87 61 q 5 5 9 1" />
		</g>
	</g>
</svg>

<style>
	.fox {
		display: block;
		overflow: visible;
		--fur: var(--peach);
		--fur-deep: color-mix(in srgb, var(--peach) 72%, var(--shell));
		--cream: color-mix(in srgb, var(--bg-2) 85%, var(--butter));
	}
	.fox.flip {
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
	.blush,
	.blush-fill {
		fill: var(--pink);
	}
	.blush {
		opacity: 0.55;
	}
	.blush-fill {
		opacity: 0.75;
	}
	.eye {
		fill: color-mix(in srgb, var(--ink) 88%, var(--shell));
	}
	.spark {
		fill: var(--bg-2);
	}
	.smile {
		fill: none;
		stroke: color-mix(in srgb, var(--ink) 80%, var(--shell));
		stroke-width: 2;
		stroke-linecap: round;
	}

	.whole {
		transform-origin: 60px 112px;
		animation: breathe 3.6s ease-in-out infinite;
	}
	.head {
		transform-origin: 84px 66px;
		animation: sway 5.4s ease-in-out infinite;
	}
	.eyes {
		transform-origin: 86px 48px;
		animation: blink 6.2s ease-in-out infinite;
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

	.fox.walk .back {
		transform-origin: 41px 98px;
		animation: step 0.62s ease-in-out infinite;
	}
	.fox.walk .front {
		transform-origin: 75px 98px;
		animation: step 0.62s ease-in-out infinite reverse;
	}
	.fox.walk .whole {
		animation: bob 0.62s ease-in-out infinite;
	}
	@keyframes step {
		0%,
		100% {
			transform: rotate(-16deg);
		}
		50% {
			transform: rotate(16deg);
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

	.fox.hop .whole {
		animation: hop 900ms var(--bounce) infinite;
	}
</style>
