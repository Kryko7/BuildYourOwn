<script lang="ts">
	/**
	 * The celebration: a burst of petals instead of confetti. Canvas, no library, cleared
	 * and released the moment the last petal is gone. Does nothing at all when the visitor
	 * asked for reduced motion.
	 */
	import { onMount } from 'svelte';
	import { reducedMotion } from '$lib/motion';

	let { burst = 0, accent = 'var(--pink)' }: { burst?: number; accent?: string } = $props();

	let canvas: HTMLCanvasElement | undefined = $state();
	let running = false;

	interface Petal {
		x: number;
		y: number;
		vx: number;
		vy: number;
		rot: number;
		vr: number;
		flutter: number;
		phase: number;
		life: number;
		colour: string;
		rx: number;
		ry: number;
	}

	/** Read once per burst: canvas needs real colours, not `var(--pink)`. */
	function palette(): string[] {
		const style = getComputedStyle(document.documentElement);
		const names = ['--pink', '--lav', '--butter', '--mint', '--sky', '--peach'];
		const out = names
			.map((n) => style.getPropertyValue(n).trim())
			.filter((v) => v.length > 0 && !v.includes('color-mix'));
		return out.length ? out : ['#f6b3ca', '#c4b2ef', '#f8dc99', '#9fdcc3', '#a6d7f2', '#fabf9c'];
	}

	function launch() {
		if (!canvas || reducedMotion() || running) return;
		const ctx = canvas.getContext('2d');
		if (!ctx) return;
		const dpr = Math.min(2, window.devicePixelRatio || 1);
		const w = (canvas.width = canvas.clientWidth * dpr);
		const h = (canvas.height = canvas.clientHeight * dpr);
		const colours = palette();
		const petals: Petal[] = [];
		for (let i = 0; i < 86; i++) {
			const angle = -Math.PI / 2 + (Math.random() - 0.5) * 2.1;
			const speed = (4 + Math.random() * 8) * dpr;
			petals.push({
				x: w / 2 + (Math.random() - 0.5) * 90 * dpr,
				y: h * 0.66,
				vx: Math.cos(angle) * speed,
				vy: Math.sin(angle) * speed,
				rot: Math.random() * Math.PI * 2,
				vr: (Math.random() - 0.5) * 0.14,
				flutter: 0.9 + Math.random() * 1.6,
				phase: Math.random() * Math.PI * 2,
				life: 1,
				colour: colours[i % colours.length],
				rx: (4 + Math.random() * 3.5) * dpr,
				ry: (7 + Math.random() * 5) * dpr
			});
		}
		running = true;
		let t = 0;
		const step = () => {
			t += 0.016;
			ctx.clearRect(0, 0, w, h);
			let alive = 0;
			for (const p of petals) {
				p.vy += 0.16 * dpr;
				p.vx *= 0.992;
				p.vy *= 0.994;
				// side-to-side drift is what makes a petal read as a petal and not a brick
				p.x += p.vx + Math.sin(t * p.flutter + p.phase) * 1.4 * dpr;
				p.y += p.vy;
				p.rot += p.vr;
				p.life -= 0.0062;
				if (p.life <= 0 || p.y > h + 40) continue;
				alive++;
				ctx.save();
				ctx.translate(p.x, p.y);
				ctx.rotate(p.rot);
				// the squash is the petal turning edge-on as it falls
				ctx.scale(0.45 + 0.55 * Math.abs(Math.cos(t * p.flutter + p.phase)), 1);
				ctx.globalAlpha = Math.max(0, Math.min(1, p.life));
				ctx.fillStyle = p.colour;
				ctx.beginPath();
				ctx.ellipse(0, 0, p.rx, p.ry, 0, 0, Math.PI * 2);
				ctx.fill();
				ctx.restore();
			}
			if (alive > 0) requestAnimationFrame(step);
			else {
				ctx.clearRect(0, 0, w, h);
				running = false;
			}
		};
		requestAnimationFrame(step);
	}

	let last = 0;
	$effect(() => {
		if (burst > last) {
			last = burst;
			launch();
		}
	});

	onMount(() => {
		last = burst;
	});
</script>

<canvas bind:this={canvas} aria-hidden="true" style="--accent:{accent}"></canvas>

<style>
	canvas {
		position: fixed;
		inset: 0;
		width: 100%;
		height: 100%;
		pointer-events: none;
		z-index: 90;
	}
</style>
