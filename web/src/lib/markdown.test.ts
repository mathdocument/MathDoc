import { describe, expect, it } from 'vitest';
import { markdownHtml } from './markdown';

const inline = (tex: string) => `<span class="latex-math" data-tex="${tex}"></span>`;
const display = (tex: string, tag = 'span') => `<${tag} class="latex-math" data-display="true" data-tex="${tex}"></${tag}>`;

describe('markdownHtml', () => {
  it('renders GitHub-flavoured Markdown', () => {
    const html = markdownHtml('# Heading\n\nA **bold** ~~old~~ claim.\n\n- one\n- two\n\n| a |\n|---|\n| b |\n');
    expect(html).toContain('<h1>Heading</h1>');
    expect(html).toContain('<strong>bold</strong> <del>old</del>');
    expect(html).toContain('<li>one</li>');
    expect(html).toContain('<td>b</td>');
  });

  it('keeps TeX away from Markdown emphasis', () => {
    expect(markdownHtml('Let $a_1 * b_2$ and \\(x_i\\).')).toBe(`<p>Let ${inline('a_1 * b_2')} and ${inline('x_i')}.</p>\n`);
    expect(markdownHtml('Then $$\\sum_i x_i$$ holds.')).toBe(`<p>Then ${display('\\sum_i x_i')} holds.</p>\n`);
  });

  it('renders display math blocks', () => {
    expect(markdownHtml('Before\n\n$$\n- a_1 \\\\\n+ b\n$$\n\nAfter')).toBe(`<p>Before</p>\n${display('- a_1 \\\\\n+ b', 'div')}\n<p>After</p>\n`);
    expect(markdownHtml('\\[\nx<y\n\\]')).toBe(`${display('x&#60;y', 'div')}\n`);
  });

  it('leaves currency, escaped dollars and code alone', () => {
    expect(markdownHtml('It costs $5 and $10.')).toBe('<p>It costs $5 and $10.</p>\n');
    expect(markdownHtml('Write \\$x\\$ or `$y$`.')).toBe('<p>Write $x$ or <code>$y$</code>.</p>\n');
  });
});
