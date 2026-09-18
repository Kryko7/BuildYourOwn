<script lang="ts">
	/** The Kafka track's mascot: a lavender bunny with ears that twitch. */
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
	class="bunny {mood}"
	class:flip
	width={size}
	height={size}
	viewBox="0 0 120 120"
	role={label ? 'img' : 'presentation'}
	aria-label={label || undefined}
	aria-hidden={label ? undefined : 'true'}
>
	<g class="whole">
		<circle class="tail" cx="29" cy="88" r="10" />

		<g class="ears">
			<g class="ear left">
				<ellipse class="fur" cx="48" cy="24" rx="8.5" ry="21" />
				<ellipse class="inner" cx="48" cy="25" rx="4.2" ry="15" />
			</g>
			<g class="ear right">
				<ellipse class="fur" cx="73" cy="22" rx="8.5" ry="21" />
				<ellipse class="inner" cx="73" cy="23" rx="4.2" ry="15" />
			</g>
		</g>

		<ellipse class="fur" cx="62" cy="86" rx="29" ry="23" />
		<ellipse class="cream" cx="64" cy="93" rx="18" ry="13" />

		<g class="feet">
			<ellipse class="fur-deep back" cx="46" cy="107" rx="11" ry="6.5" />
			<ellipse class="fur-deep front" cx="80" cy="107" rx="11" ry="6.5" />
		</g>

		<g class="head">
			<circle class="fur" cx="61" cy="55" r="24" />
			<ellipse class="blush" cx="43" cy="61" rx="5.5" ry="3.4" />
			<ellipse class="blush" cx="79" cy="61" rx="5.5" ry="3.4" />

			<g class="eyes">
				<ellipse class="eye" cx="51" cy="52" rx="3.3" ry="4.2" />
				<ellipse class="eye" cx="71" cy="52" rx="3.3" ry="4.2" />
				<circle class="spark" cx="52.2" cy="50.4" r="1.1" />
				<circle class="spark" cx="72.2" cy="50.4" r="1.1" />
			</g>

			<path class="nose" d="M57.5 61 L64.5 61 L61 65 Z" />
			<path class="smile" d="M61 65 q -4.5 5 -8.5 1 M61 65 q 4.5 5 8.5 1" />
			<path
				class="whisker"
				d="M45 63 L32 60 M45 66 L32 67 M77 63 L90 60 M77 66 L90 67"
			/>
		</g>
	</g>
</svg>

<style>
	.bunny {
		display: block;
		overflow: visible;
		--fur: color-mix(in srgb, var(--lav) 82%, var(--bg-2));
		--fur-deep: var(--lav);
		--cream: color-mix(in srgb, var(--bg-2) 88%, var(--lav));
	}
	.bunny.flip {
		transform: scaleX(-1);
	}
	.fur,
	.tail {
		fill: var(--fur);
	}
	.tail {
		fill: var(--cream);
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
		opacity: 0.55;
	}
	.eye {
		fill: color-mix(in srgb, var(--ink) 88%, var(--lav));
	}
	.spark {
		fill: var(--bg-2);
	}
	.nose {
		fill: var(--pink-ink);
	}
	.smile,
	.whisker {
		fill: none;
		stroke: color-mix(in srgb, var(--ink) 72%, var(--lav));
		stroke-width: 1.8;
		stroke-linecap: round;
	}
	.whisker {
		opacity: 0.55;
		stroke-width: 1.4;
	}

	.whole {
		transform-origin: 60px 112px;
		animation: breathe 3.9s ease-in-out infinite;
	}
	.ear.left {
		transform-origin: 48px 44px;
		animation: twitch 7s ease-in-out infinite;
	}
	.ear.right {
		transform-origin: 73px 42px;
		animation: twitch 7s ease-in-out 1.4s infinite;
	}
	@keyframes twitch {
		0%,
		84%,
		100% {
			transform: rotate(0deg);
		}
		88% {
			transform: rotate(-11deg);
		}
		93% {
			transform: rotate(5deg);
		}
	}
	.eyes {
		transform-origin: 61px 52px;
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

	.bunny.walk .whole {
		animation: hopwalk 0.72s ease-in-out infinite;
	}
	@keyframes hopwalk {
		0%,
		100% {
			transform: translateY(0) scaleY(1);
		}
		45% {
			transform: translateY(-7px) scaleY(1.04);
		}
		70% {
			transform: translateY(0) scaleY(0.96);
		}
	}
	.bunny.hop .whole {
		animation: hop 880ms var(--bounce) infinite;
	}
</style>
