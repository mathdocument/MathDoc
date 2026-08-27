// Shiki highlighter singleton with grammars loaded on demand.
//
// All modules are imported dynamically so Shiki + the oniguruma WASM
// (~1 MB, inlined as base64) stay out of the initial bundle. The chunk is
// fetched the first time a source block mounts, not on page load.

import type { HighlighterCore } from "shiki/core";

const LANGUAGE_LOADERS = {
  markdown: () => import("@shikijs/langs/markdown").then((module) => module.default),
  latex: () => import("@shikijs/langs/latex").then((module) => module.default),
  python: () => import("@shikijs/langs/python").then((module) => module.default),
  lean: () => import("@shikijs/langs/lean").then((module) => module.default),
  coq: () => import("@shikijs/langs/coq").then((module) => module.default),
} as const;

type MdcLanguage = keyof typeof LANGUAGE_LOADERS;

// Map mdc srctype → Shiki language id.
const SRCTYPE_TO_LANG: Record<string, MdcLanguage> = {
  text: "markdown",
  latex: "latex",
  python: "python",
  lean: "lean",
  rocq: "coq",
};

let highlighterPromise: Promise<HighlighterCore> | null = null;
const languagePromises = new Map<MdcLanguage, Promise<void>>();

function getHighlighterCore(): Promise<HighlighterCore> {
  if (!highlighterPromise) {
    highlighterPromise = (async () => {
      const [
        { createHighlighterCore },
        { createOnigurumaEngine },
        getWasm,
        tokyoNight,
        githubLight,
      ] = await Promise.all([
        import("shiki/core"),
        import("shiki/engine/oniguruma"),
        import("shiki/wasm"),
        import("@shikijs/themes/tokyo-night"),
        import("@shikijs/themes/github-light"),
      ]);
      return createHighlighterCore({
        langs: [],
        themes: [tokyoNight.default, githubLight.default],
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

export async function getHighlighter(lang: MdcLanguage): Promise<HighlighterCore> {
  const highlighter = await getHighlighterCore();
  let languagePromise = languagePromises.get(lang);
  if (!languagePromise) {
    languagePromise = LANGUAGE_LOADERS[lang]()
      .then((language) => highlighter.loadLanguage(language))
      .catch((error) => {
        languagePromises.delete(lang);
        throw error;
      });
    languagePromises.set(lang, languagePromise);
  }
  await languagePromise;
  return highlighter;
}

export function srctypeToLang(srctype: string): MdcLanguage {
  return SRCTYPE_TO_LANG[srctype] ?? "markdown";
}
