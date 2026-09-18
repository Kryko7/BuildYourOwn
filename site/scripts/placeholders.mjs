/**
 * Placeholder plans for tracks whose tester has not published anything yet.
 *
 * `npm run sync` prefers, in order:
 *   1. `../<tester>/catalog.json`  — the real thing
 *   2. `../<tester>/PLAN.md`       — the real stage list, every stage marked `planned`
 *   3. the plan below              — the sections only, one waypoint each
 *
 * Nothing here invents a stage list. Each section of a not-yet-written tester gets a single
 * waypoint named after the section, carrying what PLAN.md §5.3–5.5 says the section is for,
 * so the trail, the map and every `/track/N` page exist and say honestly that the stages are
 * still being written. The moment the tester lands, `npm run sync` throws all of this away.
 *
 * kafka is the exception: its 45 stages are transcribed in BuildYourOwn/PLAN.md §1.5, so its
 * placeholder is the real stage list (`kafka-placeholder.mjs`).
 */
import { kafkaPlaceholderPlan } from './kafka-placeholder.mjs';

/** Which ladder a section sits on, for tracks that have ladders (`dist`). */
const LADDERS = {
	dist: { A: 'primitives', B: 'primitives', C: 'primitives', D: 'primitives', E: 'primitives', F: 'node', G: 'cluster', H: 'cluster', I: 'cluster' }
};

/** [sectionId, title, ...what the section is expected to cover] */
const SECTIONS = {
	wasm: [
		['A', 'Binary format & decoding',
			'The module preamble (`\\0asm`, version 1) and the section framing that follows it',
			'LEB128: unsigned, signed, and the encodings a decoder must reject',
			'Types, imports, functions, exports, code — read them all before executing anything'],
		['B', 'Validation & type checking',
			'Validate the whole module before running a single instruction',
			'The stack-polymorphic type checker: operand types, block types, unreachable code',
			'A module that does not validate must fail before it has any observable effect'],
		['C', 'Numeric instructions & traps',
			'i32/i64/f32/f64 arithmetic, comparison, conversion and reinterpretation',
			'The spec’s canonical trap reasons — "integer divide by zero", "integer overflow"',
			'Float semantics: NaN propagation, rounding, saturating truncation'],
		['D', 'Control flow',
			'block, loop, if/else and the label stack br/br_if/br_table branch to',
			'Calls, returns and multi-value results',
			'`unreachable` traps with the spec’s own wording'],
		['E', 'Memory & bulk operations',
			'Linear memory: loads, stores, alignment, `memory.grow`, out-of-bounds traps',
			'Data segments, active and passive',
			'`memory.copy`, `memory.fill`, `memory.init`, `data.drop`'],
		['F', 'Tables, globals, indirect calls',
			'Tables of funcrefs, element segments, `table.get`/`set`/`grow`',
			'`call_indirect` with its type check, and the traps for a bad index or type',
			'Mutable and immutable globals, imported and exported'],
		['G', 'WASI preview1',
			'`_start`, the command ABI, and exiting with `proc_exit`',
			'`fd_write` to stdout and stderr, `args_get`/`args_sizes_get`, `environ_*`',
			'Errno values, iovecs, and the pointers a host has to bounds-check'],
		['H', 'Robustness & scale',
			'Fuzzed and truncated modules: reject them, never panic and never hang',
			'Deep recursion, big memories, long-running loops',
			'Interop: modules a real toolchain emitted, run end to end']
	],
	tls: [
		['A', 'TCP & the record layer',
			'Accept a connection and speak TLSPlaintext: type, legacy version, length',
			'Record size limits, coalescing and fragmentation across records',
			'Nothing is sent before the client speaks'],
		['B', 'ClientHello, extensions, negotiation',
			'Parse ClientHello and answer with a ServerHello that echoes the legacy session id',
			'supported_versions, key_share, signature_algorithms, server_name, ALPN',
			'Pick a cipher suite from the client’s list, or alert if there is nothing in common'],
		['C', 'Key schedule & handshake encryption',
			'X25519 (or P-256) shared secret, HKDF-Extract and HKDF-Expand-Label',
			'The RFC 8446 §7.1 ladder: early → handshake → master, with the derived secrets',
			'EncryptedExtensions onwards is protected with the handshake traffic keys'],
		['D', 'Authentication',
			'Certificate, CertificateVerify and Finished, in that order',
			'The signature covers the transcript hash under a context string',
			'Finished is HMAC over the transcript with the finished key'],
		['E', 'Application data & AEAD',
			'AEAD records: the nonce is the write IV xor the sequence number',
			'The additional data is the record header; the content type is inside the ciphertext',
			'The `-rev` echo layer: every line received comes back reversed'],
		['F', 'Alerts, errors, robustness',
			'close_notify, and the fatal alerts a malformed handshake must produce',
			'Truncated records, bad MACs, replayed sequence numbers',
			'A misbehaving client must not take the server down'],
		['G', 'Advanced',
			'HelloRetryRequest when the client’s key share is not one you support',
			'KeyUpdate in both directions',
			'Session tickets, resumption, early-data rejection, and real-client interop']
	],
	dist: [
		['A', 'Clocks & causality',
			'Lamport clocks, and what a scalar counter can and cannot tell you',
			'Vector clocks and version vectors: happens-before, concurrent, dominated',
			'Hybrid logical clocks — physical time you can still order'],
		['B', 'Partitioning & quorums',
			'Consistent hashing with virtual nodes, and what moves when a node joins',
			'Rendezvous (highest random weight) hashing as the alternative',
			'The N/R/W quorum arithmetic, and when R + W > N actually buys you something'],
		['C', 'Probabilistic & anti-entropy structures',
			'Bloom filters: bit math, hash count, and the false-positive rate you promised',
			'HyperLogLog cardinality estimation',
			'Merkle trees for finding the difference between two replicas cheaply'],
		['D', 'CRDTs & convergence',
			'G-Counter and PN-Counter: merge is a join, not an assignment',
			'LWW-Register and OR-Set, with the tombstones that make removal work',
			'RGA for ordered sequences, and convergence under any delivery order'],
		['E', 'Flow control & failure detection',
			'Token bucket rate limiting, and backoff with jitter that does not synchronize',
			'Phi-accrual failure detection instead of a fixed timeout',
			'SWIM: gossip, indirect probes, suspicion'],
		['F', 'A durable single node',
			'MVCC revisions: every key keeps its history, reads pick a revision',
			'Transactions, leases and watches',
			'Crash recovery — everything acknowledged is still there after a kill -9'],
		['G', 'Replication & consensus',
			'Leader election, terms, and the votes that decide one',
			'Log replication, commit index, and applying in order',
			'Linearizable reads: read index or lease, never just "ask the leader"'],
		['H', 'Faults & partitions',
			'A minority partition must refuse to make progress',
			'Leader failover with no lost acknowledged write',
			'Snapshots, log compaction and membership change'],
		['I', 'Linearizability under fault injection',
			'Randomized concurrent workloads recorded as a history',
			'The history is checked against a linearizability model, not against a golden file',
			'Partitions, pauses and crashes injected mid-run, with a seed so it replays']
	],
	link: [
		['A', 'Reading relocatable objects',
			'Parse ELF64: the header, the section headers, and the string tables behind them',
			'`.symtab` and `.strtab`: name, value, size, binding, type, section index',
			'Relocation sections: `.rela.text` and friends, with their addends'],
		['B', 'Emitting a runnable executable',
			'Lay out sections into segments and write program headers the kernel accepts',
			'Entry point, `_start`, and the load addresses your layout picks',
			'The output has to actually run and exit with the status the test expects'],
		['C', 'Symbol resolution',
			'Global, local and weak symbols, and which definition wins',
			'Common symbols, COMDAT groups and duplicate definitions',
			'Undefined symbols are an error with a message that names them'],
		['D', 'Relocations',
			'R_X86_64_64, PC32, PLT32, 32S and the rest of the static set',
			'Compute S + A − P correctly, and detect overflow instead of truncating',
			'Section-relative symbols and addends that point into the middle of a section'],
		['E', 'Archives and link order',
			'`.a` files: the archive format, the symbol index, and lazy member extraction',
			'Link order matters — a member is pulled in only to satisfy a pending undefined',
			'`-L` and `-l` search paths'],
		['F', 'Real programs, robustness, scale',
			'Multi-object programs that compute something and print it',
			'Fuzzed and truncated objects: reject them cleanly',
			'Many objects, many symbols, and a link that finishes fast']
	]
};

function slugify(name, number) {
	const s = String(name ?? '')
		.toLowerCase()
		.replace(/\([^)]*\)/g, ' ')
		.replace(/[^a-z0-9]+/g, '-')
		.replace(/^-|-$/g, '')
		.slice(0, 48);
	return s || `stage-${number}`;
}

/** The section titles a track's catalog should use when the tester does not name them. */
export function sectionTitles(track) {
	const rows = SECTIONS[track];
	if (!rows) return {};
	return Object.fromEntries(rows.map(([id, title]) => [id, title]));
}

/**
 * One waypoint per section, in the shape `buildCatalog()` consumes. Every stage is
 * `planned: true` — there is no tester behind it yet and the UI says so.
 */
function sectionsOnlyPlan(track) {
	const rows = SECTIONS[track];
	if (!rows) return null;
	const sections = [];
	const stages = [];
	const ladders = LADDERS[track] ?? {};
	rows.forEach(([id, title, ...hints], i) => {
		const number = i + 1;
		sections.push({ id, title, stages: [number] });
		stages.push({
			number,
			slug: slugify(title, number),
			name: title,
			ext: false,
			planned: true,
			file: '',
			planDone: false,
			plannedTests: 0,
			section: id,
			...(ladders[id] ? { ladder: ladders[id] } : {}),
			hints,
			tests: []
		});
	});
	return { sections, stages, declared: null };
}

/** The best placeholder plan for a track, or null when the track has none. */
export function placeholderPlan(track) {
	if (track === 'kafka') return kafkaPlaceholderPlan();
	return sectionsOnlyPlan(track);
}
