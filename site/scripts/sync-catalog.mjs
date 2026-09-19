#!/usr/bin/env node
/**
 * Rebuild the site's stage catalogs from the testers — one pass per registered track
 * (`src/lib/tracks.ts`, PLAN.md §5.6). No track is named in here.
 *
 * For each track, in order of preference:
 *   1. `../<dir>/catalog.json`  — the real catalog, normalized, with any gaps filled from
 *                                 `../<dir>/PLAN.md` as `planned: true` stages
 *   2. `../<dir>/PLAN.md`       — the real stage list before the tester emits a catalog;
 *                                 every stage `planned: true`, the catalog `pending: true`
 *   3. the built-in placeholder — `scripts/placeholders.mjs`, also `pending: true`
 *
 * The shell track is special only in where its data comes from: `../shelltest/PLAN.md`
 * plus `../shelltest/tests/stages/*.yaml`, which carry the tests themselves.
 *
 * Output: `src/lib/data/catalog.<track>.json` (committed), plus each track's worked
 * examples split into `src/lib/data/examples/<track>/<n>.json`.
 */
import { readFile, readdir, writeFile, mkdir, rm } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parse as parseYaml } from 'yaml';
import { parsePlan, buildCatalog, auditCounts } from './parse-catalog.mjs';
import { normalizeCatalog, mergePlannedStages, sameButForTimestamp } from './normalize-catalog.mjs';
import { placeholderPlan, sectionTitles } from './placeholders.mjs';
import { parseTesterPlan } from './parse-tester-plan.mjs';
import { trackIds, tracks } from '../src/lib/tracks.ts';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '..');
const outDir = join(root, 'src/lib/data');
const repo = resolve(root, '..');

const log = (...a) => console.log('[sync]', ...a);

/**
 * Writes only when something other than `generatedAt` changed, so a sync that found nothing
 * new leaves the committed catalogs alone instead of touching all six with a fresh timestamp.
 */
async function writeJson(name, value) {
	await mkdir(outDir, { recursive: true });
	const path = join(outDir, name);
	const next = JSON.stringify(value, null, '\t') + '\n';
	try {
		if (sameButForTimestamp(await readFile(path, 'utf8'), next)) return false;
	} catch {
		/* no committed copy yet */
	}
	await writeFile(path, next);
	return true;
}

/**
 * Worked examples are big — kafkatest's 91 of them are most of its 800 KB catalog, and a
 * single annotated frame can be a kilobyte of hex. Bundling them into `catalog.<track>.json`
 * puts all of it into the shared chunk that every page downloads, to show two examples on
 * one stage page.
 *
 * So they are split out: one file per stage, loaded on demand by `src/lib/examples/bytes.ts`
 * (and during prerender by the stage page's `load`). The catalog keeps only `exampleCount`,
 * which is all the UI needs to decide whether to draw the section at all.
 */
async function splitExamples(track, catalog) {
	const dir = join(outDir, 'examples', track);
	// Rewritten from scratch every sync, so a stage that loses its examples loses its file.
	await rm(dir, { recursive: true, force: true });
	let stages = 0;
	let examples = 0;
	for (const stage of catalog.stages) {
		const list = Array.isArray(stage.examples) ? stage.examples : [];
		delete stage.examples;
		if (list.length === 0) continue;
		if (stages === 0) await mkdir(dir, { recursive: true });
		stage.exampleCount = list.length;
		stages++;
		examples += list.length;
		await writeFile(join(dir, `${stage.number}.json`), JSON.stringify(list, null, '\t') + '\n');
	}
	return { stages, examples };
}

/** The `(planned)` stages from a tester's PLAN.md, or null when there is no readable plan. */
async function testerPlan(dir) {
	const planPath = join(dir, 'PLAN.md');
	if (!existsSync(planPath)) return null;
	try {
		const plan = parseTesterPlan(await readFile(planPath, 'utf8'));
		return plan.stages.length ? plan : null;
	} catch (err) {
		log(`! could not read ${planPath} (${err.message})`);
		return null;
	}
}

/**
 * The shell track: PLAN.md carries the stage list, the YAML suites carry the tests (and
 * the inputs and expectations the transcript examples are derived from).
 */
async function syncShell(meta, generatedAt) {
	const dir = join(repo, meta.dir);
	const planPath = join(dir, 'PLAN.md');
	if (!existsSync(planPath)) return null;
	const plan = parsePlan(await readFile(planPath, 'utf8'));
	if (plan.stages.length === 0) return null;
	const stagesDir = join(dir, 'tests/stages');
	const yamlDocs = {};
	if (existsSync(stagesDir)) {
		for (const file of (await readdir(stagesDir)).filter((f) => /\.ya?ml$/.test(f))) {
			yamlDocs[file] = parseYaml(await readFile(join(stagesDir, file), 'utf8'));
		}
	}
	const catalog = buildCatalog({
		track: meta.id,
		plan,
		yamlDocs,
		generatedAt,
		source: `${meta.dir}/PLAN.md + tests/stages/*.yaml`
	});
	for (const problem of auditCounts(plan, yamlDocs)) log(`! ${problem}`);
	if (plan.declared) {
		const { stages, tests } = plan.declared;
		if (stages !== catalog.totals.stages || tests !== catalog.totals.tests) {
			log(
				`! ${meta.dir}/PLAN.md declares ${stages} stages / ${tests} tests, parsed ` +
					`${catalog.totals.stages}/${catalog.totals.tests}`
			);
		}
	}
	return catalog;
}

/** Every other track: the tester's own `catalog.json`, its PLAN.md, or the placeholder. */
async function syncFromCatalog(meta, generatedAt) {
	const dir = join(repo, meta.dir);
	const titles = sectionTitles(meta.id);
	const file = join(dir, 'catalog.json');
	if (existsSync(file)) {
		try {
			const raw = JSON.parse(await readFile(file, 'utf8'));
			let catalog = normalizeCatalog(raw, {
				track: meta.id,
				generatedAt,
				titles,
				source: `${meta.dir}/catalog.json`
			});
			if (catalog.totals.stages > 0) {
				const shipped = catalog.totals.stages;
				const plan = await testerPlan(dir);
				if (plan) catalog = mergePlannedStages(catalog, plan, titles);
				const planned = catalog.stages.filter((s) => s.planned).length;
				log(
					`${meta.id}: ${catalog.totals.stages} stages, ${catalog.totals.tests} tests ` +
						`(${shipped} from ${meta.dir}/catalog.json` +
						(planned ? `, ${planned} still planned in ${meta.dir}/PLAN.md` : '') +
						')'
				);
				const missing = catalog.stages
					.filter((s) => !s.planned && s.tests.length === 0)
					.map((s) => s.number);
				if (missing.length) log(`! ${meta.id}: shipped stages with no tests: ${missing.join(', ')}`);
				return catalog;
			}
			log(`! ${meta.dir}/catalog.json parsed to zero stages — falling back`);
		} catch (err) {
			log(`! could not read ${meta.dir}/catalog.json (${err.message}) — falling back`);
		}
	}

	// No catalog yet: the tester's own PLAN.md is the next best truth.
	const plan = await testerPlan(dir);
	if (plan) {
		const catalog = buildCatalog({
			track: meta.id,
			plan: { ...plan, stages: plan.stages.map((s) => ({ ...s, planned: true })) },
			yamlDocs: {},
			generatedAt,
			pending: true,
			source: `${meta.dir}/PLAN.md (no catalog.json yet)`
		});
		log(`${meta.id}: ${catalog.totals.stages} planned stages from ${meta.dir}/PLAN.md (no catalog.json yet)`);
		return catalog;
	}

	const fallback = placeholderPlan(meta.id);
	if (!fallback) {
		log(`! ${meta.id}: nothing to build a catalog from — leaving catalog.${meta.id}.json alone`);
		return null;
	}
	const catalog = buildCatalog({
		track: meta.id,
		plan: fallback,
		yamlDocs: {},
		generatedAt,
		pending: true,
		source: `placeholder — ${meta.dir} has not published a catalog or a plan yet`
	});
	log(`${meta.id}: ${catalog.totals.stages} placeholder waypoints (${meta.dir}/ not found yet)`);
	return catalog;
}

const generatedAt = new Date().toISOString();
for (const id of trackIds) {
	const meta = tracks[id];
	const catalog = id === 'shell' ? await syncShell(meta, generatedAt) : await syncFromCatalog(meta, generatedAt);
	if (!catalog) {
		log(`! ${id}: no source found — the committed catalog.${id}.json is kept as it is`);
		continue;
	}
	const split = await splitExamples(id, catalog);
	await writeJson(`catalog.${id}.json`, catalog);
	if (split.examples) {
		log(`${id}: ${split.examples} examples on ${split.stages} stages → src/lib/data/examples/${id}/*.json`);
	}
	if (id === 'shell') {
		log(
			`shell: ${catalog.totals.stages} stages, ${catalog.totals.tests} tests, ` +
				`${catalog.totals.ext} ext stages`
		);
	}
}
log('done');
