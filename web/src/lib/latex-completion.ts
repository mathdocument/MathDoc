import type { editor } from 'monaco-editor';
import type { LatexSession } from './latex-session.svelte';

/** Completion data is scoped to this node's dependencies and the shared bibliography. */
export function latexCompletions(session: Pick<LatexSession, 'references' | 'catalog'>, before: string) {
  const reference = /\\(cite|ref|cref|Cref|nameref|eqref)\s*(?:\[[^\]]*\])?\{([^{}]*)$/.exec(before);
  if (reference) {
    const fragment = reference[2].split(',').pop()!;
    if (reference[1] === 'cite') {
      const words = fragment.trim().toLocaleLowerCase().split(/\s+/);
      const options = [];
      for (const cite of session.catalog?.citations ?? []) {
        const search = `${cite.key} ${cite.title} ${cite.authors} ${cite.year}`;
        const normalized = search.toLocaleLowerCase();
        if (!words.every(word => normalized.includes(word))) continue;
        options.push({label: cite.key, search, filter: fragment.trimStart(), detail: [cite.title, cite.authors, cite.year].filter(Boolean).join(' · '), insert: cite.key});
        if (options.length === 50) break;
      }
      // Ask again on typing: search the entire catalog before limiting results.
      return {length: fragment.trimStart().length, options, incomplete: true};
    }
    const options = session.references.map(ref => ({label: ref.name || ref.label, search: `${ref.label} ${ref.name} ${ref.title}`, detail: [ref.title, ref.label].filter(Boolean).join(' · '), insert: ref.label}));
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
      return {incomplete: result.incomplete ?? false, suggestions: result.options.map(item => ({
        label: {label: item.label, description: item.detail},
        filterText: 'filter' in item ? item.filter : item.search, insertText: item.insert, kind: languages.CompletionItemKind.Reference,
        range: {startLineNumber: from.lineNumber, startColumn: from.column, endLineNumber: position.lineNumber, endColumn: position.column},
      }))};
    },
  });
}
