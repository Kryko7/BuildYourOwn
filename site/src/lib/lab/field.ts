/**
 * The shape every lab decoder produces: a flat list of annotated byte ranges.
 *
 * Flat, not a tree, because that is what the hex view wants — "which field owns byte 37"
 * is a linear scan, and nesting is expressed with `depth` so the field list can still be
 * drawn as an outline. Every decoder in this directory (Kafka frames, wasm modules, TLS
 * records, ELF objects) speaks this and shares one component.
 */
export interface Field {
	name: string;
	/** The wire type, shown next to the name: `u32`, `LEB128 u32`, `Elf64_Shdr`. */
	type: string;
	/** Byte offsets into the buffer being decoded: `[start, end)`. */
	start: number;
	end: number;
	/** The decoded value, already formatted for a human. */
	value: string;
	/** One sentence of "why this byte is here", shown when the row is selected. */
	note?: string;
	/** Nesting level, for indentation only. */
	depth: number;
}

/** What every lab decoder returns: the fields, plus whatever went wrong. */
export interface Decoded {
	fields: Field[];
	/** Set when the bytes ran out or did not mean what they claimed to. */
	error?: string;
}

/** A cursor over a buffer that records a `Field` for everything it reads. */
export class Reader {
	offset = 0;
	readonly fields: Field[] = [];
	depth = 0;

	constructor(readonly bytes: Uint8Array) {}

	get left() {
		return this.bytes.length - this.offset;
	}

	need(n: number, what: string) {
		if (this.left < n) throw new Error(`${what}: wanted ${n} more bytes, ${this.left} left`);
	}

	push(name: string, type: string, start: number, end: number, value: string, note?: string) {
		this.fields.push({ name, type, start, end, value, note, depth: this.depth, ...(note ? {} : {}) });
	}

	/** Read `n` raw bytes and record them as one field. */
	take(n: number, name: string, type: string, format: (b: Uint8Array) => string, note?: string): Uint8Array {
		this.need(n, name);
		const start = this.offset;
		const slice = this.bytes.subarray(start, start + n);
		this.offset += n;
		this.push(name, type, start, this.offset, format(slice), note);
		return slice;
	}

	u8(name: string, format: (v: number) => string = String, note?: string): number {
		this.need(1, name);
		const start = this.offset;
		const v = this.bytes[this.offset++];
		this.push(name, 'u8', start, this.offset, format(v), note);
		return v;
	}

	/** Big-endian, which is what every network protocol in here uses. */
	u16(name: string, format: (v: number) => string = String, note?: string): number {
		this.need(2, name);
		const start = this.offset;
		const v = (this.bytes[start] << 8) | this.bytes[start + 1];
		this.offset += 2;
		this.push(name, 'u16', start, this.offset, format(v), note);
		return v;
	}

	u24(name: string, format: (v: number) => string = String, note?: string): number {
		this.need(3, name);
		const start = this.offset;
		const v = (this.bytes[start] << 16) | (this.bytes[start + 1] << 8) | this.bytes[start + 2];
		this.offset += 3;
		this.push(name, 'u24', start, this.offset, format(v), note);
		return v;
	}

	/** Skip bytes without describing them further — always says how many. */
	skip(n: number, name: string, type = 'bytes', note?: string) {
		this.need(n, name);
		const start = this.offset;
		this.offset += n;
		this.push(name, type, start, this.offset, `${n} bytes`, note);
	}
}

export function hexOf(bytes: Uint8Array, limit = 16): string {
	const head = [...bytes.subarray(0, limit)].map((b) => b.toString(16).padStart(2, '0')).join(' ');
	return bytes.length > limit ? `${head} … (${bytes.length} bytes)` : head || '(empty)';
}

export function asciiOf(bytes: Uint8Array): string {
	return [...bytes].map((b) => (b >= 32 && b < 127 ? String.fromCharCode(b) : '·')).join('');
}
