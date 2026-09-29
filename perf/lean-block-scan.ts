// How many Lean sources the node scanner (coordinator/src/lean-source.ts) accepts.
//
//   npx tsx perf/lean-block-scan.ts <directory of .lean files> [--json]
//
// Two granularities, because MathDoc nodes are authored per statement while library
// files hold many declarations:
//   file: each file as one Lean block (as perf/mathlib-import.py imports Mathlib);
//   item: each top-level command of every file, classified on its own.
import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import { classify, mask, scan } from "../coordinator/src/lean-source.js";

async function* files(dir: string): AsyncGenerator<string> {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) yield* files(path);
    else if (entry.name.endsWith(".lean")) yield path;
  }
}

const [root, flag] = process.argv.slice(2);
if (!root) throw new Error("usage: lean-block-scan.ts <dir> [--json]");
const fileOutcome = new Map<string, number>();
const itemKind = new Map<string, number>();
let fileCount = 0;
const bump = (m: Map<string, number>, k: string) => m.set(k, (m.get(k) ?? 0) + 1);
for await (const path of files(root)) {
  fileCount++;
  const source = await readFile(path, "utf8");
  const s = scan(source);
  const c = classify(s, {});
  bump(fileOutcome, c.role === "unsupported" ? `unsupported:${c.rejection.reason}` : c.role);
  // Items: re-scan each top-level command in isolation (imports/context are prelude).
  // Command starts come from the masked text, so comment and string lines never count.
  const lines = mask(source).split("\n");
  let offset = 0;
  const starts: number[] = [];
  for (const line of lines) {
    if (line.length && !/\s/.test(line[0])) starts.push(offset);
    offset += line.length + 1;
  }
  for (const [i, from] of starts.entries()) {
    const text = source.slice(from, starts[i + 1] ?? source.length);
    const first = text.trimStart().split(/\s+/)[0];
    const item = scan(text);
    if (!item.decls.length && !item.rejection) {
      bump(itemKind, `prelude:${first}`);
      continue;
    }
    const r = classify(item, {});
    bump(
      itemKind,
      r.role === "unsupported"
        ? `unsupported:${first}`
        : r.role === "theorem"
          ? `theorem${r.conclusion.sorry ? ":sorry" : ""}`
          : "definition",
    );
  }
}
const sorted = (m: Map<string, number>) =>
  [...m.entries()].sort((a, b) => b[1] - a[1]);
if (flag === "--json")
  console.log(JSON.stringify({ files: fileCount, file: sorted(fileOutcome), item: sorted(itemKind) }));
else {
  console.log(`files: ${fileCount}`);
  for (const [k, n] of sorted(fileOutcome)) console.log(`  file ${k}: ${n}`);
  const total = [...itemKind.values()].reduce((a, b) => a + b, 0);
  console.log(`top-level items: ${total}`);
  for (const [k, n] of sorted(itemKind).slice(0, 40))
    console.log(`  ${k}: ${n} (${((100 * n) / total).toFixed(1)}%)`);
}
