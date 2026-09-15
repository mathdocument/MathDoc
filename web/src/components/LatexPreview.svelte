<script lang="ts">
  import katex from 'katex';
  import 'katex/dist/katex.min.css';
  import { projectPath } from '../lib/project-path';
  import type { LatexLabel } from '../lib/latex';

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
<div class="latex-preview" bind:this={host} onclick={navigate}>{@html html}</div>

<style>
  .latex-preview { padding:1.2rem 1.4rem; color:var(--mdc-fg); font-size:var(--mdc-text-sm); line-height:1.8; overflow-wrap:anywhere; }
  .latex-preview :global(p) { margin:.6em 0; }
  .latex-preview :global(h2), .latex-preview :global(h3), .latex-preview :global(h4) { margin:1em 0 .5em; line-height:1.35; }
  .latex-preview :global(a) { color:var(--mdc-accent); text-decoration:underline; text-underline-offset:3px; }
  .latex-preview :global(.latex-statement), .latex-preview :global(.latex-proof) { margin:1rem 0; padding:.6rem .9rem; border-left:2px solid var(--mdc-border-strong); }
  .latex-preview :global(.latex-statement-title) { font-weight:650; }
  .latex-preview :global(.latex-anchor) { scroll-margin-block:1rem; }
  .latex-preview :global(.latex-math[data-display="true"]) { display:block; overflow-x:auto; }
  .latex-preview :global(.katex) { font-size:1.1em; }
  .latex-preview :global(.latex-error) { color:var(--mdc-error); }
  .latex-preview :global(pre) { overflow-x:auto; white-space:pre; }
  .latex-preview :global(.latex-bibliography) { border-top:1px solid var(--mdc-border); margin-top:1.5rem; font-size:.9em; }
  .latex-preview :global(dt) { font-weight:600; }
  .latex-preview :global(dd) { margin:0 0 .8rem 1rem; }
</style>
