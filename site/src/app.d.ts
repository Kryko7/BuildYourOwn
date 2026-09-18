// See https://svelte.dev/docs/kit/types#app.d.ts
// for information about these interfaces
declare global {
	namespace App {
		// interface Error {}
		// interface Locals {}
		// interface PageData {}
		// interface PageState {}
		// interface Platform {}
	}
}

interface ImportMetaEnv {
	/** Journey owner, from the repo-root `.env`; empty for an anonymous journey. */
	readonly PUBLIC_JOURNEY_OWNER?: string;
}

interface ImportMeta {
	readonly env: ImportMetaEnv;
}

export {};
