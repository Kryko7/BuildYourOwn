/**
 * The track registry — the one list of tracks the whole site reads.
 *
 * Adding a track means adding an entry here (plus its generated catalog, its mascot and
 * its accent tokens in app.css); `params/track.ts`, the home page, the nav, the command
 * palette, the map, `/resources`, `/progress` and `npm run sync` all iterate this object
 * and none of them names a track (PLAN.md §5.6).
 *
 * Deliberately dependency-free: `scripts/sync-catalog.mjs` imports this file directly
 * (Node strips the types), so it must stay plain data with no `$lib` aliases, no Svelte
 * components and no imports at all. The mascots are named here and resolved to components
 * in `src/lib/components/garden/mascots.ts`.
 */

export const trackIds = ['shell', 'kafka', 'wasm', 'tls', 'link', 'dist'] as const;

export type TrackId = (typeof trackIds)[number];

/** Which example renderer a track's `examples[]` entries want. */
export type ExampleKind = 'transcript' | 'bytes';

export type MascotName = 'fox' | 'bunny' | 'owl' | 'hedgehog' | 'squirrel' | 'duckling' | 'cat';

/**
 * A track whose stages come in ladders (dist: `primitives` → `node` → `cluster`) declares
 * them here; the track page draws a rung per ladder and every stage says which it is on.
 * Tracks without ladders leave this out and nothing about them changes.
 */
export interface Ladder {
	id: string;
	title: string;
	/** One line on what this rung is for. */
	note: string;
	/** The section letters that belong to it. */
	sections: string[];
}

export interface SectionBadge {
	/** The garden name of the meadow this section is. */
	badge: string;
	icon: string;
}

export interface TrackMeta {
	id: TrackId;
	/** Full title: "Build your own WebAssembly runtime". */
	title: string;
	/** One or two words for the nav and the tab strips. */
	short: string;
	tagline: string;
	/** What the learner is building, as a noun phrase. */
	building: string;
	blurb: string;
	glyph: string;
	/** `var(--wasm)` — the accent tokens live in app.css, AA in both themes. */
	accent: string;
	accentBright: string;
	accentSoft: string;
	mascot: MascotName;
	/** What the mascot is, for alt text and the home page. */
	mascotLabel: string;
	/** The directory the tester lives in, relative to the repo root. */
	dir: string;
	tester: string;
	binary: string;
	targetFlag: string;
	targetExample: string;
	/** The YAML file that names registered targets, quoted next to the target input. */
	targetFile: string;
	/** What `--validate` is pointed at. */
	reference: string;
	reportPath: string;
	examples: ExampleKind;
	/** Roughly how many stages the finished tester will have (PLAN.md §1.5, §5.3–5.5). */
	plannedStages: number;
	sectionBadges: Record<string, SectionBadge>;
	/** Present only on a track whose sections are grouped into ladders. */
	ladders?: Ladder[];
}

export const tracks: Record<TrackId, TrackMeta> = {
	shell: {
		id: 'shell',
		title: 'Build your own Shell',
		short: 'Shell',
		tagline: 'From a bare prompt to job control',
		building: 'a POSIX shell',
		blurb:
			'A POSIX shell, written by you, driven as a black box over pipes and a real pseudo-terminal. 57 stages from printing `$ ` to pipelines, history and line editing.',
		glyph: '❯',
		accent: 'var(--shell)',
		accentBright: 'var(--shell-bright)',
		accentSoft: 'var(--shell-soft)',
		mascot: 'fox',
		mascotLabel: 'a fox',
		dir: 'shelltest',
		tester: 'shelltest',
		binary: './target/release/shelltest',
		targetFlag: '--shell',
		targetExample: './my_shell',
		targetFile: 'shells.yaml',
		reference: 'bash',
		reportPath: '/__reports/shell.json',
		examples: 'transcript',
		plannedStages: 57,
		sectionBadges: {
			A: { badge: 'The Seedbed', icon: '🌱' },
			B: { badge: 'Quote Hollow', icon: '🌾' },
			C: { badge: 'Redirect Brook', icon: '💧' },
			D: { badge: 'Completion Grove', icon: '🌼' },
			E: { badge: 'Pipeline Glade', icon: '🍃' },
			F: { badge: 'Memory Meadow', icon: '🌸' },
			G: { badge: 'The Orchard', icon: '🌳' }
		}
	},
	kafka: {
		id: 'kafka',
		title: 'Build your own Kafka',
		short: 'Kafka',
		tagline: 'From a TCP accept to consumer groups',
		building: 'a Kafka broker',
		blurb:
			'A Kafka broker speaking the real wire protocol, checked against Apache Kafka 4.1. 45 stages from echoing a correlation id to idempotent produce and group rebalances.',
		glyph: '⟐',
		accent: 'var(--kafka)',
		accentBright: 'var(--kafka-bright)',
		accentSoft: 'var(--kafka-soft)',
		mascot: 'bunny',
		mascotLabel: 'a bunny',
		dir: 'kafkatest',
		tester: 'kafkatest',
		binary: './target/release/kafkatest',
		targetFlag: '--broker',
		targetExample: 'my_broker',
		targetFile: 'brokers.yaml',
		reference: 'Apache Kafka 4.1 (KRaft)',
		reportPath: '/__reports/kafka.json',
		examples: 'bytes',
		plannedStages: 45,
		sectionBadges: {
			A: { badge: 'Sprout Field', icon: '🌱' },
			B: { badge: 'Signpost Meadow', icon: '🌷' },
			C: { badge: 'Fetch Falls', icon: '💧' },
			D: { badge: 'Pollen Plateau', icon: '🌻' },
			E: { badge: 'Hive Hollow', icon: '🐝' },
			F: { badge: 'The Orchard', icon: '🌳' }
		}
	},
	wasm: {
		id: 'wasm',
		title: 'Build your own WebAssembly runtime',
		short: 'WebAssembly',
		tagline: 'From four magic bytes to a running program',
		building: 'a WebAssembly runtime',
		blurb:
			'A binary decoder, a validator, an interpreter and a WASI preview1 layer — checked against wasmtime. Every module in the suite is real bytes the harness encoded itself, so a failure can show you the hex.',
		glyph: '⬡',
		accent: 'var(--wasm)',
		accentBright: 'var(--wasm-bright)',
		accentSoft: 'var(--wasm-soft)',
		mascot: 'owl',
		mascotLabel: 'an owl',
		dir: 'wasmtest',
		tester: 'wasmtest',
		binary: './target/release/wasmtest',
		targetFlag: '--runtime',
		targetExample: 'my_runtime',
		targetFile: 'runtimes.yaml',
		reference: 'wasmtime 48.0.2',
		reportPath: '/__reports/wasm.json',
		examples: 'bytes',
		plannedStages: 45,
		sectionBadges: {
			A: { badge: 'Preamble Path', icon: '🌱' },
			B: { badge: 'Typecheck Thicket', icon: '🌿' },
			C: { badge: 'Numeric Nursery', icon: '🌼' },
			D: { badge: 'Branching Boughs', icon: '🌳' },
			E: { badge: 'Memory Marsh', icon: '💧' },
			F: { badge: 'Table Terrace', icon: '🌾' },
			G: { badge: 'The Walled Garden', icon: '🧺' },
			H: { badge: 'The Far Orchard', icon: '🍎' }
		}
	},
	tls: {
		id: 'tls',
		title: 'Build your own TLS 1.3 server',
		short: 'TLS 1.3',
		tagline: 'From a TCP accept to a verified handshake',
		building: 'a TLS 1.3 server',
		blurb:
			'RFC 8446 by hand: the record layer, the key schedule, the handshake state machine and AEAD — checked against `openssl s_server`. The tester is a raw TLS client, so a failure shows the exact bytes and the secrets’ labels.',
		glyph: '🔒',
		accent: 'var(--tls)',
		accentBright: 'var(--tls-bright)',
		accentSoft: 'var(--tls-soft)',
		mascot: 'hedgehog',
		mascotLabel: 'a hedgehog',
		dir: 'tlstest',
		tester: 'tlstest',
		binary: './target/release/tlstest',
		targetFlag: '--server',
		targetExample: 'my_server',
		targetFile: 'servers.yaml',
		reference: 'openssl s_server (OpenSSL 3.x)',
		reportPath: '/__reports/tls.json',
		examples: 'bytes',
		plannedStages: 45,
		sectionBadges: {
			A: { badge: 'The Gate', icon: '🌱' },
			B: { badge: 'Greeting Green', icon: '🌷' },
			C: { badge: 'Secret Spring', icon: '💧' },
			D: { badge: 'Seal Garden', icon: '🔏' },
			E: { badge: 'Cipher Copse', icon: '🍃' },
			F: { badge: 'Alarm Bells', icon: '🔔' },
			G: { badge: 'The Far Orchard', icon: '🌳' }
		}
	},
	link: {
		id: 'link',
		title: 'Build your own ELF linker',
		short: 'Linker',
		tagline: 'From relocatable objects to a file the kernel runs',
		building: 'a static ELF64 linker',
		blurb:
			'Read `.o` files, resolve symbols, apply relocations and emit an ELF64 executable that actually runs — checked against GNU ld. The tester writes the input objects itself and then runs what you linked.',
		glyph: '⛓',
		accent: 'var(--link)',
		accentBright: 'var(--link-bright)',
		accentSoft: 'var(--link-soft)',
		mascot: 'squirrel',
		mascotLabel: 'a squirrel',
		dir: 'linktest',
		tester: 'linktest',
		binary: './target/release/linktest',
		targetFlag: '--linker',
		targetExample: 'my_linker',
		targetFile: 'linkers.yaml',
		reference: '/usr/bin/ld (GNU ld)',
		reportPath: '/__reports/link.json',
		examples: 'bytes',
		plannedStages: 42,
		sectionBadges: {
			A: { badge: 'The Woodpile', icon: '🌰' },
			B: { badge: 'Assembly Arbour', icon: '🌱' },
			C: { badge: 'Name Nursery', icon: '🏷' },
			D: { badge: 'Patch Meadow', icon: '🧵' },
			E: { badge: 'Archive Alley', icon: '📚' },
			F: { badge: 'The Far Orchard', icon: '🌳' }
		}
	},
	dist: {
		id: 'dist',
		title: 'Build your own distributed system',
		short: 'Distributed',
		tagline: 'From a Lamport clock to a linearizable cluster',
		building: 'a replicated, linearizable key-value store',
		blurb:
			'A replicated, linearizable key-value store, climbed in three ladders: the primitives (clocks, quorums, CRDTs, failure detectors), then one durable node, then a real cluster that survives partitions — with the final rung checking your history against a linearizability model.',
		glyph: '⁂',
		accent: 'var(--dist)',
		accentBright: 'var(--dist-bright)',
		accentSoft: 'var(--dist-soft)',
		mascot: 'duckling',
		mascotLabel: 'a duckling',
		dir: 'disttest',
		tester: 'disttest',
		binary: './target/release/disttest',
		targetFlag: '--node',
		targetExample: 'my_node',
		targetFile: 'nodes.yaml',
		reference: 'the suite’s own model checker',
		reportPath: '/__reports/dist.json',
		examples: 'bytes',
		plannedStages: 45,
		sectionBadges: {
			A: { badge: 'Clock Clearing', icon: '🕰' },
			B: { badge: 'The Allotments', icon: '🌾' },
			C: { badge: 'Pollen Count', icon: '🐝' },
			D: { badge: 'Converging Beds', icon: '🌻' },
			E: { badge: 'Windbreak Row', icon: '🍃' },
			F: { badge: 'The Root Cellar', icon: '🥕' },
			G: { badge: 'The Rookery', icon: '🪺' },
			H: { badge: 'Storm Hedge', icon: '⛈' },
			I: { badge: 'The Far Orchard', icon: '🌳' }
		},
		ladders: [
			{
				id: 'primitives',
				title: 'Primitives',
				note: 'The pieces on their own: clocks and causality, partitioning and quorums, probabilistic structures, CRDTs, flow control and failure detection.',
				sections: ['A', 'B', 'C', 'D', 'E']
			},
			{
				id: 'node',
				title: 'A durable node',
				note: 'One process that keeps its promises: MVCC revisions, transactions, leases, watches, and coming back from a crash with everything intact.',
				sections: ['F']
			},
			{
				id: 'cluster',
				title: 'A cluster',
				note: 'Replication and consensus, then faults and partitions, then a randomized workload checked against a linearizability model.',
				sections: ['G', 'H', 'I']
			}
		]
	}
};

/** Every track, in trail order. */
export const allTracks: TrackMeta[] = trackIds.map((id) => tracks[id]);

export function isTrack(value: string): value is TrackId {
	return (trackIds as readonly string[]).includes(value);
}

export function trackMeta(id: TrackId): TrackMeta {
	return tracks[id];
}

/** `{ shell: v, kafka: v, … }` — the shape every per-track record in the site uses. */
export function byTrack<T>(make: (track: TrackId) => T): Record<TrackId, T> {
	const out = {} as Record<TrackId, T>;
	for (const id of trackIds) out[id] = make(id);
	return out;
}

/** The ladder a section sits on, or null on a track with no ladders. */
export function ladderOfSection(track: TrackId, sectionId: string): Ladder | null {
	const id = sectionId.toUpperCase();
	return tracks[track].ladders?.find((l) => l.sections.includes(id)) ?? null;
}

/** The garden name of one section of one track; unknown letters still get a name. */
export function badgeFor(track: TrackId, sectionId: string): SectionBadge {
	return tracks[track].sectionBadges[sectionId.toUpperCase()] ?? { badge: `Section ${sectionId}`, icon: '🌿' };
}
