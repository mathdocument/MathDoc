// Shiki highlighter singleton.
// Pre-loads all mdc srctype grammars so the editor has highlighting
// once the WASM engine is loaded (async, ~200ms on first use).

import { createHighlighter } from "shiki";
import type { Highlighter } from "shiki";

// Map mdc srctype → Shiki language id.
const SRCTYPE_TO_LANG: Record<string, string> = {
  text: "markdown",
  latex: "latex",
  python: "python",
  lean: "lean",
  lean4: "lean4",
  rocq: "coq",
};

let highlighterPromise: Promise<Highlighter> | null = null;

export function getHighlighter(): Promise<Highlighter> {
  if (!highlighterPromise) {
    highlighterPromise = createHighlighter({
      langs: ["markdown", "latex", "python", "lean", "lean4", "coq"],
      themes: ["tokyo-night"],
    });
  }
  return highlighterPromise;
}

export function srctypeToLang(srctype: string): string {
  return SRCTYPE_TO_LANG[srctype] ?? "markdown";
}
