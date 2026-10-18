/** One line of a line diff: kept, removed from the old text, or added in the new one. */
export interface DiffLine {
  op: " " | "-" | "+";
  text: string;
}

/** Texts above this many line pairs are shown as all removed, then all added. */
const MAX_CELLS = 4_000_000;

const lines = (text: string) => (text === "" ? [] : text.replace(/\n$/, "").split("\n"));

/**
 * The line diff of two texts by longest common subsequence: removed lines come before the
 * added lines that replace them. A writeback previews Lean blocks of a few hundred lines.
 */
export function lineDiff(before: string, after: string): DiffLine[] {
  const a = lines(before), b = lines(after);
  const m = a.length, n = b.length;
  if (m * n > MAX_CELLS)
    return [...a.map((text) => ({ op: "-" as const, text })), ...b.map((text) => ({ op: "+" as const, text }))];
  // common[i][j]: length of the longest common subsequence of a[i..] and b[j..].
  const common = Array.from({ length: m + 1 }, () => new Uint32Array(n + 1));
  for (let i = m - 1; i >= 0; i--)
    for (let j = n - 1; j >= 0; j--)
      common[i][j] = a[i] === b[j] ? common[i + 1][j + 1] + 1 : Math.max(common[i + 1][j], common[i][j + 1]);
  const out: DiffLine[] = [];
  let i = 0, j = 0;
  while (i < m && j < n) {
    if (a[i] === b[j]) { out.push({ op: " ", text: a[i] }); i++; j++; }
    else if (common[i + 1][j] >= common[i][j + 1]) out.push({ op: "-", text: a[i++] });
    else out.push({ op: "+", text: b[j++] });
  }
  while (i < m) out.push({ op: "-", text: a[i++] });
  while (j < n) out.push({ op: "+", text: b[j++] });
  return out;
}
