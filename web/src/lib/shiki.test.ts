import { expect, test } from "vitest";
import { getHighlighter, srctypeToLang } from "./shiki";

test("the JavaScript regex engine tokenizes every source type", async () => {
  const samples = {
    text: "# Heading",
    latex: "\\frac{a}{b}",
    python: "def identity(value):\n    return value",
    lean: "theorem identity (p : Prop) : p → p := fun h => h",
    rocq: "Theorem identity (P : Prop) : P -> P. Proof. auto. Qed.",
  };

  for (const [sourceType, source] of Object.entries(samples)) {
    const lang = srctypeToLang(sourceType);
    const highlighter = await getHighlighter(lang);
    expect(highlighter.codeToTokens(source, { lang, theme: "github-light" }).tokens.flat()).not.toHaveLength(0);
  }
});
