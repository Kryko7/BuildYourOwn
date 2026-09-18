import { describe, it, expect } from 'vitest';
import { tokenize } from './tokenizer';
import { crc32c, decodeRequest, parseHex, toHex, encodeUnsignedVarint, decodeUnsignedVarint, decodeZigZagVarint, encodeZigZagVarint } from './kafka';
import { encodeRecordBatch } from './recordbatch';
import { samples } from './samples';

describe('shell tokenizer', () => {
	it('splits words on unquoted whitespace and collapses runs', () => {
		const r = tokenize('echo   hello    world');
		expect(r.tokens.map((t) => t.value)).toEqual(['echo', 'hello', 'world']);
		expect(r.pipeline[0].argv).toEqual(['echo', 'hello', 'world']);
	});

	it('keeps every byte literal inside single quotes', () => {
		const r = tokenize(`echo 'a  $HOME \\n'`);
		expect(r.tokens[1].value).toBe('a  $HOME \\n');
		expect(r.spans.some((s) => s.kind === 'single')).toBe(true);
	});

	it('honours only the POSIX escapes inside double quotes', () => {
		const r = tokenize('echo "a\\"b\\\\c\\qd"');
		expect(r.tokens[1].value).toBe('a"b\\c\\qd');
	});

	it('concatenates adjacent quoted and unquoted segments into one word', () => {
		const r = tokenize(`echo he"llo"'wor'ld`);
		expect(r.tokens.map((t) => t.value)).toEqual(['echo', 'helloworld']);
	});

	it('marks an unterminated quote instead of throwing', () => {
		const r = tokenize(`echo "oops`);
		expect(r.unterminated).toBe('double');
		expect(r.warnings.join(' ')).toMatch(/continuation prompt/);
	});

	it('reports expansions outside single quotes only', () => {
		expect(tokenize('echo $HOME').tokens[1].expansions).toEqual(['HOME']);
		expect(tokenize('echo "${PATH}"').tokens[1].expansions).toEqual(['PATH']);
		expect(tokenize("echo '$HOME'").tokens[1].expansions).toEqual([]);
		expect(tokenize('echo $?').tokens[1].expansions).toEqual(['?']);
	});

	it('builds the pipeline and redirection graph', () => {
		const r = tokenize('cat < in.txt | grep -n foo 2>> err.log | wc -l > out.txt');
		expect(r.pipeline).toHaveLength(3);
		expect(r.separators).toEqual(['|', '|']);
		expect(r.pipeline[0].redirects[0]).toMatchObject({ op: '<', fd: 0, target: 'in.txt' });
		expect(r.pipeline[1].redirects[0]).toMatchObject({ op: '2>>', fd: 2, target: 'err.log' });
		expect(r.pipeline[2].argv).toEqual(['wc', '-l']);
		expect(r.pipeline[2].redirects[0]).toMatchObject({ op: '>', fd: 1, target: 'out.txt' });
	});

	// The playground paints the line by slicing it with the spans, so the spans have to tile
	// the input exactly: no gaps, no overlaps. An overlapping span used to repeat the inside
	// of every double-quoted word that contained an expansion or an escape.
	it('produces spans that tile the input exactly', () => {
		const lines = [
			`echo "hi $USER" 'and $LITERAL' | grep -n hi 2>> err.log > out.txt`,
			`echo he"llo"'wor'ld`,
			`cat < in.txt | wc -l > count.txt`,
			`echo "a \\"quoted\\" word" \\ space`,
			`ls /tmp # a trailing comment`,
			`echo "unterminated`,
			`echo "$A$B" "x\\$y" "$HOME/bin"`,
			``,
			`   `
		];
		for (const line of lines) {
			const { spans } = tokenize(line);
			expect(spans.map((s) => line.slice(s.start, s.end)).join('')).toBe(line);
			for (let i = 1; i < spans.length; i++) {
				expect(spans[i].start).toBe(spans[i - 1].end);
				expect(spans[i].end).toBeGreaterThan(spans[i].start);
			}
			expect(new Set(spans.map((s) => s.start)).size).toBe(spans.length);
		}
	});

	it('treats an unquoted # as a comment', () => {
		const r = tokenize('echo hi # not a word');
		expect(r.tokens.map((t) => t.value)).toEqual(['echo', 'hi']);
		expect(r.spans.some((s) => s.kind === 'comment')).toBe(true);
	});
});

describe('kafka varints', () => {
	it('round-trips unsigned varints', () => {
		for (const n of [0, 1, 127, 128, 300, 16383, 16384, 1048576]) {
			const bytes = new Uint8Array(encodeUnsignedVarint(n));
			expect(decodeUnsignedVarint(bytes, 0)).toEqual({ value: n, size: bytes.length });
		}
	});

	it('round-trips zigzag varints including negatives', () => {
		for (const n of [0, -1, 1, -64, 63, -1000, 1000]) {
			const bytes = new Uint8Array(encodeZigZagVarint(n));
			expect(decodeZigZagVarint(bytes, 0).value).toBe(n);
		}
	});

	it('encodes a compact array count as n + 1', () => {
		expect(encodeUnsignedVarint(0 + 1)).toEqual([1]);
		expect(encodeUnsignedVarint(1 + 1)).toEqual([2]);
	});
});

describe('crc32c', () => {
	// Well-known Castagnoli check values.
	it('matches the reference vectors', () => {
		expect(crc32c(new TextEncoder().encode('123456789')) >>> 0).toBe(0xe3069283);
		expect(crc32c(new Uint8Array(0))).toBe(0);
		expect(crc32c(new Uint8Array(32))).toBe(0x8a9136aa);
	});
});

describe('hex helpers', () => {
	it('parses spaced, 0x-prefixed and run-together hex', () => {
		expect([...parseHex('00 01 0a')]).toEqual([0, 1, 10]);
		expect([...parseHex('0x00,0x01')]).toEqual([0, 1]);
		expect(toHex(new Uint8Array([255, 0]))).toBe('ff 00');
	});

	it('rejects an odd digit count', () => {
		expect(() => parseHex('abc')).toThrow();
	});
});

describe('record batch', () => {
	it('encodes a v2 batch whose CRC32C validates', () => {
		const batch = encodeRecordBatch([{ key: null, value: 'hello' }]);
		const crcOverBody = crc32c(batch.bytes.subarray(21));
		expect(crcOverBody).toBe(batch.crc);
		expect(batch.bytes[16]).toBe(2); // magic
		const declared = new DataView(batch.bytes.buffer).getInt32(8, false);
		expect(declared).toBe(batch.bytes.length - 12);
	});

	it('sets the codec bits in the attributes', () => {
		expect(encodeRecordBatch([{ key: null, value: 'x' }], { codec: 'gzip' }).attributes & 7).toBe(1);
		expect(encodeRecordBatch([{ key: null, value: 'x' }], { codec: 'zstd' }).attributes & 7).toBe(4);
		expect(encodeRecordBatch([{ key: null, value: 'x' }], { transactional: true }).attributes & (1 << 4)).toBeTruthy();
	});

	it('describes every header field with a byte range', () => {
		const batch = encodeRecordBatch([{ key: 'k', value: 'v' }]);
		const total = batch.fields.reduce((n, f) => n + f.length, 0);
		expect(total).toBe(batch.bytes.length);
		expect(batch.fields[0].name).toBe('base_offset');
	});
});

describe('sample request decoding', () => {
	it('decodes every sample with no leftover bytes and no error', () => {
		for (const s of samples) {
			const d = decodeRequest(s.bytes);
			expect(d.error, `${s.label}: ${d.error}`).toBeUndefined();
			expect(d.fields.some((f) => f.name === 'trailing bytes')).toBe(false);
			const last = d.fields[d.fields.length - 1];
			expect(last.end).toBe(s.bytes.length);
		}
	});

	it('reads the api key, version and correlation id of ApiVersions v4', () => {
		const d = decodeRequest(samples[0].bytes);
		expect(d.apiKey).toBe(18);
		expect(d.apiVersion).toBe(4);
		expect(d.apiName).toBe('ApiVersions');
		expect(d.fields.find((f) => f.name === 'correlation_id')?.value).toBe('7');
		expect(d.fields.find((f) => f.name === 'client_id')?.value).toBe('"kafka-cli"');
	});

	it('explains the null cursor in DescribeTopicPartitions', () => {
		const d = decodeRequest(samples[1].bytes);
		expect(d.fields.find((f) => f.name === 'topics[0].name')?.value).toBe('"orders-log"');
		expect(d.fields.find((f) => f.name === 'cursor')?.note).toMatch(/null/);
	});

	it('decodes the nested RecordBatch inside Produce v11 and checks its CRC', () => {
		const d = decodeRequest(samples[3].bytes);
		const crc = d.fields.find((f) => f.name === 'batch.crc');
		expect(crc?.note).toBe('CRC32C matches the batch body');
		expect(d.fields.find((f) => f.name === 'record[0].value')?.value).toBe('"hello world"');
		expect(d.fields.find((f) => f.name === 'acks')?.value).toBe('-1');
	});

	it('flags a corrupted frame instead of throwing', () => {
		const broken = samples[0].bytes.slice(0, 12);
		const d = decodeRequest(broken);
		expect(d.error).toBeTruthy();
		expect(d.fields.length).toBeGreaterThan(0);
	});
});
