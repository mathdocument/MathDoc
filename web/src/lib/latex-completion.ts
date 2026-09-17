import type { editor } from 'monaco-editor';
import type { LatexSession } from './latex-session.svelte';

/** Completion data is scoped to this node's dependencies and the shared bibliography. */
export function latexCompletions(session: Pick<LatexSession, 'references' | 'catalog'>, before: string) {
  const reference = /\\(cite|ref|cref|Cref|nameref|eqref)\s*(?:\[[^\]]*\])?\{([^{}]*)$/.exec(before);
  if (reference) {
    const fragment = reference[2].split(',').pop()!;
    const options = reference[1] === 'cite'
      ? (session.catalog?.citations ?? []).map(cite => ({label: cite.key, search: `${cite.key} ${cite.title} ${cite.authors} ${cite.year}`, detail: [cite.title, cite.authors, cite.year].filter(Boolean).join(' · '), insert: cite.key}))
      : session.references.map(ref => ({label: ref.name || ref.label, search: `${ref.label} ${ref.name} ${ref.title}`, detail: [ref.title, ref.label].filter(Boolean).join(' · '), insert: ref.label}));
    return {length: fragment.trimStart().length, options};
  }
  const environment = /\\(?:begin|end)\{([A-Za-z*]*)$/.exec(before);
  const command = /\\([A-Za-z]*)$/.exec(before);
  const match = environment ?? command;
  if (!match) return null;
  return {length: match[1].length, options: (environment ? session.catalog?.environments : session.catalog?.commands ?? [])?.map(label => ({label, search: label, detail: '', insert: label})) ?? []};
}

export async function latexAutocomplete(session: LatexSession, target: editor.ITextModel) {
  const {languages} = await import('monaco-editor');
  return languages.registerCompletionItemProvider('latex', {
    triggerCharacters: ['\\', '{', ','],
    provideCompletionItems(model, position) {
      if (model !== target) return {suggestions: []};
      const offset = model.getOffsetAt(position);
      const before = model.getValue().slice(Math.max(0, offset - 1024), offset);
      const result = latexCompletions(session, before);
      if (!result) return {suggestions: []};
      const from = model.getPositionAt(offset - result.length);
      return {suggestions: result.options.map(item => ({
        label: {label: item.label, description: item.detail}, detail: item.detail,
        filterText: item.search, insertText: item.insert, kind: languages.CompletionItemKind.Reference,
        range: {startLineNumber: from.lineNumber, startColumn: from.column, endLineNumber: position.lineNumber, endColumn: position.column},
      }))};
    },
  });
}
