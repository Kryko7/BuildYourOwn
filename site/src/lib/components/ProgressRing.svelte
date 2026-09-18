<script lang="ts">
	let {
		value = 0,
		total = 1,
		size = 84,
		stroke = 7,
		accent = 'var(--ink)',
		label = ''
	}: { value?: number; total?: number; size?: number; stroke?: number; accent?: string; label?: string } =
		$props();

	const r = $derived((size - stroke) / 2);
	const c = $derived(2 * Math.PI * r);
	const frac = $derived(total > 0 ? Math.min(1, value / total) : 0);
	const pct = $derived(Math.round(frac * 100));
</script>

<div class="ring" style="--size:{size}px;--accent:{accent}">
	<svg width={size} height={size} viewBox="0 0 {size} {size}" aria-hidden="true">
		<circle cx={size / 2} cy={size / 2} r={r} fill="none" stroke="var(--bg-3)" stroke-width={stroke} />
		<circle
			cx={size / 2}
			cy={size / 2}
			r={r}
			fill="none"
			stroke={accent}
			stroke-width={stroke}
			stroke-linecap="round"
			stroke-dasharray="{c}"
			stroke-dashoffset={c * (1 - frac)}
			transform="rotate(-90 {size / 2} {size / 2})"
		/>
	</svg>
	<div class="mid">
		<strong>{pct}<span>%</span></strong>
		{#if label}<span class="tiny muted">{label}</span>{/if}
	</div>
	<span class="visually-hidden">{value} of {total} complete</span>
</div>

<style>
	.ring {
		position: relative;
		width: var(--size);
		height: var(--size);
		flex: none;
	}
	circle:last-of-type {
		transition: stroke-dashoffset 700ms cubic-bezier(0.22, 0.61, 0.36, 1);
	}
	.mid {
		position: absolute;
		inset: 0;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 0;
		line-height: 1.1;
	}
	strong {
		font-family: var(--font-display);
		font-size: calc(var(--size) / 3.6);
		font-weight: 600;
	}
	strong span {
		font-size: 0.55em;
		color: var(--ink-3);
	}
</style>
