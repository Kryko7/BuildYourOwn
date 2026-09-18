import { sveltekit } from '@sveltejs/kit/vite';
import adapterStatic from '@sveltejs/adapter-static';
import { defineConfig, type Plugin } from 'vitest/config';
import { readFile, stat } from 'node:fs/promises';
import { resolve } from 'node:path';

/**
 * Live mode: during `npm run dev` the two Rust testers' `--json` reports are served
 * straight off disk so the journey map turns green while a run is in flight.
 *   ../shelltest/report.json  ->  /__reports/shell.json
 *   ../kafkatest/report.json  ->  /__reports/kafka.json
 * Missing file => 204 No Content, which the client reads as "no live run".
 */
function liveReports(): Plugin {
	const sources: Record<string, string> = {
		'/__reports/shell.json': resolve('../shelltest/report.json'),
		'/__reports/kafka.json': resolve('../kafkatest/report.json')
	};
	return {
		name: 'byo-live-reports',
		apply: 'serve',
		configureServer(server) {
			server.middlewares.use(async (req, res, next) => {
				const url = (req.url ?? '').split('?')[0];
				const file = sources[url];
				if (!file) return next();
				res.setHeader('cache-control', 'no-store');
				try {
					const info = await stat(file);
					const body = await readFile(file, 'utf8');
					res.setHeader('content-type', 'application/json');
					res.setHeader('x-report-mtime', String(info.mtimeMs));
					res.end(body);
				} catch {
					res.statusCode = 204;
					res.end();
				}
			});
		}
	};
}

export default defineConfig(({ mode }) => ({
	// Personal settings (the journey owner's name) live in the repo-root `.env`.
	envDir: '..',
	envPrefix: ['VITE_', 'PUBLIC_'],
	// Component tests mount real components, so they need svelte's browser build.
	resolve: mode === 'test' ? { conditions: ['browser'] } : {},
	plugins: [
		liveReports(),
		sveltekit({
			compilerOptions: {
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},
			adapter: adapterStatic({ strict: true })
		})
	],
	test: {
		environment: 'node',
		include: ['src/**/*.test.ts', 'src/**/*.test.svelte.ts', 'scripts/**/*.test.mjs'],
		globals: false
	}
}));
