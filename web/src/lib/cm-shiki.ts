// Minimal CodeMirror 6 syntax highlighting via Shiki.
// Calls Shiki's codeToTokens on the full document and creates
// inline color decorations.
//
// Key insight: Shiki's token.offset is ABSOLUTE (document-relative),
// not line-relative. So we use it directly as the CodeMirror position.
//
// Re-tokenizing runs on a short debounce instead of on every keystroke.
// Large documents are processed incrementally so they remain highlighted
// without monopolizing the main thread.

import { ViewPlugin, type DecorationSet, Decoration, EditorView, type ViewUpdate } from "@codemirror/view";
import { StateEffect } from "@codemirror/state";
import type { Extension, Range, Text } from "@codemirror/state";
import type { HighlighterCore } from "shiki/core";
import type { GrammarState } from "@shikijs/types";

/** Trigger a re-decorate pass without touching the document. */
const recomputeEffect = StateEffect.define<null>();

/** Notify CodeMirror that a progressive chunk added decorations. */
const redrawEffect = StateEffect.define<number>();

/** Keep syntax decoration memory and retokenization work bounded. */
const MAX_HIGHLIGHT_LENGTH = 100_000;

/** Each callback handles only a small amount of text and a few grammar lines. */
const HIGHLIGHT_CHUNK_LENGTH = 4_000;
const HIGHLIGHT_CHUNK_LINES = 8;

/** Bound pathological grammar work within a single main-thread task. */
const TOKENIZE_TIME_LIMIT_MS = 5;

/** Debounce window after the last keystroke before re-tokenizing. */
const HIGHLIGHT_DEBOUNCE_MS = 140;

export function shikiHighlight(
  highlighter: HighlighterCore,
  lang: string,
  theme: string,
  onError?: (error: unknown | null) => void,
): Extension {
  return ViewPlugin.fromClass(
    class {
      decorations: DecorationSet = Decoration.none;
      private timer: ReturnType<typeof setTimeout> | null = null;
      private run = 0;
      private errorReported = false;
      private marksByColor = new Map<string, Decoration>();

      constructor(view: EditorView) {
        this.recompute(view);
      }

      update(update: ViewUpdate) {
        if (update.docChanged) {
          // Stop chunks for the old document immediately. Keep the existing
          // colors mapped until the debounced replacement is ready.
          this.run++;
          this.decorations = this.decorations.map(update.changes);
          this.schedule(update.view);
        } else if (
          update.transactions.some((transaction) =>
            transaction.effects.some((effect) => effect.is(recomputeEffect)),
          )
        ) {
          this.recompute(update.view);
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

      private recompute(view: EditorView) {
        const run = ++this.run;
        this.errorReported = false;
        const doc = view.state.doc;
        if (doc.length === 0 || doc.length > MAX_HIGHLIGHT_LENGTH) {
          this.decorations = Decoration.none;
          onError?.(null);
          return;
        }

        this.decorations = Decoration.none;
        this.scheduleChunk(view, doc, 0, undefined, run);
      }

      private scheduleChunk(
        view: EditorView,
        doc: Text,
        from: number,
        grammarState: GrammarState | undefined,
        run: number,
      ) {
        setTimeout(() => {
          if (run !== this.run || !view.dom.isConnected || view.state.doc !== doc) return;

          let to = from;
          let skipChunk = false;
          for (let lineCount = 0; lineCount < HIGHLIGHT_CHUNK_LINES && to < doc.length; lineCount++) {
            const line = doc.lineAt(to);
            const lineEnd = line.to < doc.length ? line.to + 1 : line.to;
            if (line.to - line.from > HIGHLIGHT_CHUNK_LENGTH) {
              if (to === from) {
                to = lineEnd;
                skipChunk = true;
              }
              break;
            }
            if (to > from && lineEnd - from > HIGHLIGHT_CHUNK_LENGTH) break;
            to = lineEnd;
          }
          if (skipChunk) {
            if (to < doc.length) this.scheduleChunk(view, doc, to, undefined, run);
            else if (!this.errorReported) onError?.(null);
            return;
          }
          const text = doc.sliceString(from, to);

          try {
            const result = highlighter.codeToTokens(text, {
              lang: lang as never,
              theme,
              grammarState,
              tokenizeMaxLineLength: HIGHLIGHT_CHUNK_LENGTH,
              tokenizeTimeLimit: TOKENIZE_TIME_LIMIT_MS,
            });
            const additions = this.tokenRanges(result.tokens, from, to);
            if (additions.length > 0) {
              this.decorations = this.decorations.update({ add: additions, sort: true });
            }
            view.dispatch({ effects: redrawEffect.of(run) });

            if (to < doc.length) {
              this.scheduleChunk(view, doc, to, result.grammarState, run);
            } else if (!this.errorReported) {
              onError?.(null);
            }
          } catch (error) {
            this.reportError(error);
            // A pathological chunk should not prevent later parts of a large
            // document from receiving best-effort highlighting.
            if (to < doc.length) {
              this.scheduleChunk(view, doc, to, undefined, run);
            }
          }
        }, 0);
      }

      private reportError(error: unknown) {
        if (this.errorReported) return;
        this.errorReported = true;
        if (onError) onError(error);
        else console.warn("Shiki failed to highlight a document chunk", error);
      }

      private markForColor(color: string): Decoration {
        let mark = this.marksByColor.get(color);
        if (!mark) {
          mark = Decoration.mark({ attributes: { style: `color: ${color}` } });
          this.marksByColor.set(color, mark);
        }
        return mark;
      }

      private tokenRanges(
        lines: ReturnType<HighlighterCore["codeToTokens"]>["tokens"],
        base: number,
        limit: number,
      ): Range<Decoration>[] {
        const ranges: Range<Decoration>[] = [];
        for (const lineTokens of lines) {
          for (const token of lineTokens) {
            const from = base + token.offset;
            const to = Math.min(limit, from + token.content.length);
            if (token.color && from < to) {
              ranges.push(this.markForColor(token.color).range(from, to));
            }
          }
        }
        return ranges;
      }

      destroy() {
        this.run++;
        if (this.timer) clearTimeout(this.timer);
      }
    },
    {
      decorations: (view) => view.decorations,
    },
  );
}
