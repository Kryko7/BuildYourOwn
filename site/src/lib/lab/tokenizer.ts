/**
 * A POSIX-flavoured shell tokenizer used by the /lab playground. It mirrors the
 * state machine the stages ask you to build: NORMAL / IN_SINGLE / IN_DOUBLE plus
 * backslash handling, and it reports the quote state of every single character so
 * the UI can colour the line.
 */

export type SpanKind =
	| 'plain'
	| 'single'
	| 'double'
	| 'escape'
	| 'expansion'
	| 'operator'
	| 'space'
	| 'comment'
	| 'unterminated';

export interface Span {
	start: number;
	end: number;
	kind: SpanKind;
	/** index into `tokens`, or -1 for whitespace/comments */
	token: number;
}

export interface Token {
	index: number;
	raw: string;
	value: string;
	kind: 'word' | 'operator';
	quoted: boolean;
	start: number;
	end: number;
	expansions: string[];
}

export interface Redirect {
	op: string;
	fd: number | null;
	target: string;
	description: string;
}

export interface Command {
	argv: string[];
	redirects: Redirect[];
}

export interface TokenizeResult {
	spans: Span[];
	tokens: Token[];
	pipeline: Command[];
	separators: string[];
	warnings: string[];
	unterminated: null | 'single' | 'double' | 'escape';
}

const OPERATORS = ['>>', '<<', '&&', '||', '2>>', '2>', '1>>', '1>', '&>', '>', '<', '|', ';', '&'];

const REDIRECT_HELP: Record<string, string> = {
	'>': 'truncate and write stdout',
	'1>': 'truncate and write stdout (explicit fd 1)',
	'>>': 'append stdout',
	'1>>': 'append stdout (explicit fd 1)',
	'2>': 'truncate and write stderr',
	'2>>': 'append stderr',
	'<': 'read stdin from the file',
	'&>': 'open once, dup onto both stdout and stderr'
};

function matchOperator(src: string, i: number): string | null {
	// longest match first: 2>> before 2> before >
	const candidates = [...OPERATORS].sort((a, b) => b.length - a.length);
	for (const op of candidates) if (src.startsWith(op, i)) return op;
	return null;
}

export function tokenize(input: string): TokenizeResult {
	const spans: Span[] = [];
	const tokens: Token[] = [];
	const warnings: string[] = [];
	let unterminated: TokenizeResult['unterminated'] = null;

	let i = 0;
	let word = '';
	let raw = '';
	let wordStart = -1;
	let quoted = false;
	let expansions: string[] = [];

	const push = (start: number, end: number, kind: SpanKind) => {
		if (end <= start) return;
		const prev = spans[spans.length - 1];
		if (prev && prev.kind === kind && prev.end === start) {
			prev.end = end;
			return;
		}
		spans.push({ start, end, kind, token: -1 });
	};

	const flush = (end: number) => {
		if (wordStart < 0) return;
		tokens.push({
			index: tokens.length,
			raw,
			value: word,
			kind: 'word',
			quoted,
			start: wordStart,
			end,
			expansions
		});
		word = '';
		raw = '';
		wordStart = -1;
		quoted = false;
		expansions = [];
	};

	while (i < input.length) {
		const ch = input[i];

		if (ch === ' ' || ch === '\t') {
			flush(i);
			const start = i;
			while (i < input.length && (input[i] === ' ' || input[i] === '\t')) i++;
			push(start, i, 'space');
			continue;
		}

		if (ch === '#' && wordStart < 0) {
			flush(i);
			push(i, input.length, 'comment');
			i = input.length;
			break;
		}

		const op = matchOperator(input, i);
		if (op) {
			flush(i);
			tokens.push({
				index: tokens.length,
				raw: op,
				value: op,
				kind: 'operator',
				quoted: false,
				start: i,
				end: i + op.length,
				expansions: []
			});
			push(i, i + op.length, 'operator');
			i += op.length;
			continue;
		}

		if (wordStart < 0) wordStart = i;

		if (ch === '\\') {
			if (i + 1 >= input.length) {
				push(i, i + 1, 'unterminated');
				unterminated = 'escape';
				warnings.push('Line ends with a backslash — a real shell would ask for a continuation line.');
				raw += ch;
				i++;
				continue;
			}
			push(i, i + 2, 'escape');
			word += input[i + 1];
			raw += input.slice(i, i + 2);
			quoted = true;
			i += 2;
			continue;
		}

		if (ch === "'") {
			const start = i;
			i++;
			let closed = false;
			while (i < input.length) {
				if (input[i] === "'") {
					closed = true;
					i++;
					break;
				}
				word += input[i];
				i++;
			}
			raw += input.slice(start, i);
			quoted = true;
			if (closed) push(start, i, 'single');
			else {
				push(start, i, 'unterminated');
				unterminated = 'single';
				warnings.push('Unterminated single quote — every byte after it is literal, including newlines.');
			}
			continue;
		}

		if (ch === '"') {
			const start = i;
			// `segStart` is the beginning of the run of quoted text not yet emitted. It has to
			// advance past every escape and expansion span, or the closing push re-emits the
			// whole quoted region and the rendered line shows its contents twice.
			let segStart = i;
			i++;
			let closed = false;
			while (i < input.length) {
				const c = input[i];
				if (c === '"') {
					closed = true;
					i++;
					break;
				}
				if (c === '\\' && i + 1 < input.length && '"\\$`\n'.includes(input[i + 1])) {
					push(segStart, i, 'double');
					push(i, i + 2, 'escape');
					word += input[i + 1];
					i += 2;
					segStart = i;
					continue;
				}
				if (c === '$' && i + 1 < input.length) {
					const exp = readExpansion(input, i);
					if (exp) {
						push(segStart, i, 'double');
						push(i, exp.end, 'expansion');
						expansions.push(exp.name);
						word += `\u2039${exp.name}\u203a`;
						i = exp.end;
						segStart = i;
						continue;
					}
				}
				word += c;
				i++;
			}
			raw += input.slice(start, i);
			quoted = true;
			if (closed) push(segStart, i, 'double');
			else {
				push(segStart, i, 'unterminated');
				unterminated = 'double';
				warnings.push('Unterminated double quote — the shell prints a `> ` continuation prompt (stage 57).');
			}
			continue;
		}

		if (ch === '$') {
			const exp = readExpansion(input, i);
			if (exp) {
				push(i, exp.end, 'expansion');
				expansions.push(exp.name);
				word += `\u2039${exp.name}\u203a`;
				raw += input.slice(i, exp.end);
				i = exp.end;
				continue;
			}
		}

		push(i, i + 1, 'plain');
		word += ch;
		raw += ch;
		i++;
	}
	flush(input.length);

	// Attach each span to the token that covers it, so hovering a token can highlight
	// every character that produced it (quotes included).
	for (const span of spans) {
		if (span.kind === 'space' || span.kind === 'comment') continue;
		const t = tokens.find((t) => t.start <= span.start && span.start < t.end);
		span.token = t ? t.index : -1;
	}

	const { pipeline, separators } = buildPipeline(tokens);
	return { spans, tokens, pipeline, separators, warnings, unterminated };
}

function readExpansion(src: string, i: number): { name: string; end: number } | null {
	if (src[i] !== '$') return null;
	const next = src[i + 1];
	if (next === '{') {
		const close = src.indexOf('}', i + 2);
		if (close < 0) return null;
		return { name: src.slice(i + 2, close), end: close + 1 };
	}
	if (next === '?' || next === '$' || next === '#' || next === '!') {
		return { name: next, end: i + 2 };
	}
	if (next && /[A-Za-z_]/.test(next)) {
		let j = i + 1;
		while (j < src.length && /[A-Za-z0-9_]/.test(src[j])) j++;
		return { name: src.slice(i + 1, j), end: j };
	}
	return null;
}

function buildPipeline(tokens: Token[]): { pipeline: Command[]; separators: string[] } {
	const pipeline: Command[] = [];
	const separators: string[] = [];
	let current: Command = { argv: [], redirects: [] };
	let pendingRedirect: string | null = null;

	for (const t of tokens) {
		if (t.kind === 'operator') {
			if (['>', '>>', '<', '2>', '2>>', '1>', '1>>', '&>'].includes(t.value)) {
				pendingRedirect = t.value;
				continue;
			}
			if (t.value === '|' || t.value === ';' || t.value === '&&' || t.value === '||' || t.value === '&') {
				pipeline.push(current);
				separators.push(t.value);
				current = { argv: [], redirects: [] };
				continue;
			}
			continue;
		}
		if (pendingRedirect) {
			const op = pendingRedirect;
			const fd = op.startsWith('2') ? 2 : op.startsWith('1') ? 1 : op === '<' ? 0 : op === '&>' ? null : 1;
			current.redirects.push({
				op,
				fd,
				target: t.value,
				description: REDIRECT_HELP[op] ?? 'redirect'
			});
			pendingRedirect = null;
			continue;
		}
		current.argv.push(t.value);
	}
	pipeline.push(current);
	return { pipeline, separators };
}

export const spanLegend: { kind: SpanKind; label: string }[] = [
	{ kind: 'plain', label: 'unquoted' },
	{ kind: 'single', label: "inside ' '" },
	{ kind: 'double', label: 'inside " "' },
	{ kind: 'escape', label: 'backslash escape' },
	{ kind: 'expansion', label: '$expansion' },
	{ kind: 'operator', label: 'operator' },
	{ kind: 'comment', label: 'comment' },
	{ kind: 'unterminated', label: 'unterminated' }
];
