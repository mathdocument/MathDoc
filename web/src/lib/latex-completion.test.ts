import { expect, test } from 'vitest';
import { EditorState } from '@codemirror/state';
import { CompletionContext } from '@codemirror/autocomplete';
import { latexCompletions } from './latex-completion';
import type { LatexCatalog, LatexReference } from './latex';

test('completions insert original labels while searching titles and citation authors', () => {
  const reference: LatexReference = {key: 'uuid::thm:main', fnode: 'uuid', title: 'Dependency', label: 'thm:main', name: 'Main theorem', type: 'Theorem', number: '1', anchor: 'latex-thm'};
  const catalog: LatexCatalog = {project_key: 'p', citations: [{key: 'paper', title: 'Article', authors: 'Author', year: '2020', label: 'Aut20', text: 'Article'}], commands: ['cA'], environments: ['items'], diagnostics: []};
  const session = {references: [reference], catalog};
  const complete = (doc: string) => latexCompletions(session, new CompletionContext(EditorState.create({doc}), doc.length, true));
  expect(complete('\\nameref{Main')?.options[0].apply).toBe('thm:main');
  expect(complete('\\cref{Main')?.options[0].apply).toBe('thm:main');
  const citation = '\\cite[Theorem 1]{old, Aut';
  expect(complete(citation)?.from).toBe(citation.length - 3);
  expect(complete('\\cite{Aut')?.options[0].label).toContain('Author');
  expect(complete('\\cite{Aut')?.options[0].detail).toContain('Article');
  expect(complete('\\cite{Aut')?.options[0].info).toBeUndefined();
  expect(complete('\\cref{Main')?.options[0].detail).toContain('Dependency');
  expect(complete('\\cref{Main')?.options[0].info).toBeUndefined();
  expect(complete('\\begin{it')?.options[0].label).toBe('items');
  expect(complete('\\c')?.options[0].label).toBe('cA');
  session.references = [];
  expect(complete('\\ref{')?.options).toEqual([]);
  expect(complete('ordinary prose')).toBeNull();
});
