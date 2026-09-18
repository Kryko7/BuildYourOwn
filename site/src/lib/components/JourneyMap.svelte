<script lang="ts">
	import { untrack } from 'svelte';
	import Bloom from './garden/Bloom.svelte';
	import Fox from './garden/Fox.svelte';
	import Bunny from './garden/Bunny.svelte';
	import { badgeFor } from '$lib/catalog';
	import { stageState, stateLabel } from '$lib/stage-state';
	import { progress } from '$lib/stores/progress.svelte';
	import { reducedMotion, seeded } from '$lib/motion';
	import type { Catalog, Report, StageState, TrackId } from '$lib/types';

	let {
		track,
		catalog,
		report,
		selected = null,
		celebrate = 0,
		onselect
	}: {
		track: TrackId;
		catalog: Catalog;
		report: Report | null;
		selected?: number | null;
		/** Incremented by the page when a stage is completed: the mascot hops. */
		celebrate?: number;
		onselect: (stage: number) => void;
	} = $props();

	/* ------------------------------- geometry -------------------------------
	   Everything below is in "map units". The SVG viewBox is sized so that at
	   k = 1 exactly one meadow row spans the viewport width — i.e. the default view
	   is already readable, and "fit the whole trail" is an explicit button.        */

	const GAP_X = 132;
	const ROW_H = 118;
	const SECTION_GAP = 58;
	const PAD_Y = 78;

	let host: HTMLDivElement | undefined = $state();
	let hostWidth = $state(980);
	let hostHeight = $state(560);

	const cols = $derived(hostWidth < 520 ? 3 : hostWidth < 760 ? 4 : hostWidth < 1000 ? 5 : 6);
	const PAD_X = $derived(cols <= 4 ? 62 : 86);

	interface Node {
		number: number;
		name: string;
		ext: boolean;
		planned: boolean;
		x: number;
		y: number;
		section: string;
	}

	interface Camp {
		id: string;
		title: string;
		badge: string;
		icon: string;
		x: number;
		y: number;
		count: number;
	}

	const layout = $derived.by(() => {
		const nodes: Node[] = [];
		const camps: Camp[] = [];
		let y = PAD_Y + 12;
		for (const section of catalog.sections) {
			const stages = section.stages
				.map((n) => catalog.stages.find((s) => s.number === n))
				.filter((s): s is NonNullable<typeof s> => Boolean(s));
			if (stages.length === 0) continue;
			const badge = badgeFor(track, section.id);
			// Anchored to the first row of the meadow; the label itself is offset in screen
			// pixels, so it sits the same distance above the row at every zoom.
			camps.push({
				id: section.id,
				title: section.title,
				badge: badge.badge,
				icon: badge.icon,
				x: PAD_X,
				y,
				count: stages.length
			});
			stages.forEach((s, i) => {
				const row = Math.floor(i / cols);
				const inRow = i % cols;
				const col = row % 2 === 0 ? inRow : cols - 1 - inRow;
				nodes.push({
					number: s.number,
					name: s.name,
					ext: s.ext,
					planned: s.planned === true,
					section: section.id,
					x: PAD_X + col * GAP_X,
					y: y + row * ROW_H + Math.sin(s.number * 0.9) * 9
				});
			});
			y += Math.ceil(stages.length / cols) * ROW_H + SECTION_GAP;
		}
		const width = PAD_X * 2 + (cols - 1) * GAP_X;
		return { nodes, camps, width, height: y + 20 };
	});

	/** The viewport, in map units: as wide as one meadow row, as tall as the element is. */
	const viewW = $derived(layout.width);
	const viewH = $derived(
		hostWidth > 0 ? Math.max(160, (layout.width * hostHeight) / hostWidth) : 560
	);
	/** CSS pixels per map unit at k = 1. */
	const pxPerUnit = $derived(hostWidth > 0 ? hostWidth / viewW : 1);

	/** Catmull-Rom through the waypoints, rendered as cubic beziers: a path, not a grid. */
	function trail(points: { x: number; y: number }[]): string {
		if (points.length < 2) return '';
		let d = `M ${points[0].x} ${points[0].y}`;
		for (let i = 0; i < points.length - 1; i++) {
			const p0 = points[Math.max(0, i - 1)];
			const p1 = points[i];
			const p2 = points[i + 1];
			const p3 = points[Math.min(points.length - 1, i + 2)];
			const t = 0.22;
			const c1x = p1.x + (p2.x - p0.x) * t;
			const c1y = p1.y + (p2.y - p0.y) * t;
			const c2x = p2.x - (p3.x - p1.x) * t;
			const c2y = p2.y - (p3.y - p1.y) * t;
			d += ` C ${c1x.toFixed(1)} ${c1y.toFixed(1)}, ${c2x.toFixed(1)} ${c2y.toFixed(1)}, ${p2.x} ${p2.y}`;
		}
		return d;
	}

	const pathD = $derived(trail(layout.nodes));

	const nextStage = $derived(progress.nextStage(track));

	const states = $derived.by(() => {
		const map = new Map<number, StageState>();
		for (const n of layout.nodes) {
			map.set(
				n.number,
				stageState({
					track,
					stage: n.number,
					report,
					progress: progress.stage(track, n.number),
					nextStage
				})
			);
		}
		return map;
	});

	const doneFraction = $derived.by(() => {
		const total = layout.nodes.length;
		if (total === 0) return 0;
		let walked = 0;
		for (const n of layout.nodes) {
			const st = states.get(n.number);
			if (st === 'done' || st === 'failing' || st === 'in-progress') walked++;
			else break;
		}
		return walked / total;
	});

	/** Grass tufts and fallen petals scattered over the meadow — fixed, never random per render. */
	const scatter = $derived.by(() => {
		const rnd = seeded(track === 'shell' ? 1337 : 4242);
		const w = layout.width;
		const h = layout.height;
		return Array.from({ length: Math.min(130, Math.round((w * h) / 5200)) }, (_, i) => ({
			id: i,
			x: rnd() * (w + 120) - 60,
			y: rnd() * h,
			r: 2 + rnd() * 4,
			kind: i % 5 === 0 ? ('petal' as const) : ('tuft' as const),
			rot: rnd() * 360,
			tint: i % 3
		}));
	});

	/* ------------------------------- pan & zoom ------------------------------ */

	let k = $state(1);
	let tx = $state(0);
	let ty = $state(0);
	let dragging = $state(false);
	let dragMoved = false;
	let last = { x: 0, y: 0 };

	function clampZoom(v: number) {
		return Math.min(3.2, Math.max(0.18, v));
	}

	/**
	 * Room kept above the first row of every meadow for its label. The label is drawn at a
	 * fixed screen size, so this is a constant in viewport units however far you zoom out —
	 * without it, zooming to fit pushes the first meadow's name off the top of the frame.
	 */
	const HEAD = 58;

	/** Keep the trail on screen: you can never drag the map into empty space. */
	function clampPan() {
		const w = layout.width * k;
		const h = layout.height * k;
		const slackX = Math.min(60, viewW * 0.2);
		const slackY = Math.min(48, viewH * 0.15);
		tx = w <= viewW ? (viewW - w) / 2 : Math.min(slackX, Math.max(viewW - w - slackX, tx));
		ty =
			h + HEAD <= viewH
				? HEAD + (viewH - HEAD - h) / 2
				: Math.min(HEAD, Math.max(viewH - h - slackY, ty));
	}

	function zoomBy(factor: number, cx = viewW / 2, cy = viewH / 2) {
		const nk = clampZoom(k * factor);
		tx = cx - ((cx - tx) * nk) / k;
		ty = cy - ((cy - ty) * nk) / k;
		k = nk;
		clampPan();
	}

	/** The readable default: one meadow row across the width, the next flower centred. */
	function defaultView() {
		k = 1;
		tx = 0;
		ty = 0;
		const node = layout.nodes.find((n) => n.number === nextStage) ?? layout.nodes[0];
		if (node) ty = viewH / 2 - node.y;
		clampPan();
	}

	/** The whole trail at once — always an explicit choice, never the default. */
	function fitAll() {
		k = clampZoom(Math.min(1, (viewH - HEAD) / Math.max(1, layout.height)));
		tx = (viewW - layout.width * k) / 2;
		ty = HEAD;
		clampPan();
	}

	/** Pan the minimum needed to bring one waypoint (and its label) into view. */
	function ensureVisible(n: number) {
		const node = layout.nodes.find((x) => x.number === n);
		if (!node) return;
		const margin = 56;
		const sx = node.x * k + tx;
		const sy = node.y * k + ty;
		if (sx < margin) tx += margin - sx;
		else if (sx > viewW - margin) tx -= sx - (viewW - margin);
		if (sy < margin) ty += margin - sy;
		else if (sy > viewH - margin) ty -= sy - (viewH - margin);
		clampPan();
	}

	/** Centre the view on one stage — used by deep links and the meadow lists. */
	export function focusStage(n: number) {
		const node = layout.nodes.find((x) => x.number === n);
		if (!node) return;
		if (k < 1) k = 1;
		tx = viewW / 2 - node.x * k;
		ty = viewH / 2 - node.y * k;
		clampPan();
	}

	// Re-apply the default framing whenever the trail is re-laid-out (a resize that changes
	// the column count, or a different track), but never while the visitor is panning.
	let framedFor = '';
	$effect(() => {
		const signature = `${track}:${cols}:${layout.nodes.length}:${Math.round(viewH)}`;
		if (signature === framedFor) return;
		framedFor = signature;
		untrack(defaultView);
	});

	function onWheel(e: WheelEvent) {
		const host = e.currentTarget as HTMLElement;
		const rect = host.getBoundingClientRect();
		if (e.ctrlKey || e.metaKey) {
			e.preventDefault();
			const cx = ((e.clientX - rect.left) / Math.max(1, rect.width)) * viewW;
			const cy = ((e.clientY - rect.top) / Math.max(1, rect.height)) * viewH;
			zoomBy(e.deltaY < 0 ? 1.12 : 1 / 1.12, cx, cy);
			return;
		}
		// Plain wheel pans the trail, but hands the gesture back to the page at the ends so
		// the map never traps the scroll.
		const before = ty;
		ty -= e.deltaY / Math.max(0.0001, pxPerUnit);
		clampPan();
		if (ty !== before) e.preventDefault();
	}

	// A press is only a drag once the pointer has travelled this far; below it a click on a
	// waypoint is a click, even with the small wobble a real hand (or a touch) produces.
	const DRAG_THRESHOLD_PX = 4;
	let pressedAt = { x: 0, y: 0 };

	function onPointerDown(e: PointerEvent) {
		if (e.button !== 0) return;
		// Every press starts clean, so a drag from a while ago can never swallow this click.
		dragMoved = false;
		pressedAt = { x: e.clientX, y: e.clientY };
		const target = e.target as Element;
		if (target.closest('[data-node]')) return;
		dragging = true;
		last = { x: e.clientX, y: e.clientY };
		(e.currentTarget as Element).setPointerCapture(e.pointerId);
	}

	function onPointerMove(e: PointerEvent) {
		if (!dragging) return;
		const scale = Math.max(0.0001, pxPerUnit);
		tx += (e.clientX - last.x) / scale;
		ty += (e.clientY - last.y) / scale;
		last = { x: e.clientX, y: e.clientY };
		if (Math.hypot(e.clientX - pressedAt.x, e.clientY - pressedAt.y) > DRAG_THRESHOLD_PX) {
			dragMoved = true;
		}
		clampPan();
	}

	function onPointerUp(e: PointerEvent) {
		dragging = false;
		try {
			(e.currentTarget as Element).releasePointerCapture(e.pointerId);
		} catch {
			/* pointer already released */
		}
	}

	/* ------------------------------- keyboard -------------------------------- */

	// Roving tabindex: one waypoint is in the tab order, arrows walk the trail from there.
	let focusedStage = $state<number | null>(null);
	const tabStop = $derived(
		focusedStage ??
			selected ??
			(layout.nodes.some((n) => n.number === nextStage) ? nextStage : (layout.nodes[0]?.number ?? null))
	);

	function moveTo(target: number | undefined) {
		if (target === undefined) return;
		focusedStage = target;
		ensureVisible(target);
		queueMicrotask(() => host?.querySelector<SVGGElement>(`[data-node="${target}"]`)?.focus());
	}

	function onNodeKey(e: KeyboardEvent, n: number) {
		const order = layout.nodes.map((x) => x.number);
		const i = order.indexOf(n);
		if (e.key === 'Enter' || e.key === ' ' || e.key === 'Spacebar') {
			e.preventDefault();
			onselect(n);
			return;
		}
		let target: number | undefined;
		if (e.key === 'ArrowRight' || e.key === 'ArrowDown') target = order[i + 1];
		else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') target = order[i - 1];
		else if (e.key === 'Home') target = order[0];
		else if (e.key === 'End') target = order[order.length - 1];
		else if (e.key === 'PageDown') target = order[Math.min(order.length - 1, i + cols)];
		else if (e.key === 'PageUp') target = order[Math.max(0, i - cols)];
		else if (e.key === '+' || e.key === '=') {
			e.preventDefault();
			zoomBy(1.18);
			return;
		} else if (e.key === '-' || e.key === '_') {
			e.preventDefault();
			zoomBy(1 / 1.18);
			return;
		} else return;
		e.preventDefault();
		moveTo(target);
	}

	const reduced = reducedMotion();

	/** Font sizes that stay put on screen however far the map is zoomed. */
	const screenPx = $derived((px: number) => px / Math.max(0.0001, k));

	/* ------------------------------- the mascot ------------------------------ */

	/**
	 * The track's animal stands at the flower you are up to and walks there when it moves.
	 * It lives in HTML on top of the SVG rather than inside it, so it can be a real
	 * component with its own CSS animations and stays a constant size at any zoom.
	 */
	const mascotNode = $derived(
		layout.nodes.find((n) => n.number === nextStage) ?? layout.nodes[layout.nodes.length - 1]
	);
	const mascotPos = $derived.by(() => {
		if (!mascotNode) return null;
		const x = (mascotNode.x * k + tx) * pxPerUnit;
		const y = (mascotNode.y * k + ty) * pxPerUnit;
		return { x: x - 74, y: y - 66 };
	});
	const mascotVisible = $derived(
		mascotPos !== null &&
			mascotPos.x > -110 &&
			mascotPos.y > -110 &&
			mascotPos.x < hostWidth + 20 &&
			mascotPos.y < hostHeight + 20
	);

	let mood = $state<'idle' | 'walk' | 'hop'>('idle');
	let walkTimer: ReturnType<typeof setTimeout> | undefined;
	let lastMascot = -1;
	let lastCelebrate = 0;

	$effect(() => {
		const n = mascotNode?.number ?? -1;
		if (n === lastMascot) return;
		const first = lastMascot === -1;
		lastMascot = n;
		if (first || reduced) return;
		mood = 'walk';
		clearTimeout(walkTimer);
		walkTimer = setTimeout(() => (mood = 'idle'), 1100);
	});

	$effect(() => {
		if (celebrate <= lastCelebrate) return;
		lastCelebrate = celebrate;
		if (reduced) return;
		mood = 'hop';
		clearTimeout(walkTimer);
		walkTimer = setTimeout(() => (mood = 'idle'), 1500);
	});
</script>

<div
	class="mapwrap"
	bind:this={host}
	bind:clientWidth={hostWidth}
	bind:clientHeight={hostHeight}
	role="group"
	aria-label="{track} garden trail"
>
	<div class="controls">
		<button class="btn btn-sm" onclick={() => zoomBy(1.18)} aria-label="Zoom in">+</button>
		<button class="btn btn-sm" onclick={() => zoomBy(1 / 1.18)} aria-label="Zoom out">−</button>
		<button
			class="btn btn-sm"
			onclick={() => focusStage(nextStage)}
			aria-label="Centre on the next stage">next</button
		>
		<button class="btn btn-sm" onclick={fitAll} aria-label="Fit the whole trail in view">fit</button>
	</div>

	<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
	<svg
		class:dragging
		viewBox="0 0 {viewW} {viewH}"
		width="100%"
		height="100%"
		onwheel={onWheel}
		onpointerdown={onPointerDown}
		onpointermove={onPointerMove}
		onpointerup={onPointerUp}
		onpointercancel={onPointerUp}
		role="presentation"
	>
		<g transform="translate({tx} {ty}) scale({k})">
			<!-- the meadow floor: grass tufts and a few fallen petals, deterministic -->
			<g class="scatter" aria-hidden="true">
				{#each scatter as s (s.id)}
					{#if s.kind === 'tuft'}
						<path
							class="tuft t{s.tint}"
							d="M{s.x} {s.y} l -{s.r} -{s.r * 2.2} M{s.x} {s.y} l 0 -{s.r * 2.8} M{s.x} {s.y} l {s.r} -{s.r *
								2}"
						/>
					{:else}
						<ellipse
							class="fallen p{s.tint}"
							cx={s.x}
							cy={s.y}
							rx={s.r * 1.3}
							ry={s.r * 0.8}
							transform="rotate({s.rot} {s.x} {s.y})"
						/>
					{/if}
				{/each}
			</g>

			<!-- the path: stepping stones ahead, a strewn petal path behind you -->
			<path d={pathD} class="route-soft" />
			<path d={pathD} class="route-base" />
			<path
				d={pathD}
				class="route-walked-glow"
				pathLength="1"
				stroke-dasharray="1"
				stroke-dashoffset={1 - doneFraction}
				style={reduced ? 'transition:none' : ''}
			/>
			<path
				d={pathD}
				class="route-walked"
				pathLength="1"
				stroke-dasharray="1"
				stroke-dashoffset={1 - doneFraction}
				style={reduced ? 'transition:none' : ''}
			/>

			<!-- meadows: label sizes are in screen pixels, so they stay readable at any zoom -->
			{#each layout.camps as camp (camp.id)}
				<g class="camp" transform="translate({camp.x} {camp.y})">
					<text
						class="camp-icon"
						x={screenPx(-30)}
						y={screenPx(-40)}
						style="font-size:{screenPx(17)}px">{camp.icon}</text
					>
					<text
						class="camp-badge"
						x={screenPx(-8)}
						y={screenPx(-44)}
						style="font-size:{screenPx(15)}px">{camp.badge}</text
					>
					<text
						class="camp-title"
						x={screenPx(-8)}
						y={screenPx(-29)}
						style="font-size:{screenPx(9.5)}px">{camp.id}. {camp.title} · {camp.count} stages</text
					>
				</g>
			{/each}

			<!-- waypoints: a flower per stage -->
			{#each layout.nodes as node (node.number)}
				{@const st = states.get(node.number) ?? 'locked'}
				<g
					data-node={node.number}
					class="node {st}"
					class:selected={selected === node.number}
					class:planned={node.planned}
					transform="translate({node.x} {node.y})"
					role="button"
					tabindex={tabStop === node.number ? 0 : -1}
					aria-label="Stage {node.number}: {node.name}. {stateLabel[st]}.{node.ext
						? ' Beyond the base track.'
						: ''}{node.planned ? ' Not yet in the tester.' : ''}"
					aria-pressed={selected === node.number}
					onclick={() => !dragMoved && onselect(node.number)}
					onkeydown={(e) => onNodeKey(e, node.number)}
					onfocus={() => (focusedStage = node.number)}
				>
					<!-- a generous, invisible hit area: ~44 css px across at the default zoom -->
					<circle class="hit" r="26" />
					{#if st === 'next'}
						<circle class="halo" r="26" />
					{/if}
					<circle class="ringmark" r="21" />
					<Bloom state={st} number={node.number} planned={node.planned} />
					{#if node.ext}
						<circle class="extdot" cx="14" cy="-14" r="3.6" />
					{/if}
					<text class="hover-label" y={30 + screenPx(13)} style="font-size:{screenPx(10.5)}px"
						>{node.name}</text
					>
				</g>
			{/each}
		</g>
	</svg>

	{#if mascotPos && mascotVisible}
		<div
			class="mascot"
			class:walking={mood === 'walk'}
			style="transform: translate3d({mascotPos.x}px, {mascotPos.y}px, 0)"
			aria-hidden="true"
		>
			{#if track === 'shell'}
				<Fox size={74} {mood} />
			{:else}
				<Bunny size={74} {mood} />
			{/if}
		</div>
	{/if}

	<p class="hint tiny muted">
		Drag to pan · wheel scrolls, <kbd>Ctrl</kbd>+wheel zooms · <kbd>Tab</kbd> into the trail, then
		<kbd>←</kbd><kbd>→</kbd> walk it and <kbd>Enter</kbd> opens a stage
	</p>
</div>

<style>
	.mapwrap {
		position: relative;
		border: 1px solid var(--line);
		border-radius: var(--r-4);
		background:
			radial-gradient(120% 80% at 50% 0%, color-mix(in srgb, var(--accent) 12%, transparent), transparent 72%),
			radial-gradient(90% 70% at 12% 100%, color-mix(in srgb, var(--mint) 26%, transparent), transparent 70%),
			radial-gradient(80% 60% at 90% 88%, color-mix(in srgb, var(--butter) 22%, transparent), transparent 70%),
			var(--bg-2);
		overflow: hidden;
		box-shadow: var(--shadow-1);
		height: clamp(420px, 68vh, 760px);
	}
	@media (max-width: 760px) {
		.mapwrap {
			height: clamp(360px, 62vh, 560px);
		}
	}
	svg {
		display: block;
		touch-action: none;
		cursor: grab;
		width: 100%;
		height: 100%;
	}
	svg.dragging {
		cursor: grabbing;
	}
	.controls button {
		min-width: 30px;
		font-family: var(--font-mono);
	}
	.controls {
		position: absolute;
		right: var(--s-3);
		top: var(--s-3);
		z-index: 3;
		display: flex;
		gap: var(--s-1);
		background: color-mix(in srgb, var(--bg-2) 88%, transparent);
		backdrop-filter: blur(6px);
		border-radius: var(--r-full);
		padding: 3px;
	}
	.hint {
		position: absolute;
		left: var(--s-4);
		right: var(--s-4);
		bottom: var(--s-2);
		margin: 0;
		pointer-events: none;
	}
	@media (max-width: 900px) {
		.hint {
			display: none;
		}
	}

	/* ---------- the meadow floor ---------- */
	.tuft {
		fill: none;
		stroke: var(--leaf);
		stroke-width: 1.6;
		stroke-linecap: round;
		opacity: 0.35;
	}
	.tuft.t1 {
		stroke: var(--stem);
		opacity: 0.28;
	}
	.tuft.t2 {
		stroke: var(--leaf-deep);
		opacity: 0.2;
	}
	.fallen {
		fill: var(--pink);
		opacity: 0.3;
	}
	.fallen.p1 {
		fill: var(--butter);
	}
	.fallen.p2 {
		fill: var(--lav);
	}

	/* ---------- the path ----------
	   Ahead of you: pale stepping stones. Behind you: the same stones, strewn with petals in
	   the track's colour and lit by a soft glow. non-scaling-stroke keeps both visible when
	   the whole trail is zoomed to fit. */
	.route-soft {
		fill: none;
		stroke: color-mix(in srgb, var(--bg-sunk) 80%, var(--line));
		stroke-width: 15;
		stroke-linecap: round;
		opacity: 0.55;
		vector-effect: non-scaling-stroke;
	}
	.route-base {
		fill: none;
		stroke: var(--line-strong);
		stroke-width: 6.5;
		stroke-linecap: round;
		stroke-dasharray: 0.1 17;
		opacity: 0.75;
		vector-effect: non-scaling-stroke;
	}
	.route-walked-glow {
		fill: none;
		stroke: color-mix(in srgb, var(--accent) 26%, transparent);
		stroke-width: 16;
		stroke-linecap: round;
		vector-effect: non-scaling-stroke;
		transition: stroke-dashoffset 900ms var(--ease);
	}
	.route-walked {
		fill: none;
		stroke: var(--accent);
		stroke-width: 7;
		stroke-linecap: round;
		stroke-dasharray: 0.1 17;
		opacity: 0.9;
		vector-effect: non-scaling-stroke;
		transition: stroke-dashoffset 900ms var(--ease);
	}

	.camp-icon {
		text-anchor: middle;
	}
	.camp-badge {
		font-family: var(--font-display);
		font-weight: 700;
		fill: var(--ink);
		paint-order: stroke;
		stroke: var(--bg-2);
		stroke-width: 3.5px;
		stroke-linejoin: round;
		vector-effect: non-scaling-stroke;
	}
	.camp-title {
		font-family: var(--font-mono);
		letter-spacing: 0.09em;
		text-transform: uppercase;
		fill: var(--ink-3);
		paint-order: stroke;
		stroke: var(--bg-2);
		stroke-width: 3px;
		stroke-linejoin: round;
		vector-effect: non-scaling-stroke;
	}

	.node {
		cursor: pointer;
	}
	.node .hit {
		fill: transparent;
		stroke: none;
	}
	.node .ringmark {
		fill: none;
		stroke: transparent;
		stroke-width: 2.4;
		transition: stroke 200ms var(--ease);
	}
	.node.selected .ringmark,
	.node:focus-visible .ringmark {
		stroke: var(--ink);
	}
	.node:focus-visible {
		outline: none;
	}
	.node .extdot {
		fill: var(--butter);
		stroke: var(--butter-ink);
		stroke-width: 1.2;
	}
	.node .halo {
		fill: color-mix(in srgb, var(--accent) 20%, transparent);
		animation: haloBreathe 2.8s ease-in-out infinite;
		pointer-events: none;
		transform-origin: 0 0;
	}
	@keyframes haloBreathe {
		0%,
		100% {
			transform: scale(0.86);
			opacity: 0.55;
		}
		50% {
			transform: scale(1.18);
			opacity: 0.12;
		}
	}
	.node :global(.bloom) {
		transition: transform 180ms var(--bounce);
		transform-origin: 0 0;
	}
	.node:hover :global(.bloom) {
		transform: scale(1.12);
	}

	.hover-label {
		text-anchor: middle;
		font-family: var(--font-body);
		font-weight: 600;
		fill: var(--ink-2);
		opacity: 0;
		pointer-events: none;
		transition: opacity 160ms var(--ease);
		paint-order: stroke;
		stroke: var(--bg-2);
		stroke-width: 3.5px;
		stroke-linejoin: round;
		vector-effect: non-scaling-stroke;
	}
	.node:hover .hover-label,
	.node:focus-visible .hover-label,
	.node.selected .hover-label {
		opacity: 1;
	}

	/* ---------- the mascot ---------- */
	.mascot {
		position: absolute;
		left: 0;
		top: 0;
		width: 74px;
		height: 74px;
		pointer-events: none;
		z-index: 2;
		transition: transform 950ms var(--ease);
		filter: drop-shadow(0 6px 8px color-mix(in srgb, var(--ink) 16%, transparent));
	}
	@media (prefers-reduced-motion: reduce) {
		.mascot {
			transition: none;
		}
	}
	@media (max-width: 560px) {
		.mascot {
			display: none;
		}
	}
</style>
