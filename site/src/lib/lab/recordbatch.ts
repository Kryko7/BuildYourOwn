/**
 * RecordBatch v2 encoder for the /lab anatomy builder. Produces the exact bytes a
 * broker would find in a segment file, with a per-field map so the UI can highlight
 * the bytes a field owns.
 */
import { Writer, crc32c, encodeZigZagVarint, encodeZigZagVarlong } from './kafka';

export interface RecordInput {
	key: string | null;
	value: string;
	headers?: { key: string; value: string }[];
	timestampDelta?: number;
}

export type Codec = 'none' | 'gzip' | 'snappy' | 'lz4' | 'zstd';

export const CODEC_BITS: Record<Codec, number> = { none: 0, gzip: 1, snappy: 2, lz4: 3, zstd: 4 };

export interface BatchField {
	name: string;
	start: number;
	length: number;
	value: string;
	note?: string;
	group: 'header' | 'record';
}

export interface BatchOptions {
	baseOffset?: bigint;
	baseTimestamp?: bigint;
	producerId?: bigint;
	producerEpoch?: number;
	baseSequence?: number;
	codec?: Codec;
	transactional?: boolean;
	control?: boolean;
	/** Pre-compressed record payload; when given, the codec bits are set and this is used verbatim. */
	compressedPayload?: Uint8Array;
}

export interface EncodedBatch {
	bytes: Uint8Array;
	fields: BatchField[];
	crc: number;
	attributes: number;
	recordsPayload: Uint8Array;
}

const enc = new TextEncoder();

function encodeRecord(rec: RecordInput, offsetDelta: number): Uint8Array {
	const body = new Writer();
	body.i8(0); // record attributes: unused, always 0
	body.raw(encodeZigZagVarint(rec.timestampDelta ?? offsetDelta));
	body.raw(encodeZigZagVarint(offsetDelta));
	if (rec.key === null) body.raw(encodeZigZagVarint(-1));
	else {
		const k = enc.encode(rec.key);
		body.raw(encodeZigZagVarint(k.length));
		body.raw(k);
	}
	const v = enc.encode(rec.value);
	body.raw(encodeZigZagVarint(v.length));
	body.raw(v);
	const headers = rec.headers ?? [];
	body.raw(encodeZigZagVarint(headers.length));
	for (const h of headers) {
		const hk = enc.encode(h.key);
		body.raw(encodeZigZagVarint(hk.length));
		body.raw(hk);
		const hv = enc.encode(h.value);
		body.raw(encodeZigZagVarint(hv.length));
		body.raw(hv);
	}
	const inner = body.toUint8Array();
	const out = new Writer();
	out.raw(encodeZigZagVarint(inner.length));
	out.raw(inner);
	return out.toUint8Array();
}

export function encodeRecordBatch(records: RecordInput[], opts: BatchOptions = {}): EncodedBatch {
	const {
		baseOffset = 0n,
		baseTimestamp = 1757721600000n,
		producerId = -1n,
		producerEpoch = -1,
		baseSequence = -1,
		codec = 'none',
		transactional = false,
		control = false,
		compressedPayload
	} = opts;

	const plain = new Writer();
	records.forEach((rec, i) => plain.raw(encodeRecord(rec, i)));
	const uncompressed = plain.toUint8Array();
	const payload = compressedPayload ?? uncompressed;

	let attributes = CODEC_BITS[codec] & 0x07;
	if (transactional) attributes |= 1 << 4;
	if (control) attributes |= 1 << 5;

	// Everything after the CRC field, which is what the CRC covers.
	const afterCrc = new Writer();
	afterCrc.i16(attributes);
	afterCrc.i32(Math.max(0, records.length - 1));
	afterCrc.i64(baseTimestamp);
	afterCrc.i64(baseTimestamp + BigInt(Math.max(0, records.length - 1)));
	afterCrc.i64(producerId);
	afterCrc.i16(producerEpoch);
	afterCrc.i32(baseSequence);
	afterCrc.i32(records.length);
	afterCrc.raw(payload);
	const afterCrcBytes = afterCrc.toUint8Array();
	const crc = crc32c(afterCrcBytes);

	// partition_leader_epoch(4) + magic(1) + crc(4) + afterCrc
	const batchLength = 4 + 1 + 4 + afterCrcBytes.length;

	const w = new Writer();
	w.i64(baseOffset);
	w.i32(batchLength);
	w.i32(-1); // partition_leader_epoch
	w.i8(2); // magic
	w.u32(crc);
	w.raw(afterCrcBytes);
	const bytes = w.toUint8Array();

	const fields: BatchField[] = [];
	let at = 0;
	const f = (name: string, length: number, value: string, note?: string, group: BatchField['group'] = 'header') => {
		fields.push({ name, start: at, length, value, note, group });
		at += length;
	};
	f('base_offset', 8, String(baseOffset), 'the broker rewrites this when it appends the batch');
	f('batch_length', 4, String(batchLength), 'bytes after this field');
	f('partition_leader_epoch', 4, '-1', 'set by the leader, -1 from a producer');
	f('magic', 1, '2', 'the v2 record batch format');
	f('crc', 4, `0x${crc.toString(16).padStart(8, '0')}`, 'CRC32C (Castagnoli) over every byte after this one');
	f('attributes', 2, `0x${attributes.toString(16).padStart(4, '0')}`, attributeNote(codec, transactional, control));
	f('last_offset_delta', 4, String(Math.max(0, records.length - 1)), 'record_count - 1');
	f('base_timestamp', 8, String(baseTimestamp));
	f('max_timestamp', 8, String(baseTimestamp + BigInt(Math.max(0, records.length - 1))));
	f('producer_id', 8, String(producerId), 'from InitProducerId; -1 when not idempotent');
	f('producer_epoch', 2, String(producerEpoch));
	f('base_sequence', 4, String(baseSequence), 'the idempotence counter (stage 36)');
	f('record_count', 4, String(records.length));

	if (compressedPayload) {
		f(
			`records (${codec})`,
			payload.length,
			`${payload.length} bytes, was ${uncompressed.length}`,
			'the whole record run is compressed as one blob, not per record',
			'record'
		);
	} else {
		records.forEach((rec, i) => {
			const encoded = encodeRecord(rec, i);
			f(
				`record[${i}]`,
				encoded.length,
				rec.key === null ? `value ${JSON.stringify(rec.value)}` : `${JSON.stringify(rec.key)} → ${JSON.stringify(rec.value)}`,
				'length, attributes, timestamp delta, offset delta, key, value, headers — all varints',
				'record'
			);
		});
	}

	return { bytes, fields, crc, attributes, recordsPayload: payload };
}

function attributeNote(codec: Codec, transactional: boolean, control: boolean): string {
	const parts = [`bits 0-2 = ${CODEC_BITS[codec]} (${codec})`];
	parts.push(`bit 4 transactional = ${transactional ? 1 : 0}`);
	parts.push(`bit 5 control = ${control ? 1 : 0}`);
	return parts.join(', ');
}

/** gzip through the platform's CompressionStream; null when unavailable. */
export async function gzip(bytes: Uint8Array): Promise<Uint8Array | null> {
	const CS = (globalThis as { CompressionStream?: typeof CompressionStream }).CompressionStream;
	if (!CS) return null;
	try {
		const stream = new Blob([bytes as BlobPart]).stream().pipeThrough(new CS('gzip'));
		return new Uint8Array(await new Response(stream).arrayBuffer());
	} catch {
		return null;
	}
}
