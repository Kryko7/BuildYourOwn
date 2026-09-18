/**
 * The lab's instruments, and which stages each belongs to.
 *
 * One list, read by `/lab` (the tab strip) and by the stage drawer (the "Try it" box), so
 * a tool is described once and appears in both. A track with no instrument yet simply has
 * none — `toolsForTrack` returns an empty list and the drawer draws nothing.
 */
import type { Component } from 'svelte';
import Tokenizer from './Tokenizer.svelte';
import WireInspector from './WireInspector.svelte';
import BatchBuilder from './BatchBuilder.svelte';
import WasmInspector from './WasmInspector.svelte';
import TlsInspector from './TlsInspector.svelte';
import ElfViewer from './ElfViewer.svelte';
import Replay from './Replay.svelte';
import { sectionOf } from '$lib/catalog';
import { tracks, type TrackId } from '$lib/tracks';

/** Every instrument takes an optional starting sample and nothing else. */
export type LabComponent = Component<{ initialSample?: string }>;

export interface LabTool {
	id: string;
	label: string;
	blurb: string;
	/** The track it belongs to, or null for the ones that serve every track. */
	track: TrackId | null;
	component: LabComponent;
	/** Where to link for "the stage this is about". */
	stageHref: string;
	/**
	 * Which stages embed it, and with which sample. Returning null means "not on this
	 * stage"; returning a string (possibly empty) means "embed me, starting here".
	 *
	 * The section letter is passed in as well as the number, because a tester that
	 * renumbers its stages must not silently change which instrument shows up: the
	 * sections are the stable thing.
	 */
	forStage?: (stage: number, section: string) => string | null;
}

export const labTools: LabTool[] = [
	{
		id: 'tokenizer',
		label: 'Shell tokenizer',
		track: 'shell',
		blurb:
			'Type a command line and watch the quote state machine run: every character coloured by the state that produced it, the words that survive quote removal, and the pipeline and redirection graph.',
		component: Tokenizer as LabComponent,
		stageHref: '/shell/13',
		forStage: (n) =>
			(n >= 13 && n <= 29) || (n >= 36 && n <= 40) || [49, 52, 53, 57].includes(n) ? '' : null
	},
	{
		id: 'wire',
		label: 'Kafka wire inspector',
		track: 'kafka',
		blurb:
			'Four real requests, byte by byte. Hover a byte to find its field, hover a field to find its bytes; varints, compact arrays and tagged fields are spelled out. Paste your own hex to debug a frame.',
		component: WireInspector as LabComponent,
		stageHref: '/kafka/5',
		forStage: (n: number) => {
			if ([23, 24, 27, 31, 33, 34, 35, 36, 37].includes(n)) return null; // the batch builder wins
			if (n < 2) return null;
			return n >= 29
				? 'produce-v11'
				: n >= 19
					? 'fetch-v16'
					: n >= 10
						? 'describetopicpartitions-v0'
						: 'apiversions-v4';
		}
	},
	{
		id: 'batch',
		label: 'RecordBatch anatomy',
		track: 'kafka',
		blurb:
			'Build a v2 record batch from records and watch the bytes appear, with a live CRC32C and the attribute bits that select a compression codec.',
		component: BatchBuilder as LabComponent,
		stageHref: '/kafka/34',
		forStage: (n) => ([23, 24, 27, 31, 33, 34, 35, 36, 37].includes(n) ? '' : null)
	},
	{
		id: 'wasm',
		label: 'Wasm module inspector',
		track: 'wasm',
		blurb:
			'Pick a module or paste one: the section tree, every LEB128 value next to the bytes it came from, the type and export tables, and each function body disassembled to instruction names.',
		component: WasmInspector as LabComponent,
		stageHref: '/wasm/1',
		// Every stage of the track is about bytes in a module, so the inspector is on all of
		// them; the sample follows the section's topic.
		forStage: (_n, section) =>
			section === 'G' || section === 'E' ? 'wasi-hello' : section === 'C' || section === 'H' ? 'div' : 'add'
	},
	{
		id: 'tls',
		label: 'TLS record inspector',
		track: 'tls',
		blurb:
			'Record framing, handshake messages and every extension by name — plus the RFC 8446 key schedule walked label by label, with the exact HkdfLabel bytes each derivation expands.',
		component: TlsInspector as LabComponent,
		stageHref: '/tls/1',
		forStage: (_n, section) =>
			section === 'F' ? 'alerts' : section >= 'C' && section <= 'E' ? 'server-flight' : 'client-hello'
	},
	{
		id: 'elf',
		label: 'ELF viewer',
		track: 'link',
		blurb:
			'A relocatable object, read the way a linker reads it: the file header, the section table, the symbols with their bindings, and the relocations with the formula each one applies.',
		component: ElfViewer as LabComponent,
		stageHref: '/link/1',
		forStage: (_n, section) => (section === 'C' || section === 'E' ? 'puts-o' : 'main-o')
	},
	{
		id: 'replay',
		label: 'Journey replay',
		track: null,
		blurb:
			'Step through a failing test from your latest run: the input it sent, the output it got, and the diff against what the suite expected.',
		component: Replay as LabComponent,
		stageHref: '/progress'
	}
];

export function toolById(id: string): LabTool | undefined {
	return labTools.find((t) => t.id === id);
}

export function toolsForTrack(track: TrackId): LabTool[] {
	return labTools.filter((t) => t.track === track);
}

/** The instrument to embed in one stage's drawer, with the sample it should open on. */
export function toolForStage(track: TrackId, stage: number): { tool: LabTool; sample: string } | null {
	const section = sectionOf(track, stage)?.id ?? '';
	for (const tool of toolsForTrack(track)) {
		const sample = tool.forStage?.(stage, section);
		if (sample === null || sample === undefined) continue;
		return { tool, sample };
	}
	return null;
}

/** The accent a tool is drawn in — its track's, or the alert red for the shared ones. */
export function accentFor(tool: LabTool): string {
	return tool.track ? tracks[tool.track].accent : 'var(--bad)';
}
