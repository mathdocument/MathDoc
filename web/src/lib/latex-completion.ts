import { autocompletion, completionKeymap, type CompletionContext, type CompletionResult } from '@codemirror/autocomplete';
import { keymap } from '@codemirror/view';
import type { LatexSession } from './latex-session.svelte';

export function latexCompletions(session: Pick<LatexSession, 'references' | 'catalog'>, context: CompletionContext): CompletionResult | null {
  const before = context.state.sliceDoc(Math.max(0, context.pos - 1024), context.pos);
  const reference = /\\(cite|ref|cref|Cref|nameref|eqref)\s*(?:\[[^\]]*\])?\{([^{}]*)$/.exec(before);
  if (reference) {
    const fragment = reference[2].split(',').pop()!;
    const from = context.pos - fragment.trimStart().length;
    const options = reference[1] === 'cite'
      ? (session.catalog?.citations ?? []).map(cite => ({label: `${cite.key} ${cite.title} ${cite.authors} ${cite.year}`, displayLabel: cite.key, detail: `${cite.authors} · ${cite.year}`, info: cite.title, apply: cite.key, type: 'reference'}))
      : session.references.map(ref => ({label: `${ref.key} ${ref.name} ${ref.title}`, displayLabel: ref.name || ref.label, detail: ref.title, info: ref.key, apply: ref.key, type: 'reference'}));
    return {from, options, validFor: /^[^{},]*$/};
  }
  const environment = /\\(?:begin|end)\{([A-Za-z*]*)$/.exec(before);
  if (environment) return {from: context.pos - environment[1].length, options: (session.catalog?.environments ?? []).map(label => ({label, type: 'type'})), validFor: /^[A-Za-z*]*$/};
  const command = /\\([A-Za-z]*)$/.exec(before);
  if (command) return {from: context.pos - command[1].length, options: (session.catalog?.commands ?? []).map(label => ({label, type: 'function'})), validFor: /^[A-Za-z]*$/};
  return null;
}

export function latexAutocomplete(session: LatexSession) {
  return [autocompletion({override: [context => latexCompletions(session, context)], maxRenderedOptions: 50}), keymap.of(completionKeymap)];
}
