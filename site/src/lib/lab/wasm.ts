/**
 * A WebAssembly binary decoder for the /lab module inspector.
 *
 * Enough of the core binary format to show a learner what they are about to implement:
 * the preamble, the section tree, every LEB128 value decoded next to its bytes, the type
 * and export tables, and each function body disassembled to instruction names with its
 * immediates. It decodes; it does not execute.
 *
 * Nothing here is a dependency — the sample modules at the bottom are encoded by this
 * file too, so the decoder is always exercised against bytes we can also print.
 */
import type { Decoded, Field } from './field';

/* --------------------------------- LEB128 --------------------------------- */

export interface Leb {
	value: number;
	/** How many bytes it took — the whole point of the encoding. */
	size: number;
}

/** Unsigned LEB128. Throws on a run-on encoding, which is what a validator must do. */
export function readUleb(bytes: Uint8Array, offset: number): Leb {
	let value = 0;
	let shift = 0;
	let size = 0;
	for (;;) {
		if (offset + size >= bytes.length) throw new Error(`LEB128 at ${offset} runs past the end`);
		const b = bytes[offset + size];
		value += (b & 0x7f) * 2 ** shift;
		size++;
		if ((b & 0x80) === 0) break;
		shift += 7;
		if (shift > 35) throw new Error(`LEB128 at ${offset} is longer than a u32 can be`);
	}
	return { value, size };
}

/** Signed LEB128 (sleb), sign-extended from wherever it stopped. */
export function readSleb(bytes: Uint8Array, offset: number): Leb {
	let value = 0;
	let shift = 0;
	let size = 0;
	let byte = 0;
	for (;;) {
		if (offset + size >= bytes.length) throw new Error(`signed LEB128 at ${offset} runs past the end`);
		byte = bytes[offset + size];
		value += (byte & 0x7f) * 2 ** shift;
		shift += 7;
		size++;
		if ((byte & 0x80) === 0) break;
		if (shift > 70) throw new Error(`signed LEB128 at ${offset} is too long`);
	}
	if (byte & 0x40 && shift < 64) value -= 2 ** shift;
	return { value, size };
}

export function encodeUleb(value: number): number[] {
	const out: number[] = [];
	let v = value >>> 0;
	do {
		let b = v & 0x7f;
		v = Math.floor(v / 128);
		if (v !== 0) b |= 0x80;
		out.push(b);
	} while (v !== 0);
	return out;
}

export function encodeSleb(value: number): number[] {
	const out: number[] = [];
	let more = true;
	let v = value;
	while (more) {
		let b = v & 0x7f;
		v >>= 7;
		if ((v === 0 && (b & 0x40) === 0) || (v === -1 && (b & 0x40) !== 0)) more = false;
		else b |= 0x80;
		out.push(b);
	}
	return out;
}

/* -------------------------------- constants ------------------------------- */

export const SECTION_NAMES: Record<number, string> = {
	0: 'custom',
	1: 'type',
	2: 'import',
	3: 'function',
	4: 'table',
	5: 'memory',
	6: 'global',
	7: 'export',
	8: 'start',
	9: 'element',
	10: 'code',
	11: 'data',
	12: 'data count'
};

const SECTION_NOTES: Record<number, string> = {
	0: 'Custom sections carry names and debug info; a runtime may skip them, but it still has to frame them correctly.',
	1: 'The type section is the table every function, block and call_indirect refers to by index.',
	2: 'Imports come first in the index space: an imported function is function 0, and your own functions start after them.',
	3: 'The function section is only type indices — the bodies live in the code section, in the same order.',
	5: 'A memory is a limits pair; wasm 1.0 allows exactly one.',
	7: 'Exports are how the host reaches in: a name, a kind byte and an index.',
	10: 'One body per function: locals declared in runs, then the instructions, then 0x0B.',
	11: 'Data segments seed linear memory before `_start` runs.'
};

export const VALTYPES: Record<number, string> = {
	0x7f: 'i32',
	0x7e: 'i64',
	0x7d: 'f32',
	0x7c: 'f64',
	0x7b: 'v128',
	0x70: 'funcref',
	0x6f: 'externref'
};

const EXPORT_KINDS: Record<number, string> = { 0: 'func', 1: 'table', 2: 'memory', 3: 'global' };

/** Immediate shapes, so the disassembler can walk a body without understanding it. */
type Imm =
	| 'none'
	| 'u32'
	| 'u32u32'
	| 'i32'
	| 'i64'
	| 'f32'
	| 'f64'
	| 'memarg'
	| 'blocktype'
	| 'brtable'
	| 'byte'
	| 'reftype'
	| 'selectt';

const NUMERIC_OPS =
	'i32.eqz i32.eq i32.ne i32.lt_s i32.lt_u i32.gt_s i32.gt_u i32.le_s i32.le_u i32.ge_s i32.ge_u ' +
	'i64.eqz i64.eq i64.ne i64.lt_s i64.lt_u i64.gt_s i64.gt_u i64.le_s i64.le_u i64.ge_s i64.ge_u ' +
	'f32.eq f32.ne f32.lt f32.gt f32.le f32.ge f64.eq f64.ne f64.lt f64.gt f64.le f64.ge ' +
	'i32.clz i32.ctz i32.popcnt i32.add i32.sub i32.mul i32.div_s i32.div_u i32.rem_s i32.rem_u ' +
	'i32.and i32.or i32.xor i32.shl i32.shr_s i32.shr_u i32.rotl i32.rotr ' +
	'i64.clz i64.ctz i64.popcnt i64.add i64.sub i64.mul i64.div_s i64.div_u i64.rem_s i64.rem_u ' +
	'i64.and i64.or i64.xor i64.shl i64.shr_s i64.shr_u i64.rotl i64.rotr ' +
	'f32.abs f32.neg f32.ceil f32.floor f32.trunc f32.nearest f32.sqrt f32.add f32.sub f32.mul f32.div f32.min f32.max f32.copysign ' +
	'f64.abs f64.neg f64.ceil f64.floor f64.trunc f64.nearest f64.sqrt f64.add f64.sub f64.mul f64.div f64.min f64.max f64.copysign ' +
	'i32.wrap_i64 i32.trunc_f32_s i32.trunc_f32_u i32.trunc_f64_s i32.trunc_f64_u ' +
	'i64.extend_i32_s i64.extend_i32_u i64.trunc_f32_s i64.trunc_f32_u i64.trunc_f64_s i64.trunc_f64_u ' +
	'f32.convert_i32_s f32.convert_i32_u f32.convert_i64_s f32.convert_i64_u f32.demote_f64 ' +
	'f64.convert_i32_s f64.convert_i32_u f64.convert_i64_s f64.convert_i64_u f64.promote_f32 ' +
	'i32.reinterpret_f32 i64.reinterpret_f64 f32.reinterpret_i32 f64.reinterpret_i64 ' +
	'i32.extend8_s i32.extend16_s i64.extend8_s i64.extend16_s i64.extend32_s';

const MEMORY_OPS =
	'i32.load i64.load f32.load f64.load i32.load8_s i32.load8_u i32.load16_s i32.load16_u ' +
	'i64.load8_s i64.load8_u i64.load16_s i64.load16_u i64.load32_s i64.load32_u ' +
	'i32.store i64.store f32.store f64.store i32.store8 i32.store16 i64.store8 i64.store16 i64.store32';

export const OPCODES: Record<number, { name: string; imm: Imm }> = (() => {
	const table: Record<number, { name: string; imm: Imm }> = {
		0x00: { name: 'unreachable', imm: 'none' },
		0x01: { name: 'nop', imm: 'none' },
		0x02: { name: 'block', imm: 'blocktype' },
		0x03: { name: 'loop', imm: 'blocktype' },
		0x04: { name: 'if', imm: 'blocktype' },
		0x05: { name: 'else', imm: 'none' },
		0x0b: { name: 'end', imm: 'none' },
		0x0c: { name: 'br', imm: 'u32' },
		0x0d: { name: 'br_if', imm: 'u32' },
		0x0e: { name: 'br_table', imm: 'brtable' },
		0x0f: { name: 'return', imm: 'none' },
		0x10: { name: 'call', imm: 'u32' },
		0x11: { name: 'call_indirect', imm: 'u32u32' },
		0x1a: { name: 'drop', imm: 'none' },
		0x1b: { name: 'select', imm: 'none' },
		0x1c: { name: 'select', imm: 'selectt' },
		0x20: { name: 'local.get', imm: 'u32' },
		0x21: { name: 'local.set', imm: 'u32' },
		0x22: { name: 'local.tee', imm: 'u32' },
		0x23: { name: 'global.get', imm: 'u32' },
		0x24: { name: 'global.set', imm: 'u32' },
		0x25: { name: 'table.get', imm: 'u32' },
		0x26: { name: 'table.set', imm: 'u32' },
		0x3f: { name: 'memory.size', imm: 'byte' },
		0x40: { name: 'memory.grow', imm: 'byte' },
		0x41: { name: 'i32.const', imm: 'i32' },
		0x42: { name: 'i64.const', imm: 'i64' },
		0x43: { name: 'f32.const', imm: 'f32' },
		0x44: { name: 'f64.const', imm: 'f64' },
		0xd0: { name: 'ref.null', imm: 'reftype' },
		0xd1: { name: 'ref.is_null', imm: 'none' },
		0xd2: { name: 'ref.func', imm: 'u32' }
	};
	MEMORY_OPS.split(' ').forEach((name, i) => {
		table[0x28 + i] = { name, imm: 'memarg' };
	});
	NUMERIC_OPS.split(' ').forEach((name, i) => {
		table[0x45 + i] = { name, imm: 'none' };
	});
	return table;
})();

/** The 0xFC prefix space: saturating truncation and the bulk-memory operations. */
const FC_OPS: Record<number, { name: string; imm: Imm }> = {
	0: { name: 'i32.trunc_sat_f32_s', imm: 'none' },
	1: { name: 'i32.trunc_sat_f32_u', imm: 'none' },
	2: { name: 'i32.trunc_sat_f64_s', imm: 'none' },
	3: { name: 'i32.trunc_sat_f64_u', imm: 'none' },
	4: { name: 'i64.trunc_sat_f32_s', imm: 'none' },
	5: { name: 'i64.trunc_sat_f32_u', imm: 'none' },
	6: { name: 'i64.trunc_sat_f64_s', imm: 'none' },
	7: { name: 'i64.trunc_sat_f64_u', imm: 'none' },
	8: { name: 'memory.init', imm: 'u32u32' },
	9: { name: 'data.drop', imm: 'u32' },
	10: { name: 'memory.copy', imm: 'u32u32' },
	11: { name: 'memory.fill', imm: 'byte' },
	12: { name: 'table.init', imm: 'u32u32' },
	13: { name: 'elem.drop', imm: 'u32' },
	14: { name: 'table.copy', imm: 'u32u32' },
	15: { name: 'table.grow', imm: 'u32' },
	16: { name: 'table.size', imm: 'u32' },
	17: { name: 'table.fill', imm: 'u32' }
};

/* --------------------------------- decoding -------------------------------- */

export interface WasmInstruction {
	offset: number;
	end: number;
	name: string;
	/** The immediates, already formatted (`0`, `{align 2, offset 0}`). */
	args: string;
	/** Nesting, so `block`/`loop`/`if` bodies are indented in the listing. */
	depth: number;
}

export interface WasmFunctionBody {
	index: number;
	/** The name the module exports it under, when it exports it at all. */
	exportName: string | null;
	/** `(i32, i32) -> i32`, from the type section. */
	signature: string;
	locals: string[];
	instructions: WasmInstruction[];
	start: number;
	end: number;
	error?: string;
}

export interface WasmSection {
	id: number;
	name: string;
	/** Offset of the section id byte. */
	start: number;
	/** Offset of the first payload byte. */
	bodyStart: number;
	end: number;
	size: number;
	/** "3 types", "2 exports" — whatever the section counted. */
	summary: string;
}

export interface WasmModule extends Decoded {
	valid: boolean;
	version: number | null;
	sections: WasmSection[];
	types: string[];
	imports: { module: string; name: string; kind: string }[];
	exports: { name: string; kind: string; index: number }[];
	functions: WasmFunctionBody[];
	/** Data segments, as they will be laid into linear memory. */
	data: { offset: string; bytes: Uint8Array }[];
	memory: string | null;
}

class Cursor {
	offset = 0;
	depth = 0;
	readonly fields: Field[] = [];

	constructor(readonly bytes: Uint8Array) {}

	field(name: string, type: string, start: number, end: number, value: string, note?: string) {
		const f: Field = { name, type, start, end, value, depth: this.depth };
		if (note) f.note = note;
		this.fields.push(f);
	}

	byte(name: string, format: (v: number) => string = (v) => String(v), note?: string): number {
		if (this.offset >= this.bytes.length) throw new Error(`${name}: out of bytes`);
		const start = this.offset;
		const v = this.bytes[this.offset++];
		this.field(name, 'u8', start, this.offset, format(v), note);
		return v;
	}

	uleb(name: string, note?: string): number {
		const start = this.offset;
		const { value, size } = readUleb(this.bytes, start);
		this.offset += size;
		this.field(
			name,
			`LEB128 u32 (${size} byte${size === 1 ? '' : 's'})`,
			start,
			this.offset,
			String(value),
			note
		);
		return value;
	}

	name(label: string): string {
		const len = this.uleb(`${label}.length`);
		const start = this.offset;
		if (start + len > this.bytes.length) throw new Error(`${label}: name runs past the end`);
		const text = new TextDecoder().decode(this.bytes.subarray(start, start + len));
		this.offset += len;
		this.field(label, `utf8[${len}]`, start, this.offset, JSON.stringify(text));
		return text;
	}
}

function functionType(c: Cursor, index: number): string {
	const form = c.byte(`type[${index}].form`, (v) => (v === 0x60 ? '0x60 func' : `0x${v.toString(16)}`),
		'Every entry in the type section is a function type, and it starts with 0x60.');
	if (form !== 0x60) throw new Error(`type ${index}: expected 0x60, got 0x${form.toString(16)}`);
	const params: string[] = [];
	const paramCount = c.uleb(`type[${index}].params`);
	for (let i = 0; i < paramCount; i++) {
		const v = c.byte(`type[${index}].param[${i}]`, (b) => VALTYPES[b] ?? `0x${b.toString(16)}`);
		params.push(VALTYPES[v] ?? `0x${v.toString(16)}`);
	}
	const results: string[] = [];
	const resultCount = c.uleb(`type[${index}].results`);
	for (let i = 0; i < resultCount; i++) {
		const v = c.byte(`type[${index}].result[${i}]`, (b) => VALTYPES[b] ?? `0x${b.toString(16)}`);
		results.push(VALTYPES[v] ?? `0x${v.toString(16)}`);
	}
	return `(${params.join(', ')}) -> ${results.length ? results.join(', ') : '()'}`;
}

function limits(c: Cursor, label: string): string {
	const flags = c.byte(`${label}.flags`, (v) => (v === 0 ? 'min only' : 'min and max'));
	const min = c.uleb(`${label}.min`);
	if (flags === 0) return `min ${min} page${min === 1 ? '' : 's'}`;
	const max = c.uleb(`${label}.max`);
	return `min ${min}, max ${max} pages`;
}

/** Disassemble one expression up to its matching `end`, or to `limit`. */
export function disassemble(bytes: Uint8Array, start: number, limit: number): WasmInstruction[] {
	const out: WasmInstruction[] = [];
	let offset = start;
	let depth = 0;
	while (offset < limit) {
		const at = offset;
		const op = bytes[offset++];
		let entry = OPCODES[op];
		let name = entry?.name ?? `0x${op.toString(16).padStart(2, '0')}`;
		let imm: Imm = entry?.imm ?? 'none';
		if (op === 0xfc) {
			const sub = readUleb(bytes, offset);
			offset += sub.size;
			entry = FC_OPS[sub.value];
			name = entry?.name ?? `0xfc ${sub.value}`;
			imm = entry?.imm ?? 'none';
		}
		const args: string[] = [];
		switch (imm) {
			case 'u32': {
				const v = readUleb(bytes, offset);
				offset += v.size;
				args.push(String(v.value));
				break;
			}
			case 'u32u32': {
				const a = readUleb(bytes, offset);
				offset += a.size;
				const b = readUleb(bytes, offset);
				offset += b.size;
				args.push(String(a.value), String(b.value));
				break;
			}
			case 'memarg': {
				const align = readUleb(bytes, offset);
				offset += align.size;
				const off = readUleb(bytes, offset);
				offset += off.size;
				args.push(`align=2^${align.value}`, `offset=${off.value}`);
				break;
			}
			case 'i32':
			case 'i64': {
				const v = readSleb(bytes, offset);
				offset += v.size;
				args.push(String(v.value));
				break;
			}
			case 'f32': {
				const view = new DataView(bytes.buffer, bytes.byteOffset + offset, 4);
				args.push(String(view.getFloat32(0, true)));
				offset += 4;
				break;
			}
			case 'f64': {
				const view = new DataView(bytes.buffer, bytes.byteOffset + offset, 8);
				args.push(String(view.getFloat64(0, true)));
				offset += 8;
				break;
			}
			case 'blocktype': {
				const b = bytes[offset];
				if (b === 0x40) {
					args.push('(empty)');
					offset++;
				} else if (VALTYPES[b]) {
					args.push(`-> ${VALTYPES[b]}`);
					offset++;
				} else {
					const v = readSleb(bytes, offset);
					offset += v.size;
					args.push(`type ${v.value}`);
				}
				break;
			}
			case 'brtable': {
				const count = readUleb(bytes, offset);
				offset += count.size;
				const targets: number[] = [];
				for (let i = 0; i < count.value; i++) {
					const t = readUleb(bytes, offset);
					offset += t.size;
					targets.push(t.value);
				}
				const def = readUleb(bytes, offset);
				offset += def.size;
				args.push(`[${targets.join(' ')}]`, `default ${def.value}`);
				break;
			}
			case 'byte': {
				offset++;
				break;
			}
			case 'reftype': {
				const b = bytes[offset++];
				args.push(VALTYPES[b] ?? `0x${b.toString(16)}`);
				break;
			}
			case 'selectt': {
				const count = readUleb(bytes, offset);
				offset += count.size;
				for (let i = 0; i < count.value; i++) {
					const b = bytes[offset++];
					args.push(VALTYPES[b] ?? `0x${b.toString(16)}`);
				}
				break;
			}
			default:
				break;
		}
		// `end` closes whatever is open; when it closes the expression itself (depth goes
		// below zero) the expression is over, even if there are more bytes after it — which
		// is exactly the case for a data segment's offset expression.
		let render = depth;
		if (name === 'end') {
			depth--;
			render = Math.max(0, depth);
		} else if (name === 'else') {
			render = Math.max(0, depth - 1);
		}
		out.push({ offset: at, end: offset, name, args: args.join(' '), depth: render });
		if (name === 'block' || name === 'loop' || name === 'if') depth++;
		if (name === 'end' && depth < 0) break;
	}
	return out;
}

/**
 * Decode a module. Never throws: a truncated or malformed module comes back with
 * everything that was readable plus the error, because half a decode is exactly what a
 * learner staring at a failure needs to see.
 */
export function decodeModule(bytes: Uint8Array): WasmModule {
	const module: WasmModule = {
		fields: [],
		valid: false,
		version: null,
		sections: [],
		types: [],
		imports: [],
		exports: [],
		functions: [],
		data: [],
		memory: null
	};
	const c = new Cursor(bytes);
	try {
		if (bytes.length < 8) throw new Error('a module is at least 8 bytes: the magic and the version');
		const magic = bytes.subarray(0, 4);
		const magicOk = magic[0] === 0x00 && magic[1] === 0x61 && magic[2] === 0x73 && magic[3] === 0x6d;
		c.field('magic', 'u8[4]', 0, 4, '00 61 73 6d  "\\0asm"',
			'Four bytes that say this is a wasm module at all. Anything else must be rejected before another byte is read.');
		c.offset = 4;
		if (!magicOk) throw new Error('bad magic: this is not a wasm module');
		const view = new DataView(bytes.buffer, bytes.byteOffset);
		const version = view.getUint32(4, true);
		c.field('version', 'u32 (little-endian)', 4, 8, String(version),
			'Version 1 — and it is little-endian, unlike every network protocol next door.');
		c.offset = 8;
		module.version = version;
		if (version !== 1) throw new Error(`unsupported version ${version}`);

		const typeSignatures: string[] = [];
		const funcTypeIndices: number[] = [];
		let importedFunctions = 0;

		while (c.offset < bytes.length) {
			const sectionStart = c.offset;
			const id = c.byte(
				'section.id',
				(v) => `${v} (${SECTION_NAMES[v] ?? 'unknown'})`,
				SECTION_NOTES[bytes[sectionStart]]
			);
			const size = c.uleb('section.size', 'The payload length, so a runtime can skip a section it does not care about.');
			const bodyStart = c.offset;
			const end = bodyStart + size;
			if (end > bytes.length) throw new Error(`section ${SECTION_NAMES[id] ?? id} claims ${size} bytes, only ${bytes.length - bodyStart} left`);
			c.depth = 1;
			let summary = `${size} bytes`;

			if (id === 1) {
				const count = c.uleb('type.count');
				for (let i = 0; i < count; i++) {
					const sig = functionType(c, i);
					typeSignatures.push(sig);
					module.types.push(sig);
				}
				summary = `${count} type${count === 1 ? '' : 's'}`;
			} else if (id === 2) {
				const count = c.uleb('import.count');
				for (let i = 0; i < count; i++) {
					const mod = c.name(`import[${i}].module`);
					const nm = c.name(`import[${i}].name`);
					const kind = c.byte(`import[${i}].kind`, (v) => `${v} (${EXPORT_KINDS[v] ?? '?'})`);
					if (kind === 0) {
						const typeIndex = c.uleb(`import[${i}].type`);
						funcTypeIndices.push(typeIndex);
						importedFunctions++;
					} else if (kind === 1) {
						c.byte(`import[${i}].reftype`, (v) => VALTYPES[v] ?? `0x${v.toString(16)}`);
						limits(c, `import[${i}].limits`);
					} else if (kind === 2) {
						limits(c, `import[${i}].limits`);
					} else {
						c.byte(`import[${i}].valtype`, (v) => VALTYPES[v] ?? `0x${v.toString(16)}`);
						c.byte(`import[${i}].mutable`, (v) => (v ? 'mutable' : 'const'));
					}
					module.imports.push({ module: mod, name: nm, kind: EXPORT_KINDS[kind] ?? '?' });
				}
				summary = `${count} import${count === 1 ? '' : 's'}`;
			} else if (id === 3) {
				const count = c.uleb('function.count');
				for (let i = 0; i < count; i++) funcTypeIndices.push(c.uleb(`function[${i}].type`));
				summary = `${count} function${count === 1 ? '' : 's'}`;
			} else if (id === 5) {
				const count = c.uleb('memory.count');
				for (let i = 0; i < count; i++) module.memory = limits(c, `memory[${i}]`);
				summary = module.memory ?? `${count} memories`;
			} else if (id === 7) {
				const count = c.uleb('export.count');
				for (let i = 0; i < count; i++) {
					const nm = c.name(`export[${i}].name`);
					const kind = c.byte(`export[${i}].kind`, (v) => `${v} (${EXPORT_KINDS[v] ?? '?'})`);
					const index = c.uleb(`export[${i}].index`);
					module.exports.push({ name: nm, kind: EXPORT_KINDS[kind] ?? '?', index });
				}
				summary = module.exports.map((e) => e.name).join(', ') || `${count} exports`;
			} else if (id === 8) {
				const index = c.uleb('start.function');
				summary = `start = function ${index}`;
			} else if (id === 10) {
				const count = c.uleb('code.count');
				for (let i = 0; i < count; i++) {
					const bodySize = c.uleb(`code[${i}].size`);
					const bodyStartAt = c.offset;
					const bodyEnd = bodyStartAt + bodySize;
					const localRuns = c.uleb(`code[${i}].local_runs`,
						'Locals are declared in runs of (count, type) — not one entry per local.');
					const locals: string[] = [];
					for (let r = 0; r < localRuns; r++) {
						const n = c.uleb(`code[${i}].local[${r}].count`);
						const t = c.byte(`code[${i}].local[${r}].type`, (v) => VALTYPES[v] ?? `0x${v.toString(16)}`);
						for (let k = 0; k < n; k++) locals.push(VALTYPES[t] ?? `0x${t.toString(16)}`);
					}
					const index = importedFunctions + i;
					const typeIndex = funcTypeIndices[index] ?? funcTypeIndices[i];
					let instructions: WasmInstruction[] = [];
					let error: string | undefined;
					try {
						instructions = disassemble(bytes, c.offset, bodyEnd);
					} catch (e) {
						error = e instanceof Error ? e.message : 'could not disassemble this body';
					}
					c.field(
						`code[${i}].body`,
						'expr',
						c.offset,
						bodyEnd,
						`${instructions.length} instruction${instructions.length === 1 ? '' : 's'}`,
						'The body ends with 0x0B — the same `end` byte that closes a block.'
					);
					module.functions.push({
						index,
						exportName: null,
						signature: typeSignatures[typeIndex] ?? '(unknown type)',
						locals,
						instructions,
						start: c.offset,
						end: bodyEnd,
						error
					});
					c.offset = bodyEnd;
				}
				summary = `${count} function bod${count === 1 ? 'y' : 'ies'}`;
			} else if (id === 11) {
				const count = c.uleb('data.count');
				for (let i = 0; i < count; i++) {
					const mode = c.uleb(`data[${i}].mode`, '0 = active into memory 0, 1 = passive, 2 = active with an explicit memory index.');
					let offsetText = 'passive';
					if (mode === 0 || mode === 2) {
						if (mode === 2) c.uleb(`data[${i}].memory`);
						const exprStart = c.offset;
						const instrs = disassemble(bytes, exprStart, end);
						const endInstr = instrs.find((x) => x.name === 'end');
						const exprEnd = endInstr ? endInstr.end : exprStart;
						offsetText = instrs
							.filter((x) => x.name !== 'end')
							.map((x) => `${x.name} ${x.args}`.trim())
							.join(' ');
						c.field(`data[${i}].offset`, 'expr', exprStart, exprEnd, offsetText || '(empty)');
						c.offset = exprEnd;
					}
					const len = c.uleb(`data[${i}].length`);
					const start = c.offset;
					const slice = bytes.subarray(start, start + len);
					c.offset += len;
					c.field(
						`data[${i}].bytes`,
						`u8[${len}]`,
						start,
						c.offset,
						JSON.stringify(new TextDecoder().decode(slice))
					);
					module.data.push({ offset: offsetText, bytes: slice });
				}
				summary = `${count} segment${count === 1 ? '' : 's'}`;
			} else if (id === 0) {
				const nameStart = c.offset;
				const nm = c.name('custom.name');
				const rest = end - c.offset;
				if (rest > 0) c.field('custom.payload', `u8[${rest}]`, c.offset, end, `${rest} bytes`);
				summary = `"${nm}" (${end - nameStart} bytes)`;
			}

			// Whatever a branch above did not describe byte by byte is still accounted for.
			if (c.offset < end) {
				c.field(`${SECTION_NAMES[id] ?? 'section'}.rest`, 'bytes', c.offset, end, `${end - c.offset} bytes not decoded here`);
			}
			c.offset = end;
			c.depth = 0;
			module.sections.push({
				id,
				name: SECTION_NAMES[id] ?? `unknown (${id})`,
				start: sectionStart,
				bodyStart,
				end,
				size,
				summary
			});
		}

		for (const e of module.exports) {
			if (e.kind !== 'func') continue;
			const fn = module.functions.find((f) => f.index === e.index);
			if (fn) fn.exportName = e.name;
		}
		module.valid = true;
	} catch (e) {
		module.error = e instanceof Error ? e.message : 'could not decode this module';
	}
	module.fields = c.fields;
	return module;
}

/* ------------------------------ sample modules ----------------------------- */

/** A minimal encoder, used only to build the samples the inspector ships with. */
class ModuleWriter {
	private readonly parts: number[] = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

	section(id: number, body: number[]): this {
		this.parts.push(id, ...encodeUleb(body.length), ...body);
		return this;
	}

	bytes(): Uint8Array {
		return Uint8Array.from(this.parts);
	}
}

function vec(items: number[][]): number[] {
	return [...encodeUleb(items.length), ...items.flat()];
}

function str(text: string): number[] {
	const bytes = [...new TextEncoder().encode(text)];
	return [...encodeUleb(bytes.length), ...bytes];
}

/** `(func (export "add") (param i32 i32) (result i32) (i32.add (local.get 0) (local.get 1)))` */
function addModule(): Uint8Array {
	const type = [0x60, ...vec([[0x7f], [0x7f]]), ...vec([[0x7f]])];
	const body = [0x00, 0x20, 0x00, 0x20, 0x01, 0x6a, 0x0b];
	return new ModuleWriter()
		.section(1, vec([type]))
		.section(3, vec([[0x00]]))
		.section(7, vec([[...str('add'), 0x00, 0x00]]))
		.section(10, vec([[...encodeUleb(body.length), ...body]]))
		.bytes();
}

/**
 * A WASI command: import `fd_write`, put "hello, garden\n" in memory, build the iovec at
 * runtime and write it to fd 1. This is the shape of every hello-world a real toolchain
 * emits, and the thing section G of the track has to run.
 */
function wasiHelloModule(): Uint8Array {
	const text = 'hello, garden\n';
	const typeFdWrite = [0x60, ...vec([[0x7f], [0x7f], [0x7f], [0x7f]]), ...vec([[0x7f]])];
	const typeStart = [0x60, ...vec([]), ...vec([])];
	const body = [
		0x00, // no locals
		// iov.base = 16 (where the text lives), stored at address 0
		0x41, 0x00, 0x41, 0x10, 0x36, 0x02, 0x00,
		// iov.len = text.length, stored at address 4
		0x41, 0x04, 0x41, ...encodeSleb(text.length), 0x36, 0x02, 0x00,
		// fd_write(fd = 1, iovs = 0, iovs_len = 1, nwritten = 8)
		0x41, 0x01, 0x41, 0x00, 0x41, 0x01, 0x41, 0x08, 0x10, 0x00,
		0x1a, // drop the errno
		0x0b
	];
	const data = [0x00, 0x41, 0x10, 0x0b, ...str(text)];
	return new ModuleWriter()
		.section(1, vec([typeFdWrite, typeStart]))
		.section(2, vec([[...str('wasi_snapshot_preview1'), ...str('fd_write'), 0x00, 0x00]]))
		.section(3, vec([[0x01]]))
		.section(5, vec([[0x00, 0x01]]))
		.section(7, vec([[...str('memory'), 0x02, 0x00], [...str('_start'), 0x00, 0x01]]))
		.section(10, vec([[...encodeUleb(body.length), ...body]]))
		.section(11, vec([data]))
		.bytes();
}

/** `(func (export "div") (param i32 i32) (result i32) i32.div_s)` — the trap demo. */
function divModule(): Uint8Array {
	const type = [0x60, ...vec([[0x7f], [0x7f]]), ...vec([[0x7f]])];
	const body = [0x00, 0x20, 0x00, 0x20, 0x01, 0x6d, 0x0b];
	return new ModuleWriter()
		.section(1, vec([type]))
		.section(3, vec([[0x00]]))
		.section(7, vec([[...str('div'), 0x00, 0x00]]))
		.section(10, vec([[...encodeUleb(body.length), ...body]]))
		.section(0, [...str('name'), 0x00])
		.bytes();
}

export interface WasmSample {
	id: string;
	label: string;
	blurb: string;
	/** What running it should print, so the inspector can show the expected output. */
	invoke: string;
	expected: string;
	bytes: Uint8Array;
}

export const wasmSamples: WasmSample[] = [
	{
		id: 'add',
		label: 'add.wasm',
		blurb:
			'The smallest module worth having: one type, one function, one export. Four sections and 41 bytes.',
		invoke: 'run --invoke add add.wasm 2 3',
		expected: '5',
		bytes: addModule()
	},
	{
		id: 'wasi-hello',
		label: 'hello.wasm (WASI)',
		blurb:
			'A WASI command: it imports fd_write, keeps its string in a data segment, builds the iovec in linear memory and writes it to fd 1.',
		invoke: 'run hello.wasm',
		expected: 'hello, garden',
		bytes: wasiHelloModule()
	},
	{
		id: 'div',
		label: 'div.wasm (traps)',
		blurb:
			'Signed division, which is where the canonical trap reasons live: "integer divide by zero" and "integer overflow" for INT_MIN / -1. It also carries an empty custom section.',
		invoke: 'run --invoke div div.wasm 1 0',
		expected: 'wasm trap: integer divide by zero',
		bytes: divModule()
	}
];

/** Accepts `"00 61 73 6d"`, `"0061736d"`, `0x`-prefixed and newline-separated dumps. */
export function parseWasmHex(input: string): Uint8Array {
	const cleaned = input.replace(/0x/gi, '').replace(/[\s,:_|]+/g, '');
	if (cleaned.length === 0) return new Uint8Array();
	if (cleaned.length % 2 !== 0) throw new Error('That is an odd number of hex digits.');
	if (/[^0-9a-f]/i.test(cleaned)) throw new Error('That has characters that are not hex digits.');
	const out = new Uint8Array(cleaned.length / 2);
	for (let i = 0; i < out.length; i++) out[i] = parseInt(cleaned.slice(i * 2, i * 2 + 2), 16);
	return out;
}
