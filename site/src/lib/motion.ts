/**
 * Motion plumbing for the garden.
 *
 * Three rules, applied everywhere: transforms and opacity only (no layout properties), a
 * hard stop when the visitor asked their OS for less motion, and nothing running while the
 * tab is in the background. Looping CSS animations obey the last rule by being paused through
 * the Web Animations API (one-shot entrances keep running); JS loops obey it by not
 * scheduling a frame at all.
 */
import { browser } from '$app/environment';

export function reducedMotion(): boolean {
	if (!browser) return false;
	try {
		return window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;
	} catch {
		return false;
	}
}

export function tabVisible(): boolean {
	if (!browser) return true;
	return document.visibilityState !== 'hidden';
}

/**
 * Mirror the tab's visibility onto `<html>` (`is-hidden-tab`) and, while the tab is hidden,
 * pause every *looping* animation on the page through the Web Animations API. One-shot
 * animations — a drawer sliding in, a bloom unfurling once — are deliberately left alone:
 * pausing those at their first frame leaves an invisible dialog behind if the page opens
 * something while it is not being looked at. Returns the teardown.
 */
export function trackTabVisibility(): () => void {
	if (!browser) return () => {};
	const pausedByUs = new Set<Animation>();
	const isLooping = (a: Animation): boolean => {
		try {
			return a.effect?.getTiming().iterations === Infinity;
		} catch {
			return false;
		}
	};
	const apply = () => {
		const hidden = document.visibilityState === 'hidden';
		document.documentElement.classList.toggle('is-hidden-tab', hidden);
		const animations = typeof document.getAnimations === 'function' ? document.getAnimations() : [];
		if (hidden) {
			for (const a of animations) {
				if (a.playState === 'running' && isLooping(a)) {
					a.pause();
					pausedByUs.add(a);
				}
			}
		} else {
			for (const a of pausedByUs) {
				if (a.playState === 'paused') a.play();
			}
			pausedByUs.clear();
		}
	};
	apply();
	document.addEventListener('visibilitychange', apply);
	return () => {
		document.removeEventListener('visibilitychange', apply);
		document.documentElement.classList.remove('is-hidden-tab');
		for (const a of pausedByUs) if (a.playState === 'paused') a.play();
		pausedByUs.clear();
	};
}

/**
 * A requestAnimationFrame loop that does nothing at all under reduced motion and skips
 * frames while the tab is hidden. `step` receives seconds since the previous frame,
 * clamped so a backgrounded tab does not come back with a one-minute delta.
 */
export function loop(step: (dt: number, elapsed: number) => void): () => void {
	if (!browser || reducedMotion()) return () => {};
	let handle = 0;
	let previous = 0;
	let elapsed = 0;
	let stopped = false;

	const frame = (now: number) => {
		if (stopped) return;
		if (document.visibilityState === 'hidden') {
			previous = now;
			handle = requestAnimationFrame(frame);
			return;
		}
		const dt = previous ? Math.min(0.05, (now - previous) / 1000) : 0.016;
		previous = now;
		elapsed += dt;
		step(dt, elapsed);
		handle = requestAnimationFrame(frame);
	};
	handle = requestAnimationFrame(frame);
	return () => {
		stopped = true;
		cancelAnimationFrame(handle);
	};
}

/** Deterministic pseudo-random in [0, 1) — so a garden looks the same on every render. */
export function seeded(seed: number): () => number {
	let s = seed >>> 0 || 1;
	return () => {
		s ^= s << 13;
		s ^= s >>> 17;
		s ^= s << 5;
		s >>>= 0;
		return s / 4294967296;
	};
}
