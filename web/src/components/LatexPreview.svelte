<script lang="ts">
  import katex from 'katex';
  import 'katex/dist/katex.min.css';
  import { projectPath } from '../lib/project-path';
  import type { LatexLabel } from '../lib/latex';
  import { chainEditorScroll } from '../lib/editor-scroll';

  let { html, fnode, labels, focusLabel, onNavigate }: {
    html: string; fnode: string; labels: LatexLabel[]; focusLabel?: string;
    onNavigate?: (fnode: string, label: string) => void;
  } = $props();
  let host = $state<HTMLDivElement>();
  function scroll(id: string) {
    const target = document.getElementById(id);
    if (target && host?.contains(target)) target.scrollIntoView({block: 'nearest', behavior: 'smooth'});
  }
  $effect(() => {
    void html;
    if (!host) return;
    for (const element of host.querySelectorAll<HTMLElement>('.latex-math[data-tex]')) {
      katex.render(element.dataset.tex ?? '', element, {
        displayMode: element.dataset.display === 'true', throwOnError: false, trust: false,
      });
    }
    for (const link of host.querySelectorAll<HTMLAnchorElement>('a[data-latex-node]')) {
      const params = new URLSearchParams({ref: link.dataset.latexNode!, label: link.dataset.latexLabel!});
      link.href = `${projectPath('/')}#${params}`;
    }
    const target = host;
    if (target.querySelector('.latex-diagram')) void import('../lib/latex-diagram').then(({renderDiagrams}) => renderDiagrams(target));
  });
  $effect(() => {
    void html;
    const id = labels.find(label => label.label === focusLabel)?.anchor;
    if (id) {
      const frame = requestAnimationFrame(() => scroll(id));
      return () => cancelAnimationFrame(frame);
    }
  });
  function navigate(event: MouseEvent) {
    if (event.button || event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) return;
    const link = (event.target as Element).closest<HTMLAnchorElement>('a');
    if (!link || !host?.contains(link)) return;
    if (link.dataset.latexNode) {
      event.preventDefault();
      if (link.dataset.latexNode === fnode) {
        const id = labels.find(label => label.label === link.dataset.latexLabel)?.anchor;
        if (id) scroll(id);
      } else onNavigate?.(link.dataset.latexNode, link.dataset.latexLabel!);
    } else if (link.getAttribute('href')?.startsWith('#')) {
      event.preventDefault(); scroll(link.getAttribute('href')!.slice(1));
    }
  }
</script>

<!-- HTML is escaped and assembled from fixed tags by the backend renderer. -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="latex-preview" bind:this={host} use:chainEditorScroll onclick={navigate}>{@html html}</div>

<style>
  @font-face { font-family:"Latin Modern Roman"; src:url('/fonts/latin-modern/lmroman10-regular.otf') format('opentype'); font-weight:400; font-style:normal; font-display:block; }
  @font-face { font-family:"Latin Modern Roman"; src:url('/fonts/latin-modern/lmroman10-italic.otf') format('opentype'); font-weight:400; font-style:italic; font-display:block; }
  @font-face { font-family:"Latin Modern Roman"; src:url('/fonts/latin-modern/lmroman10-bold.otf') format('opentype'); font-weight:700; font-style:normal; font-display:block; }
  @font-face { font-family:"Latin Modern Roman"; src:url('/fonts/latin-modern/lmroman10-bolditalic.otf') format('opentype'); font-weight:700; font-style:italic; font-display:block; }
  .latex-preview { min-height:0; overflow:auto; padding:1.2rem 1.4rem; color:var(--mdc-fg); font-family:"Latin Modern Roman", "Songti SC", serif; font-size:1rem; line-height:1.75; overflow-wrap:anywhere; -webkit-font-smoothing:auto; }
  .latex-preview :global(p) { margin:.6em 0; }
  .latex-preview :global(h2), .latex-preview :global(h3), .latex-preview :global(h4) { margin:1em 0 .5em; line-height:1.35; }
  .latex-preview :global(a) { color:var(--mdc-accent); text-decoration:underline; text-underline-offset:3px; }
  .latex-preview :global(.latex-statement), .latex-preview :global(.latex-proof) { position:relative; margin:.5rem 0; padding:.3rem .9rem; }
  .latex-preview :global(.latex-statement::before), .latex-preview :global(.latex-proof::before) { content:''; position:absolute; inset:.3rem auto .3rem 0; width:2px; background:var(--mdc-border-strong); }
  .latex-preview :global(.latex-statement-title) { display:inline; font-weight:700; }
  .latex-preview :global(.latex-statement-title::after) { content:'. '; }
  .latex-preview :global(.latex-statement-title + p) { display:inline; }
  .latex-preview :global(.latex-anchor) { scroll-margin-block:1rem; }
  .latex-preview :global(.latex-math[data-display="true"]) { display:block; overflow-x:auto; }
  .latex-preview :global(.katex) { font-size:1.1em; }
  .latex-preview :global(.latex-math[data-display="false"] .katex) { font-size:1em; }
  .latex-preview :global(.latex-error) { color:var(--mdc-error); }
  .latex-preview :global(.latex-table) { max-width:100%; overflow-x:auto; margin:.6em 0; }
  .latex-preview :global(.latex-table table) { border-collapse:collapse; margin:auto; }
  .latex-preview :global(.latex-table td) { padding:.25em .65em; }
  .latex-preview :global(.latex-table td p) { margin:0; }
  .latex-preview :global(.latex-diagram) { overflow-x:auto; text-align:center; margin:.8em 0; }
  .latex-preview :global(.latex-diagram svg) { display:block; margin:auto; background:white; color:black; zoom:1.5; }
  .latex-preview :global(pre) { overflow-x:auto; white-space:pre; }
  .latex-preview :global(pre), .latex-preview :global(code) { font-family:var(--mdc-mono); font-size:.85em; }
  .latex-preview :global(.latex-bibliography) { border-top:1px solid var(--mdc-border); margin-top:1.5rem; font-size:.9em; }
  .latex-preview :global(.latex-bibliography dl) { display:grid; grid-template-columns:max-content minmax(0, 1fr); column-gap:1em; row-gap:.6em; }
  .latex-preview :global(.latex-bibliography dt) { font-weight:400; }
  .latex-preview :global(.latex-bibliography dd), .latex-preview :global(.latex-bibliography p) { margin:0; }
  .latex-preview :global(dt) { font-weight:700; }
  .latex-preview :global(dd) { margin:0 0 .8rem 1rem; }
</style>
