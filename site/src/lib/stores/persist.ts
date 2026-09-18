/** localStorage helpers that never throw and never touch `window` during SSR. */
import { browser } from '$app/environment';

export function load<T>(key: string, fallback: T): T {
	if (!browser) return fallback;
	try {
		const raw = window.localStorage.getItem(key);
		if (raw === null) return fallback;
		return JSON.parse(raw) as T;
	} catch {
		return fallback;
	}
}

export function save(key: string, value: unknown): void {
	if (!browser) return;
	try {
		window.localStorage.setItem(key, JSON.stringify(value));
	} catch {
		/* private mode, quota, disabled storage — progress is a convenience, not a contract */
	}
}

export function remove(key: string): void {
	if (!browser) return;
	try {
		window.localStorage.removeItem(key);
	} catch {
		/* ignore */
	}
}
