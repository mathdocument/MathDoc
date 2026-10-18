// The Markdown toolbar's commands as text edits: each takes the text and a selection and
// returns one replacement and the selection after it, so any editor applies it as a single
// undoable edit (lib/markdown-monaco.ts). Commands toggle: applied twice, they undo themselves.

export type MarkdownCommand =
  | "heading1" | "heading2" | "heading3"
  | "bold" | "italic" | "code" | "math"
  | "displayMath" | "codeBlock"
  | "bulletList" | "numberedList" | "quote"
  | "link";

/** Replace `text[from, to)` with `insert`; `selection` is in the resulting text. */
export interface MarkdownEdit { from: number; to: number; insert: string; selection: [number, number] }

const INLINE: Partial<Record<MarkdownCommand, { mark: string; placeholder: string }>> = {
  bold: { mark: "**", placeholder: "bold text" },
  italic: { mark: "*", placeholder: "italic text" },
  code: { mark: "`", placeholder: "code" },
  math: { mark: "$", placeholder: "x^2" },
};

export function markdownEdit(text: string, from: number, to: number, command: MarkdownCommand): MarkdownEdit {
  if (from > to) [from, to] = [to, from];
  const inline = INLINE[command];
  if (inline) return wrap(text, from, to, inline.mark, inline.placeholder);
  switch (command) {
    case "heading1": case "heading2": case "heading3":
      return lines(text, from, to, (ls) => {
        const level = Number(command.at(-1));
        const current = ls.map((l) => /^(#{1,6}) /.exec(l)?.[1].length ?? 0);
        const off = current.every((n) => n === level);
        return ls.map((l, i) => (off ? "" : `${"#".repeat(level)} `) + l.slice(current[i] ? current[i] + 1 : 0));
      });
    case "bulletList":
      return prefixes(text, from, to, /^[-*+] /, () => "- ");
    case "numberedList":
      return prefixes(text, from, to, /^\d+[.)] /, (i) => `${i + 1}. `);
    case "quote":
      return prefixes(text, from, to, /^> ?/, () => "> ");
    case "displayMath":
      return block(text, from, to, "$$", "$$", "x^2");
    case "codeBlock":
      return block(text, from, to, "```lean", "```", "");
    case "link": {
      const label = text.slice(from, to) || "text";
      const insert = `[${label}](url)`;
      const at = from + label.length + 3;
      return { from, to, insert, selection: [at, at + 3] };
    }
  }
  return { from, to, insert: text.slice(from, to), selection: [from, to] };
}

/** How many `c` run up to `at` (backwards) or from it (forwards). */
function run(text: string, at: number, c: string, back: boolean) {
  let n = 0;
  while (back ? text[at - n - 1] === c : text[at + n] === c) n++;
  return n;
}

/**
 * Wrap the selection in `mark`, or unwrap it when it is wrapped already, inside or just
 * around the selection. `*` and `**` share a character: italic is on when the stars on each
 * side are odd in number, bold when there are at least two.
 */
function wrap(text: string, from: number, to: number, mark: string, placeholder: string): MarkdownEdit {
  const c = mark[0];
  const w = mark.length;
  const on = (n: number) => (c === "*" ? (w === 1 ? n % 2 === 1 : n >= 2) : n >= w);
  const sel = text.slice(from, to);
  const inside = Math.min(run(sel, 0, c, false), run(sel, sel.length, c, true));
  if (sel.length >= 2 * w && inside < sel.length && on(inside)) {
    const insert = sel.slice(w, sel.length - w);
    return { from, to, insert, selection: [from, from + insert.length] };
  }
  if (on(Math.min(run(text, from, c, true), run(text, to, c, false)))) {
    return { from: from - w, to: to + w, insert: sel, selection: [from - w, to - w] };
  }
  const inner = sel || placeholder;
  return { from, to, insert: mark + inner + mark, selection: [from + w, from + w + inner.length] };
}

/** Rewrite the whole lines the selection touches; a caret keeps its place in its line. */
function lines(text: string, from: number, to: number, rewrite: (ls: string[]) => string[]): MarkdownEdit {
  const start = text.lastIndexOf("\n", from - 1) + 1;
  // A selection ending at the start of a line does not take that line.
  const last = to > from && text[to - 1] === "\n" ? to - 1 : to;
  const nl = text.indexOf("\n", last);
  const end = nl < 0 ? text.length : nl;
  const old = text.slice(start, end).split("\n");
  const next = rewrite(old);
  const insert = next.join("\n");
  if (from === to) {
    const column = Math.min(Math.max(from - start + next[0].length - old[0].length, 0), next[0].length);
    return { from: start, to: end, insert, selection: [start + column, start + column] };
  }
  return { from: start, to: end, insert, selection: [start, start + insert.length] };
}

/** Toggle a line prefix: removed when every non-empty line has one, else added to each. */
function prefixes(text: string, from: number, to: number, has: RegExp, prefix: (i: number) => string) {
  return lines(text, from, to, (ls) => {
    const filled = ls.filter((l) => l.trim());
    const off = filled.length > 0 && filled.every((l) => has.test(l));
    let i = 0;
    return ls.map((l) => (off ? l.replace(has, "") : !l.trim() && ls.length > 1 ? l : prefix(i++) + l.replace(has, "")));
  });
}

/** Fence the selection on lines of its own: `open`, the selection (or `placeholder`), `close`. */
function block(text: string, from: number, to: number, open: string, close: string, placeholder: string): MarkdownEdit {
  const inner = text.slice(from, to) || placeholder;
  const before = from > 0 && text[from - 1] !== "\n" ? "\n" : "";
  const after = to < text.length && text[to] !== "\n" ? "\n" : "";
  const insert = `${before}${open}\n${inner}\n${close}${after}`;
  const at = from + before.length + open.length + 1;
  return { from, to, insert, selection: [at, at + inner.length] };
}

/** Apply an edit to a string, for tests and plain text fields. */
export function applyEdit(text: string, e: MarkdownEdit): string {
  return text.slice(0, e.from) + e.insert + text.slice(e.to);
}
