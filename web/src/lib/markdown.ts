import { Marked, type TokenizerAndRendererExtension } from 'marked';
import createDOMPurify from 'dompurify';

const escapeAttr = (text: string) => text.replace(/[&<>"']/g, char => `&#${char.charCodeAt(0)};`);

// Math leaves Markdown untouched (its `_` and `*` are TeX) and becomes the same
// placeholder the LaTeX preview hands to MathJax (lib/latex-math.ts).
function math(name: string, level: 'block' | 'inline', patterns: [RegExp, boolean][]): TokenizerAndRendererExtension {
  return {
    name, level,
    start: level === 'inline' ? src => src.match(/\$|\\[([]/)?.index : undefined,
    tokenizer(src) {
      for (const [pattern, display] of patterns) {
        const match = pattern.exec(src);
        if (match) return {type: name, raw: match[0], text: match[1]!.trim(), display};
      }
    },
    renderer(token) {
      const tag = level === 'block' ? 'div' : 'span';
      const display = token.display ? ' data-display="true"' : '';
      return `<${tag} class="latex-math"${display} data-tex="${escapeAttr(token.text)}"></${tag}>${level === 'block' ? '\n' : ''}`;
    },
  };
}

const markdown = new Marked({
  gfm: true,
  extensions: [
    math('mathBlock', 'block', [
      [/^ {0,3}\$\$([\s\S]+?)\$\$[ \t]*(?:\n+|$)/, true],
      [/^ {0,3}\\\[([\s\S]+?)\\\][ \t]*(?:\n+|$)/, true],
    ]),
    math('mathInline', 'inline', [
      [/^\$\$([\s\S]+?)\$\$/, true],
      // Pandoc's rule: no space inside the dollars and no digit after, so "$5 and $10" stays text.
      [/^\$(?![\s$])((?:\\[\s\S]|[^\\$])+?)(?<!\s)\$(?!\d)/, false],
      [/^\\\[([\s\S]+?)\\\]/, true],
      [/^\\\(([\s\S]+?)\\\)/, false],
    ]),
  ],
});

/** Unsanitized HTML for a text block; see renderMarkdown. */
export function markdownHtml(source: string): string {
  return markdown.parse(source, {async: false});
}

let purifier: ReturnType<typeof createDOMPurify> | undefined;
/** Markdown may carry raw HTML, so the preview only ever sees sanitized output. */
export function renderMarkdown(source: string): string {
  if (!purifier) {
    purifier = createDOMPurify(window);
    purifier.addHook('afterSanitizeAttributes', node => {
      if (node.tagName !== 'A' || !node.hasAttribute('href')) return;
      // The page's own hash is its node route, so in-document anchors would navigate away.
      if (node.getAttribute('href')!.startsWith('#')) node.removeAttribute('href');
      else { node.setAttribute('target', '_blank'); node.setAttribute('rel', 'noopener noreferrer'); }
    });
  }
  return purifier.sanitize(markdownHtml(source));
}
