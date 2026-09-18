/**
 * Parse `../kafkatest/PLAN.md` into the stage list the site needs.
 *
 * kafkatest is written by several agents at once, so `catalog.json` legitimately has
 * gaps while stages are being merged. PLAN.md always lists all 45, with the ones that
 * have no tests yet marked `(planned)`. The sync script merges those in so the site
 * never links to a stage it cannot prerender.
 *
 * Stage lines look like one of:
 *   - [ ] **Stage 01** — Bind to the broker port (`src/stages/s01_bind.rs`, 6 tests)
 *   - [ ] **Stage 15** — Response partition limit **[ext]** (planned)
 * Hints are the indented `- ` bullets underneath.
 */

const SECTION_RE = /^##\s+([A-Za-z])\.\s+(.+?)\s*$/;
const STAGE_RE = /^- \[([ xX])\]\s+\*\*Stage\s+0*(\d+)\*\*\s*[—–-]\s*(.+?)\s*$/;
const HINT_RE = /^\s{2,}-\s+(.+?)\s*$/;
const FILE_RE = /\(`([^`]+)`(?:\s*,\s*(\d+)\s*tests?)?\)\s*$/;

/** `src/stages/s15_cursor_pagination.rs` -> `cursor-pagination`. */
export function slugFromSource(file) {
	return String(file)
		.replace(/^.*\//, '')
		.replace(/\.[a-z]+$/i, '')
		.replace(/^s?\d+[_-]/, '')
		.replace(/_/g, '-');
}

export function slugifyName(name, number) {
	const s = String(name ?? '')
		.toLowerCase()
		.replace(/\([^)]*\)/g, ' ')
		.replace(/[^a-z0-9]+/g, '-')
		.replace(/^-|-$/g, '')
		.slice(0, 48);
	return s || `stage-${number}`;
}

export function parseKafkaPlan(markdown) {
	const sections = [];
	const stages = [];
	let section = null;
	let current = null;

	for (const raw of String(markdown ?? '').split('\n')) {
		const line = raw.replace(/\s+$/, '');

		const sec = SECTION_RE.exec(line);
		if (sec) {
			section = { id: sec[1].toUpperCase(), title: sec[2].trim(), stages: [] };
			sections.push(section);
			current = null;
			continue;
		}

		const st = STAGE_RE.exec(line);
		if (st) {
			const number = Number(st[2]);
			let rest = st[3].trim();

			let planned = false;
			if (/\(planned\)\s*$/i.test(rest)) {
				planned = true;
				rest = rest.replace(/\(planned\)\s*$/i, '').trim();
			}

			let file = '';
			let plannedTests = null;
			const fileMatch = FILE_RE.exec(rest);
			if (fileMatch) {
				file = fileMatch[1];
				plannedTests = fileMatch[2] === undefined ? null : Number(fileMatch[2]);
				rest = rest.slice(0, fileMatch.index).trim();
			}

			let ext = false;
			if (/\*\*\[ext\]\*\*/.test(rest)) {
				ext = true;
				rest = rest.replace(/\s*\*\*\[ext\]\*\*\s*/g, ' ').trim();
			}

			current = {
				number,
				slug: file ? slugFromSource(file) : slugifyName(rest, number),
				name: rest,
				ext,
				planned,
				file,
				plannedTests,
				planDone: st[1].toLowerCase() === 'x',
				section: section ? section.id : '?',
				hints: []
			};
			stages.push(current);
			if (section) section.stages.push(number);
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

	stages.sort((a, b) => a.number - b.number);
	for (const s of sections) s.stages.sort((a, b) => a - b);
	return { sections, stages };
}
