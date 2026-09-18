<script lang="ts">
	/** The home page's cat: sits in the hero, tail flicking, and blinks slowly at you. */
	let {
		size = 120,
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
	class="cat {mood}"
	class:flip
	width={size}
	height={size}
	viewBox="0 0 120 120"
	role={label ? 'img' : 'presentation'}
	aria-label={label || undefined}
	aria-hidden={label ? undefined : 'true'}
>
	<g class="whole">
		<path class="tail fur" d="M88 96 C 112 96 116 66 104 56 C 110 72 104 86 86 88 Z" />

		<ellipse class="fur" cx="60" cy="88" rx="28" ry="22" />
		<ellipse class="cream" cx="60" cy="96" rx="17" ry="12" />
		<ellipse class="fur-deep" cx="44" cy="106" rx="11" ry="6" />
		<ellipse class="fur-deep" cx="76" cy="106" rx="11" ry="6" />

		<g class="head">
			<path class="fur" d="M42 34 L37 10 L60 24 Z" />
			<path class="inner" d="M44 33 L41 18 L55 26 Z" />
			<path class="fur" d="M78 34 L83 10 L60 24 Z" />
			<path class="inner" d="M76 33 L79 18 L65 26 Z" />

			<circle class="fur" cx="60" cy="52" r="25" />
			<path class="stripe" d="M52 32 L54 40 M60 30 L60 38 M68 32 L66 40" />

			<ellipse class="blush" cx="41" cy="60" rx="5.6" ry="3.5" />
			<ellipse class="blush" cx="79" cy="60" rx="5.6" ry="3.5" />

			<g class="eyes">
				<ellipse class="eye" cx="50" cy="50" rx="3.6" ry="4.6" />
				<ellipse class="eye" cx="70" cy="50" rx="3.6" ry="4.6" />
				<circle class="spark" cx="51.4" cy="48.2" r="1.2" />
				<circle class="spark" cx="71.4" cy="48.2" r="1.2" />
			</g>

			<path class="nose" d="M56.5 59 L63.5 59 L60 63 Z" />
			<path class="smile" d="M60 63 q -4.5 5 -8 1 M60 63 q 4.5 5 8 1" />
			<path class="whisker" d="M43 61 L28 58 M43 65 L28 66 M77 61 L92 58 M77 65 L92 66" />
		</g>
	</g>
</svg>

<style>
	.cat {
		display: block;
		overflow: visible;
		--fur: color-mix(in srgb, var(--butter) 84%, var(--bg-2));
		--fur-deep: color-mix(in srgb, var(--butter) 70%, var(--peach));
		--cream: color-mix(in srgb, var(--bg-2) 88%, var(--butter));
	}
	.cat.flip {
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
	.inner {
		fill: var(--pink);
		opacity: 0.8;
	}
	.blush {
		fill: var(--pink);
		opacity: 0.5;
	}
	.eye {
		fill: color-mix(in srgb, var(--ink) 86%, var(--butter));
	}
	.spark {
		fill: var(--bg-2);
	}
	.nose {
		fill: var(--pink-ink);
	}
	.stripe,
	.smile,
	.whisker {
		fill: none;
		stroke: color-mix(in srgb, var(--ink) 70%, var(--butter));
		stroke-width: 1.9;
		stroke-linecap: round;
	}
	.stripe {
		stroke: var(--peach-ink);
		opacity: 0.4;
		stroke-width: 2.6;
	}
	.whisker {
		opacity: 0.5;
		stroke-width: 1.4;
	}

	.whole {
		transform-origin: 60px 112px;
		animation: breathe 4.2s ease-in-out infinite;
	}
	.tail {
		transform-origin: 88px 94px;
		animation: flick 4.6s ease-in-out infinite;
	}
	@keyframes flick {
		0%,
		100% {
			transform: rotate(-5deg);
		}
		50% {
			transform: rotate(7deg);
		}
	}
	.head {
		transform-origin: 60px 70px;
		animation: sway 6.8s ease-in-out infinite;
	}
	.eyes {
		transform-origin: 60px 50px;
		animation: blink 5.1s ease-in-out infinite;
	}
	@keyframes blink {
		0%,
		88%,
		100% {
			transform: scaleY(1);
		}
		92% {
			transform: scaleY(0.07);
		}
	}
	.cat.hop .whole {
		animation: hop 920ms var(--bounce) infinite;
	}
</style>
