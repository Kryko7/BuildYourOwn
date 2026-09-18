// @vitest-environment jsdom
/**
 * The three new instruments, mounted for real. A lab tool that throws during hydration
 * takes the whole page with it (that is how the lab broke the first time), so every test
 * here fails on any console error or warning as well as on its own assertions.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { mount, unmount, flushSync } from 'svelte';
import WasmInspector from './WasmInspector.svelte';
import TlsInspector from './TlsInspector.svelte';
import ElfViewer from './ElfViewer.svelte';
import HexFields from './HexFields.svelte';
import StageLab from './StageLab.svelte';
import { labTools, toolForStage, toolsForTrack, accentFor } from './tools';
import { catalogs, tracks, trackIds } from '$lib/catalog';
import type { Field } from '$lib/lab/field';

let host: HTMLDivElement;
let noise: unknown[] = [];

beforeEach(() => {
	host = document.createElement('div');
	document.body.appendChild(host);
	noise = [];
	vi.spyOn(console, 'error').mockImplementation((...args) => noise.push(args));
	vi.spyOn(console, 'warn').mockImplementation((...args) => noise.push(args));
});

afterEach(() => {
	host.remove();
	vi.restoreAllMocks();
});

function type(selector: string, value: string) {
	const el = host.querySelector<HTMLTextAreaElement>(selector)!;
	el.value = value;
	el.dispatchEvent(new Event('input', { bubbles: true }));
	flushSync();
}

function buttonWith(text: string): HTMLButtonElement | undefined {
	return [...host.querySelectorAll<HTMLButtonElement>('button')].find((b) =>
		b.textContent?.includes(text)
	);
}

describe('HexFields', () => {
	const fields: Field[] = [
		{ name: 'header', type: 'u8[2]', start: 0, end: 2, value: 'hi', depth: 0 },
		{ name: 'header.first', type: 'u8', start: 0, end: 1, value: 'h', depth: 1, note: 'a note' }
	];

	it('draws a byte per byte and selects the innermost field under one', () => {
		const app = mount(HexFields, {
			target: host,
			props: { bytes: Uint8Array.from([1, 2, 3]), fields }
		});
		flushSync();
		const bytes = [...host.querySelectorAll<HTMLElement>('.b')];
		expect(bytes).toHaveLength(3);
		expect(bytes[0].classList.contains('known')).toBe(true);
		expect(bytes[2].classList.contains('known')).toBe(false);

		bytes[0].dispatchEvent(new MouseEvent('mouseenter'));
		flushSync();
		// byte 0 is covered by both fields; the deeper one wins, so only byte 0 lights up
		expect(host.querySelectorAll('.b.hot')).toHaveLength(1);
		expect(host.querySelector('.fields li.on')?.textContent).toContain('header.first');
		expect(host.textContent).toContain('a note');
		unmount(app);
		expect(noise).toEqual([]);
	});

	it('says so rather than drawing an empty grid', () => {
		const app = mount(HexFields, {
			target: host,
			props: { bytes: new Uint8Array(), fields: [], emptyText: 'Nothing here.' }
		});
		flushSync();
		expect(host.textContent).toContain('Nothing here.');
		expect(host.textContent).toContain('Nothing decoded yet.');
		unmount(app);
		expect(noise).toEqual([]);
	});
});

describe('the wasm module inspector', () => {
	it('shows the section tree, the exports and a disassembled body', () => {
		const app = mount(WasmInspector, { target: host });
		flushSync();
		expect(host.textContent).toContain('41 bytes');
		expect(host.textContent).toContain('version 1');
		const sections = [...host.querySelectorAll('.sections li')].map((li) => li.textContent);
		expect(sections.join(' ')).toContain('type');
		expect(sections.join(' ')).toContain('code');
		expect(host.querySelectorAll('.listing li').length).toBe(4);
		expect(host.textContent).toContain('local.get');
		// the expected invocation and its output are shown next to the module
		expect(host.textContent).toContain('run --invoke add add.wasm 2 3');
		unmount(app);
		expect(noise).toEqual([]);
	});

	it('switches samples and shows the WASI import and data segment', () => {
		const app = mount(WasmInspector, { target: host, props: { initialSample: 'wasi-hello' } });
		flushSync();
		expect(host.textContent).toContain('wasi_snapshot_preview1.fd_write');
		expect(host.textContent).toContain('hello, garden');
		expect(host.textContent).toContain('memory: min 1 page');
		unmount(app);
		expect(noise).toEqual([]);
	});

	it('reports a pasted module that is not one, without breaking', () => {
		const app = mount(WasmInspector, { target: host });
		flushSync();
		buttonWith('paste a module')!.click();
		flushSync();
		type('textarea', 'de ad be ef 00 00 00 00');
		expect(host.textContent).toContain('bad magic');
		type('textarea', 'de ad be e');
		expect(host.textContent).toContain('odd number of hex digits');
		type('textarea', '00 61 73 6d 01 00 00 00');
		expect(host.textContent).toContain('0 sections');
		unmount(app);
		expect(noise).toEqual([]);
	});
});

describe('the TLS inspector', () => {
	it('lists the records, the handshake and every extension by name', () => {
		const app = mount(TlsInspector, { target: host });
		flushSync();
		expect(host.textContent).toContain('client_hello');
		expect(host.textContent).toContain('server_name');
		expect(host.textContent).toContain('garden.localhost');
		expect(host.textContent).toContain('TLS_AES_128_GCM_SHA256');
		expect(host.querySelectorAll('.records > li').length).toBe(1);
		unmount(app);
		expect(noise).toEqual([]);
	});

	it('opens a key-schedule step and shows the HkdfLabel bytes it expands', () => {
		const app = mount(TlsInspector, { target: host });
		flushSync();
		expect(host.textContent).toContain('The key schedule');
		const step = buttonWith('client_handshake_traffic_secret')!;
		step.click();
		flushSync();
		expect(host.textContent).toContain('ClientHello … ServerHello');
		// "tls13 c hs traffic" is 18 bytes: 0x12
		expect(host.querySelector('.bytes')?.textContent).toContain('12 74 6c 73 31 33 20');
		unmount(app);
		expect(noise).toEqual([]);
	});

	it('shows the server flight and an alert without inventing a handshake', () => {
		const app = mount(TlsInspector, { target: host, props: { initialSample: 'alerts' } });
		flushSync();
		expect(host.textContent).toContain('fatal handshake_failure');
		expect(host.textContent).toContain('warning close_notify');
		expect(host.querySelectorAll('.hs')).toHaveLength(0);
		unmount(app);
		expect(noise).toEqual([]);
	});
});

describe('the ELF viewer', () => {
	it('opens on the section table and can switch to symbols and relocations', () => {
		const app = mount(ElfViewer, { target: host });
		flushSync();
		expect(host.textContent).toContain('ET_REL');
		expect(host.textContent).toContain('.rela.text');

		const tabs = [...host.querySelectorAll<HTMLButtonElement>('[role="tab"]')];
		expect(tabs.map((t) => t.textContent?.trim())).toEqual([
			'sections (7)',
			'symbols (7)',
			'relocations (2)'
		]);

		tabs[1].click();
		flushSync();
		expect(host.textContent).toContain('SHN_UNDEF');
		// The null symbol is undefined too, so find the row that is actually `puts`.
		const undef = [...host.querySelectorAll('tr.undef')].map((r) => r.textContent ?? '');
		expect(undef.some((t) => t.includes('puts'))).toBe(true);

		tabs[2].click();
		flushSync();
		expect(host.textContent).toContain('R_X86_64_PLT32');
		// the formula is shown next to the type, not left as a number
		expect(host.textContent).toContain('L + A − P');
		expect(host.textContent).toContain('S + A, 64 bits');
		unmount(app);
		expect(noise).toEqual([]);
	});

	it('shows a weak symbol and a .bss with a size but no bytes', () => {
		const app = mount(ElfViewer, { target: host, props: { initialSample: 'puts-o' } });
		flushSync();
		expect(host.textContent).toContain('SHT_NOBITS');
		const tabs = [...host.querySelectorAll<HTMLButtonElement>('[role="tab"]')];
		tabs.find((t) => t.textContent?.includes('symbols'))!.click();
		flushSync();
		expect(host.querySelector('tr.weak')?.textContent).toContain('on_exit_hook');
		unmount(app);
		expect(noise).toEqual([]);
	});

	it('says what is wrong with a pasted file instead of showing an empty table', () => {
		const app = mount(ElfViewer, { target: host });
		flushSync();
		buttonWith('paste an object')!.click();
		flushSync();
		type('textarea', 'de ad be ef');
		expect(host.textContent).toContain('wanted 64 bytes');
		unmount(app);
		expect(noise).toEqual([]);
	});
});

describe('the lab tool registry', () => {
	it('gives every instrument a track that exists, or none at all', () => {
		for (const tool of labTools) {
			if (tool.track !== null) expect(trackIds).toContain(tool.track);
			expect(tool.blurb.length, tool.id).toBeGreaterThan(40);
			expect(tool.stageHref, tool.id).toMatch(/^\//);
			expect(accentFor(tool), tool.id).toMatch(/^var\(--/);
		}
		expect(new Set(labTools.map((t) => t.id)).size).toBe(labTools.length);
	});

	it('has an instrument for every track whose tester is being built here', () => {
		for (const track of ['shell', 'kafka', 'wasm', 'tls', 'link'] as const) {
			expect(toolsForTrack(track).length, track).toBeGreaterThan(0);
		}
	});

	it('picks one instrument per stage, and the same one for a whole section', () => {
		for (const track of trackIds) {
			const tools = toolsForTrack(track);
			for (const stage of catalogs[track].stages) {
				const chosen = toolForStage(track, stage.number);
				if (!tools.length) {
					expect(chosen, `${track} ${stage.number}`).toBeNull();
					continue;
				}
				if (chosen) expect(tools, `${track} ${stage.number}`).toContain(chosen.tool);
			}
		}
		// The wasm inspector opens on the WASI sample inside the WASI section.
		const wasi = catalogs.wasm.sections.find((s) => s.id === 'G');
		if (wasi?.stages.length) {
			expect(toolForStage('wasm', wasi.stages[0])?.sample).toBe('wasi-hello');
		}
	});

	it('embeds the right instrument in a stage, and nothing where there is none', () => {
		for (const track of trackIds) {
			const stage = catalogs[track].stages[0].number;
			const app = mount(StageLab, { target: host, props: { track, stage } });
			flushSync();
			const expected = toolForStage(track, stage);
			if (expected) {
				expect(host.textContent, track).toContain('Try it');
				expect(host.querySelector('.labbox'), track).not.toBeNull();
			} else {
				expect(host.textContent!.trim(), `${tracks[track].tester} has no instrument yet`).toBe('');
			}
			unmount(app);
			host.innerHTML = '';
		}
		expect(noise).toEqual([]);
	});
});
