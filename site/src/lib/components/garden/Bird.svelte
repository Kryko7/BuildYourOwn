<script lang="ts">
	/** The little bird perched on the nav's brand. Hops in place every few seconds. */
	let { size = 26, label = '' }: { size?: number; label?: string } = $props();
</script>

<svg
	class="bird"
	width={size}
	height={size * 0.86}
	viewBox="0 0 44 38"
	role={label ? 'img' : 'presentation'}
	aria-label={label || undefined}
	aria-hidden={label ? undefined : 'true'}
>
	<g class="whole">
		<path class="tail" d="M6 20 L-1 13 L2 24 Z" />
		<ellipse class="body" cx="20" cy="20" rx="14" ry="11.5" />
		<g class="wing">
			<path class="wingfill" d="M14 18 C 22 13 30 16 30 22 C 24 26 16 24 14 18 Z" />
		</g>
		<circle class="body" cx="31" cy="13" r="8.5" />
		<path class="beak" d="M39 13 L45 15.5 L39 17 Z" />
		<circle class="eye" cx="33" cy="11.5" r="1.9" />
		<path class="legs" d="M18 31 L17 36 M25 31 L26 36" />
	</g>
</svg>

<style>
	.bird {
		display: block;
		overflow: visible;
	}
	.body,
	.tail {
		fill: var(--sky);
	}
	.tail {
		fill: color-mix(in srgb, var(--sky) 76%, var(--lav));
	}
	.wingfill {
		fill: color-mix(in srgb, var(--sky) 70%, var(--lav));
	}
	.beak {
		fill: var(--butter);
	}
	.eye {
		fill: color-mix(in srgb, var(--ink) 88%, var(--sky));
	}
	.legs {
		stroke: var(--butter-ink);
		stroke-width: 1.6;
		stroke-linecap: round;
		fill: none;
	}
	.whole {
		transform-origin: 22px 36px;
		animation: perch 5.4s ease-in-out infinite;
	}
	@keyframes perch {
		0%,
		70%,
		100% {
			transform: translateY(0);
		}
		76% {
			transform: translateY(-4px);
		}
		82% {
			transform: translateY(0);
		}
		88% {
			transform: translateY(-3px);
		}
	}
	.wing {
		transform-origin: 16px 19px;
		animation: flap 5.4s ease-in-out infinite;
	}
	@keyframes flap {
		0%,
		72%,
		100% {
			transform: rotate(0deg) scaleY(1);
		}
		77% {
			transform: rotate(-26deg) scaleY(0.75);
		}
		84% {
			transform: rotate(6deg) scaleY(1);
		}
	}
</style>
