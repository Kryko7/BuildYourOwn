/**
 * A tiny Kafka wire codec for the /lab inspector: enough to build four realistic
 * requests, decode them field by field with byte offsets, and explain the flexible
 * -version encodings (unsigned varints, compact strings/arrays, tagged fields).
 */

/* ---------------------------------- CRC32C --------------------------------- */

const CRC32C_TABLE = (() => {
	const table = new Uint32Array(256);
	for (let n = 0; n < 256; n++) {
		let c = n;
		for (let k = 0; k < 8; k++) c = c & 1 ? 0x82f63b78 ^ (c >>> 1) : c >>> 1;
		table[n] = c >>> 0;
	}
	return table;
})();

/** Castagnoli CRC-32C, the polynomial Kafka uses for record batches. */
export function crc32c(bytes: Uint8Array, seed = 0): number {
	let crc = (seed ^ 0xffffffff) >>> 0;
	for (let i = 0; i < bytes.length; i++) {
		crc = (CRC32C_TABLE[(crc ^ bytes[i]) & 0xff] ^ (crc >>> 8)) >>> 0;
	}
	return (crc ^ 0xffffffff) >>> 0;
}

/* ---------------------------------- varints -------------------------------- */

export function encodeUnsignedVarint(value: number): number[] {
	const out: number[] = [];
	let v = value >>> 0;
	while (v > 0x7f) {
		out.push((v & 0x7f) | 0x80);
		v >>>= 7;
	}
	out.push(v);
	return out;
}

export function encodeZigZagVarint(value: number): number[] {
	return encodeUnsignedVarint(((value << 1) ^ (value >> 31)) >>> 0);
}

export function encodeZigZagVarlong(value: bigint): number[] {
	let v = (value << 1n) ^ (value >> 63n);
	const out: number[] = [];
	while (v > 0x7fn) {
		out.push(Number((v & 0x7fn) | 0x80n));
		v >>= 7n;
	}
	out.push(Number(v));
	return out;
}

export function decodeUnsignedVarint(bytes: Uint8Array, offset: number): { value: number; size: number } {
	let value = 0;
	let shift = 0;
	let size = 0;
	while (offset + size < bytes.length) {
		const b = bytes[offset + size];
		value |= (b & 0x7f) << shift;
		size++;
		if ((b & 0x80) === 0) return { value: value >>> 0, size };
		shift += 7;
		if (shift > 35) break;
	}
	throw new Error(`varint at byte ${offset} never terminates`);
}

export function decodeZigZagVarint(bytes: Uint8Array, offset: number) {
	const { value, size } = decodeUnsignedVarint(bytes, offset);
	return { value: (value >>> 1) ^ -(value & 1), size };
}

/* ---------------------------------- writer --------------------------------- */

export class Writer {
	bytes: number[] = [];

	i8(v: number) { this.bytes.push(v & 0xff); return this; }
	i16(v: number) { this.bytes.push((v >> 8) & 0xff, v & 0xff); return this; }
	i32(v: number) { this.bytes.push((v >>> 24) & 0xff, (v >>> 16) & 0xff, (v >>> 8) & 0xff, v & 0xff); return this; }
	u32(v: number) { return this.i32(v >>> 0); }
	i64(v: bigint) {
		for (let i = 7; i >= 0; i--) this.bytes.push(Number((v >> BigInt(i * 8)) & 0xffn));
		return this;
	}
	raw(v: ArrayLike<number>) { for (let i = 0; i < v.length; i++) this.bytes.push(v[i] & 0xff); return this; }
	varint(v: number) { return this.raw(encodeUnsignedVarint(v)); }
	zigzag(v: number) { return this.raw(encodeZigZagVarint(v)); }
	zigzagLong(v: bigint) { return this.raw(encodeZigZagVarlong(v)); }
	/** Legacy (non-compact) nullable string: i16 length + utf8, -1 for null. */
	legacyString(s: string | null) {
		if (s === null) return this.i16(-1);
		const b = new TextEncoder().encode(s);
		this.i16(b.length);
		return this.raw(b);
	}
	/** Compact string: unsigned varint length+1 + utf8, 0 for null. */
	compactString(s: string | null) {
		if (s === null) return this.varint(0);
		const b = new TextEncoder().encode(s);
		this.varint(b.length + 1);
		return this.raw(b);
	}
	compactBytes(b: ArrayLike<number> | null) {
		if (b === null) return this.varint(0);
		this.varint(b.length + 1);
		return this.raw(b);
	}
	compactArrayLen(n: number) { return this.varint(n + 1); }
	taggedFields(n = 0) { return this.varint(n); }
	uuid(hex: string) {
		const clean = hex.replace(/-/g, '');
		for (let i = 0; i < 16; i++) this.bytes.push(parseInt(clean.slice(i * 2, i * 2 + 2), 16) || 0);
		return this;
	}
	toUint8Array() { return new Uint8Array(this.bytes); }
	/** Prepend the 4-byte big-endian frame size, as every Kafka request is framed. */
	framed() {
		const body = this.toUint8Array();
		const out = new Uint8Array(body.length + 4);
		new DataView(out.buffer).setInt32(0, body.length, false);
		out.set(body, 4);
		return out;
	}
}

/* --------------------------------- decoding -------------------------------- */

export interface Field {
	name: string;
	type: string;
	start: number;
	end: number;
	value: string;
	note?: string;
	depth: number;
}

class Reader {
	constructor(
		public bytes: Uint8Array,
		public offset = 0,
		public fields: Field[] = [],
		public depth = 0
	) {}

	#add(name: string, type: string, start: number, value: string, note?: string) {
		this.fields.push({ name, type, start, end: this.offset, value, note, depth: this.depth });
	}

	need(n: number) {
		if (this.offset + n > this.bytes.length) throw new Error(`truncated: needed ${n} more bytes at ${this.offset}`);
	}

	i8(name: string, note?: string) {
		const s = this.offset; this.need(1);
		const v = (this.bytes[this.offset] << 24) >> 24;
		this.offset += 1; this.#add(name, 'int8', s, String(v), note); return v;
	}
	i16(name: string, note?: string) {
		const s = this.offset; this.need(2);
		const v = new DataView(this.bytes.buffer, this.bytes.byteOffset).getInt16(s, false);
		this.offset += 2; this.#add(name, 'int16', s, String(v), note); return v;
	}
	i32(name: string, note?: string) {
		const s = this.offset; this.need(4);
		const v = new DataView(this.bytes.buffer, this.bytes.byteOffset).getInt32(s, false);
		this.offset += 4; this.#add(name, 'int32', s, String(v), note); return v;
	}
	i64(name: string, note?: string) {
		const s = this.offset; this.need(8);
		const v = new DataView(this.bytes.buffer, this.bytes.byteOffset).getBigInt64(s, false);
		this.offset += 8; this.#add(name, 'int64', s, String(v), note); return v;
	}
	legacyString(name: string, note?: string) {
		const s = this.offset; this.need(2);
		const len = new DataView(this.bytes.buffer, this.bytes.byteOffset).getInt16(s, false);
		this.offset += 2;
		if (len < 0) { this.#add(name, 'nullable_string', s, 'null', note); return null; }
		this.need(len);
		const v = new TextDecoder().decode(this.bytes.subarray(this.offset, this.offset + len));
		this.offset += len;
		this.#add(name, 'nullable_string', s, JSON.stringify(v), note ?? `int16 length ${len} then ${len} bytes of UTF-8`);
		return v;
	}
	compactString(name: string, note?: string) {
		const s = this.offset;
		const { value: n, size } = decodeUnsignedVarint(this.bytes, this.offset);
		this.offset += size;
		if (n === 0) { this.#add(name, 'compact_string', s, 'null', note ?? 'varint 0 = null'); return null; }
		const len = n - 1;
		this.need(len);
		const v = new TextDecoder().decode(this.bytes.subarray(this.offset, this.offset + len));
		this.offset += len;
		this.#add(name, 'compact_string', s, JSON.stringify(v), note ?? `varint ${n} = length ${len} + 1`);
		return v;
	}
	compactBytes(name: string, note?: string) {
		const s = this.offset;
		const { value: n, size } = decodeUnsignedVarint(this.bytes, this.offset);
		this.offset += size;
		if (n === 0) { this.#add(name, 'compact_bytes', s, 'null', note ?? 'varint 0 = null'); return null; }
		const len = n - 1;
		this.need(len);
		const v = this.bytes.subarray(this.offset, this.offset + len);
		this.offset += len;
		this.#add(name, 'compact_bytes', s, `${len} bytes`, note ?? `varint ${n} = length ${len} + 1`);
		return v;
	}
	compactArrayLen(name: string) {
		const s = this.offset;
		const { value: n, size } = decodeUnsignedVarint(this.bytes, this.offset);
		this.offset += size;
		const len = Math.max(0, n - 1);
		this.#add(name, 'compact_array', s, `${len} item${len === 1 ? '' : 's'}`, `varint ${n} = count ${len} + 1`);
		return len;
	}
	uuid(name: string, note?: string) {
		const s = this.offset; this.need(16);
		const hex = [...this.bytes.subarray(s, s + 16)].map((b) => b.toString(16).padStart(2, '0')).join('');
		this.offset += 16;
		const formatted = `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
		this.#add(name, 'uuid', s, formatted, note);
		return formatted;
	}
	tagged(name = 'tagged_fields') {
		const s = this.offset;
		const { value: n, size } = decodeUnsignedVarint(this.bytes, this.offset);
		this.offset += size;
		this.#add(name, 'tagged_fields', s, `${n} tag${n === 1 ? '' : 's'}`, n === 0 ? '0x00 — the flexible-version terminator' : undefined);
		return n;
	}
	group<T>(fn: () => T): T {
		this.depth++;
		try { return fn(); } finally { this.depth--; }
	}
}

export const API_NAMES: Record<number, string> = {
	0: 'Produce',
	1: 'Fetch',
	2: 'ListOffsets',
	3: 'Metadata',
	8: 'OffsetCommit',
	9: 'OffsetFetch',
	10: 'FindCoordinator',
	11: 'JoinGroup',
	12: 'Heartbeat',
	13: 'LeaveGroup',
	14: 'SyncGroup',
	18: 'ApiVersions',
	19: 'CreateTopics',
	20: 'DeleteTopics',
	22: 'InitProducerId',
	75: 'DescribeTopicPartitions'
};

export interface DecodeResult {
	fields: Field[];
	apiKey: number;
	apiVersion: number;
	apiName: string;
	bytes: Uint8Array;
	error?: string;
}

/** Decode a framed request (4-byte size + header + body) into annotated fields. */
export function decodeRequest(frame: Uint8Array): DecodeResult {
	const r = new Reader(frame);
	let apiKey = -1;
	let apiVersion = -1;
	try {
		const size = r.i32('message_size', 'big-endian frame length, not counting these 4 bytes');
		if (size !== frame.length - 4) {
			r.fields[0].note = `frame says ${size} bytes but ${frame.length - 4} follow — a framing bug (stage 9)`;
		}
		apiKey = r.i16('api_key');
		const nameField = r.fields[r.fields.length - 1];
		nameField.note = API_NAMES[apiKey] ? `${API_NAMES[apiKey]}(${apiKey})` : `unknown api key ${apiKey}`;
		apiVersion = r.i16('api_version');
		r.i32('correlation_id', 'echoed back verbatim in the response');
		r.legacyString('client_id', 'legacy nullable string even in a flexible header (header v1/v2)');
		const flexible = isFlexibleRequest(apiKey, apiVersion);
		if (flexible) r.tagged('header tagged_fields');
		decodeBody(r, apiKey, apiVersion);
		if (r.offset < frame.length) {
			r.fields.push({
				name: 'trailing bytes',
				type: 'bytes',
				start: r.offset,
				end: frame.length,
				value: `${frame.length - r.offset} undecoded bytes`,
				depth: 0
			});
		}
	} catch (e) {
		return {
			fields: r.fields,
			apiKey,
			apiVersion,
			apiName: API_NAMES[apiKey] ?? 'unknown',
			bytes: frame,
			error: e instanceof Error ? e.message : String(e)
		};
	}
	return { fields: r.fields, apiKey, apiVersion, apiName: API_NAMES[apiKey] ?? 'unknown', bytes: frame };
}

function isFlexibleRequest(apiKey: number, version: number): boolean {
	const firstFlexible: Record<number, number> = { 0: 9, 1: 12, 3: 9, 18: 3, 19: 5, 20: 4, 75: 0 };
	const from = firstFlexible[apiKey];
	return from !== undefined && version >= from;
}

function decodeBody(r: Reader, apiKey: number, version: number) {
	switch (apiKey) {
		case 18:
			if (version >= 3) {
				r.compactString('client_software_name');
				r.compactString('client_software_version');
				r.tagged();
			}
			return;
		case 75: {
			const topics = r.compactArrayLen('topics');
			for (let i = 0; i < topics; i++) {
				r.group(() => {
					r.compactString(`topics[${i}].name`);
					r.tagged(`topics[${i}].tagged_fields`);
				});
			}
			r.i32('response_partition_limit', 'caps the partitions in one response (stage 15)');
			const cursorByte = r.bytes[r.offset];
			if (cursorByte === 0xff) {
				r.i8('cursor', '0xff = null — start from the beginning');
			} else {
				r.group(() => {
					r.i8('cursor present');
					r.compactString('cursor.topic_name');
					r.i32('cursor.partition_index');
					r.tagged('cursor.tagged_fields');
				});
			}
			r.tagged();
			return;
		}
		case 1: {
			r.i32('max_wait_ms', 'how long the broker may park this fetch (stage 28)');
			r.i32('min_bytes');
			r.i32('max_bytes');
			r.i8('isolation_level', '0 = read_uncommitted, 1 = read_committed');
			r.i32('session_id');
			r.i32('session_epoch');
			const topics = r.compactArrayLen('topics');
			for (let i = 0; i < topics; i++) {
				r.group(() => {
					r.uuid(`topics[${i}].topic_id`, 'v13+ identifies topics by uuid, not name');
					const parts = r.compactArrayLen(`topics[${i}].partitions`);
					for (let p = 0; p < parts; p++) {
						r.group(() => {
							r.i32(`partitions[${p}].partition`);
							r.i32(`partitions[${p}].current_leader_epoch`);
							r.i64(`partitions[${p}].fetch_offset`);
							r.i32(`partitions[${p}].last_fetched_epoch`);
							r.i64(`partitions[${p}].log_start_offset`);
							r.i32(`partitions[${p}].partition_max_bytes`);
							r.tagged(`partitions[${p}].tagged_fields`);
						});
					}
					r.tagged(`topics[${i}].tagged_fields`);
				});
			}
			const forgotten = r.compactArrayLen('forgotten_topics_data');
			for (let i = 0; i < forgotten; i++) {
				r.group(() => {
					r.uuid(`forgotten[${i}].topic_id`);
					const n = r.compactArrayLen(`forgotten[${i}].partitions`);
					for (let p = 0; p < n; p++) r.i32(`forgotten[${i}].partitions[${p}]`);
					r.tagged(`forgotten[${i}].tagged_fields`);
				});
			}
			r.compactString('rack_id');
			r.tagged();
			return;
		}
		case 0: {
			r.compactString('transactional_id', 'null unless the producer is transactional');
			r.i16('acks', '0 = no response at all, 1 = leader only, -1 = all in-sync replicas');
			r.i32('timeout_ms');
			const topics = r.compactArrayLen('topic_data');
			for (let i = 0; i < topics; i++) {
				r.group(() => {
					r.compactString(`topic_data[${i}].name`);
					const parts = r.compactArrayLen(`topic_data[${i}].partition_data`);
					for (let p = 0; p < parts; p++) {
						r.group(() => {
							r.i32(`partition_data[${p}].index`);
							const start = r.offset;
							const records = r.compactBytes(`partition_data[${p}].records`, 'one or more RecordBatch v2 structures');
							if (records) {
								const inner = new Reader(records, 0, r.fields, r.depth + 1);
								try {
									decodeRecordBatchInto(inner, start);
								} catch {
									/* leave the batch opaque if it is malformed */
								}
							}
							r.tagged(`partition_data[${p}].tagged_fields`);
						});
					}
					r.tagged(`topic_data[${i}].tagged_fields`);
				});
			}
			r.tagged();
			return;
		}
		default:
			return;
	}
}

/** Decode a RecordBatch, shifting reported offsets so they point into the outer frame. */
function decodeRecordBatchInto(r: Reader, frameBase: number) {
	const before = r.fields.length;
	r.i64('batch.base_offset');
	r.i32('batch.batch_length', 'bytes after this field');
	r.i32('batch.partition_leader_epoch');
	r.i8('batch.magic', '2 = the current record batch format');
	const crcStart = r.offset;
	const crc = r.i32('batch.crc', 'CRC32C over everything after this field');
	r.i16('batch.attributes', 'bits 0-2 compression, bit 3 timestamp type, bit 4 transactional, bit 5 control');
	r.i32('batch.last_offset_delta');
	r.i64('batch.base_timestamp');
	r.i64('batch.max_timestamp');
	r.i64('batch.producer_id');
	r.i16('batch.producer_epoch');
	r.i32('batch.base_sequence');
	const count = r.i32('batch.record_count');
	const actual = crc32c(r.bytes.subarray(crcStart + 4));
	const crcField = r.fields.find((f) => f.name === 'batch.crc' && f.start === crcStart);
	if (crcField) {
		const expected = (crc >>> 0).toString(16).padStart(8, '0');
		crcField.value = `0x${expected}`;
		crcField.note =
			actual === (crc >>> 0)
				? 'CRC32C matches the batch body'
				: `CRC32C mismatch — computed 0x${actual.toString(16).padStart(8, '0')} (stage 37: CORRUPT_MESSAGE)`;
	}
	for (let i = 0; i < count && r.offset < r.bytes.length; i++) {
		r.group(() => {
			const len = zigzagField(r, `record[${i}].length`);
			const end = r.offset + len;
			r.i8(`record[${i}].attributes`);
			zigzagField(r, `record[${i}].timestamp_delta`);
			zigzagField(r, `record[${i}].offset_delta`);
			varBytesField(r, `record[${i}].key`);
			varBytesField(r, `record[${i}].value`);
			const headers = zigzagField(r, `record[${i}].header_count`);
			for (let h = 0; h < headers; h++) {
				varBytesField(r, `record[${i}].headers[${h}].key`);
				varBytesField(r, `record[${i}].headers[${h}].value`);
			}
			r.offset = end;
		});
	}
	// shift every field this pass produced into the outer frame's coordinates
	for (let i = before; i < r.fields.length; i++) {
		r.fields[i].start += frameBase;
		r.fields[i].end += frameBase;
	}
}

function zigzagField(r: Reader, name: string): number {
	const s = r.offset;
	const { value, size } = decodeZigZagVarint(r.bytes, r.offset);
	r.offset += size;
	r.fields.push({ name, type: 'varint (zigzag)', start: s, end: r.offset, value: String(value), depth: r.depth });
	return value;
}

function varBytesField(r: Reader, name: string) {
	const s = r.offset;
	const { value: len, size } = decodeZigZagVarint(r.bytes, r.offset);
	r.offset += size;
	if (len < 0) {
		r.fields.push({ name, type: 'varint bytes', start: s, end: r.offset, value: 'null', depth: r.depth });
		return null;
	}
	const body = r.bytes.subarray(r.offset, r.offset + len);
	r.offset += len;
	let text: string;
	try {
		text = JSON.stringify(new TextDecoder('utf-8', { fatal: true }).decode(body));
	} catch {
		text = `${len} bytes`;
	}
	r.fields.push({ name, type: 'varint bytes', start: s, end: r.offset, value: text, depth: r.depth });
	return body;
}

/* ------------------------------- hex helpers ------------------------------- */

export function toHex(bytes: Uint8Array): string {
	return [...bytes].map((b) => b.toString(16).padStart(2, '0')).join(' ');
}

export function parseHex(text: string): Uint8Array {
	const cleaned = text.replace(/0x/gi, ' ').replace(/[^0-9a-fA-F]/g, '');
	if (cleaned.length === 0) return new Uint8Array();
	if (cleaned.length % 2 !== 0) throw new Error('Odd number of hex digits.');
	const out = new Uint8Array(cleaned.length / 2);
	for (let i = 0; i < out.length; i++) out[i] = parseInt(cleaned.slice(i * 2, i * 2 + 2), 16);
	return out;
}

export function printable(byte: number): string {
	return byte >= 0x20 && byte < 0x7f ? String.fromCharCode(byte) : '.';
}
