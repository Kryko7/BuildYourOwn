import { browser } from '$app/environment';
import { load, save } from './persist';

export type ThemeChoice = 'system' | 'light' | 'dark';
const KEY = 'byo:theme:v1';

class ThemeStore {
	choice = $state<ThemeChoice>('system');
	resolved = $state<'light' | 'dark'>('dark');

	constructor() {
		if (!browser) return;
		const stored = load<ThemeChoice>(KEY, 'system');
		this.choice = stored === 'light' || stored === 'dark' ? stored : 'system';
		// matchMedia is missing in some embedded webviews and in jsdom; fall back to light.
		const mq = typeof window.matchMedia === 'function' ? window.matchMedia('(prefers-color-scheme: dark)') : null;
		const apply = () => {
			this.resolved = this.choice === 'system' ? (mq?.matches ? 'dark' : 'light') : this.choice;
			document.documentElement.dataset.theme = this.resolved;
		};
		apply();
		mq?.addEventListener('change', apply);
		this.#apply = apply;
	}

	#apply: () => void = () => {};

	set(choice: ThemeChoice) {
		this.choice = choice;
		save(KEY, choice);
		this.#apply();
	}

	cycle() {
		const order: ThemeChoice[] = ['system', 'light', 'dark'];
		this.set(order[(order.indexOf(this.choice) + 1) % order.length]);
	}
}

export const theme = new ThemeStore();

/** True when the visitor asked the OS for less motion. */
export function prefersReducedMotion(): boolean {
	if (!browser) return false;
	try {
		return window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;
	} catch {
		return false;
	}
}
