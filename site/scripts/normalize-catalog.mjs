/**
 * Turn whatever a tester's `--list --json` emits into the site's Catalog shape.
 *
 * Every tester writes the same schema (PLAN.md §5.2) but they are all being written in
 * parallel, so this is deliberately tolerant: accept both `stage`/`number`,
 * `skip_on`/`skipOn`, sections given either as a top-level array or as a per-stage letter,
 * and lower- or upper-case section ids.
 */

/** Section titles to fall back on, per track, when the catalog does not name them. */
const FALLBACK_TITLES = {
	kafka: {
		A: 'Bootstrap & framing',
		B: 'Metadata & topics',
		C: 'Fetch',
		D: 'Produce',
		E: 'Offsets & consumer groups',
		F: 'Interop, robustness, performance'
	}
};

function slugify(name, number) {
	const s = String(name ?? '')
		.toLowerCase()
		.replace(/\([^)]*\)/g, ' ')
		.replace(/[^a-z0-9]+/g, '-')
		.replace(/^-|-$/g, '')
		.slice(0, 48);
	return s || `stage-${number}`;
}

function normalizeTest(t) {
	if (typeof t === 'string') return { name: t };
	const tags = Array.isArray(t.tags) ? t.tags.map(String) : [];
	const skipOn = t.skipOn ?? t.skip_on;
	const out = { name: String(t.name ?? 'unnamed') };
	if (t.ext === true || tags.includes('ext')) out.ext = true;
	if (tags.length) out.tags = tags;
	if (Array.isArray(skipOn) && skipOn.length) out.skipOn = skipOn.map(String);
	return out;
}

/**
 * @param raw   the parsed `<tester>/catalog.json`
 * @param opts  `{ track, generatedAt, titles }` — `titles` maps a section letter to a name
 *              for the case where the tester emits stage letters but no section list.
 */
export function normalizeCatalog(raw, { track, generatedAt, titles = {}, source }) {
	const fallbackTitles = { ...(FALLBACK_TITLES[track] ?? {}), ...titles };
	const rawStages = Array.isArray(raw) ? raw : (raw.stages ?? []);
	const stages = rawStages.map((s) => {
		const number = Number(s.number ?? s.stage ?? 0);
		const name = String(s.name ?? `Stage ${number}`);
		const tests = Array.isArray(s.tests) ? s.tests.map(normalizeTest) : [];
		return {
			number,
			slug: s.slug ? String(s.slug) : slugify(name, number),
			name,
			ext: Boolean(s.ext),
			// Testers write their section letters lower case; the site's camp badges are keyed
			// by the upper-case letter used everywhere else (the testers' PLAN.md headings).
			section: String(s.section ?? sectionForNumber(raw, number)).toUpperCase(),
			file: String(s.file ?? ''),
			planDone: Boolean(s.planDone ?? s.done ?? false),
			hints: Array.isArray(s.hints) ? s.hints.map(String) : [],
			tests,
			// Worked examples (PLAN.md §4.2, §5.2) are copied through verbatim, snake_case keys
			// and all: the tester owns that schema and the site reads it tolerantly
			// (src/lib/examples/bytes.ts), so a new field here needs no change in this script.
			...(Array.isArray(s.examples) && s.examples.length ? { examples: s.examples } : {})
		};
	});
	stages.sort((a, b) => a.number - b.number);

	let sections;
	if (Array.isArray(raw.sections) && raw.sections.length) {
		sections = raw.sections.map((s) => {
			const id = String(s.id ?? s.letter ?? '?').toUpperCase();
			return {
				id,
				title: String(s.title ?? fallbackTitles[id] ?? 'Section'),
				stages: Array.isArray(s.stages)
					? s.stages.map(Number)
					: stages.filter((st) => st.section === id).map((st) => st.number)
			};
		});
	} else {
		const seen = new Map();
		for (const st of stages) {
			if (!seen.has(st.section)) seen.set(st.section, []);
			seen.get(st.section).push(st.number);
		}
		sections = [...seen.entries()].map(([id, nums]) => ({
			id,
			title: fallbackTitles[id] ?? `Section ${id}`,
			stages: nums
		}));
	}

	return {
		track,
		generatedAt,
		sections,
		stages,
		totals: totalsOf(stages),
		source: source ?? `${track}test/catalog.json`
	};
}

/** Back-compat wrapper: the kafka-shaped call this module started life with. */
export function normalizeKafkaCatalog(raw, generatedAt) {
	return normalizeCatalog(raw, { track: 'kafka', generatedAt });
}

function totalsOf(stages) {
	return {
		stages: stages.length,
		tests: stages.reduce((n, s) => n + s.tests.length, 0),
		ext: stages.filter((s) => s.ext).length
	};
}

/**
 * Fill the gaps in a partially-merged `catalog.json` from the `(planned)` entries in the
 * tester's PLAN.md, so every stage the plan promises exists on the site (flagged
 * `planned: true`, with no tests). Stages already in the catalog always win; this only
 * ever adds. Once every stage is merged upstream this is a no-op.
 */
export function mergePlannedStages(catalog, plan, titles = {}) {
	if (!plan || !Array.isArray(plan.stages) || plan.stages.length === 0) return catalog;
	const fallbackTitles = { ...(FALLBACK_TITLES[catalog.track] ?? {}), ...titles };

	const byNumber = new Map(catalog.stages.map((s) => [s.number, s]));
	const added = [];
	for (const p of plan.stages) {
		if (byNumber.has(p.number)) continue;
		const stage = {
			number: p.number,
			slug: p.slug,
			name: p.name,
			ext: Boolean(p.ext),
			section: String(p.section ?? '?').toUpperCase(),
			file: String(p.file ?? ''),
			planDone: Boolean(p.planDone),
			planned: true,
			hints: Array.isArray(p.hints) ? p.hints.map(String) : [],
			tests: []
		};
		byNumber.set(stage.number, stage);
		added.push(stage);
	}
	if (added.length === 0) return catalog;

	const stages = [...catalog.stages, ...added].sort((a, b) => a.number - b.number);

	// Every stage must sit in exactly one section, or the map and the camp lists lose it.
	const sections = catalog.sections.map((s) => ({ ...s, stages: [...s.stages] }));
	const byId = new Map(sections.map((s) => [s.id, s]));
	const covered = new Set(sections.flatMap((s) => s.stages));
	for (const stage of added) {
		if (covered.has(stage.number)) continue;
		let section = byId.get(stage.section);
		if (!section) {
			const fromPlan = (plan.sections ?? []).find((s) => s.id === stage.section);
			section = {
				id: stage.section,
				title: fromPlan?.title ?? fallbackTitles[stage.section] ?? `Section ${stage.section}`,
				stages: []
			};
			sections.push(section);
			byId.set(section.id, section);
		}
		section.stages.push(stage.number);
		covered.add(stage.number);
	}
	for (const s of sections) s.stages.sort((a, b) => a - b);
	sections.sort((a, b) => a.id.localeCompare(b.id));

	return {
		...catalog,
		sections,
		stages,
		totals: totalsOf(stages),
		source: `${catalog.source ?? 'catalog.json'} + PLAN.md (planned stages)`
	};
}

/**
 * Kafka's catalog.json once shipped stages with no section letter at all; the ranges below
 * are its PLAN.md §1.5 grouping. Any other track with no section information falls into
 * section A rather than inventing a shape for it.
 */
function sectionForNumber(raw, number) {
	if (Array.isArray(raw.sections)) {
		for (const s of raw.sections) {
			if (Array.isArray(s.stages) && s.stages.map(Number).includes(number)) return String(s.id ?? '?');
		}
	}
	if (raw.track !== undefined && raw.track !== 'kafka') return 'A';
	if (number <= 9) return 'A';
	if (number <= 18) return 'B';
	if (number <= 28) return 'C';
	if (number <= 37) return 'D';
	if (number <= 42) return 'E';
	return 'F';
}
