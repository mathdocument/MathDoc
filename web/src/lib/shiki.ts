// Shiki highlighter singleton with grammars loaded on demand.
//
// All modules are imported dynamically so Shiki stays out of the initial
// bundle. The chunks are fetched the first time a source block mounts.

import type { HighlighterCore } from "@shikijs/core";

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
        { createJavaScriptRegexEngine },
        tokyoNight,
        githubLight,
      ] = await Promise.all([
        import("@shikijs/core"),
        import("@shikijs/engine-javascript"),
        import("@shikijs/themes/tokyo-night"),
        import("@shikijs/themes/github-light"),
      ]);
      return createHighlighterCore({
        langs: [],
        themes: [tokyoNight.default, githubLight.default],
        engine: createJavaScriptRegexEngine(),
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
  const core = getHighlighterCore();
  let languagePromise = languagePromises.get(lang);
  if (!languagePromise) {
    languagePromise = Promise.all([core, LANGUAGE_LOADERS[lang]()])
      .then(([highlighter, language]) => highlighter.loadLanguage(language))
      .catch((error) => {
        languagePromises.delete(lang);
        throw error;
      });
    languagePromises.set(lang, languagePromise);
  }
  await languagePromise;
  return core;
}

export function srctypeToLang(srctype: string): MdcLanguage {
  return SRCTYPE_TO_LANG[srctype] ?? "markdown";
}
