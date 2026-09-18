#!/usr/bin/env node
/**
 * Rebuild the site's stage catalogs from the two Rust testers.
 *
 *   shell : ../shelltest/PLAN.md + ../shelltest/tests/stages/*.yaml
 *   kafka : ../kafkatest/catalog.json when it exists, otherwise a placeholder
 *           transcribed from BuildYourOwn/PLAN.md section 1.5 (pending: true)
 *
 * Output: src/lib/data/catalog.shell.json, src/lib/data/catalog.kafka.json
 */
import { readFile, readdir, writeFile, mkdir, rm } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parse as parseYaml } from 'yaml';
import { parsePlan, buildCatalog, auditCounts } from './parse-catalog.mjs';
import { normalizeKafkaCatalog, mergePlannedStages } from './normalize-kafka.mjs';
import { kafkaPlaceholderPlan } from './kafka-placeholder.mjs';
import { parseKafkaPlan } from './parse-kafka-plan.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '..');
const outDir = join(root, 'src/lib/data');
const shelltest = resolve(root, '../shelltest');
const kafkatest = resolve(root, '../kafkatest');

const log = (...a) => console.log('[sync]', ...a);

async function writeJson(name, value) {
	await mkdir(outDir, { recursive: true });
	await writeFile(join(outDir, name), JSON.stringify(value, null, '\t') + '\n');
}

const kafkaExamplesDir = join(outDir, 'examples/kafka');

/**
 * Worked examples are big — kafkatest's 91 of them are most of its 800 KB catalog, and a
 * single annotated frame can be a kilobyte of hex. Bundling them into `catalog.kafka.json`
 * puts all of it into the shared chunk that every page downloads, to show two examples on
 * one stage page.
 *
 * So they are split out: one file per stage, loaded on demand by
 * `src/lib/examples/kafka.ts` (and during prerender by the stage page's `load`). The
 * catalog keeps only `exampleCount`, which is all the UI needs to decide whether to draw
 * the section at all. Returns how many stages and examples were written.
 */
async function splitKafkaExamples(catalog) {
	// Rewritten from scratch every sync, so a stage that loses its examples loses its file.
	await rm(kafkaExamplesDir, { recursive: true, force: true });
	await mkdir(kafkaExamplesDir, { recursive: true });
	let stages = 0;
	let examples = 0;
	for (const stage of catalog.stages) {
		const list = Array.isArray(stage.examples) ? stage.examples : [];
		delete stage.examples;
		if (list.length === 0) continue;
		stage.exampleCount = list.length;
		stages++;
		examples += list.length;
		await writeFile(
			join(kafkaExamplesDir, `${stage.number}.json`),
			JSON.stringify(list, null, '\t') + '\n'
		);
	}
	return { stages, examples };
}

async function syncShell(generatedAt) {
	const planPath = join(shelltest, 'PLAN.md');
	if (!existsSync(planPath)) {
		log(`! ${planPath} not found — leaving catalog.shell.json alone`);
		return null;
	}
	const plan = parsePlan(await readFile(planPath, 'utf8'));
	const stagesDir = join(shelltest, 'tests/stages');
	const yamlDocs = {};
	for (const file of (await readdir(stagesDir)).filter((f) => /\.ya?ml$/.test(f))) {
		yamlDocs[file] = parseYaml(await readFile(join(stagesDir, file), 'utf8'));
	}
	const catalog = buildCatalog({
		track: 'shell',
		plan,
		yamlDocs,
		generatedAt,
		source: 'shelltest/PLAN.md + tests/stages/*.yaml'
	});
	for (const problem of auditCounts(plan, yamlDocs)) log(`! ${problem}`);
	if (plan.declared) {
		const { stages, tests } = plan.declared;
		if (stages !== catalog.totals.stages || tests !== catalog.totals.tests) {
			log(`! PLAN.md declares ${stages} stages / ${tests} tests, parsed ${catalog.totals.stages}/${catalog.totals.tests}`);
		}
	}
	await writeJson('catalog.shell.json', catalog);
	log(`shell: ${catalog.totals.stages} stages, ${catalog.totals.tests} tests, ${catalog.totals.ext} ext stages`);
	return catalog;
}

/**
 * The `(planned)` stages from kafkatest/PLAN.md — the gap-filler for a catalog.json that
 * is still being merged. Returns null when the plan cannot be read, which just means the
 * catalog is used as-is.
 */
async function kafkaPlan() {
	const planPath = join(kafkatest, 'PLAN.md');
	if (!existsSync(planPath)) return null;
	try {
		const plan = parseKafkaPlan(await readFile(planPath, 'utf8'));
		return plan.stages.length ? plan : null;
	} catch (err) {
		log(`! could not read kafkatest/PLAN.md (${err.message})`);
		return null;
	}
}

async function syncKafka(generatedAt) {
	const real = join(kafkatest, 'catalog.json');
	if (existsSync(real)) {
		try {
			const raw = JSON.parse(await readFile(real, 'utf8'));
			let catalog = normalizeKafkaCatalog(raw, generatedAt);
			if (catalog.totals.stages > 0) {
				const shipped = catalog.totals.stages;
				const plan = await kafkaPlan();
				if (plan) catalog = mergePlannedStages(catalog, plan);
				const planned = catalog.stages.filter((s) => s.planned).length;
				const split = await splitKafkaExamples(catalog);
				await writeJson('catalog.kafka.json', catalog);
				log(
					`kafka: ${catalog.totals.stages} stages, ${catalog.totals.tests} tests ` +
						`(${shipped} from kafkatest/catalog.json` +
						(planned ? `, ${planned} still planned in kafkatest/PLAN.md` : '') +
						')'
				);
				log(
					split.examples
						? `kafka: ${split.examples} examples on ${split.stages} stages → src/lib/data/examples/kafka/*.json`
						: 'kafka: no worked examples in catalog.json yet'
				);
				const missing = catalog.stages
					.filter((s) => !s.planned && s.tests.length === 0)
					.map((s) => s.number);
				if (missing.length) log(`! shipped stages with no tests: ${missing.join(', ')}`);
				return catalog;
			}
			log('! kafkatest/catalog.json parsed to zero stages — using the placeholder');
		} catch (err) {
			log(`! could not read kafkatest/catalog.json (${err.message}) — using the placeholder`);
		}
	}
	const catalog = buildCatalog({
		track: 'kafka',
		plan: kafkaPlaceholderPlan(),
		yamlDocs: {},
		generatedAt,
		pending: true,
		source: 'placeholder from BuildYourOwn/PLAN.md §1.5'
	});
	// Also clears any example files left behind by a previous, real catalog.
	await splitKafkaExamples(catalog);
	await writeJson('catalog.kafka.json', catalog);
	log(`kafka: ${catalog.totals.stages} placeholder stages (kafkatest/catalog.json not found yet)`);
	return catalog;
}

const generatedAt = new Date().toISOString();
await syncShell(generatedAt);
await syncKafka(generatedAt);
log('done');
