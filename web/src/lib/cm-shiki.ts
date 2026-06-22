// Minimal CodeMirror 6 syntax highlighting via Shiki.
// Calls Shiki's codeToTokens on the full document and creates
// inline color decorations.
//
// Key insight: Shiki's token.offset is ABSOLUTE (document-relative),
// not line-relative. So we use it directly as the CodeMirror position.

import { ViewPlugin, type DecorationSet, Decoration, EditorView, type ViewUpdate } from "@codemirror/view";
import { RangeSetBuilder } from "@codemirror/state";
import type { Extension } from "@codemirror/state";
import type { Highlighter } from "shiki";

export function shikiHighlight(
  highlighter: Highlighter,
  lang: string,
  theme: string,
): Extension {
  return ViewPlugin.fromClass(
    class {
      decorations: DecorationSet = Decoration.none;

      constructor(view: EditorView) {
        this.decorations = this.highlight(view);
      }

      update(update: ViewUpdate) {
        // Only re-highlight when the document text actually changes.
        // viewportChanged alone (e.g. scrolling) doesn't need re-tokenizing.
        if (update.docChanged) {
          this.decorations = this.highlight(update.view);
        }
      }

      highlight(view: EditorView): DecorationSet {
        const doc = view.state.doc;
        if (doc.length === 0) return Decoration.none;

        try {
          const text = doc.toString();
          const result = highlighter.codeToTokens(text, { lang: lang as any, theme });
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
      decorations: (v) => v.decorations,
    },
  );
}
