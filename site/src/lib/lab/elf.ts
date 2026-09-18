/**
 * An ELF64 reader for the /lab viewer — the half of a linker you write first.
 *
 * It parses the file header, the program and section headers, the symbol table with its
 * string table, and the RELA relocations with their x86-64 type names. It also builds the
 * sample objects it ships with, using its own writer, so the reader is always exercised
 * against bytes this file can also print. Little-endian, ELF64, x86-64 — the one shape the
 * linker track targets.
 */
import type { Decoded, Field } from './field';

/* -------------------------------- constants ------------------------------- */

export const ELF_TYPES: Record<number, string> = {
	0: 'ET_NONE',
	1: 'ET_REL (relocatable object)',
	2: 'ET_EXEC (executable)',
	3: 'ET_DYN (shared object or PIE)',
	4: 'ET_CORE'
};

export const MACHINES: Record<number, string> = {
	0x3e: 'EM_X86_64',
	0xb7: 'EM_AARCH64',
	0xf3: 'EM_RISCV',
	0x28: 'EM_ARM'
};

export const SECTION_TYPES: Record<number, string> = {
	0: 'SHT_NULL',
	1: 'SHT_PROGBITS',
	2: 'SHT_SYMTAB',
	3: 'SHT_STRTAB',
	4: 'SHT_RELA',
	5: 'SHT_HASH',
	6: 'SHT_DYNAMIC',
	7: 'SHT_NOTE',
	8: 'SHT_NOBITS',
	9: 'SHT_REL',
	11: 'SHT_DYNSYM',
	14: 'SHT_INIT_ARRAY',
	15: 'SHT_FINI_ARRAY',
	17: 'SHT_GROUP',
	18: 'SHT_SYMTAB_SHNDX'
};

const SECTION_FLAGS: [bigint, string, string][] = [
	[0x1n, 'W', 'writable'],
	[0x2n, 'A', 'occupies memory at run time'],
	[0x4n, 'X', 'executable'],
	[0x10n, 'M', 'mergeable'],
	[0x20n, 'S', 'null-terminated strings'],
	[0x40n, 'I', 'sh_info is a section index'],
	[0x200n, 'G', 'part of a COMDAT group'],
	[0x400n, 'T', 'thread-local storage']
];

export const SYMBOL_BINDINGS: Record<number, string> = {
	0: 'LOCAL',
	1: 'GLOBAL',
	2: 'WEAK',
	10: 'GNU_UNIQUE'
};

export const SYMBOL_TYPES: Record<number, string> = {
	0: 'NOTYPE',
	1: 'OBJECT',
	2: 'FUNC',
	3: 'SECTION',
	4: 'FILE',
	5: 'COMMON',
	6: 'TLS',
	10: 'GNU_IFUNC'
};

export const SYMBOL_VISIBILITY: Record<number, string> = {
	0: 'DEFAULT',
	1: 'INTERNAL',
	2: 'HIDDEN',
	3: 'PROTECTED'
};

export const SEGMENT_TYPES: Record<number, string> = {
	0: 'PT_NULL',
	1: 'PT_LOAD',
	2: 'PT_DYNAMIC',
	3: 'PT_INTERP',
	4: 'PT_NOTE',
	6: 'PT_PHDR',
	7: 'PT_TLS',
	0x6474e550: 'PT_GNU_EH_FRAME',
	0x6474e551: 'PT_GNU_STACK',
	0x6474e552: 'PT_GNU_RELRO'
};

/** The static x86-64 relocation types a linker for this track has to implement. */
export const RELOC_TYPES: Record<number, { name: string; formula: string }> = {
	0: { name: 'R_X86_64_NONE', formula: 'nothing' },
	1: { name: 'R_X86_64_64', formula: 'S + A, 64 bits' },
	2: { name: 'R_X86_64_PC32', formula: 'S + A − P, 32 bits, must fit signed' },
	3: { name: 'R_X86_64_GOT32', formula: 'G + A' },
	4: { name: 'R_X86_64_PLT32', formula: 'L + A − P, 32 bits — a call to a function that may go via a PLT' },
	9: { name: 'R_X86_64_GOTPCREL', formula: 'G + GOT + A − P' },
	10: { name: 'R_X86_64_32', formula: 'S + A, 32 bits, zero-extended — overflows if the address is above 4 GiB' },
	11: { name: 'R_X86_64_32S', formula: 'S + A, 32 bits, sign-extended' },
	12: { name: 'R_X86_64_16', formula: 'S + A, 16 bits' },
	13: { name: 'R_X86_64_PC16', formula: 'S + A − P, 16 bits' },
	14: { name: 'R_X86_64_8', formula: 'S + A, 8 bits' },
	15: { name: 'R_X86_64_PC8', formula: 'S + A − P, 8 bits' },
	24: { name: 'R_X86_64_PC64', formula: 'S + A − P, 64 bits' },
	42: { name: 'R_X86_64_REX_GOTPCRELX', formula: 'G + GOT + A − P, relaxable to a LEA' }
};

const SPECIAL_SECTIONS: Record<number, string> = {
	0: 'SHN_UNDEF',
	0xfff1: 'SHN_ABS',
	0xfff2: 'SHN_COMMON'
};

/* --------------------------------- reading -------------------------------- */

export interface ElfSection {
	index: number;
	name: string;
	type: string;
	flags: string;
	addr: bigint;
	offset: number;
	size: number;
	link: number;
	info: number;
	entsize: number;
	align: number;
}

export interface ElfSymbol {
	index: number;
	name: string;
	bind: string;
	type: string;
	visibility: string;
	section: string;
	value: bigint;
	size: number;
}

export interface ElfRelocation {
	/** Which section the relocation patches (`.text`, `.data`). */
	section: string;
	offset: bigint;
	type: string;
	formula: string;
	symbol: string;
	addend: bigint;
}

export interface ElfSegment {
	type: string;
	flags: string;
	offset: number;
	vaddr: bigint;
	filesz: number;
	memsz: number;
	align: number;
}

export interface ElfFile extends Decoded {
	valid: boolean;
	class: string;
	endianness: string;
	fileType: string;
	machine: string;
	entry: bigint;
	sections: ElfSection[];
	segments: ElfSegment[];
	symbols: ElfSymbol[];
	relocations: ElfRelocation[];
}

class Cursor {
	offset = 0;
	depth = 0;
	readonly fields: Field[] = [];
	readonly view: DataView;

	constructor(readonly bytes: Uint8Array) {
		this.view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
	}

	field(name: string, type: string, start: number, end: number, value: string, note?: string) {
		const f: Field = { name, type, start, end, value, depth: this.depth };
		if (note) f.note = note;
		this.fields.push(f);
	}

	need(n: number, what: string) {
		if (this.bytes.length - this.offset < n) {
			throw new Error(`${what}: wanted ${n} bytes, ${this.bytes.length - this.offset} left`);
		}
	}

	u8(name: string, format: (v: number) => string = String, note?: string): number {
		this.need(1, name);
		const start = this.offset;
		const v = this.view.getUint8(start);
		this.offset += 1;
		this.field(name, 'u8', start, this.offset, format(v), note);
		return v;
	}

	/** Little-endian, which is what ELF on x86-64 always is. */
	u16(name: string, format: (v: number) => string = String, note?: string): number {
		this.need(2, name);
		const start = this.offset;
		const v = this.view.getUint16(start, true);
		this.offset += 2;
		this.field(name, 'Elf64_Half', start, this.offset, format(v), note);
		return v;
	}

	u32(name: string, format: (v: number) => string = String, note?: string): number {
		this.need(4, name);
		const start = this.offset;
		const v = this.view.getUint32(start, true);
		this.offset += 4;
		this.field(name, 'Elf64_Word', start, this.offset, format(v), note);
		return v;
	}

	u64(name: string, format: (v: bigint) => string = (v) => `0x${v.toString(16)}`, note?: string): bigint {
		this.need(8, name);
		const start = this.offset;
		const v = this.view.getBigUint64(start, true);
		this.offset += 8;
		this.field(name, 'Elf64_Xword', start, this.offset, format(v), note);
		return v;
	}
}

function flagLetters(flags: bigint): string {
	const letters = SECTION_FLAGS.filter(([bit]) => (flags & bit) !== 0n).map(([, letter]) => letter);
	return letters.length ? letters.join('') : '—';
}

function flagWords(flags: bigint): string {
	const words = SECTION_FLAGS.filter(([bit]) => (flags & bit) !== 0n).map(([, , word]) => word);
	return words.length ? words.join(', ') : 'no flags';
}

function stringAt(bytes: Uint8Array, table: { offset: number; size: number } | null, index: number): string {
	if (!table) return `@${index}`;
	const start = table.offset + index;
	if (start >= bytes.length) return `@${index}`;
	let end = start;
	while (end < bytes.length && end < table.offset + table.size && bytes[end] !== 0) end++;
	return new TextDecoder().decode(bytes.subarray(start, end));
}

/**
 * Parse an ELF64 file. Never throws: a truncated object comes back with everything that
 * did parse plus the error, because that is exactly the state a half-written linker
 * produces.
 */
export function parseElf(bytes: Uint8Array): ElfFile {
	const c = new Cursor(bytes);
	const out: ElfFile = {
		fields: [],
		valid: false,
		class: '',
		endianness: '',
		fileType: '',
		machine: '',
		entry: 0n,
		sections: [],
		segments: [],
		symbols: [],
		relocations: []
	};
	try {
		c.need(64, 'the ELF header');
		const magicOk = bytes[0] === 0x7f && bytes[1] === 0x45 && bytes[2] === 0x4c && bytes[3] === 0x46;
		c.field('e_ident.magic', 'u8[4]', 0, 4, '7f 45 4c 46  "\\x7fELF"',
			'Four bytes, and the only thing every ELF file in the world agrees on.');
		c.offset = 4;
		if (!magicOk) throw new Error('bad magic: this is not an ELF file');
		const klass = c.u8('e_ident.class', (v) => (v === 2 ? '2 (ELFCLASS64)' : v === 1 ? '1 (ELFCLASS32)' : String(v)));
		out.class = klass === 2 ? 'ELF64' : klass === 1 ? 'ELF32' : `unknown (${klass})`;
		const data = c.u8('e_ident.data', (v) => (v === 1 ? '1 (little-endian)' : v === 2 ? '2 (big-endian)' : String(v)));
		out.endianness = data === 1 ? 'little-endian' : data === 2 ? 'big-endian' : `unknown (${data})`;
		c.u8('e_ident.version');
		c.u8('e_ident.osabi', (v) => (v === 0 ? '0 (System V)' : v === 3 ? '3 (Linux)' : String(v)));
		c.u8('e_ident.abiversion');
		c.field('e_ident.pad', 'u8[7]', c.offset, c.offset + 7, 'zero', 'Seven reserved bytes that must be zero.');
		c.offset += 7;
		if (klass !== 2) throw new Error('this reader only understands ELF64 (class 2)');

		const type = c.u16('e_type', (v) => `${v} (${ELF_TYPES[v] ?? 'unknown'})`,
			'ET_REL is what a compiler emits and a linker reads; ET_EXEC is what a linker writes.');
		out.fileType = ELF_TYPES[type] ?? `unknown (${type})`;
		const machine = c.u16('e_machine', (v) => `${v} (${MACHINES[v] ?? 'unknown'})`);
		out.machine = MACHINES[machine] ?? `unknown (${machine})`;
		c.u32('e_version');
		out.entry = c.u64('e_entry', (v) => (v === 0n ? '0 (none — a relocatable object has no entry point)' : `0x${v.toString(16)}`),
			'Where the kernel starts executing. A linker computes it from the entry symbol, `_start` unless -e says otherwise.');
		const phoff = Number(c.u64('e_phoff'));
		const shoff = Number(c.u64('e_shoff', (v) => `0x${v.toString(16)}`,
			'Where the section header table starts. Everything a linker reads hangs off this one offset.'));
		c.u32('e_flags');
		c.u16('e_ehsize', (v) => `${v} bytes`);
		const phentsize = c.u16('e_phentsize', (v) => `${v} bytes`);
		const phnum = c.u16('e_phnum');
		const shentsize = c.u16('e_shentsize', (v) => `${v} bytes`);
		const shnum = c.u16('e_shnum');
		const shstrndx = c.u16('e_shstrndx', String,
			'The index of the section whose contents are the section *names*. Without it every section is anonymous.');

		/* ---- program headers ---- */
		if (phnum > 0 && phoff > 0) {
			for (let i = 0; i < phnum; i++) {
				c.offset = phoff + i * phentsize;
				if (c.offset + 56 > bytes.length) break;
				c.depth = 1;
				const ptype = c.u32(`phdr[${i}].p_type`, (v) => `${v} (${SEGMENT_TYPES[v] ?? 'unknown'})`);
				const pflags = c.u32(`phdr[${i}].p_flags`, (v) =>
					`${(v & 4 ? 'R' : '') + (v & 2 ? 'W' : '') + (v & 1 ? 'X' : '') || '—'}`);
				const poffset = Number(c.u64(`phdr[${i}].p_offset`));
				const vaddr = c.u64(`phdr[${i}].p_vaddr`);
				c.u64(`phdr[${i}].p_paddr`);
				const filesz = Number(c.u64(`phdr[${i}].p_filesz`));
				const memsz = Number(c.u64(`phdr[${i}].p_memsz`, (v) => String(v),
					'Bigger than p_filesz exactly when the segment has a .bss — the kernel zero-fills the difference.'));
				const align = Number(c.u64(`phdr[${i}].p_align`));
				out.segments.push({
					type: SEGMENT_TYPES[ptype] ?? `0x${ptype.toString(16)}`,
					flags: (pflags & 4 ? 'R' : '') + (pflags & 2 ? 'W' : '') + (pflags & 1 ? 'X' : '') || '—',
					offset: poffset,
					vaddr,
					filesz,
					memsz,
					align
				});
			}
		}

		/* ---- section headers ---- */
		const rawSections: { nameIndex: number; type: number; offset: number; size: number; link: number; info: number; entsize: number }[] = [];
		if (shnum > 0 && shoff > 0) {
			for (let i = 0; i < shnum; i++) {
				c.offset = shoff + i * shentsize;
				if (c.offset + 64 > bytes.length) throw new Error(`section header ${i} runs past the end of the file`);
				c.depth = 1;
				const nameIndex = c.u32(`shdr[${i}].sh_name`, (v) => `${v} (an offset into .shstrtab)`);
				const stype = c.u32(`shdr[${i}].sh_type`, (v) => `${v} (${SECTION_TYPES[v] ?? 'unknown'})`);
				const flags = c.u64(`shdr[${i}].sh_flags`, (v) => `${flagLetters(v)} — ${flagWords(v)}`);
				const addr = c.u64(`shdr[${i}].sh_addr`);
				const offset = Number(c.u64(`shdr[${i}].sh_offset`));
				const size = Number(c.u64(`shdr[${i}].sh_size`));
				const link = c.u32(`shdr[${i}].sh_link`, String,
					'For a symbol table this is its string table; for a relocation section it is the symbol table it indexes.');
				const info = c.u32(`shdr[${i}].sh_info`, String,
					'For a relocation section this is the section being patched; for a symbol table it is the index of the first non-local symbol.');
				const align = Number(c.u64(`shdr[${i}].sh_addralign`));
				const entsize = Number(c.u64(`shdr[${i}].sh_entsize`));
				rawSections.push({ nameIndex, type: stype, offset, size, link, info, entsize });
				out.sections.push({
					index: i,
					name: '',
					type: SECTION_TYPES[stype] ?? `0x${stype.toString(16)}`,
					flags: flagLetters(flags),
					addr,
					offset,
					size,
					link,
					info,
					entsize,
					align
				});
			}
		}

		const shstr = rawSections[shstrndx] ? { offset: rawSections[shstrndx].offset, size: rawSections[shstrndx].size } : null;
		out.sections.forEach((s, i) => {
			s.name = stringAt(bytes, shstr, rawSections[i].nameIndex);
		});

		/* ---- symbols ---- */
		for (let i = 0; i < rawSections.length; i++) {
			const raw = rawSections[i];
			if (raw.type !== 2 && raw.type !== 11) continue;
			const strTab = rawSections[raw.link] ? { offset: rawSections[raw.link].offset, size: rawSections[raw.link].size } : null;
			const entsize = raw.entsize || 24;
			const count = Math.floor(raw.size / entsize);
			for (let n = 0; n < count; n++) {
				const at = raw.offset + n * entsize;
				if (at + 24 > bytes.length) break;
				const nameIndex = c.view.getUint32(at, true);
				const info = c.view.getUint8(at + 4);
				const other = c.view.getUint8(at + 5);
				const shndx = c.view.getUint16(at + 6, true);
				const value = c.view.getBigUint64(at + 8, true);
				const size = Number(c.view.getBigUint64(at + 16, true));
				const sectionName =
					SPECIAL_SECTIONS[shndx] ?? out.sections[shndx]?.name ?? `section ${shndx}`;
				out.symbols.push({
					index: n,
					name: stringAt(bytes, strTab, nameIndex) || (shndx !== 0 ? sectionName : '(unnamed)'),
					bind: SYMBOL_BINDINGS[info >> 4] ?? String(info >> 4),
					type: SYMBOL_TYPES[info & 0xf] ?? String(info & 0xf),
					visibility: SYMBOL_VISIBILITY[other & 3] ?? String(other & 3),
					section: sectionName,
					value,
					size
				});
			}
		}

		/* ---- relocations ---- */
		for (let i = 0; i < rawSections.length; i++) {
			const raw = rawSections[i];
			if (raw.type !== 4) continue;
			const target = out.sections[raw.info]?.name ?? `section ${raw.info}`;
			const symbolSection = rawSections[raw.link];
			const symStr = symbolSection && rawSections[symbolSection.link]
				? { offset: rawSections[symbolSection.link].offset, size: rawSections[symbolSection.link].size }
				: null;
			const entsize = raw.entsize || 24;
			const count = Math.floor(raw.size / entsize);
			for (let n = 0; n < count; n++) {
				const at = raw.offset + n * entsize;
				if (at + 24 > bytes.length) break;
				const offset = c.view.getBigUint64(at, true);
				const info = c.view.getBigUint64(at + 8, true);
				const addend = c.view.getBigInt64(at + 16, true);
				const symIndex = Number(info >> 32n);
				const relType = Number(info & 0xffffffffn);
				let symbolName = `symbol ${symIndex}`;
				if (symbolSection) {
					const symAt = symbolSection.offset + symIndex * (symbolSection.entsize || 24);
					if (symAt + 8 <= bytes.length) {
						const nameIndex = c.view.getUint32(symAt, true);
						const shndx = c.view.getUint16(symAt + 6, true);
						symbolName =
							stringAt(bytes, symStr, nameIndex) ||
							SPECIAL_SECTIONS[shndx] ||
							out.sections[shndx]?.name ||
							symbolName;
					}
				}
				const known = RELOC_TYPES[relType];
				out.relocations.push({
					section: target,
					offset,
					type: known?.name ?? `type ${relType}`,
					formula: known?.formula ?? 'unknown to this reader',
					symbol: symbolName,
					addend
				});
			}
		}

		out.valid = true;
	} catch (e) {
		out.error = e instanceof Error ? e.message : 'could not read this ELF file';
	}
	c.fields.sort((a, b) => a.start - b.start);
	out.fields = c.fields;
	return out;
}

/* ------------------------------ sample objects ----------------------------- */

class Buf {
	bytes: number[] = [];
	get length() {
		return this.bytes.length;
	}
	u8(v: number) {
		this.bytes.push(v & 0xff);
		return this;
	}
	u16(v: number) {
		this.bytes.push(v & 0xff, (v >> 8) & 0xff);
		return this;
	}
	u32(v: number) {
		this.bytes.push(v & 0xff, (v >> 8) & 0xff, (v >> 16) & 0xff, (v >>> 24) & 0xff);
		return this;
	}
	u64(v: bigint | number) {
		let x = BigInt(v);
		for (let i = 0; i < 8; i++) {
			this.bytes.push(Number(x & 0xffn));
			x >>= 8n;
		}
		return this;
	}
	raw(v: ArrayLike<number>) {
		for (let i = 0; i < v.length; i++) this.bytes.push(v[i] & 0xff);
		return this;
	}
	pad(to: number) {
		while (this.bytes.length % to !== 0) this.bytes.push(0);
		return this;
	}
}

interface SectionPlan {
	name: string;
	type: number;
	flags: bigint;
	data: number[];
	link?: number;
	info?: number;
	entsize?: number;
	align?: number;
	addr?: bigint;
	/** For SHT_NOBITS, which has a size in memory but no bytes in the file. */
	size?: number;
}

/** Assemble a whole ELF64 file from section plans — headers last, once offsets are known. */
function buildElf(type: number, entry: bigint, plans: SectionPlan[]): Uint8Array {
	const strtab = new Buf().u8(0);
	const nameOffsets = new Map<string, number>();
	nameOffsets.set('', 0);
	const all: SectionPlan[] = [{ name: '', type: 0, flags: 0n, data: [] }, ...plans, {
		name: '.shstrtab',
		type: 3,
		flags: 0n,
		data: []
	}];
	for (const s of all) {
		if (s.name === '' || nameOffsets.has(s.name)) continue;
		nameOffsets.set(s.name, strtab.length);
		strtab.raw(new TextEncoder().encode(s.name)).u8(0);
	}
	all[all.length - 1].data = strtab.bytes;

	const body = new Buf();
	body.raw(new Array(64).fill(0)); // the header goes in later
	const offsets: number[] = [];
	for (const s of all) {
		if (s.type === 0) {
			offsets.push(0);
			continue;
		}
		body.pad(s.align && s.align > 1 ? s.align : 8);
		offsets.push(body.length);
		body.raw(s.data);
	}
	body.pad(8);
	const shoff = body.length;
	all.forEach((s, i) => {
		body
			.u32(nameOffsets.get(s.name) ?? 0)
			.u32(s.type)
			.u64(s.flags)
			.u64(s.addr ?? 0n)
			.u64(offsets[i])
			.u64(s.type === 0 ? 0 : (s.size ?? s.data.length))
			.u32(s.link ?? 0)
			.u32(s.info ?? 0)
			.u64(s.align ?? 1)
			.u64(s.entsize ?? 0);
	});

	const header = new Buf();
	header
		.raw([0x7f, 0x45, 0x4c, 0x46, 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0])
		.u16(type)
		.u16(0x3e) // EM_X86_64
		.u32(1)
		.u64(entry)
		.u64(0) // e_phoff
		.u64(shoff)
		.u32(0)
		.u16(64)
		.u16(56)
		.u16(0)
		.u16(64)
		.u16(all.length)
		.u16(all.length - 1); // .shstrtab is last
	for (let i = 0; i < 64; i++) body.bytes[i] = header.bytes[i] ?? 0;
	return Uint8Array.from(body.bytes);
}

function symbol(nameOffset: number, info: number, shndx: number, value: bigint, size: number): number[] {
	return new Buf().u32(nameOffset).u8(info).u8(0).u16(shndx).u64(value).u64(size).bytes;
}

function rela(offset: number, symIndex: number, type: number, addend: number): number[] {
	return new Buf().u64(offset).u64((BigInt(symIndex) << 32n) | BigInt(type)).u64(BigInt(addend)).bytes;
}

/**
 * `main.o`: calls `puts("hi")` and returns 0. It has exactly the three things a linker has
 * to deal with — an undefined symbol, a PLT32 call and a 64-bit reference into `.rodata`.
 */
function mainObject(): Uint8Array {
	const text = [
		0x55, // push rbp
		0x48, 0x89, 0xe5, // mov rbp, rsp
		0x48, 0xbf, 0, 0, 0, 0, 0, 0, 0, 0, // movabs rdi, <message>   (R_X86_64_64 at +6)
		0xe8, 0, 0, 0, 0, // call puts                                 (R_X86_64_PLT32 at +15)
		0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0
		0x5d, // pop rbp
		0xc3 // ret
	];
	const rodata = [...new TextEncoder().encode('hello from the garden'), 0];

	const strtab = new Buf().u8(0);
	const names = new Map<string, number>();
	for (const n of ['main.c', 'message', 'main', 'puts']) {
		names.set(n, strtab.length);
		strtab.raw(new TextEncoder().encode(n)).u8(0);
	}

	// Section indices: 1 .text, 2 .rela.text, 3 .rodata, 4 .symtab, 5 .strtab
	const symbols = [
		...symbol(0, 0, 0, 0n, 0), // the mandatory null symbol
		...symbol(names.get('main.c')!, 4, 0xfff1, 0n, 0), // FILE
		...symbol(0, 3, 1, 0n, 0), // SECTION .text
		...symbol(0, 3, 3, 0n, 0), // SECTION .rodata
		...symbol(names.get('message')!, 1 | (1 << 4), 3, 0n, rodata.length), // GLOBAL OBJECT
		...symbol(names.get('main')!, 2 | (1 << 4), 1, 0n, text.length), // GLOBAL FUNC
		...symbol(names.get('puts')!, 0 | (1 << 4), 0, 0n, 0) // GLOBAL NOTYPE, undefined
	];
	const relocations = [
		...rela(6, 4, 1, 0), // R_X86_64_64 against `message`
		...rela(15, 6, 4, -4) // R_X86_64_PLT32 against `puts`
	];

	return buildElf(1, 0n, [
		{ name: '.text', type: 1, flags: 0x6n, data: text, align: 16 },
		{ name: '.rela.text', type: 4, flags: 0x40n, data: relocations, link: 4, info: 1, entsize: 24, align: 8 },
		{ name: '.rodata', type: 1, flags: 0x2n, data: rodata, align: 1 },
		{ name: '.symtab', type: 2, flags: 0n, data: symbols, link: 5, info: 4, entsize: 24, align: 8 },
		{ name: '.strtab', type: 3, flags: 0n, data: strtab.bytes, align: 1 }
	]);
}

/** `puts.o`: the definition `main.o` is missing, plus a `.bss` and a weak symbol. */
function libObject(): Uint8Array {
	const text = [0x48, 0x31, 0xc0, 0xc3]; // xor rax, rax ; ret
	const strtab = new Buf().u8(0);
	const names = new Map<string, number>();
	for (const n of ['puts.c', 'puts', 'buffer', 'on_exit_hook']) {
		names.set(n, strtab.length);
		strtab.raw(new TextEncoder().encode(n)).u8(0);
	}
	// 1 .text, 2 .bss, 3 .symtab, 4 .strtab
	const symbols = [
		...symbol(0, 0, 0, 0n, 0),
		...symbol(names.get('puts.c')!, 4, 0xfff1, 0n, 0),
		...symbol(0, 3, 1, 0n, 0),
		...symbol(names.get('puts')!, 2 | (1 << 4), 1, 0n, text.length), // GLOBAL FUNC
		...symbol(names.get('buffer')!, 1 | (1 << 4), 2, 0n, 4096), // GLOBAL OBJECT in .bss
		...symbol(names.get('on_exit_hook')!, 2 | (2 << 4), 1, 0n, 0) // WEAK FUNC
	];
	return buildElf(1, 0n, [
		{ name: '.text', type: 1, flags: 0x6n, data: text, align: 16 },
		{ name: '.bss', type: 8, flags: 0x3n, data: [], size: 4096, align: 32 },
		{ name: '.symtab', type: 2, flags: 0n, data: symbols, link: 4, info: 3, entsize: 24, align: 8 },
		{ name: '.strtab', type: 3, flags: 0n, data: strtab.bytes, align: 1 }
	]);
}

export interface ElfSample {
	id: string;
	label: string;
	blurb: string;
	/** What the linked program prints, when this sample is part of a program. */
	runs: string | null;
	bytes: Uint8Array;
}

export const elfSamples: ElfSample[] = [
	{
		id: 'main-o',
		label: 'main.o',
		blurb:
			'A relocatable object with everything a linker trips over first: a .text with two holes in it, a .rela.text that says how to fill them, a string in .rodata, and `puts` left undefined for someone else to define.',
		runs: 'hello from the garden',
		bytes: mainObject()
	},
	{
		id: 'puts-o',
		label: 'puts.o',
		blurb:
			'The other half: it defines `puts`, carries a 4 KiB .bss (SHT_NOBITS — size without bytes), and has a weak symbol that link order is allowed to leave undefined.',
		runs: null,
		bytes: libObject()
	}
];

/** Accepts `"7f 45 4c 46"`, `"7f454c46"`, `0x`-prefixed and newline-separated dumps. */
export function parseElfHex(input: string): Uint8Array {
	const cleaned = input.replace(/0x/gi, '').replace(/[\s,:_|]+/g, '');
	if (cleaned.length === 0) return new Uint8Array();
	if (cleaned.length % 2 !== 0) throw new Error('That is an odd number of hex digits.');
	if (/[^0-9a-f]/i.test(cleaned)) throw new Error('That has characters that are not hex digits.');
	const out = new Uint8Array(cleaned.length / 2);
	for (let i = 0; i < out.length; i++) out[i] = parseInt(cleaned.slice(i * 2, i * 2 + 2), 16);
	return out;
}
