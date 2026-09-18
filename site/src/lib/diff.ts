/** A tiny line diff (LCS) so failures render like shelltest's `-expected / +actual` blocks. */

export interface DiffLine {
	sign: ' ' | '-' | '+';
	text: string;
}

export function lineDiff(expected: string, actual: string): DiffLine[] {
	const a = expected.split('\n');
	const b = actual.split('\n');
	const n = a.length;
	const m = b.length;
	// LCS table; the inputs here are test fixtures, never megabytes.
	const lcs: number[][] = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0));
	for (let i = n - 1; i >= 0; i--) {
		for (let j = m - 1; j >= 0; j--) {
			lcs[i][j] = a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
		}
	}
	const out: DiffLine[] = [];
	let i = 0;
	let j = 0;
	while (i < n && j < m) {
		if (a[i] === b[j]) {
			out.push({ sign: ' ', text: a[i] });
			i++;
			j++;
		} else if (lcs[i + 1][j] >= lcs[i][j + 1]) {
			out.push({ sign: '-', text: a[i++] });
		} else {
			out.push({ sign: '+', text: b[j++] });
		}
	}
	while (i < n) out.push({ sign: '-', text: a[i++] });
	while (j < m) out.push({ sign: '+', text: b[j++] });
	return out;
}

export function hasDifference(lines: DiffLine[]): boolean {
	return lines.some((l) => l.sign !== ' ');
}
