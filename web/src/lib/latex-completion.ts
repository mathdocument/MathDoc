import { autocompletion, type CompletionContext, type CompletionResult } from '@codemirror/autocomplete';
import { EditorView } from '@codemirror/view';
import type { LatexSession } from './latex-session.svelte';

export function latexCompletions(session: Pick<LatexSession, 'references' | 'catalog'>, context: CompletionContext): CompletionResult | null {
  const before = context.state.sliceDoc(Math.max(0, context.pos - 1024), context.pos);
  const reference = /\\(cite|ref|cref|Cref|nameref|eqref)\s*(?:\[[^\]]*\])?\{([^{}]*)$/.exec(before);
  if (reference) {
    const fragment = reference[2].split(',').pop()!;
    const from = context.pos - fragment.trimStart().length;
    const options = reference[1] === 'cite'
      ? (session.catalog?.citations ?? []).map(cite => ({label: `${cite.key} ${cite.title} ${cite.authors} ${cite.year}`, displayLabel: cite.key, detail: [cite.title, cite.authors, cite.year].filter(Boolean).join(' · '), apply: cite.key, type: 'reference'}))
      : session.references.map(ref => ({label: `${ref.key} ${ref.name} ${ref.title}`, displayLabel: ref.name || ref.label, detail: [ref.title, ref.label].filter(Boolean).join(' · '), apply: ref.key, type: 'reference'}));
    return {from, options, validFor: /^[^{},]*$/};
  }
  const environment = /\\(?:begin|end)\{([A-Za-z*]*)$/.exec(before);
  if (environment) return {from: context.pos - environment[1].length, options: (session.catalog?.environments ?? []).map(label => ({label, type: 'type'})), validFor: /^[A-Za-z*]*$/};
  const command = /\\([A-Za-z]*)$/.exec(before);
  if (command) return {from: context.pos - command[1].length, options: (session.catalog?.commands ?? []).map(label => ({label, type: 'function'})), validFor: /^[A-Za-z]*$/};
  return null;
}

export function latexAutocomplete(session: LatexSession) {
  return [
    autocompletion({override: [context => latexCompletions(session, context)], maxRenderedOptions: 30, icons: false}),
    EditorView.theme({
      '.cm-tooltip.cm-tooltip-autocomplete': {
        backgroundColor: 'var(--mdc-panel)', color: 'var(--mdc-fg)', border: '1px solid var(--mdc-border-strong)',
        borderRadius: '8px', padding: '4px', boxShadow: 'var(--mdc-shadow-panel)', overflow: 'hidden',
      },
      '.cm-tooltip.cm-tooltip-autocomplete > ul': {
        fontFamily: 'var(--mdc-font)', fontSize: 'var(--mdc-text-xs)',
        width: 'min(26rem, calc(100vw - 2rem))', minWidth: '0', maxWidth: 'none', maxHeight: '16rem',
      },
      '.cm-tooltip.cm-tooltip-autocomplete > ul > li': {padding: '7px 10px', borderRadius: '5px', lineHeight: '1.45'},
      '.cm-tooltip.cm-tooltip-autocomplete > ul > li[aria-selected]': {backgroundColor: 'var(--mdc-card-selected)', color: 'var(--mdc-fg)'},
      '.cm-completionLabel': {display: 'block', overflow: 'hidden', textOverflow: 'ellipsis', fontFamily: 'var(--mdc-mono)', fontWeight: '600'},
      '.cm-completionDetail': {display: 'block', overflow: 'hidden', textOverflow: 'ellipsis', margin: '2px 0 0', color: 'var(--mdc-dim)', fontStyle: 'normal'},
      '.cm-completionMatchedText': {color: 'var(--mdc-accent)', textDecoration: 'none'},
    }),
  ];
}
