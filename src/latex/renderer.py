"""Bounded JSON-lines worker. plasTeX parses macros; MDC resolves graph references.

No source files or TeX executables are read or run by document commands. Only
explicitly supported built-in plasTeX packages may extend the parser context.
"""
import hashlib
import html
import json
import logging
import sys
from collections import OrderedDict
from dataclasses import dataclass
from urllib.parse import quote, urlparse

from plasTeX import Command, Context, Environment, IfFalse, IfTrue, NewIf, TeXDocument
from plasTeX.Base.TeX.Primitives import IfCommand
from plasTeX.Packages.amsthm import newtheorem
from plasTeX.TeX import TeX
from pybtex.database import parse_string
from pybtex.exceptions import PybtexError
from pybtex.plugin import find_plugin

for name in ("plasTeX", "status", "parse", "context", "packages"):
    logging.getLogger(name).setLevel(logging.ERROR)

PACKAGES = {"article", "amsmath", "amssymb", "amsfonts", "amsthm", "mathtools", "mathrsfs", "xcolor", "color", "hyperref", "cleveref"}
RESERVED = {"input", "include", "includeonly", "externaldocument", "externalcitedocument", "openin", "openout", "read", "write", "immediate", "special", "includegraphics", "bibliography", "bibliographystyle"}
MAX_SOURCE = 2 * 1024 * 1024


def esc(value):
    return html.escape(str(value), quote=True)


def plain(value):
    return str(getattr(value, "textContent", value) or "")


def anchor(label):
    return "latex-" + quote(label, safe="")


class SafeContext(Context.Context):
    def loadPackage(self, tex, file_name, options=None):
        file_name = file_name.removesuffix('.sty').removesuffix('.cls')
        if file_name not in PACKAGES:
            raise ValueError(f"Unsupported LaTeX package: {file_name}; provide ordinary macro definitions in the shared preamble")
        result = super().loadPackage(tex, file_name, options)
        for name, cls in getattr(self, 'protected', {}).items():
            self.addGlobal(name, cls)
        return result


class SafeTeX(TeX):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.steps = 0

    def __iter__(self):
        for token in super().__iter__():
            self.steps += 1
            if self.steps > 200_000:
                raise ValueError("LaTeX macro expansion limit exceeded")
            yield token

    def kpsewhich(self, name):
        raise ValueError("LaTeX preview cannot read workspace files")

    def loadPackage(self, file, options=None):
        raise ValueError("LaTeX preview cannot load files from the TeX installation")


class PreambleTeX(SafeTeX):
    def processIfContent(self, which, debug=False):
        # Definitions and setup arguments are opaque here: an uncalled macro's
        # \ifstrempty must not be mistaken for a nested primitive \if...\fi.
        cases = [[]]
        nesting = braces = 0
        tokens = self.itertokens()
        for token in tokens:
            name = token.macroName
            if token.catcode == token.CC_BGROUP:
                braces += 1
            elif token.catcode == token.CC_EGROUP:
                braces -= 1
            elif not braces:
                if name == 'newif':
                    cases[-1].extend([token, next(tokens)])
                    continue
                if name and name.startswith('if'):
                    command = self.ownerDocument.context.get(name)
                    if command is None:
                        self.readOptionalSpaces()
                        following = next(tokens, None)
                        self.pushToken(following)
                        primitive = following is not None and following.catcode != following.CC_BGROUP
                    else:
                        primitive = issubclass(command, (IfCommand, NewIf))
                    if primitive:
                        nesting += 1
                elif name == 'fi':
                    if not nesting:
                        break
                    nesting -= 1
                elif not nesting and name in ('else', 'or'):
                    cases.append([])
                    continue
            cases[-1].append(token)
        else:
            raise ValueError('Unterminated conditional in shared LaTeX preamble')
        index = int(not which) if isinstance(which, bool) else which
        if index is not None and 0 <= index < len(cases):
            self.pushTokens(cases[index])


def load_preamble(document, preamble):
    """Import declarations without executing document setup or its arguments."""
    if len(preamble.encode()) > MAX_SOURCE:
        raise ValueError('LaTeX preamble exceeds 2 MiB')
    tex = PreambleTeX(ownerDocument=document)
    context = document.context
    declarations = {
        'newcommand', 'renewcommand', 'providecommand', 'DeclareRobustCommand',
        'newenvironment', 'renewenvironment', 'newtheorem', 'theoremstyle',
        'def', 'gdef', 'edef', 'xdef', 'let', 'newif', 'makeatletter', 'makeatother',
        'newcounter', 'setcounter', 'DeclareMathOperator',
    }
    tex.input(preamble)
    for token in tex.itertokens():
        name = token.macroName
        if token.catcode == token.CC_BGROUP or token == '[':
            tex.pushToken(token)
            tex.readArgument('[]' if token == '[' else None)
        elif name == 'endinput':
            tex.endInput()
        elif name in ('RequirePackage', 'usepackage'):
            # Print-only or unsupported packages neither load local .sty files
            # nor prevent independent content macros from being imported.
            tex.readArgument('[]')
            for package in tex.readArgument(type='list', subtype='str'):
                if package.removesuffix('.sty') in PACKAGES:
                    context.loadPackage(tex, package)
        elif name == '@ifclassloaded':
            cls = tex.readArgument(type='str')
            yes, no = tex.readArgument(), tex.readArgument()
            # HTML uses article structure, independently of print class options.
            tex.pushTokens(yes if cls == 'article' else no)
        elif name in declarations or (name and context.get(name) and
                issubclass(context.get(name), (IfCommand, NewIf, IfTrue, IfFalse))):
            document.createElement(name).invoke(tex)
        elif name and name.startswith('if'):
            # Unknown braced tests are setup calls; unknown primitive conditionals
            # have no trustworthy active branch, so import neither branch.
            if tex.readArgument('{}') is None:
                tex.processIfContent(None)
    for name, cls in context.protected.items():
        context.top.lets.pop(name, None)
        context.addGlobal(name, cls)


class NewTheorem(newtheorem):
    # Keep expanded captions as fragments; plasTeX's header:str stringifies
    # protected macros into Python object addresses instead of their text.
    args = '* name:str [ shared:str ] header [ parent:str ]'


class Ref(Command):
    args = '* key:str'


class Cite(Command):
    args = '[note] keys:list:str'


class Label(Command):
    args = 'label:id'

    @property
    def source(self):
        # Labels are HTML anchors, not KaTeX input.
        return ""


class Forbidden(Command):
    def invoke(self, tex):
        raise ValueError(f"\\{self.nodeName} is not allowed in node source; imports come from dep and project files come from settings")


class UsePackage(Command):
    args = '[options:dict] names:list:str'

    def invoke(self, tex):
        attrs = self.parse(tex)
        for name in attrs['names']:
            self.ownerDocument.context.loadPackage(tex, name, attrs['options'])


class ClassDeclaration(Command):
    args = '[options] name:str'


class Provides(Command):
    args = 'name:str [version]'


class Options(Command):
    args = 'name:str definition:nox'


class ProcessOptions(Command):
    args = '*'


class PassOptions(Command):
    args = 'options:str name:str'


@dataclass
class Parsed:
    parts: list
    labels: list
    commands: list
    environments: list
    diagnostics: list


# Cache by content, never by whichever node a browser last selected.
PARSED = OrderedDict()
PARSED_BYTES = 0
BIB = OrderedDict()


def parse(preamble, source):
    global PARSED_BYTES
    if len(source.encode()) > MAX_SOURCE:
        raise ValueError("LaTeX block exceeds 2 MiB")
    key = hashlib.sha256((preamble + "\0" + source).encode()).hexdigest()
    if key in PARSED:
        result, size = PARSED.pop(key)
        PARSED[key] = (result, size)
        return result
    document = TeXDocument(context=SafeContext(load=True))
    document.config['general']['load-tex-packages'] = False
    document.config['general']['packages-dirs'] = []
    document.config['general']['plugins'] = []
    document.config['general']['tex-packages'] = []
    tex = SafeTeX(ownerDocument=document)
    document.context.loadPackage(tex, 'article')
    document.context.loadPackage(tex, 'amsmath')
    document.context.loadPackage(tex, 'amsthm')
    definitions = {
        **{name: Ref for name in ('ref', 'cref', 'Cref', 'nameref', 'eqref')},
        'cite': Cite, 'label': Label, 'usepackage': UsePackage, 'RequirePackage': UsePackage,
        'newtheorem': NewTheorem,
        'documentclass': ClassDeclaration, 'LoadClass': ClassDeclaration,
        'ProvidesClass': Provides, 'ProvidesPackage': Provides, 'NeedsTeXFormat': Provides,
        'DeclareOption': Options, 'ProcessOptions': ProcessOptions, 'PassOptionsToPackage': PassOptions,
        **{name: Forbidden for name in RESERVED},
    }
    document.context.protected = {name: type(name, (base,), {'nodeName': name}) for name, base in definitions.items()}
    for name, cls in document.context.protected.items():
        document.context.addGlobal(name, cls)
    load_preamble(document, preamble)
    tex.input('\\begin{document}\n' + source + '\n\\end{document}')
    tex.parse()
    diagnostics = []
    label_nodes = document.getElementsByTagName('label')
    seen = set()
    for node in label_nodes:
        name = node.attributes.get('label', '')
        if name in seen:
            diagnostics.append(f"Duplicate label: {name}")
        seen.add(name)
    labels = []
    targets = {}
    for name, target in document.context.labels.items():
        attrs = getattr(target, 'attributes', {})
        title = plain(attrs.get('title'))
        kind = plain(getattr(target, 'caption', '')) or target.nodeName
        number = plain(getattr(target, 'ref', ''))
        item = {'label': name, 'name': title, 'type': kind, 'number': number, 'anchor': anchor(name)}
        labels.append(item)
        targets.setdefault(id(target), []).append(item)

    parts = []
    math_counter = 0
    ignored = {'label', 'documentclass', 'LoadClass', 'ProvidesClass', 'ProvidesPackage', 'NeedsTeXFormat', 'DeclareOption', 'ProcessOptions', 'PassOptionsToPackage', 'usepackage', 'RequirePackage', 'newcommand', 'renewcommand', 'providecommand', 'newenvironment', 'renewenvironment', 'newtheorem', 'theoremstyle', 'makeatletter', 'makeatother', 'parindent', 'parskip', 'pagestyle', 'thispagestyle'}
    tags = {'par': 'p', 'itemize': 'ul', 'enumerate': 'ol', 'item': 'li', 'description': 'dl', 'textbf': 'strong', 'textit': 'em', 'emph': 'em', 'texttt': 'code', 'textsc': 'span', 'underline': 'u', 'quote': 'blockquote', 'quotation': 'blockquote', 'center': 'div', 'flushleft': 'div', 'flushright': 'div'}
    sections = {'part': 'h2', 'chapter': 'h2', 'section': 'h2', 'subsection': 'h3', 'subsubsection': 'h4', 'paragraph': 'h5', 'subparagraph': 'h6'}
    math_names = {'math', 'displaymath', 'equation', 'equation*', 'align', 'align*', 'gather', 'gather*', 'multline', 'multline*', 'eqnarray', 'eqnarray*'}

    def render(node, depth=0):
        nonlocal math_counter
        if depth > 60:
            raise ValueError("LaTeX nesting limit exceeded")
        name = node.nodeName
        attrs = getattr(node, 'attributes', {}) or {}
        children = getattr(node, 'childNodes', [])
        for label in targets.get(id(node), []):
            parts.append(f'<span id="{esc(label["anchor"])}" class="latex-anchor"></span>')
        if name in ignored:
            return
        if name == '#text':
            parts.append(esc(str(node)))
            return
        if name in math_names:
            source = node.source
            if name == 'math': source = source[1:-1]
            elif name == 'displaymath': source = source[2:-2]
            else:
                start = source.find('}') + 1
                end = source.rfind('\\end')
                source = source[start:end]
                env = {'align': 'aligned', 'gather': 'gathered', 'eqnarray': 'aligned'}.get(name.rstrip('*'))
                if env: source = '\\begin{' + env + '}' + source + '\\end{' + env + '}'
            parts.append(f'<span class="latex-math" data-display="{str(name != "math").lower()}" data-tex="{esc(source)}"></span>')
            math_counter += 1
            return
        if name in ('ref', 'cref', 'Cref', 'nameref', 'eqref'):
            key = attrs.get('key', attrs.get('label', ''))
            keys = key.split(',') if name in ('cref', 'Cref') else [key]
            for index, key in enumerate(keys):
                if index: parts.append(', ')
                parts.append({'ref': key.strip(), 'command': name})
            return
        if name == 'cite':
            parts.append({'cite': attrs.get('keys', []), 'note': plain(attrs.get('note'))})
            return
        if name in sections:
            tag = sections[name]
            parts.append(f'<{tag}>')
            for child in getattr(attrs.get('title'), 'childNodes', []): render(child, depth + 1)
            parts.append(f'</{tag}>')
            for child in children: render(child, depth + 1)
            return
        if name == 'thmenv':
            caption = plain(getattr(node, 'caption', 'Theorem'))
            title = attrs.get('title')
            parts.append(f'<section class="latex-statement"><div class="latex-statement-title">{esc(caption)}')
            if title:
                parts.append(' (')
                for child in title.childNodes: render(child, depth + 1)
                parts.append(')')
            parts.append('</div>')
            for child in children: render(child, depth + 1)
            parts.append('</section>')
            return
        if name == 'proof':
            parts.append('<section class="latex-proof"><div class="latex-statement-title">Proof</div>')
            for child in children: render(child, depth + 1)
            parts.append('</section>')
            return
        if name in ('href', 'url'):
            url = plain(attrs.get('url'))
            safe = urlparse(url).scheme in ('http', 'https', 'mailto')
            if not safe: diagnostics.append('Unsupported link URL')
            parts.append(f'<a href="{esc(url)}" target="_blank" rel="noopener noreferrer">' if safe else '<span>')
            if children:
                for child in children: render(child, depth + 1)
            else: parts.append(esc(url))
            parts.append('</a>' if safe else '</span>')
            return
        if name in ('\\', 'newline', 'linebreak'):
            parts.append('<br>')
            return
        if name in ('verbatim', 'verb'):
            parts.append(f'<pre>{esc(plain(node))}</pre>' if name == 'verbatim' else f'<code>{esc(plain(node))}</code>')
            return
        if name in tags:
            tag = tags[name]
            parts.append(f'<{tag}>')
            for child in children: render(child, depth + 1)
            parts.append(f'</{tag}>')
            return
        if name in ('#document', '#document-fragment', 'document', 'bgroup', 'mbox', 'textrm', 'textnormal'):
            for child in children: render(child, depth + 1)
            return
        if name in ('%', '&', '#', '_', '$', '{', '}'):
            parts.append(esc(name))
            return
        if name in ('LaTeX', 'TeX'):
            parts.append(name)
            return
        if name in (' ', 'nobreakspace', 'quad', 'qquad', ',', ';', '!'):
            parts.append(' ')
            return
        # Unknown content must remain visible and diagnostic, never silently vanish.
        diagnostics.append(f'Unsupported command or environment: \\{name}')
        parts.append(esc(node.source))

    bodies = document.getElementsByTagName('document')
    if len(bodies) != 1:
        raise ValueError('Node source is a document body; remove begin/end document wrappers')
    render(bodies[0])
    if sum(len(str(part)) for part in parts) > 8 * 1024 * 1024:
        raise ValueError('Rendered LaTeX exceeds 8 MiB')
    commands = sorted(set(document.context.keys()) - set(SafeContext(load=True).keys()))
    commands = [name for name in commands if name.isalpha() and name not in definitions]
    environments = [name for name in commands if isinstance(document.context.get(name), type) and issubclass(document.context.get(name), Environment)]
    result = Parsed(parts, labels, commands, environments, list(dict.fromkeys(diagnostics)))
    size = len(source) + sum(len(str(p)) for p in parts)
    PARSED[key] = (result, size)
    PARSED_BYTES += size
    while len(PARSED) > 128 or PARSED_BYTES > 32 * 1024 * 1024:
        _, (_, removed_size) = PARSED.popitem(last=False)
        PARSED_BYTES -= removed_size
    return result


def bibliography(source, cited_keys=()):
    style = find_plugin('pybtex.style.formatting', 'alpha')()
    if source not in BIB:
        data = parse_string(source, 'bibtex')
        entries = style.sort(list(data.entries.values()))
        labels = dict(zip((entry.key for entry in entries), style.format_labels(entries)))
        citations = {}
        for key, entry in data.entries.items():
            authors = ', '.join(str(p) for p in entry.persons.get('author', []))
            text = '. '.join(value for value in (
                authors or ', '.join(str(p) for p in entry.persons.get('editor', [])),
                entry.fields.get('title'), entry.fields.get('year'),
                entry.fields.get('doi') or entry.fields.get('url'),
            ) if value) or key
            citations[key] = {'key': key, 'label': labels[key], 'title': entry.fields.get('title', ''),
                              'authors': authors, 'year': entry.fields.get('year', ''), 'text': text}
        BIB[source] = data, citations, set()
    BIB.move_to_end(source)
    while len(BIB) > 2: BIB.popitem(last=False)
    data, citations, formatted = BIB[source]
    # Completion needs metadata only. Format just the cited entries, once each.
    for key in set(cited_keys) - formatted:
        if key not in citations:
            continue
        try:
            citations[key]['text'] = style.format_entry(citations[key]['label'], data.entries[key], bib_data=data).text.render_as('text')
        except (PybtexError, AttributeError):
            # Retain available metadata when a print style requires missing fields
            # or does not implement this entry type; never invent bibliographic data.
            pass
        formatted.add(key)
    return list(citations.values())


def handle(request):
    project = request['project']
    preamble = project['preamble']
    if request['kind'] == 'catalog':
        parsed = parse(preamble, '')
        return {'citations': bibliography(project['bibliography']), 'commands': sorted(set(parsed.commands + ['frac', 'sqrt', 'sum', 'prod', 'int', 'left', 'right', 'mathbb', 'mathcal', 'mathrm', 'mathbf', 'operatorname', 'begin', 'end', 'section', 'subsection', 'label', 'ref', 'cref', 'nameref', 'cite', 'textbf', 'emph', 'item'])), 'environments': sorted(set(parsed.environments + ['itemize', 'enumerate', 'description', 'equation', 'align', 'gather', 'proof'])), 'diagnostics': parsed.diagnostics}
    target = request['target']
    dependencies = request['dependencies']
    imports = [{'fnode': n['fnode'], 'title': n['title'], 'prefix': n['fnode'] + '::'} for n in dependencies]
    refs = []
    diagnostics = []
    for node in [target] + dependencies:
        if not node['source'].strip(): continue
        try:
            parsed = parse(preamble, node['source'])
        except Exception as error:
            if node['fnode'] == target['fnode'] and request['kind'] != 'context': raise
            diagnostics.append(f'Node {node["title"]}: {error}')
            continue
        for label in parsed.labels:
            key = label['label'] if node['fnode'] == target['fnode'] else node['fnode'] + '::' + label['label']
            refs.append({**label, 'key': key, 'fnode': node['fnode'], 'title': node['title']})
    if request['kind'] == 'context':
        return {'imports': imports, 'references': refs, 'diagnostics': diagnostics}
    current = parse(preamble, target['source'])
    diagnostics += current.diagnostics
    cited_keys = [key for part in current.parts if isinstance(part, dict) for key in part.get('cite', [])]
    citations = {entry['key']: entry for entry in bibliography(project['bibliography'], cited_keys)}
    used_citations = []
    exact = {item['key']: item for item in refs}
    output = []
    for part in current.parts:
        if isinstance(part, str):
            output.append(part)
        elif 'ref' in part:
            key = part['ref']
            matches = [exact[key]] if key in exact else [r for r in refs if r['label'] == key]
            if len(matches) != 1:
                message = ('Ambiguous' if matches else 'Unknown or undeclared') + f' reference: {key}'
                diagnostics.append(message)
                output.append(f'<span class="latex-error" title="{esc(message)}">?? {esc(key)}</span>')
                continue
            ref = matches[0]
            command = part['command']
            name = ref['name'] or f'{ref["title"]} — {ref["label"]}'
            text = name if command == 'nameref' else ref['number'] or name
            if command in ('cref', 'Cref'): text = ref['type'] + ' ' + text
            if command == 'eqref': text = '(' + text + ')'
            output.append(f'<a href="#{esc(ref["anchor"])}" data-latex-node="{esc(ref["fnode"])}" data-latex-label="{esc(ref["label"])}" title="{esc(ref["title"])}">{esc(text)}</a>')
        elif 'cite' in part:
            links = []
            for key in part['cite']:
                if key not in citations:
                    diagnostics.append(f'Unknown citation: {key}')
                    links.append(f'<span class="latex-error">?? {esc(key)}</span>')
                    continue
                entry = citations[key]
                if key not in used_citations: used_citations.append(key)
                links.append(f'<a href="#latex-cite-{esc(quote(key, safe=""))}" title="{esc(entry["title"])}">{esc(entry["label"])}</a>')
            output.append('[' + '; '.join(links) + (', ' + esc(part['note']) if part['note'] else '') + ']')
    if used_citations:
        output.append('<section class="latex-bibliography"><h3>References</h3><dl>')
        for key in used_citations:
            entry = citations[key]
            output.append(f'<dt id="latex-cite-{esc(quote(key, safe=""))}">[{esc(entry["label"])}]</dt><dd>{esc(entry["text"])}</dd>')
        output.append('</dl></section>')
    return {'html': ''.join(output), 'labels': current.labels, 'diagnostics': list(dict.fromkeys(diagnostics))}


def main():
    for line in sys.stdin:
        try:
            request = json.loads(line)
            result = {'result': handle(request)}
        except Exception as error:
            result = {'error': str(error)}
        sys.stdout.write(json.dumps(result, ensure_ascii=False) + '\n')
        sys.stdout.flush()


if __name__ == '__main__':
    main()
