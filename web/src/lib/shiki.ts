// Shiki highlighter singleton.
// Pre-loads all mdc srctype grammars so the editor has highlighting
// once the WASM engine is loaded (async, ~200ms on first use).
//
// All modules are imported dynamically so Shiki + the oniguruma WASM
// (~1 MB, inlined as base64) stay out of the initial bundle. The chunk is
// fetched the first time a source block mounts, not on page load.

import type { HighlighterCore } from "shiki/core";

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
    highlighterPromise = (async () => {
      const [
        { createHighlighterCore },
        { createOnigurumaEngine },
        getWasm,
        markdown,
        latex,
        python,
        lean,
        coq,
        tokyoNight,
      ] = await Promise.all([
        import("shiki/core"),
        import("shiki/engine/oniguruma"),
        import("shiki/wasm"),
        import("@shikijs/langs/markdown"),
        import("@shikijs/langs/latex"),
        import("@shikijs/langs/python"),
        import("@shikijs/langs/lean"),
        import("@shikijs/langs/coq"),
        import("@shikijs/themes/tokyo-night"),
      ]);
      return createHighlighterCore({
        langs: [
          markdown.default,
          latex.default,
          python.default,
          lean.default,
          coq.default,
        ],
        themes: [tokyoNight.default],
        engine: createOnigurumaEngine(getWasm.default),
      });
    })().catch((error) => {
      // Allow a retry on the next call instead of caching a rejection forever.
      highlighterPromise = null;
      throw error;
    });
  }
  return highlighterPromise;
}

export function srctypeToLang(srctype: string): string {
  return SRCTYPE_TO_LANG[srctype] ?? "markdown";
}
