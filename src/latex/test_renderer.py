"""Run with: python -B -m unittest discover -s src/latex -p 'test_*.py'."""
import unittest
from unittest.mock import patch
from pybtex.style.formatting import BaseStyle
import renderer

A = '11111111-1111-4111-8111-111111111111'
B = '22222222-2222-4222-8222-222222222222'
PROJECT = {
    'preamble': r'\ProvidesClass{example}\RequirePackage{amsmath,amsthm}\newcommand{\cA}{\mathcal{A}}\newcommand{\wrap}[1]{\textbf{#1}}\newenvironment{items}{\begin{itemize}}{\end{itemize}}\newtheorem{thm}{Theorem}\endinput',
    'bibliography': '@article{ref,title={A paper},author={Author, A.},journal={Journal},year={2020}}',
}

class RendererTest(unittest.TestCase):
    def test_incomplete_bibliography_entries_remain_citable(self):
        project = {**PROJECT, 'bibliography': PROJECT['bibliography'] + r'''
            @book{Fox1957, title={A title}, year={1957}}
            @article{Empty}
            @customtype{Custom, title={<script>unsafe</script>}, url={https://example.org}}
            @book{FoxOther1957, title={Another title}, year={1957}}
        '''}
        with patch.object(BaseStyle, 'format_entry', side_effect=AssertionError('Completion must not format entries')):
            catalog = renderer.handle({'kind': 'catalog', 'project': project})
        citations = {entry['key']: entry for entry in catalog['citations']}
        self.assertEqual(len(citations), 5)
        self.assertNotEqual(citations['Fox1957']['label'], citations['FoxOther1957']['label'])
        self.assertEqual(citations['Empty']['text'], 'Empty')
        preview = renderer.handle({'kind': 'preview', 'project': project, 'dependencies': [],
            'target': {'fnode': A, 'title': 'A', 'source': r'\cite{Fox1957,Empty,Custom,ref}'}})
        self.assertEqual(preview['diagnostics'], [])
        self.assertIn('A title', preview['html'])
        self.assertIn('1957', preview['html'])
        self.assertIn('Aut20', preview['html'])
        self.assertIn('A paper', preview['html'])
        self.assertIn('&lt;script&gt;', preview['html'])
        self.assertNotIn('<script>', preview['html'])

    def test_print_setup_is_not_executed_when_importing_content_macros(self):
        project = {**PROJECT, 'preamble': r'''
            \ProvidesClass{example}
            \RequirePackage{etoolbox,geometry,fontspec,pythontex}
            \geometry{left=20mm,unused={\input{must-not-be-read}}}
            \newfontfamily\PrintFont{Unavailable Font}
            \AtBeginDocument{\input{must-not-be-read}\newcommand{\leaked}{WRONG}}
            \NewEnviron{layout}[1][]{\ifnum#1=0\input{bad}\fi}
            \ifUnknownEngine
                \newcommand{\engineLeak}{WRONG}
            \else
                \newcommand{\engineLeak}{ALSO WRONG}
            \fi
            \makeatletter
            \@ifclassloaded{beamer}{\newcommand{\chosen}{WRONG}}{
                \newcommand{\chosen}{Article}
            }
            \makeatother
            \newif\iflocalized
            \localizedtrue
            \iflocalized
                \def\statementName{Theorem}
                \newcommand{\confighead}[1]{\ifstrempty{#1}{EMPTY}{TITLE}}
                \ifstrempty{}{\newcommand{\bracedLeak}{WRONG}}{}
            \else
                \def\statementName{WRONG}
            \fi
            \confighead{\input{must-not-be-read}}
            \RequirePackage{amsmath,amsthm}
            \newcommand{\cA}{\mathcal{A}}
            \newcommand{\wrap}[2][Default]{\textbf{#1: #2}}
            \newenvironment{items}{\begin{itemize}}{\end{itemize}}
            \newtheorem{thm}{\protect\statementName}[section]
            \let\ref=X
            \endinput
            \newcommand{\afterEnd}{WRONG}
        '''}
        request = {'kind': 'preview', 'project': project, 'dependencies': [],
                   'target': {'fnode': A, 'title': 'A', 'source': r'\section{Intro}\begin{thm}[Named]\label{t}$\cA$\end{thm}\nameref{t}, \cite{ref}. \chosen\wrap{Text}\begin{items}\item Item\end{items}'}}
        preview = renderer.handle(request)
        self.assertEqual(preview['diagnostics'], [])
        self.assertIn('Theorem (Named)', preview['html'])
        self.assertIn('Default: Text', preview['html'])
        self.assertIn('Article', preview['html'])
        self.assertIn('Aut20', preview['html'])
        self.assertIn('data-tex="\\mathcal{A}"', preview['html'])
        self.assertIn('<ul><li>', preview['html'])
        self.assertEqual(preview['labels'][0]['type'], 'Theorem')
        self.assertEqual(preview['labels'][0]['number'], '1.1')
        catalog = renderer.handle({**request, 'kind': 'catalog'})
        self.assertTrue({'cA', 'confighead', 'chosen'} <= set(catalog['commands']))
        for name in ('leaked', 'engineLeak', 'bracedLeak', 'afterEnd'):
            self.assertNotIn(name, catalog['commands'])
        self.assertIn({'ref': 't', 'command': 'ref'}, renderer.parse(project['preamble'], r'\ref{t}').parts)
        # A skipped package is not an implementation of its content commands.
        invoked = renderer.handle({**request, 'target': {**request['target'], 'source': r'\confighead{Test}'}})
        self.assertTrue(any('ifstrempty' in e for e in invoked['diagnostics']))
        with self.assertRaisesRegex(ValueError, 'Unterminated conditional'):
            renderer.parse(r'\iftrue\newcommand{\x}{X}', '')

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
