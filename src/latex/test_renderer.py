"""Run with: python -B -m unittest discover -s src/latex -p 'test_*.py'."""
import unittest
import renderer

A = '11111111-1111-4111-8111-111111111111'
B = '22222222-2222-4222-8222-222222222222'
PROJECT = {
    'preamble': r'\ProvidesClass{example}\RequirePackage{amsmath,amsthm}\newcommand{\cA}{\mathcal{A}}\newcommand{\wrap}[1]{\textbf{#1}}\newenvironment{items}{\begin{itemize}}{\end{itemize}}\newtheorem{thm}{Theorem}\endinput',
    'bibliography': '@article{ref,title={A paper},author={Author, A.},journal={Journal},year={2020}}',
}

class RendererTest(unittest.TestCase):
    def test_macros_references_citations_and_isolated_drafts(self):
        dependency = {'fnode': A, 'title': 'A', 'source': r'\begin{thm}[Named result]\label{thm:a}$\cA$\end{thm}'}
        target = {'fnode': B, 'title': 'B', 'source': r'\section{Intro}\label{intro}By \nameref{thm:a}, see \cite[2]{ref}. \begin{items}\item $\cA$\end{items}'}
        request = {'kind': 'preview', 'project': PROJECT, 'target': target, 'dependencies': [dependency]}
        preview = renderer.handle(request)
        self.assertEqual(preview['diagnostics'], [])
        self.assertIn('Named result</a>', preview['html'])
        self.assertIn('data-latex-node="' + A, preview['html'])
        self.assertIn('data-tex="\\mathcal{A}"', preview['html'])
        self.assertIn('<ul><li>', preview['html'])
        self.assertIn('Aut20', preview['html'])
        self.assertIn('A paper', preview['html'])
        self.assertEqual(preview['labels'][0]['label'], 'intro')
        multiple = renderer.handle({**request, 'target': {**target, 'source': r'\cref{thm:a,thm:a}'}})
        self.assertEqual(multiple['diagnostics'], [])
        self.assertEqual(multiple['html'].count('data-latex-node='), 2)
        context = renderer.handle({**request, 'kind': 'context'})
        self.assertIn(A + '::thm:a', [r['key'] for r in context['references']])
        missing = renderer.handle({**request, 'dependencies': []})
        self.assertTrue(any('undeclared' in e for e in missing['diagnostics']))
        self.assertNotIn('data-latex-node="' + A, missing['html'])
        # A local draft never overwrites the dependency's cached exported labels.
        changed = renderer.handle({**request, 'target': {**target, 'source': r'\section{Draft}\label{new}'}})
        self.assertEqual(changed['labels'][0]['label'], 'new')
        self.assertEqual(renderer.handle(request), preview)
        for bad in [r'\input{/etc/passwd}', r'\externaldocument{other}', r'\usepackage{pythontex}']:
            with self.assertRaises(ValueError):
                renderer.handle({**request, 'target': {**target, 'source': bad}})
        broken = renderer.handle({**request, 'kind': 'context', 'target': {**target, 'source': r'\input{bad}'}})
        self.assertTrue(any('not allowed' in e for e in broken['diagnostics']))
        self.assertIn(A + '::thm:a', [r['key'] for r in broken['references']])
        hostile = renderer.handle({**request, 'target': {**target, 'source': '<script>alert(1)</script>'}})
        self.assertNotIn('<script>', hostile['html'])
        duplicate = renderer.handle({**request, 'target': {**target, 'source': r'\section{A}\label{x}\section{B}\label{x}'}})
        self.assertIn('Duplicate label: x', duplicate['diagnostics'])

if __name__ == '__main__':
    unittest.main()
