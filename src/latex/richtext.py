"""HTML tables and colors from plasTeX's parsed structure and resolved styles."""
import html
import re

from plasTeX import sourceChildren
from plasTeX.Packages import xcolor


class Color(xcolor.color):
    @property
    def source(self):
        return r'{\color{' + self.color.html + '}' + sourceChildren(self) + '}'


class TextColor(xcolor.textcolor):
    @property
    def source(self):
        return r'\textcolor{' + self.color.html + '}{' + sourceChildren(self) + '}'


class ColorBox(xcolor.colorbox):
    @property
    def source(self):
        return r'\colorbox{' + self.color.html + '}{' + sourceChildren(self) + '}'


class FramedColorBox(xcolor.fcolorbox):
    @property
    def source(self):
        return r'\fcolorbox{' + self.f_color.html + '}{' + self.color.html + '}{' + sourceChildren(self) + '}'


COLORS = {'color': Color, 'textcolor': TextColor, 'colorbox': ColorBox, 'fcolorbox': FramedColorBox}


class ColorDeclaration:
    @property
    def source(self):
        # Resolved uses already carry the color; KaTeX needs no declarations.
        return sourceChildren(self)


COLOR_DEFINITIONS = {name: type(name, (ColorDeclaration, getattr(xcolor, name)), {}) for name in
                     ('definecolor', 'providecolor', 'DefineNamedColor', 'colorlet', 'definecolorset', 'providecolorset')}
TABLE_COMMANDS = {'hline', 'vline', 'cline', 'tabularnewline'}


def style(node):
    """Only emit the CSS values produced for table layout and resolved colors."""
    values = []
    styles = dict(node.style)
    width = node.attributes.get('width')
    if width:
        styles['width'] = width
    for key, value in styles.items():
        value = str(value)
        valid = False
        if key in ('color', 'background-color') or re.fullmatch(r'border-(top|bottom|left|right)-color', key):
            valid = bool(re.fullmatch(r'#[0-9A-Fa-f]{3,8}|black|white|currentColor', value))
        elif re.fullmatch(r'border(?:-(?:top|bottom|left|right))?', key):
            valid = bool(re.fullmatch(r'\d+(?:\.\d+)?(?:px|pt) (?:solid|dashed|dotted) (?:#[0-9A-Fa-f]{3,8}|black|white)', value))
        elif re.fullmatch(r'border-(top|bottom|left|right)-style', key):
            valid = value in ('solid', 'dashed', 'dotted', 'double', 'none')
        elif key in ('width', 'height') or re.fullmatch(r'border-(top|bottom|left|right)-width', key):
            valid = bool(re.fullmatch(r'\d+(?:\.\d+)?(?:px|pt|em|cm|mm|in|%)', value))
        elif key == 'text-align':
            valid = value in ('left', 'center', 'right', 'justify')
        elif key == 'vertical-align':
            valid = value in ('top', 'middle', 'bottom', 'baseline')
        if valid:
            values.append(key + ':' + value)
    return html.escape(';'.join(values), quote=True)


def render(node, parts, render_child):
    name = node.nodeName
    if name in COLOR_DEFINITIONS or name in TABLE_COMMANDS:
        return True
    tags = {'tabular': 'table', 'tabular*': 'table', 'ArrayRow': 'tr', 'ArrayCell': 'td'}
    tag = tags.get(name)
    if name in COLORS:
        tag = 'div' if any(child.nodeName == 'par' for child in node.childNodes) else 'span'
    if name == 'multicolumn':
        for child in node.childNodes:
            render_child(child)
        return True
    if not tag:
        return False
    if tag == 'table':
        parts.append(f'<div class="latex-table"><table style="{style(node)}">')
    else:
        span = int(node.attributes.get('colspan', 1)) if tag == 'td' else 1
        parts.append(f'<{tag} style="{style(node)}"' + (f' colspan="{span}"' if span > 1 else '') + '>')
    for child in node.childNodes:
        render_child(child)
    parts.append(f'</{tag}>')
    if tag == 'table':
        parts.append('</div>')
    return True
