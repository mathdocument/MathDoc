// Minimal CodeMirror 6 syntax highlighting via Shiki.
// Calls Shiki's codeToTokens on the full document and creates
// inline color decorations.
//
// Key insight: Shiki's token.offset is ABSOLUTE (document-relative),
// not line-relative. So we use it directly as the CodeMirror position.
//
// Re-tokenizing runs on a short debounce instead of on every keystroke,
// and is skipped entirely for very large documents, so typing stays
// responsive even in long blocks.

import { ViewPlugin, type DecorationSet, Decoration, EditorView, type ViewUpdate } from "@codemirror/view";
import { RangeSetBuilder, StateEffect } from "@codemirror/state";
import type { Extension } from "@codemirror/state";
import type { HighlighterCore } from "shiki/core";

/** Trigger a re-decorate pass without touching the document. */
const recomputeEffect = StateEffect.define<null>();

/** Beyond this size, whole-doc tokenization would stall typing — fall back to plain text. */
const MAX_HIGHLIGHT_LENGTH = 40_000;

/** Debounce window after the last keystroke before re-tokenizing. */
const HIGHLIGHT_DEBOUNCE_MS = 140;

export function shikiHighlight(
  highlighter: HighlighterCore,
  lang: string,
  theme: string,
): Extension {
  return ViewPlugin.fromClass(
    class {
      decorations: DecorationSet = Decoration.none;
      private timer: ReturnType<typeof setTimeout> | null = null;

      constructor(view: EditorView) {
        this.decorations = this.highlight(view);
      }

      update(update: ViewUpdate) {
        if (update.docChanged) {
          this.schedule(update.view);
        } else if (
          update.transactions.some((transaction) =>
            transaction.effects.some((effect) => effect.is(recomputeEffect)),
          )
        ) {
          this.decorations = this.highlight(update.view);
        }
      }

      private schedule(view: EditorView) {
        if (this.timer) clearTimeout(this.timer);
        this.timer = setTimeout(() => {
          this.timer = null;
          // `destroy()` removes the editor DOM, so this doubles as the
          // "view still alive" check.
          if (view.dom.isConnected) {
            view.dispatch({ effects: recomputeEffect.of(null) });
          }
        }, HIGHLIGHT_DEBOUNCE_MS);
      }

      highlight(view: EditorView): DecorationSet {
        const doc = view.state.doc;
        if (doc.length === 0 || doc.length > MAX_HIGHLIGHT_LENGTH) return Decoration.none;

        try {
          const text = doc.toString();
          const result = highlighter.codeToTokens(text, { lang: lang as never, theme });
          const builder = new RangeSetBuilder<Decoration>();

          for (const lineTokens of result.tokens) {
            for (const token of lineTokens) {
              if (token.color) {
                builder.add(
                  token.offset,
                  token.offset + token.content.length,
                  Decoration.mark({
                    attributes: { style: `color: ${token.color}` },
                  }),
                );
              }
            }
          }

          return builder.finish();
        } catch {
          return Decoration.none;
        }
      }
    },
    {
      decorations: (view) => view.decorations,
    },
  );
}
