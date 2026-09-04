<script lang="ts">
  import renderMathInElement from "katex/contrib/auto-render";
  import "katex/dist/katex.min.css";

  interface Props {
    source: string;
  }

  let { source }: Props = $props();
  let host = $state<HTMLDivElement | null>(null);
  let renderError = $state<string | null>(null);

  const statementNames: Record<string, string> = {
    theorem: "Theorem",
    lemma: "Lemma",
    proposition: "Proposition",
    corollary: "Corollary",
    definition: "Definition",
    remark: "Remark",
    example: "Example",
    proof: "Proof",
  };

  function stripComment(line: string): string {
    for (let i = 0; i < line.length; i++) {
      if (line[i] !== "%") continue;
      let slashes = 0;
      for (let j = i - 1; j >= 0 && line[j] === "\\"; j--) slashes++;
      if (slashes % 2 === 0) return line.slice(0, i);
    }
    return line;
  }

  function unwrapTitle(value: string): string {
    let unwrapped = value;
    let previous: string;
    do {
      previous = unwrapped;
      unwrapped = unwrapped.replace(/\\(?:textbf|textit|emph|texttt)\{((?:\\[{}]|[^{}])*)\}/g, "$1");
    } while (unwrapped !== previous);
    return unwrapped.replace(/\\([%&#_${}])/g, "$1");
  }

  function parseHeading(line: string): { level: string; title: string; trailing: string } | null {
    const start = line.match(/^\\(section|subsection|subsubsection|paragraph)\*?/);
    if (!start) return null;
    let cursor = start[0].length;
    while (/\s/.test(line[cursor] ?? "")) cursor++;
    if (line[cursor] === "[") {
      let optionalDepth = 1;
      let braceDepth = 0;
      for (cursor++; cursor < line.length && optionalDepth > 0; cursor++) {
        const character = line[cursor];
        let slashes = 0;
        for (let previous = cursor - 1; previous >= 0 && line[previous] === "\\"; previous--) slashes++;
        if (slashes % 2 !== 0) continue;
        if (character === "{") braceDepth++;
        else if (character === "}") braceDepth = Math.max(0, braceDepth - 1);
        else if (braceDepth === 0 && character === "[") optionalDepth++;
        else if (braceDepth === 0 && character === "]") optionalDepth--;
      }
      if (optionalDepth !== 0) return null;
      while (/\s/.test(line[cursor] ?? "")) cursor++;
    }
    if (line[cursor] !== "{") return null;
    const titleStart = cursor + 1;
    let depth = 1;
    for (let index = titleStart; index < line.length; index++) {
      const character = line[index];
      if (character !== "{" && character !== "}") continue;
      let slashes = 0;
      for (let previous = index - 1; previous >= 0 && line[previous] === "\\"; previous--) slashes++;
      if (slashes % 2 !== 0) continue;
      depth += character === "{" ? 1 : -1;
      if (depth === 0) {
        return {
          level: start[1]!,
          title: line.slice(titleStart, index),
          trailing: line.slice(index + 1).trimStart(),
        };
      }
    }
    return null;
  }

  function buildDocument(root: HTMLElement, latex: string) {
    root.replaceChildren();
    const stack: HTMLElement[] = [root];
    let paragraph: HTMLParagraphElement | null = null;
    let paragraphText: Text | null = null;

    const current = () => stack[stack.length - 1]!;
    const closeParagraph = () => {
      paragraph = null;
      paragraphText = null;
    };
    const appendText = (text: string) => {
      if (!paragraph) {
        paragraph = document.createElement("p");
        paragraphText = document.createTextNode("");
        paragraph.append(paragraphText);
        current().append(paragraph);
      }
      paragraphText!.appendData(`${paragraphText!.data ? "\n" : ""}${text}`);
    };

    for (const rawLine of latex.replace(/\r\n?/g, "\n").split("\n")) {
      const line = stripComment(rawLine);
      const trimmed = line.trim();

      if (!trimmed) {
        closeParagraph();
        continue;
      }
      if (/^\\(?:documentclass|usepackage|newcommand|renewcommand|providecommand|newtheorem)\b/.test(trimmed)) {
        continue;
      }
      if (/^\\(?:begin|end)\{document\}$/.test(trimmed)) continue;

      const heading = parseHeading(trimmed);
      if (heading) {
        closeParagraph();
        const levels: Record<string, string> = {
          section: "h2",
          subsection: "h3",
          subsubsection: "h4",
          paragraph: "h5",
        };
        const element = document.createElement(levels[heading.level]!);
        element.textContent = unwrapTitle(heading.title);
        current().append(element);
        if (heading.trailing) appendText(heading.trailing);
        continue;
      }

      const statementStart = trimmed.match(
        /^\\begin\{(theorem|lemma|proposition|corollary|definition|remark|example|proof)\}(?:\[([^\]]+)\])?\s*(.*)$/,
      );
      if (statementStart) {
        closeParagraph();
        const environment = statementStart[1]!;
        const section = document.createElement("section");
        section.className = `latex-statement ${environment === "proof" ? "proof" : ""}`;
        const label = document.createElement("div");
        label.className = "latex-statement-label";
        label.textContent = statementNames[environment]! +
          (statementStart[2] ? ` (${unwrapTitle(statementStart[2])})` : "");
        const body = document.createElement("div");
        body.className = "latex-statement-body";
        section.append(label, body);
        current().append(section);
        stack.push(body);
        const tail = statementStart[3]!;
        const endMarker = `\\end{${environment}}`;
        const endIndex = tail.indexOf(endMarker);
        const statementBody = endIndex < 0 ? tail : tail.slice(0, endIndex).trimEnd();
        if (statementBody) appendText(statementBody);
        if (endIndex >= 0) {
          closeParagraph();
          stack.pop();
          const trailing = tail.slice(endIndex + endMarker.length).trimStart();
          if (trailing) appendText(trailing);
        }
        continue;
      }

      const statementEnd = trimmed.match(
        /^\\end\{(theorem|lemma|proposition|corollary|definition|remark|example|proof)\}\s*(.*)$/,
      );
      if (statementEnd) {
        closeParagraph();
        if (stack.length > 1) stack.pop();
        if (statementEnd[2]) appendText(statementEnd[2]);
        continue;
      }

      const listStart = trimmed.match(/^\\begin\{(itemize|enumerate)\}$/);
      if (listStart) {
        closeParagraph();
        const list = document.createElement(listStart[1] === "enumerate" ? "ol" : "ul");
        current().append(list);
        stack.push(list);
        continue;
      }
      const listEnd = trimmed.match(/^\\end\{(?:itemize|enumerate)\}\s*(.*)$/);
      if (listEnd) {
        closeParagraph();
        if (current().tagName === "LI") stack.pop();
        if (stack.length > 1 && /^(?:UL|OL)$/.test(current().tagName)) stack.pop();
        if (listEnd[1]) appendText(listEnd[1]);
        continue;
      }
      const item = trimmed.match(/^\\item(?:\[([^\]]+)\])?\s*(.*)$/);
      if (item && current().tagName === "LI") {
        closeParagraph();
        stack.pop();
      }
      if (item && /^(?:UL|OL)$/.test(current().tagName)) {
        closeParagraph();
        const listItem = document.createElement("li");
        if (item[1]) {
          const label = document.createElement("strong");
          label.textContent = `${unwrapTitle(item[1])} `;
          listItem.append(label);
        }
        current().append(listItem);
        stack.push(listItem);
        if (item[2]) appendText(item[2]);
        continue;
      }

      appendText(line);
    }
  }

  function formatTextCommands(root: HTMLElement) {
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    const textNodes: Text[] = [];
    while (walker.nextNode()) {
      const node = walker.currentNode as Text;
      if (!node.parentElement?.closest(".katex")) textNodes.push(node);
    }

    for (const node of textNodes) {
      const pattern = /\\(textbf|textit|emph|texttt)\{([^{}]*)\}|\\([%&#_$])/g;
      if (!pattern.test(node.data)) continue;
      pattern.lastIndex = 0;
      const fragment = document.createDocumentFragment();
      let cursor = 0;
      for (const match of node.data.matchAll(pattern)) {
        fragment.append(document.createTextNode(node.data.slice(cursor, match.index)));
        if (match[3]) {
          fragment.append(document.createTextNode(match[3]));
        } else {
          const tags: Record<string, string> = {
            textbf: "strong",
            textit: "em",
            emph: "em",
            texttt: "code",
          };
          const element = document.createElement(tags[match[1]!]!);
          element.textContent = match[2]!;
          fragment.append(element);
        }
        cursor = match.index + match[0].length;
      }
      fragment.append(document.createTextNode(node.data.slice(cursor)));
      node.replaceWith(fragment);
    }
  }

  function render() {
    if (!host) return;
    renderError = null;
    buildDocument(host, source);
    const errors: string[] = [];
    try {
      renderMathInElement(host, {
        delimiters: [
          { left: "$$", right: "$$", display: true },
          { left: "\\[", right: "\\]", display: true },
          { left: "\\begin{equation}", right: "\\end{equation}", display: true },
          { left: "\\begin{equation*}", right: "\\end{equation*}", display: true },
          { left: "\\begin{align}", right: "\\end{align}", display: true },
          { left: "\\begin{align*}", right: "\\end{align*}", display: true },
          { left: "\\begin{gather}", right: "\\end{gather}", display: true },
          { left: "\\begin{gather*}", right: "\\end{gather*}", display: true },
          { left: "\\begin{multline}", right: "\\end{multline}", display: true },
          { left: "\\begin{multline*}", right: "\\end{multline*}", display: true },
          { left: "\\(", right: "\\)", display: false },
          { left: "$", right: "$", display: false },
        ],
        throwOnError: false,
        errorColor: "var(--mdc-error)",
        strict: "warn",
        trust: false,
        ignoredTags: ["script", "noscript", "style", "textarea", "pre", "code"],
        errorCallback: (message) => errors.push(message),
      });
      formatTextCommands(host);
      if (errors.length > 0) {
        renderError = errors[0]!;
      } else {
        const katexError = host.querySelector<HTMLElement>(".katex-error");
        if (katexError) {
          renderError = katexError.title || katexError.textContent || "invalid LaTeX";
        }
      }
    } catch (error) {
      renderError = error instanceof Error ? error.message : String(error);
    }
  }

  $effect(() => {
    source;
    host;
    render();
  });
</script>

<div class="latex-preview" bind:this={host} aria-label="rendered LaTeX preview"></div>
{#if renderError}
  <div class="preview-error" role="status">Some LaTeX could not be rendered: {renderError}</div>
{/if}

<style>
  .latex-preview {
    min-height: 9rem;
    padding: 1.4rem clamp(1rem, 4cqw, 2rem) 1.65rem;
    overflow-x: auto;
    color: var(--mdc-fg);
    background:
      radial-gradient(circle at 20% 0%, color-mix(in srgb, var(--mdc-accent-up) 7%, transparent), transparent 38%),
      var(--mdc-panel);
    font-family: Georgia, "Times New Roman", serif;
    font-size: 1rem;
    line-height: 1.7;
  }
  .latex-preview :global(p) {
    margin: 0 0 0.85rem;
    white-space: pre-wrap;
  }
  .latex-preview > :global(p) {
    content-visibility: auto;
    contain-intrinsic-block-size: auto 1lh;
  }
  .latex-preview :global(h2),
  .latex-preview :global(h3),
  .latex-preview :global(h4),
  .latex-preview :global(h5) {
    margin: 1.15rem 0 0.7rem;
    color: var(--mdc-fg);
    font-family: var(--mdc-font);
    line-height: 1.25;
  }
  .latex-preview :global(h2) { font-size: 1.35rem; }
  .latex-preview :global(h3) { font-size: 1.18rem; }
  .latex-preview :global(h4),
  .latex-preview :global(h5) { font-size: 1rem; }
  .latex-preview :global(.latex-statement) {
    margin: 0.9rem 0;
    padding: 0.85rem 1rem 0.15rem;
    border-left: 3px solid var(--mdc-accent-up);
    border-radius: 0 var(--mdc-radius-sm) var(--mdc-radius-sm) 0;
    background: color-mix(in srgb, var(--mdc-accent-up) 7%, transparent);
  }
  .latex-preview :global(.latex-statement.proof) {
    border-left-color: var(--mdc-muted);
    background: color-mix(in srgb, var(--mdc-fg) 2%, transparent);
  }
  .latex-preview :global(.latex-statement-label) {
    margin-bottom: 0.28rem;
    color: var(--mdc-fg);
    font-family: var(--mdc-font);
    font-weight: 700;
  }
  .latex-preview :global(ul),
  .latex-preview :global(ol) {
    margin: 0.65rem 0 0.9rem;
    padding-left: 1.6rem;
  }
  .latex-preview :global(code) {
    padding: 0.1rem 0.28rem;
    border-radius: 4px;
    background: var(--mdc-panel-raised);
    font-family: var(--mdc-mono);
    font-size: 0.86em;
  }
  .latex-preview :global(.katex) {
    color: var(--mdc-fg);
    font-size: 1.08em;
  }
  .latex-preview :global(.katex-display) {
    margin: 1.1rem 0;
    padding: 0.5rem 0;
    overflow-x: auto;
    overflow-y: hidden;
  }
  .preview-error {
    padding: 0.55rem 0.8rem;
    border-top: 1px solid rgba(255, 125, 143, 0.2);
    color: var(--mdc-error);
    background: rgba(255, 125, 143, 0.08);
    font-family: var(--mdc-mono);
    font-size: 0.68rem;
  }
</style>
