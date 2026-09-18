<script lang="ts">
	/**
	 * The distributed-systems track's mascot: a duckling, because a cluster is a line of
	 * them following whoever is in front. Idle it breathes and blinks; `walk` paddles its
	 * feet and waddles; `hop` bounces. Transforms only, so `prefers-reduced-motion` stops
	 * all of it (app.css).
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
	class="duck {mood}"
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
			<path class="web back" d="M46 104 l-8 9 h17 Z" />
			<path class="web front" d="M74 104 l-8 9 h17 Z" />
		</g>

		<!-- body -->
		<ellipse class="down" cx="58" cy="84" rx="30" ry="24" />
		<!-- a stubby tail, so the body has a direction -->
		<path class="down" d="M28 76 l-12 -5 l9 14 Z" />
		<ellipse class="fluff" cx="60" cy="90" rx="19" ry="14" />

		<g class="wing">
			<path class="down-deep" d="M50 78 C 62 74 76 80 78 90 C 68 96 54 92 50 78 Z" />
			<path class="quill" d="M58 82 q 8 3 15 8 M56 87 q 8 2 14 6" />
		</g>

		<g class="head">
			<circle class="down" cx="80" cy="46" r="21" />
			<path class="down" d="M66 58 q 6 8 14 9 q -10 3 -18 -3 Z" />
			<ellipse class="blush" cx="66" cy="52" rx="5.2" ry="3.3" />

			<g class="eyes">
				<ellipse class="eye" cx="76" cy="41" rx="3.3" ry="4.2" />
				<ellipse class="eye" cx="90" cy="42" rx="3.3" ry="4.2" />
				<circle class="spark" cx="77.2" cy="39.4" r="1.1" />
				<circle class="spark" cx="91.2" cy="40.4" r="1.1" />
			</g>

			<g class="bill">
				<ellipse class="beak" cx="102" cy="52" rx="12" ry="6.5" />
				<path class="beakline" d="M92 53 q 10 3 19 0" />
			</g>
			<!-- one small crest feather -->
			<path class="down-deep" d="M78 25 q 3 -12 10 -12 q -4 6 -3 13 Z" />
		</g>
	</g>
</svg>

<style>
	.duck {
		display: block;
		overflow: visible;
		--down: var(--butter);
		--down-deep: color-mix(in srgb, var(--butter) 68%, var(--dist));
		--fluff: color-mix(in srgb, var(--bg-2) 68%, var(--butter));
	}
	.duck.flip {
		transform: scaleX(-1);
	}
	.down {
		fill: var(--down);
	}
	.down-deep {
		fill: var(--down-deep);
	}
	.fluff {
		fill: var(--fluff);
	}
	.beak,
	.web {
		fill: color-mix(in srgb, var(--peach) 72%, var(--butter));
	}
	.beakline {
		fill: none;
		stroke: color-mix(in srgb, var(--ink) 40%, transparent);
		stroke-width: 1.6;
		stroke-linecap: round;
	}
	.quill {
		fill: none;
		stroke: color-mix(in srgb, var(--dist) 45%, transparent);
		stroke-width: 1.8;
		stroke-linecap: round;
	}
	.eye {
		fill: color-mix(in srgb, var(--ink) 88%, var(--dist));
	}
	.spark {
		fill: var(--bg-2);
	}
	.blush {
		fill: var(--pink);
		opacity: 0.55;
	}

	.whole {
		transform-origin: 60px 112px;
		animation: breathe 3.5s ease-in-out infinite;
	}
	.head {
		transform-origin: 74px 60px;
		animation: sway 5s ease-in-out infinite;
	}
	.eyes {
		transform-origin: 83px 42px;
		animation: blink 5.6s ease-in-out infinite;
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

	.duck.walk .back {
		transform-origin: 46px 104px;
		animation: paddle 0.54s ease-in-out infinite;
	}
	.duck.walk .front {
		transform-origin: 74px 104px;
		animation: paddle 0.54s ease-in-out infinite reverse;
	}
	/* A duckling waddles: the body rocks side to side, it does not bob straight up. */
	.duck.walk .whole {
		animation: waddle 0.54s ease-in-out infinite;
	}
	.duck.walk .wing {
		transform-origin: 56px 86px;
		animation: flap 0.54s ease-in-out infinite;
	}
	@keyframes paddle {
		0%,
		100% {
			transform: translateX(-3px) translateY(0);
		}
		50% {
			transform: translateX(3px) translateY(-2px);
		}
	}
	@keyframes waddle {
		0%,
		100% {
			transform: rotate(-3deg);
		}
		50% {
			transform: rotate(3deg);
		}
	}
	@keyframes flap {
		0%,
		100% {
			transform: rotate(0deg);
		}
		50% {
			transform: rotate(-9deg);
		}
	}

	.duck.hop .whole {
		animation: hop 900ms var(--bounce) infinite;
	}
</style>
