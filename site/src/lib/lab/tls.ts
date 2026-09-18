/**
 * A TLS 1.3 record and handshake decoder for the /lab inspector (RFC 8446).
 *
 * It parses framing and structure — records, handshake messages, ClientHello and
 * ServerHello field by field, every extension by name — and it walks the key schedule's
 * labels, including the exact bytes of the `HkdfLabel` struct each derivation hashes.
 * There is no cryptography here and there is meant to be none: this is the half of TLS
 * you can read with your eyes, which is the half that is usually wrong first.
 */
import type { Decoded, Field } from './field';

/* -------------------------------- constants ------------------------------- */

export const CONTENT_TYPES: Record<number, string> = {
	20: 'change_cipher_spec',
	21: 'alert',
	22: 'handshake',
	23: 'application_data',
	24: 'heartbeat'
};

export const HANDSHAKE_TYPES: Record<number, string> = {
	1: 'client_hello',
	2: 'server_hello',
	4: 'new_session_ticket',
	5: 'end_of_early_data',
	8: 'encrypted_extensions',
	11: 'certificate',
	13: 'certificate_request',
	15: 'certificate_verify',
	20: 'finished',
	24: 'key_update',
	254: 'message_hash'
};

export const CIPHER_SUITES: Record<number, string> = {
	0x1301: 'TLS_AES_128_GCM_SHA256',
	0x1302: 'TLS_AES_256_GCM_SHA384',
	0x1303: 'TLS_CHACHA20_POLY1305_SHA256',
	0x1304: 'TLS_AES_128_CCM_SHA256',
	0x1305: 'TLS_AES_128_CCM_8_SHA256',
	0x00ff: 'TLS_EMPTY_RENEGOTIATION_INFO_SCSV',
	0xc02b: 'TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256 (1.2)',
	0xc02f: 'TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256 (1.2)',
	0xc030: 'TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384 (1.2)'
};

export const EXTENSIONS: Record<number, string> = {
	0: 'server_name',
	5: 'status_request',
	10: 'supported_groups',
	11: 'ec_point_formats',
	13: 'signature_algorithms',
	14: 'use_srtp',
	15: 'heartbeat',
	16: 'application_layer_protocol_negotiation',
	18: 'signed_certificate_timestamp',
	19: 'client_certificate_type',
	20: 'server_certificate_type',
	21: 'padding',
	22: 'encrypt_then_mac',
	23: 'extended_master_secret',
	35: 'session_ticket',
	41: 'pre_shared_key',
	42: 'early_data',
	43: 'supported_versions',
	44: 'cookie',
	45: 'psk_key_exchange_modes',
	47: 'certificate_authorities',
	48: 'oid_filters',
	49: 'post_handshake_auth',
	50: 'signature_algorithms_cert',
	51: 'key_share'
};

export const NAMED_GROUPS: Record<number, string> = {
	0x0017: 'secp256r1',
	0x0018: 'secp384r1',
	0x0019: 'secp521r1',
	0x001d: 'x25519',
	0x001e: 'x448',
	0x0100: 'ffdhe2048',
	0x0101: 'ffdhe3072'
};

export const SIGNATURE_SCHEMES: Record<number, string> = {
	0x0401: 'rsa_pkcs1_sha256',
	0x0501: 'rsa_pkcs1_sha384',
	0x0601: 'rsa_pkcs1_sha512',
	0x0403: 'ecdsa_secp256r1_sha256',
	0x0503: 'ecdsa_secp384r1_sha384',
	0x0603: 'ecdsa_secp521r1_sha512',
	0x0804: 'rsa_pss_rsae_sha256',
	0x0805: 'rsa_pss_rsae_sha384',
	0x0806: 'rsa_pss_rsae_sha512',
	0x0807: 'ed25519',
	0x0808: 'ed448'
};

export const ALERT_LEVELS: Record<number, string> = { 1: 'warning', 2: 'fatal' };

export const ALERT_DESCRIPTIONS: Record<number, string> = {
	0: 'close_notify',
	10: 'unexpected_message',
	20: 'bad_record_mac',
	22: 'record_overflow',
	40: 'handshake_failure',
	42: 'bad_certificate',
	43: 'unsupported_certificate',
	47: 'illegal_parameter',
	48: 'unknown_ca',
	50: 'decode_error',
	51: 'decrypt_error',
	70: 'protocol_version',
	71: 'insufficient_security',
	80: 'internal_error',
	109: 'missing_extension',
	110: 'unsupported_extension',
	112: 'unrecognized_name',
	116: 'certificate_required',
	120: 'no_application_protocol'
};

const VERSIONS: Record<number, string> = {
	0x0300: 'SSL 3.0',
	0x0301: 'TLS 1.0',
	0x0302: 'TLS 1.1',
	0x0303: 'TLS 1.2',
	0x0304: 'TLS 1.3'
};

const versionName = (v: number) => VERSIONS[v] ?? `0x${v.toString(16).padStart(4, '0')}`;

/* --------------------------------- reading -------------------------------- */

class Cursor {
	offset = 0;
	depth = 0;
	readonly fields: Field[] = [];

	constructor(readonly bytes: Uint8Array) {}

	field(name: string, type: string, start: number, end: number, value: string, note?: string) {
		const f: Field = { name, type, start, end, value, depth: this.depth };
		if (note) f.note = note;
		this.fields.push(f);
	}

	need(n: number, what: string) {
		if (this.bytes.length - this.offset < n) {
			throw new Error(`${what}: wanted ${n} byte${n === 1 ? '' : 's'}, ${this.bytes.length - this.offset} left`);
		}
	}

	u8(name: string, format: (v: number) => string = String, note?: string): number {
		this.need(1, name);
		const start = this.offset;
		const v = this.bytes[this.offset++];
		this.field(name, 'uint8', start, this.offset, format(v), note);
		return v;
	}

	u16(name: string, format: (v: number) => string = String, note?: string): number {
		this.need(2, name);
		const start = this.offset;
		const v = (this.bytes[start] << 8) | this.bytes[start + 1];
		this.offset += 2;
		this.field(name, 'uint16', start, this.offset, format(v), note);
		return v;
	}

	u24(name: string, note?: string): number {
		this.need(3, name);
		const start = this.offset;
		const v = (this.bytes[start] << 16) | (this.bytes[start + 1] << 8) | this.bytes[start + 2];
		this.offset += 3;
		this.field(name, 'uint24', start, this.offset, String(v), note);
		return v;
	}

	blob(n: number, name: string, note?: string): Uint8Array {
		this.need(n, name);
		const start = this.offset;
		const slice = this.bytes.subarray(start, start + n);
		this.offset += n;
		this.field(name, `opaque[${n}]`, start, this.offset, shortHex(slice), note);
		return slice;
	}
}

export function shortHex(bytes: Uint8Array, limit = 12): string {
	const head = [...bytes.subarray(0, limit)].map((b) => b.toString(16).padStart(2, '0')).join('');
	return bytes.length > limit ? `${head}… (${bytes.length} bytes)` : head || '(empty)';
}

function ascii(bytes: Uint8Array): string {
	return new TextDecoder().decode(bytes);
}

/* ------------------------------- extensions ------------------------------- */

export interface TlsExtension {
	type: number;
	name: string;
	length: number;
	/** What is inside it, in one line. */
	summary: string;
}

function extensionBody(c: Cursor, type: number, length: number, isServer: boolean): string {
	const end = c.offset + length;
	const name = EXTENSIONS[type] ?? `unknown (${type})`;
	c.depth = 3;
	let summary = `${length} bytes`;
	try {
		if (type === 43) {
			// supported_versions: a list from the client, a single version from the server.
			if (isServer) {
				const v = c.u16('selected_version', versionName,
					'A TLS 1.3 ServerHello says 1.3 *here*; its legacy_version field still says 1.2.');
				summary = versionName(v);
			} else {
				const listLen = c.u8('versions.length');
				const names: string[] = [];
				for (let i = 0; i + 1 < listLen + 1 && c.offset < end; i += 2) {
					names.push(versionName(c.u16(`version[${names.length}]`, versionName)));
				}
				summary = names.join(', ');
			}
		} else if (type === 51) {
			// key_share: the whole point of 1.3's one-round-trip handshake.
			const entries: string[] = [];
			if (!isServer) c.u16('client_shares.length');
			while (c.offset < end) {
				const group = c.u16('key_share.group', (v) => NAMED_GROUPS[v] ?? `0x${v.toString(16)}`);
				const len = c.u16('key_share.key_exchange.length');
				c.blob(Math.min(len, end - c.offset), 'key_share.key_exchange',
					'The raw public key for that group — 32 bytes for x25519, 65 for an uncompressed P-256 point.');
				entries.push(`${NAMED_GROUPS[group] ?? group} (${len} bytes)`);
			}
			summary = entries.join(', ');
		} else if (type === 10) {
			const listLen = c.u16('supported_groups.length');
			const names: string[] = [];
			const stop = Math.min(end, c.offset + listLen);
			while (c.offset < stop) {
				const g = c.u16(`group[${names.length}]`, (v) => NAMED_GROUPS[v] ?? `0x${v.toString(16)}`);
				names.push(NAMED_GROUPS[g] ?? `0x${g.toString(16)}`);
			}
			summary = names.join(', ');
		} else if (type === 13 || type === 50) {
			const listLen = c.u16('signature_algorithms.length');
			const names: string[] = [];
			const stop = Math.min(end, c.offset + listLen);
			while (c.offset < stop) {
				const s = c.u16(`scheme[${names.length}]`, (v) => SIGNATURE_SCHEMES[v] ?? `0x${v.toString(16)}`);
				names.push(SIGNATURE_SCHEMES[s] ?? `0x${s.toString(16)}`);
			}
			summary = names.join(', ');
		} else if (type === 0) {
			c.u16('server_name_list.length');
			const kind = c.u8('name_type', (v) => (v === 0 ? 'host_name' : String(v)));
			const len = c.u16('host_name.length');
			const host = c.blob(Math.min(len, end - c.offset), 'host_name');
			summary = kind === 0 ? ascii(host) : `${len} bytes`;
		} else if (type === 16) {
			c.u16('alpn_list.length');
			const names: string[] = [];
			while (c.offset < end) {
				const len = c.u8(`protocol[${names.length}].length`);
				names.push(ascii(c.blob(Math.min(len, end - c.offset), `protocol[${names.length}]`)));
			}
			summary = names.join(', ');
		} else if (type === 45) {
			const len = c.u8('psk_key_exchange_modes.length');
			const modes: string[] = [];
			for (let i = 0; i < len && c.offset < end; i++) {
				const m = c.u8(`mode[${i}]`, (v) => (v === 0 ? 'psk_ke' : v === 1 ? 'psk_dhe_ke' : String(v)));
				modes.push(m === 0 ? 'psk_ke' : m === 1 ? 'psk_dhe_ke' : String(m));
			}
			summary = modes.join(', ');
		} else if (type === 44) {
			const len = c.u16('cookie.length');
			c.blob(Math.min(len, end - c.offset), 'cookie');
			summary = `${len} bytes`;
		} else if (length > 0) {
			c.blob(length, `${name}.data`);
		} else {
			summary = 'empty';
		}
	} catch (e) {
		summary = e instanceof Error ? e.message : 'could not parse';
	}
	// Whatever the branch did not read is still part of the extension.
	if (c.offset < end) {
		c.field(`${name}.rest`, 'opaque', c.offset, end, `${end - c.offset} bytes`);
	}
	c.offset = end;
	c.depth = 2;
	return summary;
}

function extensions(c: Cursor, isServer: boolean, out: TlsExtension[]) {
	const total = c.u16('extensions.length',
		String,
		'Extensions are where TLS 1.3 actually lives: the version, the key share and the signature algorithms are all in here.');
	const end = c.offset + total;
	while (c.offset + 4 <= end) {
		c.depth = 2;
		const type = c.u16('extension.type', (v) => `${v} (${EXTENSIONS[v] ?? 'unknown'})`);
		const length = c.u16('extension.length');
		const summary = extensionBody(c, type, Math.min(length, end - c.offset), isServer);
		out.push({ type, name: EXTENSIONS[type] ?? `unknown (${type})`, length, summary });
	}
	c.offset = end;
	c.depth = 1;
}

/* ------------------------------- handshakes ------------------------------- */

export interface TlsHandshake {
	type: number;
	name: string;
	start: number;
	end: number;
	length: number;
	summary: string;
	extensions: TlsExtension[];
	cipherSuites: string[];
}

function handshakeBody(c: Cursor, type: number, length: number, hs: TlsHandshake) {
	const end = c.offset + length;
	c.depth = 1;
	if (type === 1 || type === 2) {
		const isServer = type === 2;
		c.u16('legacy_version', versionName,
			'Always 0x0303 ("TLS 1.2") on the wire in TLS 1.3 — middleboxes would drop anything else. The real version is in supported_versions.');
		c.blob(32, 'random',
			isServer
				? 'The server’s 32 random bytes. The last 8 carry a fixed value when the server is downgrading on purpose.'
				: 'The client’s 32 random bytes — they end up in the transcript, not in the key schedule directly.');
		const sidLen = c.u8('legacy_session_id.length');
		const sid = c.blob(Math.min(sidLen, end - c.offset), 'legacy_session_id',
			'TLS 1.3 has no session ids; the server echoes whatever the client sent so the exchange looks like a resumed 1.2 handshake.');
		if (isServer) {
			const suite = c.u16('cipher_suite', (v) => CIPHER_SUITES[v] ?? `0x${v.toString(16)}`);
			hs.cipherSuites = [CIPHER_SUITES[suite] ?? `0x${suite.toString(16)}`];
			c.u8('legacy_compression_method', (v) => (v === 0 ? 'null' : String(v)));
		} else {
			const suitesLen = c.u16('cipher_suites.length');
			const stop = Math.min(end, c.offset + suitesLen);
			while (c.offset < stop) {
				const s = c.u16(`cipher_suite[${hs.cipherSuites.length}]`, (v) => CIPHER_SUITES[v] ?? `0x${v.toString(16)}`);
				hs.cipherSuites.push(CIPHER_SUITES[s] ?? `0x${s.toString(16)}`);
			}
			const compLen = c.u8('legacy_compression_methods.length');
			c.blob(Math.min(compLen, end - c.offset), 'legacy_compression_methods',
				'Must be exactly one byte, 0x00. Compression is gone.');
		}
		extensions(c, isServer, hs.extensions);
		hs.summary = isServer
			? `${hs.cipherSuites[0] ?? '?'}, ${hs.extensions.length} extensions`
			: `${hs.cipherSuites.length} cipher suites, ${hs.extensions.length} extensions${sid.length ? `, ${sid.length}-byte session id` : ''}`;
	} else if (type === 8) {
		extensions(c, true, hs.extensions);
		hs.summary = `${hs.extensions.length} extension${hs.extensions.length === 1 ? '' : 's'}`;
	} else if (type === 11) {
		const ctxLen = c.u8('certificate_request_context.length');
		if (ctxLen) c.blob(Math.min(ctxLen, end - c.offset), 'certificate_request_context');
		const listLen = c.u24('certificate_list.length');
		let n = 0;
		const stop = Math.min(end, c.offset + listLen);
		while (c.offset + 3 <= stop) {
			const certLen = c.u24(`certificate[${n}].length`);
			c.blob(Math.min(certLen, stop - c.offset), `certificate[${n}].cert_data`, 'A DER-encoded X.509 certificate.');
			const extLen = c.u16(`certificate[${n}].extensions.length`);
			if (extLen) c.blob(Math.min(extLen, stop - c.offset), `certificate[${n}].extensions`);
			n++;
		}
		hs.summary = `${n} certificate${n === 1 ? '' : 's'}`;
	} else if (type === 15) {
		const scheme = c.u16('algorithm', (v) => SIGNATURE_SCHEMES[v] ?? `0x${v.toString(16)}`,
			'The scheme has to be one the client offered in signature_algorithms.');
		const sigLen = c.u16('signature.length');
		c.blob(Math.min(sigLen, end - c.offset), 'signature',
			'Over 64 spaces, the context string, a zero byte and the transcript hash — not over the certificate.');
		hs.summary = `${SIGNATURE_SCHEMES[scheme] ?? scheme}, ${sigLen}-byte signature`;
	} else if (type === 20) {
		c.blob(Math.min(length, end - c.offset), 'verify_data',
			'HMAC(finished_key, Transcript-Hash(everything so far)) — 32 bytes for SHA-256, 48 for SHA-384.');
		hs.summary = `${length}-byte verify_data`;
	} else if (type === 24) {
		const req = c.u8('request_update', (v) => (v === 0 ? 'update_not_requested' : 'update_requested'));
		hs.summary = req === 0 ? 'update_not_requested' : 'update_requested';
	} else if (length > 0) {
		c.blob(length, `${HANDSHAKE_TYPES[type] ?? 'handshake'}.body`);
		hs.summary = `${length} bytes`;
	}
	if (c.offset < end) {
		c.field('handshake.rest', 'opaque', c.offset, end, `${end - c.offset} bytes`);
		c.offset = end;
	}
	c.depth = 0;
}

/* --------------------------------- records -------------------------------- */

export interface TlsRecord {
	type: number;
	name: string;
	version: string;
	start: number;
	end: number;
	length: number;
	summary: string;
	handshakes: TlsHandshake[];
}

export interface TlsDecode extends Decoded {
	records: TlsRecord[];
	/** Every handshake message across every record, in order. */
	handshakes: TlsHandshake[];
	/** True when the first record is a ClientHello, i.e. this is a capture of a real start. */
	fromClient: boolean;
}

/**
 * Parse a stream of TLS records. Never throws: a truncated capture keeps everything that
 * did parse, plus the reason it stopped — which is usually the interesting part.
 */
export function decodeTls(bytes: Uint8Array): TlsDecode {
	const c = new Cursor(bytes);
	const out: TlsDecode = { fields: [], records: [], handshakes: [], fromClient: false };
	try {
		while (c.offset + 5 <= bytes.length) {
			const start = c.offset;
			c.depth = 0;
			const type = c.u8('record.type', (v) => `${v} (${CONTENT_TYPES[v] ?? 'unknown'})`,
				'The outer type. Once the handshake is encrypted every record says 23 (application_data) and the real type is the last byte inside the plaintext.');
			const version = c.u16('record.legacy_version', versionName,
				'Frozen at 0x0303 for compatibility. The negotiated version never appears here.');
			const length = c.u16('record.length', String,
				'At most 2^14 bytes of plaintext (plus overhead once it is encrypted) — a record is not a message, and a message can span several.');
			const bodyStart = c.offset;
			const end = Math.min(bodyStart + length, bytes.length);
			const record: TlsRecord = {
				type,
				name: CONTENT_TYPES[type] ?? `unknown (${type})`,
				version: versionName(version),
				start,
				end,
				length,
				summary: `${length} bytes`,
				handshakes: []
			};

			if (type === 22) {
				while (c.offset + 4 <= end) {
					const hsStart = c.offset;
					c.depth = 1;
					const hsType = c.u8('handshake.msg_type', (v) => `${v} (${HANDSHAKE_TYPES[v] ?? 'unknown'})`);
					const hsLength = c.u24('handshake.length',
						'Three bytes, not four — and it is the length of the body, not of the whole message.');
					const hs: TlsHandshake = {
						type: hsType,
						name: HANDSHAKE_TYPES[hsType] ?? `unknown (${hsType})`,
						start: hsStart,
						end: Math.min(c.offset + hsLength, end),
						length: hsLength,
						summary: `${hsLength} bytes`,
						extensions: [],
						cipherSuites: []
					};
					handshakeBody(c, hsType, Math.min(hsLength, end - c.offset), hs);
					record.handshakes.push(hs);
					out.handshakes.push(hs);
				}
				record.summary = record.handshakes.map((h) => h.name).join(', ') || `${length} bytes`;
			} else if (type === 21 && end - c.offset >= 2) {
				c.depth = 1;
				const level = c.u8('alert.level', (v) => `${v} (${ALERT_LEVELS[v] ?? '?'})`);
				const desc = c.u8('alert.description', (v) => `${v} (${ALERT_DESCRIPTIONS[v] ?? '?'})`);
				record.summary = `${ALERT_LEVELS[level] ?? level} ${ALERT_DESCRIPTIONS[desc] ?? desc}`;
			} else if (type === 20) {
				c.depth = 1;
				c.u8('change_cipher_spec', String,
					'A single 0x01 that means nothing in TLS 1.3 — it is sent only so the exchange looks like a 1.2 one.');
				record.summary = 'legacy, ignored';
			} else if (type === 23) {
				c.depth = 1;
				c.blob(end - c.offset, 'encrypted_record',
					'AEAD ciphertext plus a 16-byte tag. The real content type is the last byte of the plaintext, after any zero padding.');
				record.summary = `${length} bytes of ciphertext`;
			}

			if (c.offset < end) {
				c.field('record.rest', 'opaque', c.offset, end, `${end - c.offset} bytes`);
			}
			c.offset = end;
			record.end = end;
			out.records.push(record);
		}
		if (c.offset < bytes.length) {
			throw new Error(`${bytes.length - c.offset} trailing byte(s): a record header is 5 bytes`);
		}
	} catch (e) {
		out.error = e instanceof Error ? e.message : 'could not parse these records';
	}
	out.fields = c.fields;
	out.fromClient = out.handshakes[0]?.type === 1;
	return out;
}

/* ------------------------------ the key schedule --------------------------- */

/**
 * `HkdfLabel` from RFC 8446 §7.1, encoded exactly as it goes into HKDF-Expand:
 * `uint16 length, opaque label<7..255> (prefixed with "tls13 "), opaque context<0..255>`.
 * No hashing happens here — this is the struct you have to get byte-identical.
 */
export function hkdfLabelBytes(label: string, contextLength = 0, length = 32): Uint8Array {
	const full = new TextEncoder().encode(`tls13 ${label}`);
	const out = new Uint8Array(2 + 1 + full.length + 1 + contextLength);
	out[0] = (length >> 8) & 0xff;
	out[1] = length & 0xff;
	out[2] = full.length;
	out.set(full, 3);
	out[3 + full.length] = contextLength;
	return out;
}

export interface ScheduleStep {
	id: string;
	/** What the step produces. */
	output: string;
	/** `HKDF-Extract` or `Derive-Secret`. */
	op: string;
	/** Its inputs, spelled out. */
	inputs: string;
	/** The `tls13 ` label, when there is one. */
	label: string | null;
	/** What the transcript hash covers at that point, when it covers anything. */
	transcript: string | null;
	why: string;
	/** Indent, so the ladder reads as a ladder. */
	depth: number;
}

/** The TLS 1.3 key schedule (RFC 8446 §7.1), top to bottom. */
export const KEY_SCHEDULE: ScheduleStep[] = [
	{
		id: 'early-secret',
		output: 'Early Secret',
		op: 'HKDF-Extract',
		inputs: 'salt = 0, IKM = PSK (or a hash-length string of zeros)',
		label: null,
		transcript: null,
		why: 'Without resumption the PSK is all zeros — but you still have to run the extract, because everything below is derived from its output.',
		depth: 0
	},
	{
		id: 'derived-1',
		output: 'Derived (for the handshake)',
		op: 'Derive-Secret',
		inputs: 'Early Secret',
		label: 'derived',
		transcript: 'the empty string',
		why: 'Every stage ends with a "derived" step over an empty transcript so the next Extract has a domain-separated salt.',
		depth: 1
	},
	{
		id: 'handshake-secret',
		output: 'Handshake Secret',
		op: 'HKDF-Extract',
		inputs: 'salt = Derived, IKM = the (EC)DHE shared secret',
		label: null,
		transcript: null,
		why: 'This is the first point at which the X25519 (or P-256) shared secret enters the schedule.',
		depth: 0
	},
	{
		id: 'c-hs-traffic',
		output: 'client_handshake_traffic_secret',
		op: 'Derive-Secret',
		inputs: 'Handshake Secret',
		label: 'c hs traffic',
		transcript: 'ClientHello … ServerHello',
		why: 'Protects everything the client sends from its Finished onwards. Get the transcript boundary wrong and the server’s decryption fails with bad_record_mac.',
		depth: 1
	},
	{
		id: 's-hs-traffic',
		output: 'server_handshake_traffic_secret',
		op: 'Derive-Secret',
		inputs: 'Handshake Secret',
		label: 's hs traffic',
		transcript: 'ClientHello … ServerHello',
		why: 'EncryptedExtensions, Certificate, CertificateVerify and the server Finished all travel under this.',
		depth: 1
	},
	{
		id: 'key-iv',
		output: 'write_key / write_iv',
		op: 'HKDF-Expand-Label',
		inputs: 'a traffic secret',
		label: 'key / iv',
		transcript: 'none — the context is empty here',
		why: 'The record keys themselves: key is the AEAD key length, iv is 12 bytes and gets xor-ed with the record sequence number.',
		depth: 2
	},
	{
		id: 'finished',
		output: 'finished_key',
		op: 'HKDF-Expand-Label',
		inputs: 'a handshake traffic secret',
		label: 'finished',
		transcript: 'none — the context is empty here',
		why: 'The HMAC key for the Finished message. The transcript goes into the HMAC, not into this derivation.',
		depth: 2
	},
	{
		id: 'derived-2',
		output: 'Derived (for the master secret)',
		op: 'Derive-Secret',
		inputs: 'Handshake Secret',
		label: 'derived',
		transcript: 'the empty string',
		why: 'The same domain-separation step again, one rung down.',
		depth: 1
	},
	{
		id: 'master-secret',
		output: 'Master Secret',
		op: 'HKDF-Extract',
		inputs: 'salt = Derived, IKM = 0',
		label: null,
		transcript: null,
		why: 'No new key material goes in — the extract exists to give the application secrets a clean root.',
		depth: 0
	},
	{
		id: 'c-ap-traffic',
		output: 'client_application_traffic_secret_0',
		op: 'Derive-Secret',
		inputs: 'Master Secret',
		label: 'c ap traffic',
		transcript: 'ClientHello … server Finished',
		why: 'Application data keys. The transcript now runs through the server’s Finished — a different boundary from the handshake secrets.',
		depth: 1
	},
	{
		id: 's-ap-traffic',
		output: 'server_application_traffic_secret_0',
		op: 'Derive-Secret',
		inputs: 'Master Secret',
		label: 's ap traffic',
		transcript: 'ClientHello … server Finished',
		why: 'The server side of the same thing. KeyUpdate replaces these with "traffic upd" derivations later.',
		depth: 1
	},
	{
		id: 'res-master',
		output: 'resumption_master_secret',
		op: 'Derive-Secret',
		inputs: 'Master Secret',
		label: 'res master',
		transcript: 'ClientHello … client Finished',
		why: 'What a session ticket resumes from — and the reason the client Finished has to be in the transcript.',
		depth: 1
	}
];

/* -------------------------------- the samples ------------------------------ */

class Writer {
	readonly bytes: number[] = [];
	u8(v: number) {
		this.bytes.push(v & 0xff);
		return this;
	}
	u16(v: number) {
		this.bytes.push((v >> 8) & 0xff, v & 0xff);
		return this;
	}
	u24(v: number) {
		this.bytes.push((v >> 16) & 0xff, (v >> 8) & 0xff, v & 0xff);
		return this;
	}
	raw(v: ArrayLike<number>) {
		for (let i = 0; i < v.length; i++) this.bytes.push(v[i] & 0xff);
		return this;
	}
	str(s: string) {
		return this.raw(new TextEncoder().encode(s));
	}
	/** A deterministic filler, so a sample's bytes are the same on every render. */
	fill(n: number, seed: number) {
		let s = seed >>> 0 || 1;
		for (let i = 0; i < n; i++) {
			s ^= s << 13;
			s ^= s >>> 17;
			s ^= s << 5;
			s >>>= 0;
			this.bytes.push(s & 0xff);
		}
		return this;
	}
}

function extension(type: number, body: number[]): number[] {
	return [(type >> 8) & 0xff, type & 0xff, (body.length >> 8) & 0xff, body.length & 0xff, ...body];
}

function record(type: number, body: number[]): number[] {
	return [type, 0x03, 0x03, (body.length >> 8) & 0xff, body.length & 0xff, ...body];
}

function handshake(type: number, body: number[]): number[] {
	return [type, (body.length >> 16) & 0xff, (body.length >> 8) & 0xff, body.length & 0xff, ...body];
}

function clientHello(): number[] {
	const w = new Writer();
	w.u16(0x0303); // legacy_version
	w.fill(32, 0xc0ffee); // random
	w.u8(32).fill(32, 0x5e551024); // legacy_session_id (a fake one, as real clients send)
	const suites = [0x1301, 0x1302, 0x1303];
	w.u16(suites.length * 2);
	for (const s of suites) w.u16(s);
	w.u8(1).u8(0); // legacy_compression_methods = { null }

	// server_name: a list of one host_name entry.
	const host = [...new TextEncoder().encode('garden.localhost')];
	const listLength = host.length + 3;
	const sni = [
		(listLength >> 8) & 0xff,
		listLength & 0xff,
		0x00, // name_type = host_name
		(host.length >> 8) & 0xff,
		host.length & 0xff,
		...host
	];

	const keyShare = new Writer();
	keyShare.u16(2 + 2 + 32); // client_shares length
	keyShare.u16(0x001d).u16(32).fill(32, 0x2551971d);

	const exts = [
		...extension(0, sni),
		...extension(43, [2, 0x03, 0x04]),
		...extension(10, [0, 6, 0x00, 0x1d, 0x00, 0x17, 0x00, 0x18]),
		...extension(13, [0, 8, 0x08, 0x04, 0x04, 0x03, 0x08, 0x07, 0x04, 0x01]),
		...extension(45, [1, 1]),
		...extension(51, keyShare.bytes),
		...extension(16, [0, 3, 2, 0x68, 0x32])
	];
	w.u16(exts.length).raw(exts);
	return record(22, handshake(1, w.bytes));
}

/** ServerHello + the legacy change_cipher_spec + one encrypted record, as a server sends it. */
function serverFlight(): number[] {
	const w = new Writer();
	w.u16(0x0303);
	w.fill(32, 0x5e12a1);
	w.u8(32).fill(32, 0x5e551024); // echoes the client's legacy_session_id
	w.u16(0x1301); // TLS_AES_128_GCM_SHA256
	w.u8(0); // legacy_compression_method

	const keyShare = new Writer();
	keyShare.u16(0x001d).u16(32).fill(32, 0x7e114a);
	const exts = [...extension(43, [0x03, 0x04]), ...extension(51, keyShare.bytes)];
	w.u16(exts.length).raw(exts);

	const encrypted = new Writer().fill(96, 0xa1c0de);
	return [
		...record(22, handshake(2, w.bytes)),
		...record(20, [0x01]),
		...record(23, encrypted.bytes)
	];
}

function alertRecord(): number[] {
	return [...record(21, [2, 40]), ...record(21, [1, 0])];
}

export interface TlsSample {
	id: string;
	label: string;
	blurb: string;
	bytes: Uint8Array;
}

export const tlsSamples: TlsSample[] = [
	{
		id: 'client-hello',
		label: 'ClientHello',
		blurb:
			'One handshake record from a 1.3 client: the frozen legacy version, a faked session id, three cipher suites, and the extensions that carry the real negotiation — supported_versions, key_share, signature_algorithms, ALPN and SNI.',
		bytes: Uint8Array.from(clientHello())
	},
	{
		id: 'server-flight',
		label: 'ServerHello + CCS + encrypted',
		blurb:
			'What the server answers with: a ServerHello that echoes the session id and picks one suite, the legacy change_cipher_spec byte, and then a record that says application_data but is really the encrypted rest of the handshake.',
		bytes: Uint8Array.from(serverFlight())
	},
	{
		id: 'alerts',
		label: 'Alerts',
		blurb:
			'A fatal handshake_failure and a warning close_notify — two bytes each, and the difference between them decides whether the connection is dead.',
		bytes: Uint8Array.from(alertRecord())
	}
];

/** Accepts `"16 03 01"`, `"160301"`, `0x`-prefixed and newline-separated dumps. */
export function parseTlsHex(input: string): Uint8Array {
	const cleaned = input.replace(/0x/gi, '').replace(/[\s,:_|]+/g, '');
	if (cleaned.length === 0) return new Uint8Array();
	if (cleaned.length % 2 !== 0) throw new Error('That is an odd number of hex digits.');
	if (/[^0-9a-f]/i.test(cleaned)) throw new Error('That has characters that are not hex digits.');
	const out = new Uint8Array(cleaned.length / 2);
	for (let i = 0; i < out.length; i++) out[i] = parseInt(cleaned.slice(i * 2, i * 2 + 2), 16);
	return out;
}
