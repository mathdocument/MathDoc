// Shiki highlighter singleton.
// Pre-loads all mdc srctype grammars so the editor has highlighting
// once the WASM engine is loaded (async, ~200ms on first use).

import { createHighlighterCore, type HighlighterCore } from "shiki/core";
import { createOnigurumaEngine } from "shiki/engine/oniguruma";
import getWasm from "shiki/wasm";
import markdown from "@shikijs/langs/markdown";
import latex from "@shikijs/langs/latex";
import python from "@shikijs/langs/python";
import lean from "@shikijs/langs/lean";
import coq from "@shikijs/langs/coq";
import tokyoNight from "@shikijs/themes/tokyo-night";

// Map mdc srctype → Shiki language id.
const SRCTYPE_TO_LANG: Record<string, string> = {
  text: "markdown",
  latex: "latex",
  python: "python",
  lean: "lean",
  rocq: "coq",
};

let highlighterPromise: Promise<HighlighterCore> | null = null;

export function getHighlighter(): Promise<HighlighterCore> {
  if (!highlighterPromise) {
    highlighterPromise = createHighlighterCore({
      langs: [markdown, latex, python, lean, coq],
      themes: [tokyoNight],
      engine: createOnigurumaEngine(getWasm),
    });
  }
  return highlighterPromise;
}

export function srctypeToLang(srctype: string): string {
  return SRCTYPE_TO_LANG[srctype] ?? "markdown";
}
