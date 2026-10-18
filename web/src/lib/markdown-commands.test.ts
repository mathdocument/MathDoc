import { describe, expect, it } from 'vitest';
import { applyEdit, markdownEdit, type MarkdownCommand } from './markdown-commands';

/** Apply a command to text with the selection marked by `[` and `]` (or a caret `|`); show the result the same way. */
function run(marked: string, command: MarkdownCommand): string {
  const caret = marked.indexOf('|');
  const from = caret >= 0 ? caret : marked.indexOf('[');
  const text = caret >= 0 ? marked.replace('|', '') : marked.replace('[', '').replace(']', '');
  const to = caret >= 0 ? caret : marked.indexOf(']') - 1;
  const e = markdownEdit(text, from, to, command);
  const out = applyEdit(text, e);
  const [a, b] = e.selection;
  return a === b ? out.slice(0, a) + '|' + out.slice(a) : out.slice(0, a) + '[' + out.slice(a, b) + ']' + out.slice(b);
}

describe('the Markdown toolbar commands', () => {
  it('wrap the selection inline and unwrap it again, inside or around it', () => {
    expect(run('a [word] b', 'bold')).toBe('a **[word]** b');
    expect(run('a **[word]** b', 'bold')).toBe('a [word] b');
    expect(run('a [**word**] b', 'bold')).toBe('a [word] b');
    expect(run('x|', 'math')).toBe('x$[x^2]$');
    expect(run('[f x]', 'code')).toBe('`[f x]`');
  });
  it('tell italic from bold, which share the star', () => {
    expect(run('**[w]**', 'italic')).toBe('***[w]***');
    expect(run('***[w]***', 'italic')).toBe('**[w]**');
    expect(run('***[w]***', 'bold')).toBe('*[w]*');
    // Stars work inside a word, as in Chinese text without spaces.
    expect(run('中[强调]中', 'italic')).toBe('中*[强调]*中');
  });
  it('toggle headings, lists and quotes on every line the selection touches', () => {
    expect(run('ti|tle', 'heading2')).toBe('## ti|tle');
    expect(run('## ti|tle', 'heading2')).toBe('ti|tle');
    expect(run('## ti|tle', 'heading1')).toBe('# ti|tle');
    expect(run('[a\nb]', 'bulletList')).toBe('[- a\n- b]');
    expect(run('[- a\n- b]', 'bulletList')).toBe('[a\nb]');
    expect(run('[a\n\nb]', 'numberedList')).toBe('[1. a\n\n2. b]');
    expect(run('[> a\n> b]', 'quote')).toBe('[a\nb]');
    // A selection ending at the start of a line leaves that line alone.
    expect(run('[a\n]b', 'quote')).toBe('[> a]\nb');
  });
  it('fence display math and Lean code on lines of their own', () => {
    expect(run('see [a+b] here', 'displayMath')).toBe('see \n$$\n[a+b]\n$$\n here');
    expect(run('|', 'displayMath')).toBe('$$\n[x^2]\n$$');
    expect(run('|', 'codeBlock')).toBe('```lean\n|\n```');
  });
  it('link the selection and select the address to type', () => {
    expect(run('see [docs]', 'link')).toBe('see [docs]([url])');
    expect(run('|', 'link')).toBe('[text]([url])');
  });
});
