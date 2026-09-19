import { describe, it, expect } from 'vitest';
import { readFile, readdir } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parse as parseYaml } from 'yaml';
import { parsePlan, buildCatalog, testsFromYaml, slugFromFile, auditCounts } from './parse-catalog.mjs';
import {
	normalizeCatalog,
	normalizeKafkaCatalog,
	mergePlannedStages,
	sameButForTimestamp
} from './normalize-catalog.mjs';
import { parseTesterPlan as parseKafkaPlan } from './parse-tester-plan.mjs';
import { kafkaPlaceholderPlan } from './kafka-placeholder.mjs';
import { placeholderPlan, sectionTitles } from './placeholders.mjs';
import { tracks, trackIds } from '../src/lib/tracks.ts';

const here = dirname(fileURLToPath(import.meta.url));
const shelltest = resolve(here, '../../shelltest');
const kafkatest = resolve(here, '../../kafkatest');
const hasShelltest = existsSync(join(shelltest, 'PLAN.md'));

const PLAN_SNIPPET = `# shelltest stage plan

Intro line that must be ignored.

## A. Basics

- [x] **Stage 01** — Print the prompt and wait for input (\`01_prompt.yaml\`, 6 tests)
  - Write the prompt to stdout and flush before blocking on a read
  - Read one line at a time; treat EOF as 'exit'
- [ ] **Stage 02** — Invalid command reports an error (\`02_invalid_command.yaml\`, 6 tests)
  - Split the line on whitespace; the first word is the command

## B. Quoting & parsing

- [ ] **Stage 20** — Variable expansion **[ext]** (\`20_variables.yaml\`, 9 tests)
  - Expand \`$NAME\`, \`\${NAME}\`, \`$?\` while scanning outside single quotes

**3 stages, 21 tests.**
`;

describe('parsePlan', () => {
	const plan = parsePlan(PLAN_SNIPPET);

	it('reads sections with their letter ids', () => {
		expect(plan.sections.map((s) => s.id)).toEqual(['A', 'B']);
		expect(plan.sections[0].title).toBe('Basics');
		expect(plan.sections[0].stages).toEqual([1, 2]);
		expect(plan.sections[1].stages).toEqual([20]);
	});

	it('reads stage number, name, file and test count', () => {
		expect(plan.stages).toHaveLength(3);
		expect(plan.stages[0]).toMatchObject({
			number: 1,
			name: 'Print the prompt and wait for input',
			file: '01_prompt.yaml',
			plannedTests: 6,
			ext: false,
			planDone: true,
			section: 'A'
		});
	});

	it('strips the **[ext]** marker into a flag', () => {
		const s20 = plan.stages.find((s) => s.number === 20);
		expect(s20.ext).toBe(true);
		expect(s20.name).toBe('Variable expansion');
		expect(s20.planDone).toBe(false);
	});

	it('extracts the indented hint bullets per stage', () => {
		expect(plan.stages[0].hints).toEqual([
			'Write the prompt to stdout and flush before blocking on a read',
			"Read one line at a time; treat EOF as 'exit'"
		]);
		expect(plan.stages[1].hints).toHaveLength(1);
	});

	it('reads the declared totals footer', () => {
		expect(plan.declared).toEqual({ stages: 3, tests: 21 });
	});
});

describe('slugFromFile', () => {
	it('drops the number prefix and extension', () => {
		expect(slugFromFile('01_prompt.yaml')).toBe('prompt');
		expect(slugFromFile('29_redirect_combos.yaml')).toBe('redirect-combos');
	});
});

describe('testsFromYaml', () => {
	it('maps tags, ext, skip_on and pty mode', () => {
		const tests = testsFromYaml({
			tests: [
				{ name: 'plain' },
				{ name: 'extended', tags: ['ext'] },
				{ name: 'skipped', skip_on: ['zsh'], reason: 'wording' },
				{ name: 'terminal', mode: 'pty' }
			]
		});
		expect(tests[0]).toEqual({ name: 'plain' });
		expect(tests[1]).toEqual({ name: 'extended', ext: true, tags: ['ext'] });
		expect(tests[2]).toEqual({ name: 'skipped', reason: 'wording', skipOn: ['zsh'] });
		expect(tests[3]).toEqual({ name: 'terminal', mode: 'pty' });
	});

	it('tolerates a document with no tests', () => {
		expect(testsFromYaml(null)).toEqual([]);
		expect(testsFromYaml({})).toEqual([]);
	});
});

describe.skipIf(!hasShelltest)('the real shelltest suite', () => {
	async function load() {
		const plan = parsePlan(await readFile(join(shelltest, 'PLAN.md'), 'utf8'));
		const dir = join(shelltest, 'tests/stages');
		const yamlDocs = {};
		for (const f of (await readdir(dir)).filter((f) => /\.ya?ml$/.test(f))) {
			yamlDocs[f] = parseYaml(await readFile(join(dir, f), 'utf8'));
		}
		return { plan, yamlDocs };
	}

	it('parses what the plan declares, down to the footer', async () => {
		const { plan, yamlDocs } = await load();
		const catalog = buildCatalog({
			track: 'shell',
			plan,
			yamlDocs,
			generatedAt: '2026-01-01T00:00:00.000Z'
		});
		// Numbers counted from the suite, not written down here: the invariant worth holding
		// is that the YAML on disk and PLAN.md's own footer agree, whatever the suite has
		// grown to since.
		expect(catalog.totals.stages).toBe(plan.declared.stages);
		expect(catalog.totals.tests).toBe(plan.declared.tests);
		expect(catalog.totals.stages).toBeGreaterThan(50);
	});

	it('agrees with PLAN.md on every per-stage test count', async () => {
		const { plan, yamlDocs } = await load();
		expect(auditCounts(plan, yamlDocs)).toEqual([]);
	});

	it('marks the ext stages and keeps hints for every stage', async () => {
		const { plan, yamlDocs } = await load();
		const catalog = buildCatalog({ track: 'shell', plan, yamlDocs, generatedAt: 'x' });
		const ext = catalog.stages.filter((s) => s.ext).map((s) => s.number);
		expect(ext).toContain(20);
		expect(ext).toContain(57);
		expect(ext).not.toContain(1);
		expect(catalog.stages.every((s) => s.hints.length >= 2)).toBe(true);
	});

	it('carries per-test ext and skipOn flags through', async () => {
		const { plan, yamlDocs } = await load();
		const catalog = buildCatalog({ track: 'shell', plan, yamlDocs, generatedAt: 'x' });
		const globbing = catalog.stages.find((s) => s.number === 23);
		expect(globbing.tests.every((t) => t.ext)).toBe(true);
		const history = catalog.stages.find((s) => s.number === 41);
		expect(history.tests.some((t) => (t.skipOn ?? []).includes('zsh'))).toBe(true);
	});

	it('covers a contiguous run of sections with no stage left out', async () => {
		const { plan, yamlDocs } = await load();
		const catalog = buildCatalog({ track: 'shell', plan, yamlDocs, generatedAt: 'x' });
		const ids = catalog.sections.map((s) => s.id);
		expect(ids[0]).toBe('A');
		expect(ids).toEqual(ids.map((_, i) => String.fromCharCode('A'.charCodeAt(0) + i)));
		const covered = catalog.sections.flatMap((s) => s.stages).sort((a, b) => a - b);
		expect(covered).toEqual(catalog.stages.map((s) => s.number));
	});
});

describe('kafka placeholder', () => {
	it('is internally consistent, whatever size it has grown to', () => {
		const plan = kafkaPlaceholderPlan();
		// The placeholder is a transcription of the plan, so what is worth holding is that it
		// hangs together — contiguous sections, every stage covered — rather than a count
		// that has to be edited in two places whenever the plan grows.
		const ids = plan.sections.map((s) => s.id);
		expect(ids[0]).toBe('A');
		expect(ids).toEqual(ids.map((_, i) => String.fromCharCode('A'.charCodeAt(0) + i)));
		expect(plan.stages.length).toBeGreaterThan(40);
		expect(plan.stages.every((s) => s.hints.length >= 1)).toBe(true);
		expect(plan.stages.filter((s) => s.ext).length).toBeGreaterThan(15);
	});

	it('builds a catalog flagged pending', () => {
		const catalog = buildCatalog({
			track: 'kafka',
			plan: kafkaPlaceholderPlan(),
			yamlDocs: {},
			generatedAt: 'x',
			pending: true
		});
		expect(catalog.pending).toBe(true);
		expect(catalog.totals.stages).toBe(45);
		expect(catalog.totals.tests).toBe(0);
	});
});

describe('normalizeKafkaCatalog', () => {
	it('accepts kafkatest field spellings and sorts by stage number', () => {
		const catalog = normalizeKafkaCatalog(
			{
				target: 'apache_kafka',
				stages: [
					{ stage: 2, name: 'Correlation id', tests: [{ name: 'echoes id' }] },
					{
						stage: 1,
						name: 'Bind to port 9092',
						hints: ['listen'],
						tests: [{ name: 'accepts', tags: ['ext'], skip_on: ['apache_kafka'] }]
					}
				]
			},
			'2026-01-01'
		);
		expect(catalog.track).toBe('kafka');
		expect(catalog.stages.map((s) => s.number)).toEqual([1, 2]);
		expect(catalog.stages[0].tests[0]).toEqual({
			name: 'accepts',
			ext: true,
			tags: ['ext'],
			skipOn: ['apache_kafka']
		});
		expect(catalog.totals.tests).toBe(2);
		expect(catalog.sections[0].id).toBe('A');
		expect(catalog.pending).toBeUndefined();
	});
});

describe('parseKafkaPlan', () => {
	const PLAN_SNIPPET = `# kafkatest stage plan

Stages marked **(planned)** have no tests yet.

## A. Bootstrap & framing

- [x] **Stage 01** — Bind to the broker port (\`src/stages/s01_bind.rs\`, 6 tests)
  - Create a TCP listener on 0.0.0.0:9092 and accept connections in a loop
  - Set SO_REUSEADDR so a restart does not hit 'address already in use'
- [ ] **Stage 02** — Respond with the correlation id (\`src/stages/s02_correlation_id.rs\`, 6 tests)
  - Read the 4-byte big-endian message size, then that many bytes

## B. Metadata & topics

- [ ] **Stage 15** — Response partition limit and cursor pagination **[ext]** (planned)
  - response_partition_limit caps the partitions in one response, across topics
  - A request carrying a cursor starts at that topic/partition
`;

	const plan = parseKafkaPlan(PLAN_SNIPPET);

	it('reads both the shipped and the planned stage spellings', () => {
		expect(plan.stages.map((s) => s.number)).toEqual([1, 2, 15]);
		expect(plan.stages[0]).toMatchObject({
			number: 1,
			name: 'Bind to the broker port',
			file: 'src/stages/s01_bind.rs',
			slug: 'bind',
			plannedTests: 6,
			planned: false,
			planDone: true,
			ext: false,
			section: 'A'
		});
	});

	it('flags a (planned) stage, strips the marker and keeps the [ext] flag', () => {
		const s15 = plan.stages.find((s) => s.number === 15);
		expect(s15.planned).toBe(true);
		expect(s15.ext).toBe(true);
		expect(s15.name).toBe('Response partition limit and cursor pagination');
		expect(s15.file).toBe('');
		expect(s15.slug).toBe('response-partition-limit-and-cursor-pagination');
		expect(s15.section).toBe('B');
		expect(s15.hints).toHaveLength(2);
	});

	it('groups stages under their upper-case section letters', () => {
		expect(plan.sections.map((s) => s.id)).toEqual(['A', 'B']);
		expect(plan.sections[0].stages).toEqual([1, 2]);
		expect(plan.sections[1].stages).toEqual([15]);
	});
});

describe('mergePlannedStages (a gapped kafkatest catalog)', () => {
	// kafkatest is written by several agents at once, so catalog.json legitimately has
	// holes mid-merge. The site must still know about all 45 stages.
	const gapped = normalizeKafkaCatalog(
		{
			sections: [
				{ id: 'a', title: 'Bootstrap & framing', stages: [1, 2] },
				{ id: 'b', title: 'Metadata & topics', stages: [15] }
			],
			stages: [
				{ number: 1, name: 'Bind to the broker port', tests: [{ name: 'accepts' }] },
				{ number: 2, name: 'Respond with the correlation id', tests: [{ name: 'echoes' }] }
			]
		},
		'2026-01-01'
	);
	const plan = parseKafkaPlan(`## A. Bootstrap & framing

- [x] **Stage 01** — Bind to the broker port (\`src/stages/s01_bind.rs\`, 6 tests)
  - listen
- [ ] **Stage 02** — Respond with the correlation id (\`src/stages/s02_correlation_id.rs\`, 6 tests)
  - echo it

## B. Metadata & topics

- [ ] **Stage 15** — Response partition limit **[ext]** (planned)
  - cursor pagination
- [ ] **Stage 16** — Metadata v12 **[ext]** (planned)
  - topic ids

## C. Fetch

- [ ] **Stage 19** — Advertise Fetch **[ext]** (planned)
  - api key 1
`);

	const merged = mergePlannedStages(gapped, plan);

	it('fills every hole in the catalog from the plan', () => {
		expect(gapped.stages.map((s) => s.number)).toEqual([1, 2]);
		expect(merged.stages.map((s) => s.number)).toEqual([1, 2, 15, 16, 19]);
	});

	it('flags the filled-in stages as planned and leaves them testless', () => {
		const planned = merged.stages.filter((s) => s.planned);
		expect(planned.map((s) => s.number)).toEqual([15, 16, 19]);
		expect(planned.every((s) => s.tests.length === 0)).toBe(true);
		expect(planned.every((s) => s.hints.length > 0)).toBe(true);
		expect(merged.stages.find((s) => s.number === 1).planned).toBeUndefined();
	});

	it('never overwrites a stage the tester already shipped', () => {
		expect(merged.stages.find((s) => s.number === 1).tests).toEqual([{ name: 'accepts' }]);
		expect(merged.totals.tests).toBe(2);
		expect(merged.totals.stages).toBe(5);
	});

	it('puts every stage in exactly one section, adding sections the catalog lacked', () => {
		expect(merged.sections.map((s) => s.id)).toEqual(['A', 'B', 'C']);
		const covered = merged.sections.flatMap((s) => s.stages);
		expect([...covered].sort((a, b) => a - b)).toEqual([1, 2, 15, 16, 19]);
		expect(new Set(covered).size).toBe(covered.length);
		expect(merged.sections.find((s) => s.id === 'C').title).toBe('Fetch');
	});

	it('is a no-op once the tester has merged everything', () => {
		const complete = normalizeKafkaCatalog(
			{
				sections: [{ id: 'a', title: 'Bootstrap & framing', stages: [1, 2] }],
				stages: [
					{ number: 1, name: 'Bind', tests: [{ name: 'a' }] },
					{ number: 2, name: 'Correlation id', tests: [{ name: 'b' }] }
				]
			},
			'2026-01-01'
		);
		const plan2 = parseKafkaPlan(`## A. Bootstrap & framing

- [x] **Stage 01** — Bind (\`src/stages/s01_bind.rs\`, 1 tests)
  - listen
- [x] **Stage 02** — Correlation id (\`src/stages/s02.rs\`, 1 tests)
  - echo
`);
		expect(mergePlannedStages(complete, plan2)).toBe(complete);
	});

	it('leaves the catalog alone when the plan cannot be read', () => {
		expect(mergePlannedStages(gapped, null)).toBe(gapped);
		expect(mergePlannedStages(gapped, { sections: [], stages: [] })).toBe(gapped);
	});
});

describe.skipIf(!existsSync(join(kafkatest, 'PLAN.md')))('the real kafkatest sources', () => {
	it('reaches every planned stage however far catalog.json has been merged', async () => {
		const plan = parseKafkaPlan(await readFile(join(kafkatest, 'PLAN.md'), 'utf8'));
		const ids = plan.sections.map((s) => s.id);
		expect(ids[0]).toBe('A');
		expect(ids).toEqual(ids.map((_, i) => String.fromCharCode('A'.charCodeAt(0) + i)));
		expect(plan.stages.length).toBeGreaterThan(40);
		expect(plan.stages.every((s) => s.hints.length >= 1)).toBe(true);

		const rawCatalog = existsSync(join(kafkatest, 'catalog.json'))
			? JSON.parse(await readFile(join(kafkatest, 'catalog.json'), 'utf8'))
			: { sections: [], stages: [] };
		const merged = mergePlannedStages(normalizeKafkaCatalog(rawCatalog, 'x'), plan);
		expect(merged.totals.stages).toBe(plan.stages.length);
		expect(merged.stages.map((s) => s.number)).toEqual(
			Array.from({ length: plan.stages.length }, (_, i) => i + 1)
		);
		// every stage is reachable from exactly one camp, so the map draws all of them
		const covered = merged.sections.flatMap((s) => s.stages).sort((a, b) => a - b);
		expect(covered).toEqual(merged.stages.map((s) => s.number));
		// and no stage is both shipped and testless, which would render an empty test list
		for (const s of merged.stages) {
			expect(s.planned === true || s.tests.length > 0).toBe(true);
		}
	});

	// The real catalog.json fills up as kafkatest's stages are merged, so the interesting
	// case — the one that used to break `npm run build` with `404 /kafka/16/` — has to be
	// reproduced deliberately: knock holes in the real catalog and check the plan fills them.
	it('survives any hole in catalog.json, from every stage present to none', async () => {
		const plan = parseKafkaPlan(await readFile(join(kafkatest, 'PLAN.md'), 'utf8'));
		const rawCatalog = existsSync(join(kafkatest, 'catalog.json'))
			? JSON.parse(await readFile(join(kafkatest, 'catalog.json'), 'utf8'))
			: { sections: [], stages: [] };
		const full = normalizeKafkaCatalog(rawCatalog, 'x');
		const n = plan.stages.length;
		const all = Array.from({ length: n }, (_, i) => i + 1);

		for (const keep of [n, 24, 14, 9, 1, 0]) {
			const gapped = normalizeKafkaCatalog(
				{ ...rawCatalog, stages: full.stages.slice(0, keep) },
				'x'
			);
			const merged = mergePlannedStages(gapped, plan);
			expect({ keep, numbers: merged.stages.map((s) => s.number) }).toEqual({ keep, numbers: all });
			expect(merged.stages.filter((s) => s.planned).length).toBe(n - keep);
			// what the prerenderer walks: a section entry for every stage, and no duplicates
			const covered = merged.sections.flatMap((s) => s.stages);
			expect(new Set(covered).size).toBe(covered.length);
			expect([...covered].sort((a, b) => a - b)).toEqual(all);
			// and the tests that did ship are untouched
			expect(merged.totals.tests).toBe(
				full.stages.slice(0, keep).reduce((n, s) => n + s.tests.length, 0)
			);
		}
	});
});

describe('the placeholder plans (a tester that has not landed yet)', () => {
	const placeholders = trackIds
		.map((track) => ({ track, plan: placeholderPlan(track) }))
		.filter(({ plan }) => plan !== null);

	it('covers every track whose tester may not exist yet', () => {
		// shell is parsed from its own PLAN.md and never needs one; every other track does,
		// because `npm run sync` has to produce a complete site from an empty repo.
		expect(placeholders.map((p) => p.track).sort()).toEqual(
			trackIds.filter((t) => t !== 'shell').sort()
		);
	});

	it('gives each section one honest waypoint, with what it will cover', () => {
		for (const { track, plan } of placeholders) {
			if (track === 'kafka') continue; // kafka's placeholder is its real stage list
			expect(plan.stages.length, track).toBe(plan.sections.length);
			for (const stage of plan.stages) {
				expect(stage.planned, `${track} ${stage.number}`).toBe(true);
				expect(stage.tests, `${track} ${stage.number}`).toEqual([]);
				expect(stage.hints.length, `${track} ${stage.number}`).toBeGreaterThanOrEqual(2);
				expect(stage.name, `${track} ${stage.number}`).toMatch(/\S/);
			}
			// Every section is covered exactly once, in order.
			expect(plan.sections.flatMap((s) => s.stages)).toEqual(plan.stages.map((s) => s.number));
		}
	});

	it('uses the section letters the registry has garden names for', () => {
		for (const { track, plan } of placeholders) {
			for (const section of plan.sections) {
				expect(tracks[track].sectionBadges[section.id], `${track} ${section.id}`).toBeDefined();
			}
			expect(Object.keys(sectionTitles(track)).length === 0 || plan.sections.length).toBeTruthy();
		}
	});

	it('tags the dist waypoints with the ladders the registry declares', () => {
		const plan = placeholderPlan('dist');
		const ladders = tracks.dist.ladders;
		for (const stage of plan.stages) {
			const ladder = ladders.find((l) => l.sections.includes(stage.section));
			expect(ladder, `dist section ${stage.section}`).toBeDefined();
			expect(stage.ladder, `dist stage ${stage.number}`).toBe(ladder.id);
		}
	});

	it('builds a pending catalog that still prerenders every stage', () => {
		for (const { track, plan } of placeholders) {
			const catalog = buildCatalog({
				track,
				plan,
				yamlDocs: {},
				generatedAt: 'x',
				pending: true,
				source: 'placeholder'
			});
			expect(catalog.pending, track).toBe(true);
			expect(catalog.totals.tests, track).toBe(0);
			expect(catalog.totals.stages, track).toBe(plan.stages.length);
			const covered = catalog.sections.flatMap((s) => s.stages).sort((a, b) => a - b);
			expect(covered, track).toEqual(catalog.stages.map((s) => s.number));
		}
	});
});

describe('normalizeCatalog, for any tester', () => {
	it('reads a wasm-shaped catalog without knowing anything about wasm', () => {
		const catalog = normalizeCatalog(
			{
				track: 'wasm',
				sections: [{ id: 'a', title: 'Binary format & decoding', stages: [1] }],
				stages: [
					{
						number: 1,
						name: 'The magic number and the version',
						hints: ['read the first eight bytes'],
						tests: [{ name: 'rejects a bad magic' }],
						examples: [{ title: 'x', request_hex: '00' }]
					}
				]
			},
			{ track: 'wasm', generatedAt: 'x', titles: sectionTitles('wasm'), source: 'wasmtest/catalog.json' }
		);
		expect(catalog.track).toBe('wasm');
		expect(catalog.sections[0].id).toBe('A');
		expect(catalog.totals).toEqual({ stages: 1, tests: 1, ext: 0 });
		// examples are copied through verbatim for the sync script to split out
		expect(catalog.stages[0].examples).toHaveLength(1);
		expect(catalog.source).toBe('wasmtest/catalog.json');
	});

	it('carries a ladder through, under either spelling, and omits it otherwise', () => {
		const withLadder = normalizeCatalog(
			{ stages: [{ number: 1, name: 'Clocks', section: 'a', ladder: 'primitives' }] },
			{ track: 'dist', generatedAt: 'x' }
		);
		expect(withLadder.stages[0].ladder).toBe('primitives');
		const withTier = normalizeCatalog(
			{ stages: [{ number: 1, name: 'Clocks', section: 'a', tier: 'cluster' }] },
			{ track: 'dist', generatedAt: 'x' }
		);
		expect(withTier.stages[0].ladder).toBe('cluster');
		const without = normalizeCatalog(
			{ stages: [{ number: 1, name: 'Bind', section: 'a' }] },
			{ track: 'tls', generatedAt: 'x' }
		);
		expect(without.stages[0].ladder).toBeUndefined();
	});

	it('does not apply kafka’s stage-number section ranges to another track', () => {
		const wasm = normalizeCatalog(
			{ track: 'wasm', stages: [{ number: 40, name: 'fd_write' }] },
			{ track: 'wasm', generatedAt: 'x' }
		);
		expect(wasm.stages[0].section).toBe('A');
	});
});

describe('the generated catalogs on disk', () => {
	async function catalogOf(track) {
		return JSON.parse(await readFile(join(here, `../src/lib/data/catalog.${track}.json`), 'utf8'));
	}

	it('has a committed catalog for every registered track', async () => {
		for (const track of trackIds) {
			const catalog = await catalogOf(track);
			expect(catalog.track, track).toBe(track);
			expect(catalog.stages.length, track).toBeGreaterThan(0);
			// Every stage sits in exactly one section, or the map and the camps lose it.
			const covered = catalog.sections.flatMap((s) => s.stages).sort((a, b) => a - b);
			expect(covered, track).toEqual(catalog.stages.map((s) => s.number).sort((a, b) => a - b));
			expect(new Set(covered).size, track).toBe(covered.length);
		}
	});

	it('keeps every trail at the size its own tester reports', async () => {
		for (const track of trackIds) {
			const catalog = await catalogOf(track);
			expect(catalog.totals.stages, track).toBe(catalog.stages.length);
			expect(catalog.totals.stages, track).toBeGreaterThan(20);
			expect(catalog.totals.tests, track).toBe(
				catalog.stages.reduce((n, s) => n + (s.tests?.length ?? 0), 0)
			);
		}
	});

	it('has a resource file for every track, and every link points at a real stage', async () => {
		for (const track of trackIds) {
			const numbers = new Set((await catalogOf(track)).stages.map((s) => s.number));
			const resources = JSON.parse(
				await readFile(join(here, `../src/lib/data/resources.${track}.json`), 'utf8')
			);
			expect(Array.isArray(resources), track).toBe(true);
			for (const r of resources) {
				for (const stage of r.stages ?? []) {
					expect(
						{ id: r.id, stage, exists: numbers.has(stage) },
						`${track} resource ${r.id} links to stage ${stage}`
					).toEqual({ id: r.id, stage, exists: true });
				}
			}
		}
	});

	it('keeps the worked examples out of the catalogs and in per-stage files', async () => {
		for (const track of trackIds) {
			const catalog = await catalogOf(track);
			for (const stage of catalog.stages) {
				expect(stage.examples, `${track} ${stage.number}`).toBeUndefined();
				if (!stage.exampleCount) continue;
				const file = join(here, `../src/lib/data/examples/${track}/${stage.number}.json`);
				expect(existsSync(file), `${track} stage ${stage.number} example file`).toBe(true);
				expect(JSON.parse(await readFile(file, 'utf8'))).toHaveLength(stage.exampleCount);
			}
		}
	});
});

describe('rewriting a catalog only when it changed', () => {
	const at = (stamp, tests = 3) =>
		JSON.stringify({ track: 'dist', generatedAt: stamp, totals: { tests } }, null, '\t') + '\n';

	it('treats a fresh timestamp over identical content as no change', () => {
		expect(sameButForTimestamp(at('2026-09-19T05:05:20.894Z'), at('2026-09-19T06:08:03.028Z'))).toBe(
			true
		);
	});

	it('still sees a real change, timestamp or not', () => {
		expect(sameButForTimestamp(at('2026-09-19T05:05:20.894Z'), at('2026-09-19T05:05:20.894Z', 4))).toBe(
			false
		);
	});

	it('says nothing is the same as unparseable', () => {
		expect(sameButForTimestamp('{ not json', at('2026-09-19T05:05:20.894Z'))).toBe(false);
	});
});
