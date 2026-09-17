"""Run with: python -B -m unittest discover -s src/latex -p 'test_*.py'."""
import unittest
from pathlib import Path
from unittest.mock import patch
import renderer

renderer.AMSALPHA = list(renderer.bst.parse_string(Path(__file__).with_name('amsalpha.bst').read_text()))

A = '11111111-1111-4111-8111-111111111111'
B = '22222222-2222-4222-8222-222222222222'
PROJECT = {
    'preamble': r'\ProvidesClass{example}\RequirePackage{amsmath,amsthm}\newcommand{\cA}{\mathcal{A}}\newcommand{\wrap}[1]{\textbf{#1}}\newenvironment{items}{\begin{itemize}}{\end{itemize}}\newtheorem{thm}{Theorem}\endinput',
    'bibliography': '@article{ref,title={A paper},author={Author, A.},journal={Journal},year={2020}}',
}

class RendererTest(unittest.TestCase):
    def test_math_wrapped_diagrams_use_tex_and_preserve_surrounding_math(self):
        diagram = r'\begin{tikzcd}A&B\\ C&D\end{tikzcd}'
        for source in (r'\[' + diagram + r'\]', '$' + diagram + '$',
                       r'\begin{equation}X = ' + diagram + r'\end{equation}'):
            with self.subTest(source=source):
                parsed = renderer.parse('', source)
                self.assertEqual(parsed.diagnostics, [])
                output = ''.join(parsed.parts)
                self.assertIn('class="latex-diagram"', output)
                self.assertIn(r'\begin{tikzcd}', output)
                self.assertNotIn('class="latex-math"', output)
                if 'X =' in source:
                    self.assertIn('X =', output)

    def test_diagrams_preserve_tex_and_only_import_preamble_declarations(self):
        preamble = r'\newcommand{\cA}{\mathcal{A}}\AtBeginDocument{\input{private}}\definecolor{brand}{HTML}{336699}'
        parsed = renderer.parse(preamble, r'\begin{tikzcd}\cA \arrow[r,"f"] & B\end{tikzcd}')
        self.assertEqual(parsed.diagnostics, [])
        output = ''.join(parsed.parts)
        self.assertIn('class="latex-diagram"', output)
        self.assertIn(r'\arrow[r,&quot;f&quot;]', output)
        self.assertIn(r'\newcommand{\cA', output)
        self.assertIn(r'\definecolor{brand}{HTML}{336699}', output)
        self.assertNotIn('private', output)

    def test_tables_and_colors_use_native_parsed_structure(self):
        parsed = renderer.parse(r'\definecolor{brand}{HTML}{336699}\colorlet{soft}{brand!50!white}', r'''
            {\color{soft}Tinted} normal $\colorlet{local}{brand}\textcolor{local}{x}+{\color{red!50!blue}y}$
            \begin{tabular}{|p{3cm}|c|}\hline A & \textcolor{red}{B}\\\hline
            \multicolumn{2}{c}{Together}\\\hline\end{tabular}
        ''')
        self.assertEqual(parsed.diagnostics, [])
        output = ''.join(parsed.parts)
        for expected in ('<table ', '<tr', 'colspan="2"', 'text-align:center',
                         'border-top-style:solid', 'color:#99B2CC', r'\textcolor{#336699}', r'\color{#7F007F}'):
            self.assertIn(expected, output)
        self.assertNotIn(r'\require', output)
        self.assertNotIn(r'\colorlet', output)

    def test_incomplete_bibliography_entries_remain_citable(self):
        project = {**PROJECT, 'bibliography': PROJECT['bibliography'] + r'''
            @book{Fox1957, title={A title}, year={1957}}
            @article{Empty}
            @customtype{Custom, title={<script>unsafe</script>}, url={https://example.org}}
            @book{FoxOther1957, title={Another title}, year={1957}}
        '''}
        with patch.object(renderer.Interpreter, 'run', side_effect=AssertionError('Completion must not format entries')):
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

    def test_proof_captions_survive_environment_aliases(self):
        preamble = r'\newenvironment{prf}{\begin{proof}}{\end{proof}}'
        parsed = renderer.parse(preamble, r'''
            \begin{proof}Default body.\end{proof}
            \begin{prf}[Sketch of the \emph{proof}]Sketch body.\end{prf}
            After the proof.
        ''')
        self.assertEqual(parsed.diagnostics, [])
        output = ''.join(parsed.parts)
        self.assertIn('>Proof</div>', output)
        self.assertIn('>Sketch of the <em>proof</em></div>', output)
        self.assertIn('After the proof.', output)
        self.assertEqual(output.count('class="latex-proof"'), 2)

    def test_native_character_text_is_preserved_and_escaped(self):
        parsed = renderer.parse('', r'''Erd\H{o}s, Szemer\'edi, M\"uller, Fran\c{c}ois \& Co. & More''')
        self.assertEqual(parsed.diagnostics, [])
        self.assertIn('Erdős, Szemerédi, Müller, François &amp; Co. &amp; More', ''.join(parsed.parts))

    def test_independent_preambles_preserve_ordinary_macro_semantics(self):
        cases = [
            (r'\newcommand{\pair}[2][first]{#1/#2}\providecommand{\pair}{wrong}',
             r'\pair{second}; \pair[other]{last}', 'first/second; other/last'),
            (r'\def\word{before}\def\message{Hello \word}\def\word{after}',
             r'\message', 'Hello after'),
            (r'\newif\ifdraft\draftfalse\ifdraft\def\chosen{wrong}\else\def\chosen{right}\fi',
             r'\chosen', 'right'),
            (r'\newif\ifdraft\drafttrue\ifdraft\def\chosen{right}\else\def\chosen{wrong}\fi',
             r'\chosen', 'right'),
            (r'\newenvironment{argument}[1][Proof]{\begin{proof}[#1]}{\end{proof}}',
             r'\begin{argument}[Sketch]Complete.\end{argument}', '>Sketch</div>'),
            (r'\DeclareMathOperator{\rank}{rank}\newcommand{\norm}[1]{\left\lVert#1\right\rVert}',
             r'$\rank A+\norm{x}$', r'\operatorname{rank}'),
        ]
        for preamble, source, expected in cases:
            with self.subTest(preamble=preamble):
                parsed = renderer.parse(preamble, source)
                self.assertEqual(parsed.diagnostics, [])
                self.assertIn(expected, ''.join(parsed.parts))
        for declaration in ('edef', 'xdef'):
            preamble = r'\def\word{before}' + '\\' + declaration + r'\message{Hello \word}\def\word{after}'
            self.assertEqual(renderer.parse(preamble, 'Unrelated content.').diagnostics, [])
            with self.assertRaisesRegex(ValueError, 'unsupported'):
                renderer.parse(preamble, r'\message')

    def test_theorem_numbers_are_local_and_references_identify_the_node(self):
        project = {'bibliography': '', 'preamble': r'''
            \newtheorem{thm}{Theorem}[section]
            \newtheorem{lem}[thm]{Lemma}
            \newtheorem{defn}{Definition}[section]
            \newtheorem*{setup}{Setup}
        '''}
        source = r'''
            \section{First}
            \begin{thm}[Main result]\label{main}First.\end{thm}
            \begin{lem}\label{lemma}Second.\end{lem}
            \begin{defn}\label{definition}Independent.\end{defn}
            \section{Next}
            \begin{thm}\label{next}Third.\end{thm}
            \begin{setup}[Notation]\label{notation}Unnumbered.\end{setup}
        '''
        dependency = {'fnode': A, 'title': 'External node', 'source': source}
        target = {'fnode': B, 'title': 'Current node', 'source': r'''
            \begin{thm}\label{local}Local.\end{thm}
            \cref{local,main,lemma,definition,next}, \ref{main}, \nameref{main}.
        '''}
        request = {'kind': 'preview', 'project': project, 'target': target, 'dependencies': [dependency]}
        preview = renderer.handle(request)
        self.assertEqual(preview['diagnostics'], [])
        self.assertIn('>Theorem 1</div>', preview['html'])
        self.assertIn('>Theorem 1</a>', preview['html'])
        for text in ('Theorem 1', 'Lemma 2', 'Definition 1', 'Theorem 3'):
            self.assertIn('>11111111::' + text + '</a>', preview['html'])
        self.assertEqual(preview['html'].count('>11111111::Theorem 1</a>'), 3)
        labels = renderer.parse(project['preamble'], source).labels
        self.assertEqual({x['label']: x['number'] for x in labels},
                         {'main': '1', 'lemma': '2', 'definition': '1', 'next': '3', 'notation': ''})
        standalone = renderer.handle({**request, 'target': dependency, 'dependencies': []})
        self.assertIn('>Theorem 1 (Main result)</div>', standalone['html'])
        self.assertIn('>Lemma 2</div>', standalone['html'])
        self.assertIn('>Setup (Notation)</div>', standalone['html'])
        collision = renderer.handle({**request, 'target': {**target, 'source': r'''
            \begin{thm}\label{main}Local.\end{thm}\cref{main}\nameref{main}
        '''}})
        self.assertIn('Ambiguous reference: main', collision['diagnostics'])
        self.assertNotIn('data-latex-node=', collision['html'])
        duplicate = {**dependency, 'source': r'''
            \begin{thm}\label{main}First.\end{thm}
            \begin{thm}\label{main}Second.\end{thm}
        '''}
        for kind in ('context', 'preview'):
            ambiguous = renderer.handle({**request, 'kind': kind, 'dependencies': [duplicate]})
            self.assertIn('Node External node: Duplicate label: main', ambiguous['diagnostics'])
            if kind == 'context':
                self.assertFalse(any(ref['label'] == 'main' for ref in ambiguous['references']))
            else:
                self.assertNotIn('data-latex-label="main"', ambiguous['html'])

    def test_amsalpha_format_sorting_and_citation_set_cache(self):
        project = {**PROJECT, 'bibliography': r'''
            @article{GT2008, author={Green, Ben and Tao, Terence},
              title={The primes contain arbitrarily long arithmetic progressions},
              journal={Ann. of Math.}, volume={167}, number={2}, year={2008}, pages={481--547}}
            @article{GT2008Other, author={Green, Ben and Tao, Terence},
              title={Another result}, journal={Journal}, year={2008}}
            @article{BadUnused, title={\input{must-not-be-read}}}
        '''}
        request = {'kind': 'preview', 'project': project, 'dependencies': [],
                   'target': {'fnode': A, 'title': 'A', 'source': r'\cite{GT2008}'}}
        preview = renderer.handle(request)
        output = preview['html']
        self.assertEqual(preview['diagnostics'], [])
        self.assertIn('>GT08</a>', output)
        self.assertIn('Ben Green and Terence Tao, <em>The primes contain arbitrarily long arithmetic', output)
        self.assertIn('Ann. of Math. <strong>167</strong> (2008), no.\u00a02, 481–547.', output)
        self.assertNotIn('<p><section', output)
        both = renderer.handle({**request, 'target': {**request['target'], 'source': r'\cite{GT2008,GT2008Other}'}})
        self.assertEqual(both['diagnostics'], [])
        self.assertIn('>GT08a</a>', both['html'])
        self.assertIn('>GT08b</a>', both['html'])
        self.assertLess(both['html'].index('id="latex-cite-GT2008Other"'), both['html'].index('id="latex-cite-GT2008"'))
        self.assertIn('———', both['html'])
        with patch.object(renderer.Interpreter, 'run', side_effect=AssertionError('Reuse unchanged cited sets')):
            self.assertEqual(renderer.handle(request), preview)

        crossref = {**project, 'bibliography': r'''
            @incollection{Child,author={Aaa, A},title={Chapter},pages={1--3},crossref={Parent}}
            @book{Parent,editor={Bbb, B},title={Proceedings},publisher={Press},year={2020}}
            @preamble{"\input{must-not-be-read}"}
        '''}
        inherited = renderer.handle({**request, 'project': crossref,
            'target': {**request['target'], 'source': r'\cite{Child}'}})
        self.assertEqual(inherited['diagnostics'], [])
        self.assertIn('>Aaa20</a>', inherited['html'])
        self.assertIn('>Bbb20</a>', inherited['html'])
        self.assertIn('id="latex-cite-Parent"', inherited['html'])
        self.assertIn('Proceedings', inherited['html'])

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
        self.assertIn('Theorem 1 (Named)', preview['html'])
        self.assertIn('Default: Text', preview['html'])
        self.assertIn('Article', preview['html'])
        self.assertIn('Aut20', preview['html'])
        self.assertIn('data-tex="\\mathcal{A}"', preview['html'])
        self.assertIn('<ul><li>', preview['html'])
        self.assertEqual(preview['labels'][0]['type'], 'Theorem')
        self.assertEqual(preview['labels'][0]['number'], '1')
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
        self.assertIn('11111111::Theorem 1</a>', preview['html'])
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
