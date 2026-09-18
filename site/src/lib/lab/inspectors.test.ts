/**
 * The three new lab decoders. Each one ships its own sample bytes, encoded by the same
 * file that reads them, so these tests are a real round trip: encode a module / a
 * handshake / an object, decode it, and check the decode says what the encoder meant.
 *
 * They also check the failure path, because that is what a learner will actually hit: a
 * truncated or malformed input must come back with a message and whatever did parse, not
 * an exception that blanks the page.
 */
import { describe, it, expect } from 'vitest';
import {
	decodeModule,
	disassemble,
	encodeSleb,
	encodeUleb,
	parseWasmHex,
	readSleb,
	readUleb,
	wasmSamples
} from './wasm';
import {
	decodeTls,
	hkdfLabelBytes,
	parseTlsHex,
	tlsSamples,
	KEY_SCHEDULE,
	CIPHER_SUITES
} from './tls';
import { elfSamples, parseElf, parseElfHex, RELOC_TYPES } from './elf';

/** One sample by id, typed as whatever list it came from. */
const sample = <T extends { id: string }>(id: string, list: T[]): T => {
	const found = list.find((s) => s.id === id);
	if (!found) throw new Error(`no sample ${id}`);
	return found;
};

describe('LEB128', () => {
	it('round-trips unsigned values, using the fewest bytes', () => {
		for (const value of [0, 1, 63, 64, 127, 128, 300, 16383, 16384, 624485, 0xffffffff]) {
			const bytes = Uint8Array.from(encodeUleb(value));
			const read = readUleb(bytes, 0);
			expect(read.value, String(value)).toBe(value);
			expect(read.size, String(value)).toBe(bytes.length);
		}
		expect(encodeUleb(0)).toEqual([0x00]);
		expect(encodeUleb(624485)).toEqual([0xe5, 0x8e, 0x26]);
	});

	it('round-trips signed values, including the negative ones', () => {
		for (const value of [0, 1, -1, 63, -64, 64, -65, 127, -128, 1000, -1000, -123456]) {
			const bytes = Uint8Array.from(encodeSleb(value));
			const read = readSleb(bytes, 0);
			expect(read.value, String(value)).toBe(value);
			expect(read.size, String(value)).toBe(bytes.length);
		}
		expect(encodeSleb(-1)).toEqual([0x7f]);
		expect(encodeSleb(64)).toEqual([0xc0, 0x00]);
	});

	it('refuses an encoding that never terminates instead of looping', () => {
		expect(() => readUleb(Uint8Array.from([0x80, 0x80, 0x80]), 0)).toThrow(/past the end/);
		expect(() => readUleb(Uint8Array.from([]), 0)).toThrow();
	});
});

describe('the wasm module decoder', () => {
	it('reads the smallest module: four sections, one export, one body', () => {
		const m = decodeModule(sample('add', wasmSamples).bytes);
		expect(m.error).toBeUndefined();
		expect(m.valid).toBe(true);
		expect(m.version).toBe(1);
		expect(m.sections.map((s) => s.name)).toEqual(['type', 'function', 'export', 'code']);
		expect(m.types).toEqual(['(i32, i32) -> i32']);
		expect(m.exports).toEqual([{ name: 'add', kind: 'func', index: 0 }]);
		expect(m.functions).toHaveLength(1);
		expect(m.functions[0].exportName).toBe('add');
		expect(m.functions[0].instructions.map((i) => i.name)).toEqual([
			'local.get',
			'local.get',
			'i32.add',
			'end'
		]);
		expect(m.functions[0].instructions[1].args).toBe('1');
	});

	it('annotates the preamble and every LEB128 it reads, in file order', () => {
		const m = decodeModule(sample('add', wasmSamples).bytes);
		const names = m.fields.map((f) => f.name);
		expect(names[0]).toBe('magic');
		expect(names[1]).toBe('version');
		expect(names).toContain('type[0].form');
		expect(names).toContain('export[0].name');
		for (const f of m.fields) {
			expect(f.end, f.name).toBeLessThanOrEqual(sample('add', wasmSamples).bytes.length);
			expect(f.end, f.name).toBeGreaterThanOrEqual(f.start);
		}
		// The section sizes the decoder reports are the ones on the wire.
		for (const s of m.sections) expect(s.end - s.bodyStart).toBe(s.size);
	});

	it('reads a WASI command: the import, the memory, the data segment and the call', () => {
		const m = decodeModule(sample('wasi-hello', wasmSamples).bytes);
		expect(m.error).toBeUndefined();
		expect(m.imports).toEqual([
			{ module: 'wasi_snapshot_preview1', name: 'fd_write', kind: 'func' }
		]);
		expect(m.memory).toBe('min 1 page');
		expect(m.exports.map((e) => e.name)).toEqual(['memory', '_start']);
		// The imported function takes index 0, so `_start` is function 1.
		expect(m.functions[0].index).toBe(1);
		expect(m.functions[0].exportName).toBe('_start');
		expect(m.data).toHaveLength(1);
		expect(new TextDecoder().decode(m.data[0].bytes)).toBe('hello, garden\n');
		expect(m.data[0].offset).toBe('i32.const 16');
		const ops = m.functions[0].instructions.map((i) => i.name);
		expect(ops).toContain('i32.store');
		expect(ops).toContain('call');
	});

	it('stops a data segment’s offset expression at its own `end`', () => {
		// The bytes after that `end` are the string, not more instructions: reading them as
		// code is what made the first version of this decoder fall off the end of the file.
		const m = decodeModule(sample('wasi-hello', wasmSamples).bytes);
		expect(m.error).toBeUndefined();
		expect(m.sections.at(-1)?.name).toBe('data');
	});

	it('indents a nested block and closes it at the right depth', () => {
		//  block (i32.const 1) (br_if 0) end  end
		const body = Uint8Array.from([0x02, 0x40, 0x41, 0x01, 0x0d, 0x00, 0x0b, 0x0b]);
		const listing = disassemble(body, 0, body.length);
		expect(listing.map((i) => `${i.depth}:${i.name}`)).toEqual([
			'0:block',
			'1:i32.const',
			'1:br_if',
			'0:end',
			'0:end'
		]);
		expect(listing[0].args).toBe('(empty)');
	});

	it('decodes memory arguments and a br_table', () => {
		const load = disassemble(Uint8Array.from([0x28, 0x02, 0x10, 0x0b]), 0, 4);
		expect(load[0].name).toBe('i32.load');
		expect(load[0].args).toBe('align=2^2 offset=16');
		const table = disassemble(Uint8Array.from([0x0e, 0x02, 0x00, 0x01, 0x03, 0x0b]), 0, 6);
		expect(table[0].name).toBe('br_table');
		expect(table[0].args).toBe('[0 1] default 3');
	});

	it('says what is wrong instead of throwing, and keeps what it read', () => {
		expect(decodeModule(new Uint8Array()).error).toMatch(/at least 8 bytes/);
		expect(decodeModule(Uint8Array.from([1, 2, 3, 4, 5, 6, 7, 8])).error).toMatch(/bad magic/);

		const truncated = sample('add', wasmSamples).bytes.slice(0, 20);
		const m = decodeModule(truncated);
		expect(m.error).toBeTruthy();
		expect(m.valid).toBe(false);
		expect(m.version).toBe(1); // everything before the break is still there
		expect(m.fields.length).toBeGreaterThan(3);

		const badVersion = Uint8Array.from(sample('add', wasmSamples).bytes);
		badVersion[4] = 2;
		expect(decodeModule(badVersion).error).toMatch(/unsupported version 2/);
	});

	it('reads hex the way a learner will paste it', () => {
		expect([...parseWasmHex('00 61 73 6d')]).toEqual([0, 0x61, 0x73, 0x6d]);
		expect([...parseWasmHex('0061736d')]).toEqual([0, 0x61, 0x73, 0x6d]);
		expect([...parseWasmHex('0x00,0x61')]).toEqual([0, 0x61]);
		expect([...parseWasmHex('')]).toEqual([]);
		expect(() => parseWasmHex('000')).toThrow(/odd number/);
		expect(() => parseWasmHex('zz')).toThrow(/not hex/);
	});
});

describe('the TLS record and handshake decoder', () => {
	it('reads a ClientHello down to its extensions', () => {
		const d = decodeTls(sample('client-hello', tlsSamples).bytes);
		expect(d.error).toBeUndefined();
		expect(d.fromClient).toBe(true);
		expect(d.records).toHaveLength(1);
		expect(d.records[0].name).toBe('handshake');
		// The record's declared length is exactly the bytes that follow the 5-byte header.
		expect(d.records[0].end - d.records[0].start).toBe(d.records[0].length + 5);

		const hello = d.handshakes[0];
		expect(hello.name).toBe('client_hello');
		expect(hello.cipherSuites).toEqual([
			CIPHER_SUITES[0x1301],
			CIPHER_SUITES[0x1302],
			CIPHER_SUITES[0x1303]
		]);
		const byName = Object.fromEntries(hello.extensions.map((e) => [e.name, e.summary]));
		expect(byName.supported_versions).toBe('TLS 1.3');
		expect(byName.key_share).toBe('x25519 (32 bytes)');
		expect(byName.server_name).toBe('garden.localhost');
		expect(byName.application_layer_protocol_negotiation).toBe('h2');
		expect(byName.supported_groups).toContain('secp256r1');
		expect(byName.signature_algorithms).toContain('ed25519');
	});

	it('reads the server flight: a ServerHello, the legacy CCS, then ciphertext', () => {
		const d = decodeTls(sample('server-flight', tlsSamples).bytes);
		expect(d.error).toBeUndefined();
		expect(d.records.map((r) => r.name)).toEqual([
			'handshake',
			'change_cipher_spec',
			'application_data'
		]);
		expect(d.records[1].summary).toBe('legacy, ignored');
		expect(d.records[2].summary).toMatch(/ciphertext/);
		const hello = d.handshakes[0];
		expect(hello.name).toBe('server_hello');
		expect(hello.cipherSuites).toEqual(['TLS_AES_128_GCM_SHA256']);
		// The server sends a single selected version, not a list.
		expect(hello.extensions.find((e) => e.name === 'supported_versions')?.summary).toBe('TLS 1.3');
	});

	it('names both halves of an alert', () => {
		const d = decodeTls(sample('alerts', tlsSamples).bytes);
		expect(d.error).toBeUndefined();
		expect(d.records.map((r) => r.summary)).toEqual([
			'fatal handshake_failure',
			'warning close_notify'
		]);
	});

	it('reports a truncated capture instead of throwing', () => {
		const whole = sample('client-hello', tlsSamples).bytes;
		const cut = decodeTls(whole.slice(0, whole.length - 20));
		expect(cut.error).toBeTruthy();
		expect(cut.fields.length).toBeGreaterThan(3);

		const stray = decodeTls(Uint8Array.from([0x16, 0x03]));
		expect(stray.records).toHaveLength(0);
		expect(stray.error).toMatch(/trailing/);
		expect(decodeTls(new Uint8Array()).error).toBeUndefined();
	});

	it('encodes HkdfLabel exactly as RFC 8446 §7.1 spells it', () => {
		const bytes = hkdfLabelBytes('c hs traffic', 32, 32);
		// uint16 length, then the length-prefixed "tls13 " + label, then the context length.
		expect(bytes[0]).toBe(0x00);
		expect(bytes[1]).toBe(0x20);
		expect(bytes[2]).toBe('tls13 c hs traffic'.length);
		expect(new TextDecoder().decode(bytes.subarray(3, 3 + bytes[2]))).toBe('tls13 c hs traffic');
		expect(bytes[3 + bytes[2]]).toBe(32);
		expect(bytes.length).toBe(2 + 1 + bytes[2] + 1 + 32);
		expect(hkdfLabelBytes('finished', 0, 32).length).toBe(2 + 1 + 'tls13 finished'.length + 1);
	});

	it('walks the key schedule in the order the RFC derives it', () => {
		const ids = KEY_SCHEDULE.map((s) => s.id);
		expect(ids.indexOf('early-secret')).toBeLessThan(ids.indexOf('handshake-secret'));
		expect(ids.indexOf('handshake-secret')).toBeLessThan(ids.indexOf('master-secret'));
		expect(ids.indexOf('c-hs-traffic')).toBeLessThan(ids.indexOf('c-ap-traffic'));
		// The transcript boundary is the thing people get wrong, so every secret states it.
		const hs = KEY_SCHEDULE.find((s) => s.id === 's-hs-traffic')!;
		expect(hs.transcript).toBe('ClientHello … ServerHello');
		expect(KEY_SCHEDULE.find((s) => s.id === 's-ap-traffic')!.transcript).toMatch(/Finished/);
		for (const step of KEY_SCHEDULE) expect(step.why.length, step.id).toBeGreaterThan(30);
	});

	it('reads pasted hex', () => {
		expect([...parseTlsHex('16 03 01')]).toEqual([0x16, 0x03, 0x01]);
		expect(() => parseTlsHex('16 03 0')).toThrow(/odd number/);
	});
});

describe('the ELF reader', () => {
	it('reads a relocatable object: header, sections, symbols, relocations', () => {
		const elf = parseElf(sample('main-o', elfSamples).bytes);
		expect(elf.error).toBeUndefined();
		expect(elf.valid).toBe(true);
		expect(elf.class).toBe('ELF64');
		expect(elf.endianness).toBe('little-endian');
		expect(elf.fileType).toMatch(/^ET_REL/);
		expect(elf.machine).toBe('EM_X86_64');
		expect(elf.entry).toBe(0n);

		expect(elf.sections.map((s) => s.name)).toEqual([
			'',
			'.text',
			'.rela.text',
			'.rodata',
			'.symtab',
			'.strtab',
			'.shstrtab'
		]);
		const text = elf.sections.find((s) => s.name === '.text')!;
		expect(text.type).toBe('SHT_PROGBITS');
		expect(text.flags).toBe('AX'); // allocated and executable
		expect(text.size).toBeGreaterThan(0);
	});

	it('resolves symbol names, bindings and the section each one lives in', () => {
		const elf = parseElf(sample('main-o', elfSamples).bytes);
		const byName = Object.fromEntries(elf.symbols.map((s) => [s.name, s]));
		expect(byName.main.bind).toBe('GLOBAL');
		expect(byName.main.type).toBe('FUNC');
		expect(byName.main.section).toBe('.text');
		expect(byName.message.type).toBe('OBJECT');
		expect(byName.message.section).toBe('.rodata');
		// The undefined symbol is the whole reason a linker exists.
		expect(byName.puts.section).toBe('SHN_UNDEF');
		expect(byName.puts.bind).toBe('GLOBAL');
	});

	it('decodes relocations with the symbol they name and the formula they apply', () => {
		const elf = parseElf(sample('main-o', elfSamples).bytes);
		expect(elf.relocations).toHaveLength(2);
		const [abs, call] = elf.relocations;
		expect(abs).toMatchObject({
			section: '.text',
			offset: 6n,
			type: 'R_X86_64_64',
			symbol: 'message',
			addend: 0n
		});
		expect(call).toMatchObject({
			section: '.text',
			offset: 15n,
			type: 'R_X86_64_PLT32',
			symbol: 'puts',
			addend: -4n
		});
		expect(call.formula).toBe(RELOC_TYPES[4].formula);
	});

	it('reads a weak symbol and a .bss that has a size but no bytes', () => {
		const elf = parseElf(sample('puts-o', elfSamples).bytes);
		expect(elf.error).toBeUndefined();
		const bss = elf.sections.find((s) => s.name === '.bss')!;
		expect(bss.type).toBe('SHT_NOBITS');
		expect(bss.size).toBe(4096);
		const weak = elf.symbols.find((s) => s.name === 'on_exit_hook')!;
		expect(weak.bind).toBe('WEAK');
		expect(elf.symbols.find((s) => s.name === 'buffer')!.section).toBe('.bss');
		expect(elf.relocations).toEqual([]);
	});

	it('annotates the header byte by byte, in order', () => {
		const elf = parseElf(sample('main-o', elfSamples).bytes);
		const names = elf.fields.map((f) => f.name);
		expect(names[0]).toBe('e_ident.magic');
		expect(names).toContain('e_shoff');
		expect(names).toContain('shdr[1].sh_name');
		let cursor = -1;
		for (const f of elf.fields) {
			expect(f.start, f.name).toBeGreaterThanOrEqual(cursor);
			cursor = f.start;
		}
	});

	it('refuses what is not an ELF64 object, with a reason', () => {
		expect(parseElf(new Uint8Array()).error).toMatch(/wanted 64 bytes/);
		expect(parseElf(new Uint8Array(64)).error).toMatch(/bad magic/);

		const elf32 = Uint8Array.from(sample('main-o', elfSamples).bytes);
		elf32[4] = 1; // ELFCLASS32
		expect(parseElf(elf32).error).toMatch(/only understands ELF64/);

		const cut = parseElf(sample('main-o', elfSamples).bytes.slice(0, 200));
		expect(cut.error).toBeTruthy();
		expect(cut.machine).toBe('EM_X86_64'); // the header still parsed
	});

	it('reads pasted hex', () => {
		expect([...parseElfHex('7f 45 4c 46')]).toEqual([0x7f, 0x45, 0x4c, 0x46]);
		expect(() => parseElfHex('7f 45 4')).toThrow(/odd number/);
	});
});
