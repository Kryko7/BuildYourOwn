/**
 * Pure parsers for the shelltest sources. No file IO lives here so the whole
 * pipeline can be unit-tested from fixtures (see parse-catalog.test.mjs).
 */

const STAGE_RE = /^- \[([ xX])\] \*\*Stage (\d+)\*\*\s+[—-]\s+(.+?)\s+\(`([^`]+)`,\s*(\d+)\s*tests?\)\s*$/;
const SECTION_RE = /^##\s+([A-Z])\.\s+(.+?)\s*$/;
const HINT_RE = /^\s{2,}-\s+(.+?)\s*$/;
const TOTAL_RE = /\*\*(\d+)\s+stages?,\s*(\d+)\s+tests?\.?\*\*/;

/** `01_prompt.yaml` -> `prompt`; `29_redirect_combos.yaml` -> `redirect-combos`. */
export function slugFromFile(file) {
	return file
		.replace(/\.ya?ml$/i, '')
		.replace(/^\d+[_-]/, '')
		.replace(/_/g, '-');
}

/**
 * Parse shelltest/PLAN.md into sections + stages (number, name, ext, file, hints).
 * Hint bullets are the indented `- ` lines directly under a stage line.
 */
export function parsePlan(markdown) {
	const sections = [];
	const stages = [];
	let section = null;
	let current = null;

	for (const raw of markdown.split('\n')) {
		const line = raw.replace(/\s+$/, '');
		const sec = SECTION_RE.exec(line);
		if (sec) {
			const title = sec[2].replace(/\s*\[ext\]\s*$/i, '').trim();
			section = { id: sec[1], title, stages: [] };
			sections.push(section);
			current = null;
			continue;
		}
		const st = STAGE_RE.exec(line);
		if (st) {
			let name = st[3].trim();
			let ext = false;
			if (/\*\*\[ext\]\*\*/.test(name)) {
				ext = true;
				name = name.replace(/\s*\*\*\[ext\]\*\*\s*/g, ' ').trim();
			}
			current = {
				number: Number(st[2]),
				slug: slugFromFile(st[4]),
				name,
				ext,
				file: st[4],
				planDone: st[1].toLowerCase() === 'x',
				plannedTests: Number(st[5]),
				section: section ? section.id : '?',
				hints: [],
				tests: []
			};
			stages.push(current);
			if (section) section.stages.push(current.number);
			continue;
		}
		if (current) {
			const hint = HINT_RE.exec(raw);
			if (hint) {
				current.hints.push(hint[1]);
				continue;
			}
			if (line.trim() === '') continue;
			current = null;
		}
	}

	const totals = TOTAL_RE.exec(markdown);
	return {
		sections,
		stages,
		declared: totals ? { stages: Number(totals[1]), tests: Number(totals[2]) } : null
	};
}

const MATCHER_KINDS = ['contains', 'regex', 'not_contains', 'lines_unordered', 'lines_ordered_subset'];

/** Summarize one `expect:` entry into {label, kind, value} rows the site can render. */
export function summarizeExpect(expect) {
	const rows = [];
	if (!expect || typeof expect !== 'object') return rows;
	for (const [label, value] of Object.entries(expect)) {
		if (value === null || value === undefined) continue;
		if (label === 'exit_code') {
			rows.push({ label: 'exit code', kind: 'exact', value: String(value) });
		} else if (label === 'files' && typeof value === 'object') {
			for (const [file, contents] of Object.entries(value)) {
				rows.push({ label: `file ${file}`, kind: 'exact', value: String(contents) });
			}
		} else if (label === 'file_absent' && Array.isArray(value)) {
			rows.push({ label: 'files absent', kind: 'absent', value: value.map(String).join(', ') });
		} else if (typeof value === 'string') {
			rows.push({ label, kind: 'exact', value });
		} else if (typeof value === 'object') {
			for (const kind of MATCHER_KINDS) {
				if (value[kind] === undefined) continue;
				const v = Array.isArray(value[kind]) ? value[kind].map(String).join('\n') : String(value[kind]);
				rows.push({ label, kind, value: v });
			}
		}
	}
	return rows;
}

/** Turn one parsed stage YAML document into the catalog's test list. */
export function testsFromYaml(doc) {
	if (!doc || !Array.isArray(doc.tests)) return [];
	return doc.tests.map((t) => {
		const tags = Array.isArray(t.tags) ? t.tags.map(String) : [];
		const out = { name: String(t.name ?? 'unnamed') };
		if (tags.includes('ext')) out.ext = true;
		if (tags.length) out.tags = tags;
		if (Array.isArray(t.skip_on) && t.skip_on.length) out.skipOn = t.skip_on.map(String);
		if (t.mode === 'pty') out.mode = 'pty';
		if (t.reason) out.reason = String(t.reason);
		if (Array.isArray(t.input) && t.input.length) out.input = t.input.map(String);
		if (Array.isArray(t.keys) && t.keys.length) out.keys = t.keys.map(String);
		if (Array.isArray(t.steps) && t.steps.length) {
			out.steps = t.steps.map((s) => {
				if (s && typeof s === 'object') {
					const [k, v] = Object.entries(s)[0] ?? ['?', ''];
					return { op: String(k), value: String(v) };
				}
				return { op: 'send', value: String(s) };
			});
		}
		const expect = summarizeExpect(t.expect);
		if (expect.length) out.expect = expect;
		return out;
	});
}

/**
 * Merge the PLAN.md stages with the per-stage YAML suites into a Catalog.
 * `yamlDocs` maps a stage file name (`01_prompt.yaml`) to its parsed document.
 */
export function buildCatalog({ track, plan, yamlDocs, generatedAt, pending = false, source }) {
	const stages = plan.stages.map((s) => {
		const doc = yamlDocs[s.file];
		const tests = testsFromYaml(doc);
		return {
			number: s.number,
			slug: s.slug,
			name: doc && doc.name ? String(doc.name) : s.name,
			ext: s.ext,
			section: s.section,
			file: s.file,
			planDone: s.planDone,
			hints: s.hints,
			tests
		};
	});
	const catalog = {
		track,
		generatedAt,
		sections: plan.sections.map((s) => ({ id: s.id, title: s.title, stages: s.stages })),
		stages,
		totals: {
			stages: stages.length,
			tests: stages.reduce((n, s) => n + s.tests.length, 0),
			ext: stages.filter((s) => s.ext).length
		}
	};
	if (pending) catalog.pending = true;
	if (source) catalog.source = source;
	return catalog;
}

/** Count mismatches between PLAN.md's "(N tests)" and the YAML suites. */
export function auditCounts(plan, yamlDocs) {
	const problems = [];
	for (const s of plan.stages) {
		const doc = yamlDocs[s.file];
		if (!doc) {
			problems.push(`stage ${s.number}: missing ${s.file}`);
			continue;
		}
		const actual = testsFromYaml(doc).length;
		if (actual !== s.plannedTests) {
			problems.push(`stage ${s.number}: PLAN.md says ${s.plannedTests} tests, ${s.file} has ${actual}`);
		}
	}
	return problems;
}
