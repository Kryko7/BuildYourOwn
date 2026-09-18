/**
 * "What to expect" for the Kafka track (PLAN.md §4.2).
 *
 * kafkatest generates the examples at catalog time and writes them into `catalog.json` in
 * snake_case, exactly as the Rust structs are named:
 *
 * ```jsonc
 * {
 *   "title": "ApiVersions v4",
 *   "request": "ApiVersions v4, client_id \"kafka-cli\"",
 *   "request_hex": "00 00 00 23 …",
 *   "response": "error_code 0, 3 api keys",
 *   "response_hex": "…",
 *   "note": "the response header for ApiVersions is v0 — no tagged fields",
 *   "request_fields":  [{ "offset": 0, "length": 4, "field": "size", "value": "35" }],
 *   "response_fields": [ … ]
 * }
 * ```
 *
 * This reader is deliberately forgiving, because the field is produced by a crate that is
 * still moving: a missing `examples` array means "no examples", an unknown key is ignored,
 * an empty hex string means the side is prose rather than a frame, and a field whose range
 * falls outside the bytes is clamped or dropped instead of breaking the page.
 */
import type {
	KafkaExample,
	KafkaExampleEnv,
	KafkaExampleField,
	KafkaExampleKind,
	KafkaExampleSide,
	KafkaExampleTopic,
	StageSpec
} from '../types';

const HEX_RE = /^[0-9a-fA-F]{2}$/;

/** Accepts `"00 01 02"`, `"000102"`, `"0x00,0x01"` and newline-separated dumps. */
export function readHex(input: unknown): Uint8Array | null {
	if (input instanceof Uint8Array) return input;
	if (Array.isArray(input)) {
		const out = input.map((n) => Number(n)).filter((n) => Number.isInteger(n) && n >= 0 && n <= 255);
		return out.length === input.length ? Uint8Array.from(out) : null;
	}
	if (typeof input !== 'string') return null;
	const cleaned = input.replace(/0x/gi, '').replace(/[\s,:_|]+/g, '');
	if (cleaned.length === 0) return null;
	if (cleaned.length % 2 !== 0) return null;
	const out = new Uint8Array(cleaned.length / 2);
	for (let i = 0; i < out.length; i++) {
		const pair = cleaned.slice(i * 2, i * 2 + 2);
		if (!HEX_RE.test(pair)) return null;
		out[i] = parseInt(pair, 16);
	}
	return out;
}

function text(value: unknown): string {
	if (typeof value === 'string') return value;
	if (typeof value === 'number' || typeof value === 'boolean') return String(value);
	return '';
}

function normalizeFields(raw: unknown, byteLength: number): KafkaExampleField[] {
	if (!Array.isArray(raw)) return [];
	const out: KafkaExampleField[] = [];
	for (const entry of raw) {
		if (!entry || typeof entry !== 'object') continue;
		const o = entry as Record<string, unknown>;
		const offset = Number(o.offset ?? o.start ?? NaN);
		const rawLength = Number(o.length ?? o.len ?? NaN);
		const name = text(o.field ?? o.name);
		if (!name) continue;
		if (!Number.isFinite(offset) || offset < 0) continue;
		const length = Number.isFinite(rawLength) && rawLength > 0 ? Math.floor(rawLength) : 0;
		// A frame that lost bytes in transit must not paint highlights past its end.
		const start = Math.min(Math.floor(offset), byteLength);
		const end = Math.min(start + length, byteLength);
		const field: KafkaExampleField = {
			offset: start,
			length: Math.max(0, end - start),
			name,
			value: text(o.value)
		};
		if (o.varies === true) field.varies = true;
		out.push(field);
	}
	return out.sort((a, b) => a.offset - b.offset || a.length - b.length);
}

const KINDS: KafkaExampleKind[] = ['wire', 'text', 'closed', 'silence'];

function normalizeKind(raw: unknown, hasBytes: boolean): KafkaExampleKind {
	if (typeof raw === 'string' && (KINDS as string[]).includes(raw)) return raw as KafkaExampleKind;
	// No `kind` at all (an older catalog): infer it from whether there are bytes to show.
	if (raw === undefined || raw === null || raw === '') return hasBytes ? 'wire' : 'text';
	return 'other';
}

/**
 * The fixture state the exchange was captured against. Shaped as
 * `{ topics: { alias: { name, id, partitions } }, group }`, and every part is optional.
 */
function normalizeEnv(raw: unknown): KafkaExampleEnv | null {
	if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null;
	const o = raw as Record<string, unknown>;
	const topics: KafkaExampleTopic[] = [];
	if (o.topics && typeof o.topics === 'object' && !Array.isArray(o.topics)) {
		for (const [key, value] of Object.entries(o.topics as Record<string, unknown>)) {
			if (!value || typeof value !== 'object') continue;
			const t = value as Record<string, unknown>;
			const name = text(t.name);
			if (!name) continue;
			const partitions = Number(t.partitions);
			topics.push({
				key,
				name,
				id: text(t.id) || null,
				partitions: Number.isFinite(partitions) && partitions > 0 ? partitions : null
			});
		}
	}
	const group = text(o.group) || null;
	if (topics.length === 0 && group === null) return null;
	return { topics, group };
}

function normalizeSide(summary: unknown, hex: unknown, fields: unknown): KafkaExampleSide {
	const bytes = readHex(hex);
	return {
		summary: text(summary),
		bytes,
		fields: normalizeFields(fields, bytes?.length ?? 0)
	};
}

/** One catalog example → the shape the renderer wants, or `null` when there is nothing in it. */
export function normalizeKafkaExample(raw: unknown, index = 0): KafkaExample | null {
	if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null;
	const o = raw as Record<string, unknown>;
	const request = normalizeSide(o.request, o.request_hex ?? o.requestHex, o.request_fields ?? o.requestFields);
	const response = normalizeSide(
		o.response,
		o.response_hex ?? o.responseHex,
		o.response_fields ?? o.responseFields
	);
	const hasAnything =
		request.summary || response.summary || request.bytes?.length || response.bytes?.length;
	if (!hasAnything) return null;
	const note = text(o.note);
	return {
		title: text(o.title) || `Example ${index + 1}`,
		kind: normalizeKind(o.kind, Boolean(request.bytes?.length || response.bytes?.length)),
		note: note === '' ? null : note,
		env: normalizeEnv(o.env),
		request,
		response
	};
}

/** A list of raw catalog entries → the examples the renderer can draw. */
export function normalizeKafkaExamples(raw: unknown): KafkaExample[] {
	if (!Array.isArray(raw)) return [];
	const out: KafkaExample[] = [];
	raw.forEach((entry, i) => {
		const example = normalizeKafkaExample(entry, i);
		if (example) out.push(example);
	});
	return out;
}

/**
 * The examples carried *inline* on a stage. The generated catalog does not carry them (see
 * `loadKafkaExamples`), so this is the path for fixtures and tests — and for a catalog
 * served by a newer `byo` that still inlines them.
 */
export function kafkaExamples(stage: StageSpec | null | undefined): KafkaExample[] {
	return normalizeKafkaExamples(stage?.examples);
}

/**
 * One stage's examples, fetched on demand.
 *
 * `npm run sync` writes a file per stage, and `import.meta.glob` turns each into its own
 * chunk — so opening one Kafka stage downloads that stage's frames and nothing else,
 * instead of every page paying for all 91 of them. A stage with no file (a planned stage,
 * or a tester that has not generated examples) resolves to `[]`, not an error.
 */
const exampleFiles = import.meta.glob<{ default: unknown }>('../data/examples/kafka/*.json');

export async function loadKafkaExamples(
	stage: StageSpec | null | undefined
): Promise<KafkaExample[]> {
	if (!stage) return [];
	const inline = kafkaExamples(stage);
	if (inline.length) return inline;
	return normalizeKafkaExamples(await loadKafkaExampleData(stage.number));
}

/** The raw catalog entries for one stage — what a route's `load` hands to the component. */
export async function loadKafkaExampleData(stageNumber: number): Promise<unknown[] | null> {
	const loader = exampleFiles[`../data/examples/kafka/${stageNumber}.json`];
	if (!loader) return null;
	try {
		const mod = await loader();
		const list = mod?.default ?? mod;
		return Array.isArray(list) ? list : null;
	} catch {
		// A chunk that will not load is not worth breaking the stage page over.
		return null;
	}
}

/** How many examples a Kafka stage has, without loading any of them. */
export function kafkaExampleCount(stage: StageSpec | null | undefined): number {
	if (!stage) return 0;
	if (Array.isArray(stage.examples) && stage.examples.length) return kafkaExamples(stage).length;
	return typeof stage.exampleCount === 'number' ? stage.exampleCount : 0;
}

/** `true` when a side carries bytes worth drawing a hex dump for. */
export function hasBytes(side: KafkaExampleSide): boolean {
	return Boolean(side.bytes && side.bytes.length > 0);
}
